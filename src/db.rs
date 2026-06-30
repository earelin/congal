//! Persistencia en SQLite (rusqlite, bundled).

use crate::model::{
    ContractDetail, ContractSummary, ContratoAdxudicado, EmpresaContratos, EmpresaNodo,
    GrupoRelacion, LocalFilters, LocalRow, MenorRow, Resolucion, TipoContrato, Ute, UteRelacion,
    company_key, format_data_gl, format_importe, normalize_search, parse_data, parse_importe,
};
use anyhow::Result;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, params};
use std::path::Path;

pub struct Db {
    conn: Connection,
}

/// Estatísticas da base de datos para amosar na interface.
#[derive(Debug, Clone, Default)]
pub struct DbStats {
    pub total: i64,
    pub con_detalle: i64,
    pub ultima_sync: Option<String>,
}

/// Valores distintos presentes na base de datos, para os despregables da busca local.
#[derive(Debug, Clone, Default)]
pub struct LocalOptions {
    pub adxudicatarios: Vec<String>,
    pub organismos: Vec<String>,
    pub anos: Vec<String>,
}

/// Extrae o ano (19xx/20xx) dunha cadea de data en calquera formato.
fn extract_year(s: &str) -> Option<String> {
    let b = s.as_bytes();
    for w in b.windows(4) {
        if w.iter().all(|c| c.is_ascii_digit()) && (w.starts_with(b"19") || w.starts_with(b"20")) {
            return Some(String::from_utf8_lossy(w).into_owned());
        }
    }
    None
}

/// Constrúe a cláusula WHERE (e os seus argumentos posicionais) común á busca
/// local a partir dos filtros. As condicións refírense aos alias `c` (contracts),
/// `o` (organismos) e `e` (estados), que o chamador debe ter no FROM. Reutilízase
/// tanto na táboa de contratos como na vista de relacións para que os filtros do
/// panel lateral se apliquen ás dúas pestanas.
fn local_where(f: &LocalFilters) -> (String, Vec<String>) {
    let mut sql = String::new();
    let mut args: Vec<String> = Vec::new();
    if !f.texto.trim().is_empty() {
        sql.push_str(" AND (nrm(c.asunto) LIKE ? OR nrm(c.referencia) LIKE ?)");
        let like = format!("%{}%", normalize_search(&f.texto));
        args.push(like.clone());
        args.push(like);
    }
    if !f.organismo.trim().is_empty() {
        sql.push_str(" AND nrm(o.nome) LIKE ?");
        args.push(format!("%{}%", normalize_search(&f.organismo)));
    }
    if !f.year.trim().is_empty() {
        sql.push_str(" AND c.data_publicacion LIKE ?");
        args.push(format!("%{}%", f.year.trim()));
    }
    if !f.adxudicatario.trim().is_empty() {
        // O contrato inclúese se ALGÚN dos seus lotes casa co adxudicatario.
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM contract_resolucion x \
               WHERE x.contract_id = c.id AND nrm(x.adxudicatario) LIKE ?)",
        );
        args.push(format!("%{}%", normalize_search(&f.adxudicatario)));
    }
    (sql, args)
}

/// CTE `filtrados`: os ids dos contratos que casan cos filtros do panel lateral
/// (a mesma base que o listado local). `where_sql` é a cláusula que devolve
/// [`local_where`]; os seus argumentos pásanse por posición na consulta final.
/// Compártena as vistas que agregan sobre os contratos filtrados (empresas e
/// relacións), que prefixan `WITH ` e fan `... IN (SELECT id FROM filtrados)`.
fn filtrados_cte(where_sql: &str) -> String {
    format!(
        "filtrados AS (SELECT c.id FROM contracts c \
           LEFT JOIN organismos o ON o.cod_organismo = c.cod_organismo \
           LEFT JOIN estados e ON e.cod_estado = c.cod_estado \
           WHERE 1=1{where_sql})"
    )
}

/// SELECT + FROM común do listado local (unha fila por contrato, cos lotes/
/// resolucións agregados). O chamador engade a cláusula WHERE, a orde e, de ser
/// o caso, LIMIT/OFFSET. As columnas están na orde que espera [`map_local_row`].
const LOCAL_SELECT: &str = r#"SELECT c.id, COALESCE(c.tipo,'licitacion'), COALESCE(c.referencia,''),
              COALESCE(c.asunto,''), c.importe_num, COALESCE(e.nome,''),
              COALESCE(c.data_publicacion,''), COALESCE(o.nome,''),
              COALESCE(r.adxudicatarios,''), COALESCE(r.nifs,''), r.importe_total,
              COALESCE(c.duracion,''), COALESCE(d.enlace_resolucion,''),
              r.max_part, r.min_part
       FROM contracts c
       LEFT JOIN organismos o ON o.cod_organismo = c.cod_organismo
       LEFT JOIN estados e ON e.cod_estado = c.cod_estado
       LEFT JOIN contract_detail d ON d.contract_id = c.id
       LEFT JOIN (
           SELECT contract_id,
                  GROUP_CONCAT(DISTINCT NULLIF(TRIM(adxudicatario),'')) AS adxudicatarios,
                  GROUP_CONCAT(DISTINCT NULLIF(TRIM(nif),'')) AS nifs,
                  SUM(importe_resolucion_num) AS importe_total,
                  MAX(CAST(participacion AS INTEGER)) AS max_part,
                  MIN(CAST(participacion AS INTEGER)) AS min_part
           FROM contract_resolucion
           GROUP BY contract_id
       ) r ON r.contract_id = c.id"#;

/// FROM/WHERE/GROUP BY común da agregación de empresas adxudicatarias: agrupa as
/// resolucións dos contratos filtrados (CTE `filtrados`) por empresa (`ekey`,
/// definido no SELECT do chamador = NIF ou, se falta, a clave do nome). Só contan
/// as resolucións cun adxudicatario. Compárteno [`Db::empresas_inner`] (listado
/// paxinable) e [`Db::empresas_resumo`] (total de empresas e importe).
const EMPRESAS_GROUP: &str = "FROM contract_resolucion r \
     WHERE r.contract_id IN (SELECT id FROM filtrados) \
       AND TRIM(COALESCE(r.adxudicatario,'')) <> '' \
     GROUP BY ekey";

/// Expresión que identifica unha empresa adxudicataria (a `ekey` de [`EMPRESAS_GROUP`]):
/// o NIF se o hai, e se non a clave normalizada do nome (`cokey`). Compárteno
/// [`Db::empresas_inner`] e [`Db::empresas_resumo`] para garantir que ambas agrupan
/// polas mesmas empresas (o resumo só casa co listado se a clave é idéntica).
const EMPRESAS_EKEY: &str = "COALESCE(NULLIF(TRIM(r.nif),''), cokey(r.adxudicatario))";

/// Constrúe un [`LocalRow`] a partir dunha fila de [`LOCAL_SELECT`].
fn map_local_row(row: &rusqlite::Row) -> rusqlite::Result<LocalRow> {
    let tipo_txt: String = row.get(1)?;
    let importe_num: Option<f64> = row.get(4)?;
    let data_iso: String = row.get(6)?;
    let importe_total: Option<f64> = row.get(10)?;
    let max_part: Option<i64> = row.get(13)?;
    let min_part: Option<i64> = row.get(14)?;
    Ok(LocalRow {
        id: row.get(0)?,
        tipo: TipoContrato::from_db(&tipo_txt),
        referencia: row.get(2)?,
        asunto: row.get(3)?,
        importe_txt: importe_num.map(format_importe).unwrap_or_default(),
        estado: row.get(5)?,
        publicacion: format_data_gl(&data_iso),
        organismo: row.get(7)?,
        adxudicatario: row.get(8)?,
        nif: row.get(9)?,
        duracion: row.get(11)?,
        importe_resolucion_txt: importe_total.map(format_importe).unwrap_or_default(),
        enlace_resolucion: row.get(12)?,
        // Único participante só se TODOS os lotes constan cun único participante:
        // se algún ten participación descoñecida/baleira (CAST → 0) ou maior, non
        // se marca.
        participante_unico: max_part == Some(1) && min_part == Some(1),
    })
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Función SQL `nrm(x)`: normaliza texto (minúsculas, sen acentos) para
        // buscas insensibles a maiúsculas e acentos.
        conn.create_scalar_function(
            "nrm",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                // Tolerante a NULL (p.ex. adxudicatario nun LEFT JOIN sen resolución).
                let s = ctx.get::<Option<String>>(0)?;
                Ok(s.as_deref().map(normalize_search).unwrap_or_default())
            },
        )?;
        // Función SQL `cokey(x)`: clave de comparación de nomes de empresa
        // (minúsculas, sen acentos nin puntuación), para unir o nome da UTE
        // adxudicataria (`ute_membro.ute_key`) coa súa resolución
        // (`cokey(contract_resolucion.adxudicatario)`) e atribuírlle o importe.
        conn.create_scalar_function(
            "cokey",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let s = ctx.get::<Option<String>>(0)?;
                Ok(s.as_deref().map(company_key).unwrap_or_default())
            },
        )?;
        let db = Db { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Volca o WAL ao ficheiro principal e trúncao. Útil ao pechar a aplicación
    /// para non deixar atrás os ficheiros `-wal`/`-shm` cheos.
    pub fn checkpoint(&self) {
        let _ = self.conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            -- Nome dos organismos, normalizado nunha táboa propia e referenciado
            -- desde `contracts` polo seu código.
            CREATE TABLE IF NOT EXISTS organismos (
                cod_organismo TEXT PRIMARY KEY,
                nome          TEXT
            );

            -- Estados do contrato, normalizados nunha táboa propia. O servidor só
            -- devolve o texto do estado (non un código), así que `cod_estado` é
            -- unha clave subrogada que se asigna soa ao inserir un nome novo.
            CREATE TABLE IF NOT EXISTS estados (
                cod_estado INTEGER PRIMARY KEY,
                nome       TEXT NOT NULL UNIQUE
            );

            CREATE TABLE IF NOT EXISTS contracts (
                id                TEXT PRIMARY KEY,
                tipo              TEXT NOT NULL DEFAULT 'licitacion', -- 'licitacion' | 'menor'
                referencia        TEXT,
                asunto            TEXT,
                importe_num       REAL,
                cod_estado        INTEGER REFERENCES estados(cod_estado),
                data_publicacion  TEXT,   -- ISO 8601 'YYYY-MM-DD'
                cod_organismo     TEXT REFERENCES organismos(cod_organismo),
                -- Duración (só contratos menores; nas licitacións queda NULL).
                duracion          TEXT,
                detalle_descargado INTEGER NOT NULL DEFAULT 0,
                actualizado_en    TEXT    -- ISO 8601 'YYYY-MM-DD HH:MM:SS'
            );

            -- Tipos de tramitación, procedemento e contrato, normalizados en táboas
            -- propias e referenciados desde `contract_detail` polo seu código. Coma
            -- nos `estados`, o servidor só devolve o texto, así que o código é unha
            -- clave subrogada que se asigna soa ao inserir un nome novo.
            CREATE TABLE IF NOT EXISTS tipos_tramitacion (
                cod_tipo_tramitacion INTEGER PRIMARY KEY,
                nome                 TEXT NOT NULL UNIQUE
            );

            CREATE TABLE IF NOT EXISTS tipos_procedemento (
                cod_tipo_procedemento INTEGER PRIMARY KEY,
                nome                  TEXT NOT NULL UNIQUE
            );

            CREATE TABLE IF NOT EXISTS tipos_contrato (
                cod_tipo_contrato INTEGER PRIMARY KEY,
                nome              TEXT NOT NULL UNIQUE
            );

            CREATE TABLE IF NOT EXISTS contract_detail (
                contract_id          TEXT PRIMARY KEY REFERENCES contracts(id) ON DELETE CASCADE,
                referencia           TEXT,
                obxecto              TEXT,
                cod_tipo_tramitacion  INTEGER REFERENCES tipos_tramitacion(cod_tipo_tramitacion),
                cod_tipo_procedemento INTEGER REFERENCES tipos_procedemento(cod_tipo_procedemento),
                cod_tipo_contrato     INTEGER REFERENCES tipos_contrato(cod_tipo_contrato),
                orzamento_base       REAL,   -- con IVE (só o valor numérico)
                valor_estimado       REAL,   -- sen IVE (só o valor numérico)
                num_lotes            TEXT,
                sistema_contratacion TEXT,
                observacions         TEXT,
                data_difusion        TEXT,
                sara                 TEXT,
                centralizada         TEXT,
                lei_aplicacion       TEXT,
                enlace_resolucion    TEXT,
                extra_json           TEXT
            );

            -- Estados da resolución, normalizados nunha táboa propia (coma os
            -- `estados` do contrato): o servidor só dá o texto, así que o código
            -- é unha clave subrogada asignada soa ao inserir un nome novo.
            CREATE TABLE IF NOT EXISTS estados_resolucion (
                cod_estado_resolucion INTEGER PRIMARY KEY,
                nome                  TEXT NOT NULL UNIQUE
            );

            CREATE TABLE IF NOT EXISTS contract_resolucion (
                contract_id            TEXT NOT NULL REFERENCES contracts(id) ON DELETE CASCADE,
                lote                   TEXT,
                participacion          TEXT,
                cod_estado_resolucion  INTEGER REFERENCES estados_resolucion(cod_estado_resolucion),
                adxudicatario          TEXT,
                nif                    TEXT,
                importe_resolucion_num REAL,
                data_difusion          TEXT,
                prazo_execucion        TEXT,
                recurso                TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_contracts_org   ON contracts(cod_organismo);
            CREATE INDEX IF NOT EXISTS idx_contracts_tipo  ON contracts(cod_organismo, tipo);
            CREATE INDEX IF NOT EXISTS idx_contracts_estado ON contracts(cod_estado);
            CREATE INDEX IF NOT EXISTS idx_contracts_data  ON contracts(data_publicacion);
            CREATE INDEX IF NOT EXISTS idx_res_contract    ON contract_resolucion(contract_id);
            CREATE INDEX IF NOT EXISTS idx_res_adx         ON contract_resolucion(adxudicatario);

            CREATE TABLE IF NOT EXISTS meta (
                clave TEXT PRIMARY KEY,
                valor TEXT
            );

            -- Composición das UTE adxudicatarias: unha fila por empresa membro.
            -- Dato do propio contrato (popup de licitadores), sen fontes externas.
            CREATE TABLE IF NOT EXISTS ute_membro (
                contract_id TEXT NOT NULL REFERENCES contracts(id) ON DELETE CASCADE,
                ute_key     TEXT NOT NULL,   -- company_key(nome da UTE)
                ute_nome    TEXT NOT NULL,
                ute_nif     TEXT,
                membro_cif  TEXT NOT NULL,
                membro_nome TEXT NOT NULL,
                PRIMARY KEY (contract_id, ute_key, membro_cif)
            );
            "#,
        )?;
        Ok(())
    }

    /// Devolve o estado previo dun contrato, ou `None` se non existe.
    pub fn estado_previo(&self, id: &str) -> Result<Option<String>> {
        let r = self
            .conn
            .query_row(
                "SELECT e.nome FROM contracts c
                 LEFT JOIN estados e ON e.cod_estado = c.cod_estado
                 WHERE c.id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        Ok(r)
    }

    /// Inserta/actualiza un lote de rexistros do listado.
    pub fn upsert_summaries(&mut self, rows: &[ContractSummary], now: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            // Primeiro os organismos (a FK de `contracts` apunta a esta táboa).
            let mut org_stmt = tx.prepare(
                r#"INSERT INTO organismos (cod_organismo, nome) VALUES (?1, ?2)
                   ON CONFLICT(cod_organismo) DO UPDATE SET nome=excluded.nome"#,
            )?;
            for r in rows {
                if !r.cod_organismo.trim().is_empty() {
                    org_stmt.execute(params![r.cod_organismo, r.organismo])?;
                }
            }

            // Despois os estados (a FK de `contracts` apunta a esta táboa). O
            // `cod_estado` asígnase soa; aquí só garantimos que cada nome exista.
            let mut est_stmt = tx.prepare(r#"INSERT OR IGNORE INTO estados (nome) VALUES (?1)"#)?;
            for r in rows {
                if !r.estado.trim().is_empty() {
                    est_stmt.execute(params![r.estado.trim()])?;
                }
            }

            // `cod_estado` resólvese por subconsulta sobre `estados` (NULL se o
            // estado vén baleiro: a subconsulta non casa con ningunha fila).
            let mut stmt = tx.prepare(
                r#"INSERT INTO contracts
                    (id, tipo, referencia, asunto, importe_num, cod_estado,
                     data_publicacion, cod_organismo, actualizado_en)
                   VALUES (?1,?2,?3,?4,?5,
                     (SELECT cod_estado FROM estados WHERE nome=?6),
                     ?7,?8,?9)
                   ON CONFLICT(id) DO UPDATE SET
                     tipo=excluded.tipo,
                     referencia=excluded.referencia,
                     asunto=excluded.asunto,
                     importe_num=excluded.importe_num,
                     cod_estado=excluded.cod_estado,
                     data_publicacion=excluded.data_publicacion,
                     cod_organismo=excluded.cod_organismo,
                     actualizado_en=excluded.actualizado_en"#,
            )?;
            for r in rows {
                // `cod_organismo` baleiro gárdase como NULL para non violar a FK.
                let cod = Some(r.cod_organismo.trim()).filter(|c| !c.is_empty());
                stmt.execute(params![
                    r.id,
                    r.tipo.as_str(),
                    r.referencia,
                    r.asunto,
                    parse_importe(&r.importe),
                    r.estado.trim(),
                    parse_data(&r.publicacion),
                    cod,
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Inserta/actualiza os contratos menores dun organismo. Cada menor garda
    /// ademais unha única fila en `contract_resolucion` co seu adxudicatario
    /// (nome + NIF + importe), para integrarse na agregación do listado e na
    /// análise de relacións igual ca as licitacións.
    pub fn upsert_menores(
        &mut self,
        org_id: &str,
        org_nome: &str,
        rows: &[MenorRow],
        now: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            tx.execute(
                r#"INSERT INTO organismos (cod_organismo, nome) VALUES (?1, ?2)
                   ON CONFLICT(cod_organismo) DO UPDATE SET nome=excluded.nome"#,
                params![org_id, org_nome],
            )?;

            let mut up = tx.prepare(
                r#"INSERT INTO contracts
                    (id, tipo, asunto, importe_num, data_publicacion, cod_organismo,
                     duracion, actualizado_en)
                   VALUES (?1,'menor',?2,?3,?4,?5,?6,?7)
                   ON CONFLICT(id) DO UPDATE SET
                     tipo='menor',
                     asunto=excluded.asunto,
                     importe_num=excluded.importe_num,
                     data_publicacion=excluded.data_publicacion,
                     cod_organismo=excluded.cod_organismo,
                     duracion=excluded.duracion,
                     actualizado_en=excluded.actualizado_en"#,
            )?;
            let mut del_res =
                tx.prepare("DELETE FROM contract_resolucion WHERE contract_id = ?1")?;
            // O adxudicatario do menor gárdase como unha única resolución (sen
            // lote nin estado), reutilizando a táboa que xa agrega o listado.
            let mut ins_res = tx.prepare(
                r#"INSERT INTO contract_resolucion
                    (contract_id, lote, participacion, cod_estado_resolucion, adxudicatario,
                     nif, importe_resolucion_num, data_difusion, prazo_execucion, recurso)
                   VALUES (?1,'','',NULL,?2,?3,?4,'','','')"#,
            )?;
            for r in rows {
                let id = r.id.to_string();
                up.execute(params![
                    id,
                    r.objeto,
                    r.importe,
                    parse_data(&r.publicado),
                    org_id,
                    r.duracion,
                    now,
                ])?;
                del_res.execute(params![id])?;
                ins_res.execute(params![id, r.adjudicatario, r.nif, r.importe])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Garda o detalle e substitúe as resolucións dun contrato.
    pub fn upsert_detail(&mut self, d: &ContractDetail, resolucions: &[Resolucion]) -> Result<()> {
        let extra_json = serde_json::to_string(&d.extra).unwrap_or_else(|_| "{}".to_string());
        let tx = self.conn.transaction()?;
        // Garantir que existe a fila do contrato (evita violar a FK se aínda non
        // se gardou o resumo, p.ex. ao actualizar só o detalle).
        tx.execute(
            "INSERT OR IGNORE INTO contracts(id) VALUES(?1)",
            params![d.contract_id],
        )?;
        // Garantir que existen os nomes dos tipos (o código asígnase soa); os
        // baleiros ignóranse e a subconsulta resólveos a NULL.
        for (sql, nome) in [
            (
                "INSERT OR IGNORE INTO tipos_tramitacion (nome) VALUES (?1)",
                d.tipo_tramitacion.trim(),
            ),
            (
                "INSERT OR IGNORE INTO tipos_procedemento (nome) VALUES (?1)",
                d.tipo_procedemento.trim(),
            ),
            (
                "INSERT OR IGNORE INTO tipos_contrato (nome) VALUES (?1)",
                d.tipo_contrato.trim(),
            ),
        ] {
            if !nome.is_empty() {
                tx.execute(sql, params![nome])?;
            }
        }
        tx.execute(
            r#"INSERT INTO contract_detail
                (contract_id, referencia, obxecto, cod_tipo_tramitacion, cod_tipo_procedemento,
                 cod_tipo_contrato, orzamento_base, valor_estimado, num_lotes,
                 sistema_contratacion, observacions, data_difusion, sara, centralizada,
                 lei_aplicacion, enlace_resolucion, extra_json)
               VALUES (?1,?2,?3,
                 (SELECT cod_tipo_tramitacion FROM tipos_tramitacion WHERE nome=?4),
                 (SELECT cod_tipo_procedemento FROM tipos_procedemento WHERE nome=?5),
                 (SELECT cod_tipo_contrato FROM tipos_contrato WHERE nome=?6),
                 ?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
               ON CONFLICT(contract_id) DO UPDATE SET
                 referencia=excluded.referencia, obxecto=excluded.obxecto,
                 cod_tipo_tramitacion=excluded.cod_tipo_tramitacion,
                 cod_tipo_procedemento=excluded.cod_tipo_procedemento,
                 cod_tipo_contrato=excluded.cod_tipo_contrato,
                 orzamento_base=excluded.orzamento_base,
                 valor_estimado=excluded.valor_estimado, num_lotes=excluded.num_lotes,
                 sistema_contratacion=excluded.sistema_contratacion,
                 observacions=excluded.observacions, data_difusion=excluded.data_difusion,
                 sara=excluded.sara, centralizada=excluded.centralizada,
                 lei_aplicacion=excluded.lei_aplicacion,
                 enlace_resolucion=excluded.enlace_resolucion, extra_json=excluded.extra_json"#,
            params![
                d.contract_id,
                d.referencia,
                d.obxecto,
                d.tipo_tramitacion.trim(),
                d.tipo_procedemento.trim(),
                d.tipo_contrato.trim(),
                d.orzamento_base,
                d.valor_estimado,
                d.num_lotes,
                d.sistema_contratacion,
                d.observacions,
                d.data_difusion,
                d.sara,
                d.centralizada,
                d.lei_aplicacion,
                d.enlace_resolucion,
                extra_json,
            ],
        )?;
        tx.execute(
            "DELETE FROM contract_resolucion WHERE contract_id = ?1",
            params![d.contract_id],
        )?;
        {
            // Garantir que existe o nome do estado de resolución (o código
            // asígnase soa); os baleiros ignóranse e a subconsulta resólveos a NULL.
            let mut est_stmt =
                tx.prepare("INSERT OR IGNORE INTO estados_resolucion (nome) VALUES (?1)")?;
            for r in resolucions {
                if !r.estado_resolucion.trim().is_empty() {
                    est_stmt.execute(params![r.estado_resolucion.trim()])?;
                }
            }

            // `importe_resolucion_txt` non se garda: é só o renderizado de
            // `importe_resolucion_num` (derívase ao cargar con `format_importe`).
            let mut stmt = tx.prepare(
                r#"INSERT INTO contract_resolucion
                    (contract_id, lote, participacion, cod_estado_resolucion, adxudicatario,
                     nif, importe_resolucion_num, data_difusion, prazo_execucion, recurso)
                   VALUES (?1,?2,?3,
                     (SELECT cod_estado_resolucion FROM estados_resolucion WHERE nome=?4),
                     ?5,?6,?7,?8,?9,?10)"#,
            )?;
            for r in resolucions {
                stmt.execute(params![
                    d.contract_id,
                    r.lote,
                    r.participacion,
                    r.estado_resolucion.trim(),
                    r.adxudicatario,
                    r.nif,
                    r.importe_num,
                    r.data_difusion,
                    r.prazo_execucion,
                    r.recurso,
                ])?;
            }
        }
        tx.execute(
            "UPDATE contracts SET detalle_descargado = 1 WHERE id = ?1",
            params![d.contract_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_meta(&self, clave: &str, valor: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(clave,valor) VALUES(?1,?2)
             ON CONFLICT(clave) DO UPDATE SET valor=excluded.valor",
            params![clave, valor],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<DbStats> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM contracts", [], |r| r.get(0))?;
        let con_detalle: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM contracts WHERE detalle_descargado = 1",
            [],
            |r| r.get(0),
        )?;
        let ultima_sync: Option<String> = self
            .conn
            .query_row(
                "SELECT valor FROM meta WHERE clave = 'ultima_sync'",
                [],
                |r| r.get(0),
            )
            .ok();
        Ok(DbStats {
            total,
            con_detalle,
            ultima_sync,
        })
    }

    /// Consulta local con filtros, devolvendo **unha fila por contrato**. Os
    /// lotes/resolucións agréganse: a columna de adxudicatario lista os distintos
    /// adxudicatarios e a de importe de resolución amosa a suma adxudicada (o
    /// desglose por lote vese na vista de detalle).
    pub fn query_local(&self, f: &LocalFilters) -> Result<Vec<LocalRow>> {
        self.query_local_inner(f, None)
    }

    /// Como [`Self::query_local`] pero devolve só unha **páxina** do resultado
    /// (para a carga progresiva da táboa virtual): as filas `[offset, offset+limit)`
    /// na orde actual. Combínase con [`Self::count_local`] para coñecer o total.
    pub fn query_local_page(
        &self,
        f: &LocalFilters,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<LocalRow>> {
        self.query_local_inner(f, Some((offset, limit)))
    }

    /// Número total de contratos que casan cos filtros (sen traer as filas).
    pub fn count_local(&self, f: &LocalFilters) -> Result<usize> {
        let mut sql = String::from(
            "SELECT COUNT(*) FROM contracts c \
             LEFT JOIN organismos o ON o.cod_organismo = c.cod_organismo \
             WHERE c.tipo = ?",
        );
        let (where_sql, mut args) = local_where(f);
        args.insert(0, f.tipo.as_str().to_string());
        sql.push_str(&where_sql);
        let params: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let n: i64 = self.conn.query_row(&sql, params.as_slice(), |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Carga unha única fila do listado polo seu id (para saltar a un contrato
    /// concreto sen ter cargada a súa páxina).
    pub fn query_local_by_id(&self, id: &str) -> Result<Option<LocalRow>> {
        let sql = format!("{LOCAL_SELECT} WHERE c.id = ?1");
        let row = self.conn.query_row(&sql, params![id], map_local_row).ok();
        Ok(row)
    }

    fn query_local_inner(
        &self,
        f: &LocalFilters,
        page: Option<(usize, usize)>,
    ) -> Result<Vec<LocalRow>> {
        let mut sql = format!("{LOCAL_SELECT} WHERE c.tipo = ?");
        let (where_sql, mut args) = local_where(f);
        // O primeiro parámetro posicional é o tipo (sub-pestana activa).
        args.insert(0, f.tipo.as_str().to_string());
        sql.push_str(&where_sql);
        // Orde escollida na cabeceira (expresión fixa, sen entrada do usuario);
        // os valores baleiros/NULL van ao final, e o id (numérico) desempata de
        // xeito estable.
        let dir = if f.sort_asc { "ASC" } else { "DESC" };
        sql.push_str(&format!(
            " ORDER BY {} {dir} NULLS LAST, CAST(c.id AS INTEGER) DESC",
            f.sort_col.order_sql()
        ));
        // Paxinación opcional: a táboa virtual carga por chuncos.
        let (limit, offset) = page.map(|(o, l)| (l as i64, o as i64)).unwrap_or((-1, 0));
        if page.is_some() {
            sql.push_str(" LIMIT ? OFFSET ?");
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        if page.is_some() {
            params.push(&limit);
            params.push(&offset);
        }
        let rows = stmt.query_map(params.as_slice(), map_local_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Carga o detalle textual e as resolucións dun contrato para o panel de detalle.
    pub fn load_detail(&self, id: &str) -> Result<Option<(ContractDetail, Vec<Resolucion>)>> {
        let detail = self
            .conn
            .query_row(
                r#"SELECT d.referencia, d.obxecto,
                          COALESCE(tt.nome,''), COALESCE(tp.nome,''), COALESCE(tc.nome,''),
                          d.orzamento_base, d.valor_estimado, d.num_lotes,
                          d.sistema_contratacion, d.observacions, d.data_difusion, d.sara,
                          d.centralizada, d.lei_aplicacion, d.enlace_resolucion, d.extra_json
                   FROM contract_detail d
                   LEFT JOIN tipos_tramitacion  tt ON tt.cod_tipo_tramitacion  = d.cod_tipo_tramitacion
                   LEFT JOIN tipos_procedemento tp ON tp.cod_tipo_procedemento = d.cod_tipo_procedemento
                   LEFT JOIN tipos_contrato     tc ON tc.cod_tipo_contrato     = d.cod_tipo_contrato
                   WHERE d.contract_id = ?1"#,
                params![id],
                |row| {
                    let extra_json: String = row.get(15)?;
                    Ok(ContractDetail {
                        contract_id: id.to_string(),
                        referencia: row.get(0)?,
                        obxecto: row.get(1)?,
                        tipo_tramitacion: row.get(2)?,
                        tipo_procedemento: row.get(3)?,
                        tipo_contrato: row.get(4)?,
                        orzamento_base: row.get(5)?,
                        valor_estimado: row.get(6)?,
                        num_lotes: row.get(7)?,
                        sistema_contratacion: row.get(8)?,
                        observacions: row.get(9)?,
                        data_difusion: row.get(10)?,
                        sara: row.get(11)?,
                        centralizada: row.get(12)?,
                        lei_aplicacion: row.get(13)?,
                        enlace_resolucion: row.get(14)?,
                        extra: serde_json::from_str(&extra_json).unwrap_or_default(),
                    })
                },
            )
            .ok();

        let Some(detail) = detail else {
            return Ok(None);
        };

        let mut stmt = self.conn.prepare(
            r#"SELECT r.lote, r.participacion, COALESCE(er.nome,''), r.adxudicatario,
                      COALESCE(r.nif,''), r.importe_resolucion_num, r.data_difusion,
                      r.prazo_execucion, r.recurso
               FROM contract_resolucion r
               LEFT JOIN estados_resolucion er
                      ON er.cod_estado_resolucion = r.cod_estado_resolucion
               WHERE r.contract_id = ?1"#,
        )?;
        let res = stmt.query_map(params![id], |row| {
            let importe_num: Option<f64> = row.get(5)?;
            Ok(Resolucion {
                lote: row.get(0)?,
                participacion: row.get(1)?,
                estado_resolucion: row.get(2)?,
                adxudicatario: row.get(3)?,
                nif: row.get(4)?,
                importe_num,
                // `importe_txt` é o renderizado de `importe_num` (a columna de
                // texto eliminouse da táboa).
                importe_txt: importe_num.map(format_importe).unwrap_or_default(),
                data_difusion: row.get(6)?,
                prazo_execucion: row.get(7)?,
                recurso: row.get(8)?,
            })
        })?;
        let mut resolucions = Vec::new();
        for r in res {
            resolucions.push(r?);
        }
        Ok(Some((detail, resolucions)))
    }

    /// Valores distintos da BD para poboar os despregables da busca local.
    pub fn local_options(&self) -> Result<LocalOptions> {
        let adxudicatarios = self.distinct(
            "SELECT DISTINCT adxudicatario FROM contract_resolucion \
             WHERE TRIM(COALESCE(adxudicatario,'')) <> '' ORDER BY adxudicatario COLLATE NOCASE",
        )?;
        let organismos = self.distinct(
            "SELECT nome FROM organismos \
             WHERE TRIM(COALESCE(nome,'')) <> '' ORDER BY nome COLLATE NOCASE",
        )?;
        let datas = self.distinct(
            "SELECT DISTINCT data_publicacion FROM contracts \
             WHERE TRIM(COALESCE(data_publicacion,'')) <> ''",
        )?;
        let mut anos: Vec<String> = datas
            .iter()
            .filter_map(|d| extract_year(d))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        anos.sort_unstable_by(|a, b| b.cmp(a)); // descendente
        Ok(LocalOptions {
            adxudicatarios,
            organismos,
            anos,
        })
    }

    /// Executa unha consulta que devolve unha única columna de texto.
    fn distinct(&self, sql: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ───────────────────── UTEs e relacións ───────────────────────────────────

    /// Substitúe a composición das UTE dun contrato. Bórraa se a lista é baleira.
    pub fn upsert_utes(&mut self, contract_id: &str, utes: &[Ute]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM ute_membro WHERE contract_id = ?1",
            params![contract_id],
        )?;
        {
            let mut stmt = tx.prepare(
                r#"INSERT OR IGNORE INTO ute_membro
                    (contract_id, ute_key, ute_nome, ute_nif, membro_cif, membro_nome)
                   VALUES (?1,?2,?3,?4,?5,?6)"#,
            )?;
            for u in utes {
                let key = company_key(&u.nome);
                for m in &u.membros {
                    stmt.execute(params![contract_id, key, u.nome, u.nif, m.cif, m.nome])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Membros das UTE adxudicatarias dun contrato: `(ute_nome, membro_nome,
    /// membro_cif)`, para reflectir a composición na ficha do contrato.
    pub fn ute_membros_de_contrato(
        &self,
        contract_id: &str,
    ) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT ute_nome, membro_nome, membro_cif
               FROM ute_membro WHERE contract_id = ?1
               ORDER BY ute_nome COLLATE NOCASE, membro_nome COLLATE NOCASE"#,
        )?;
        let out = stmt
            .query_map(params![contract_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Unha páxina (chunco) do listado de empresas adxudicatarias dos contratos
    /// que casan cos `filtros` (os mesmos do panel lateral), acoutada a
    /// `[offset, offset+limit)`. Cada empresa agrega o número de contratos distintos
    /// e a suma dos importes adxudicados; identifícase polo seu NIF ou, se non se
    /// coñece, pola clave normalizada do nome (`cokey`). Considera os dous tipos de
    /// contrato (licitacións e menores), igual ca a vista de relacións, e ordénase
    /// por importe total descendente. Úsaa a táboa virtual da pestana Empresas para
    /// cargar progresivamente sen manter a lista enteira en memoria.
    pub fn query_empresas_page(
        &self,
        filtros: &LocalFilters,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<EmpresaContratos>> {
        self.empresas_inner(filtros, Some((offset, limit)))
    }

    /// Listado de empresas adxudicatarias agregadas, opcionalmente paxinado
    /// (`page = Some((offset, limit))`). Sen `page` devolve todas as filas.
    fn empresas_inner(
        &self,
        filtros: &LocalFilters,
        page: Option<(usize, usize)>,
    ) -> Result<Vec<EmpresaContratos>> {
        let (where_sql, args) = local_where(filtros);
        let filtrados = filtrados_cte(&where_sql);
        // Unha empresa por clave (`ekey` = NIF, ou clave do nome se non hai NIF).
        // O nome e o NIF amosados son representativos (MAX) por se a mesma empresa
        // aparece con grafías distintas. Só contan as resolucións con adxudicatario.
        // Orde dinámica segundo a columna premida na cabeceira. `ekey` final como
        // desempate estable: a táboa virtual paxina e, sen orde total determinista, as
        // filas poderían duplicarse/saltarse entre chuncos (igual ca o `id` de contratos).
        let dir = if filtros.empresas_sort_asc {
            "ASC"
        } else {
            "DESC"
        };
        let mut sql = format!(
            "WITH {filtrados} \
             SELECT {EMPRESAS_EKEY} AS ekey, \
                    MAX(TRIM(r.adxudicatario)) AS nome, \
                    MAX(COALESCE(TRIM(r.nif),'')) AS nif, \
                    COUNT(DISTINCT r.contract_id) AS num_contratos, \
                    SUM(r.importe_resolucion_num) AS importe_total \
             {EMPRESAS_GROUP} \
             ORDER BY {} {dir} NULLS LAST, ekey",
            filtros.empresas_sort_col.order_sql()
        );
        // Paxinación opcional: a táboa virtual carga por chuncos.
        let (limit, offset) = page.map(|(o, l)| (l as i64, o as i64)).unwrap_or((-1, 0));
        if page.is_some() {
            sql.push_str(" LIMIT ? OFFSET ?");
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        if page.is_some() {
            params.push(&limit);
            params.push(&offset);
        }
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(EmpresaContratos {
                nome: row.get::<_, String>(1)?,
                nif: row.get::<_, String>(2)?,
                num_contratos: row.get::<_, i64>(3)?,
                importe_total: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Resumo da agregación de empresas: nº de empresas distintas e importe total
    /// adxudicado a todas elas, para os contratos que casan cos `filtros`. Calcúlao
    /// nunha soa consulta (subconsulta sobre o mesmo grupo) en vez de sumar as filas
    /// en memoria, de xeito que o resumo sexa correcto aínda que o listado se cargue
    /// paxinado.
    pub fn empresas_resumo(&self, filtros: &LocalFilters) -> Result<(usize, f64)> {
        let (where_sql, args) = local_where(filtros);
        let params: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let filtrados = filtrados_cte(&where_sql);
        let sql = format!(
            "WITH {filtrados} \
             SELECT COUNT(*), COALESCE(SUM(importe_total),0) FROM ( \
                SELECT {EMPRESAS_EKEY} AS ekey, \
                       SUM(r.importe_resolucion_num) AS importe_total \
                {EMPRESAS_GROUP} \
             )"
        );
        let (n, total): (i64, f64) = self
            .conn
            .query_row(&sql, params.as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((n.max(0) as usize, total))
    }

    /// Grupos (tramas) de razóns sociais que concorreron xuntas nunha mesma UTE
    /// adxudicataria. Cada grupo é unha compoñente conexa do grafo empresa↔empresa
    /// onde a aresta é a coparticipación nunha UTE. Constrúese só con datos de
    /// contratosdegalicia.gal (a táboa `ute_membro`), sen fontes externas.
    ///
    /// Os `filtros` (os mesmos do panel lateral) limitan os contratos tidos en
    /// conta, de xeito que a vista de relacións reflicte o listado actual.
    pub fn relacions_compartidas(&self, filtros: &LocalFilters) -> Result<Vec<GrupoRelacion>> {
        use std::collections::{HashMap, HashSet};
        let (where_sql, args) = local_where(filtros);
        let params = || -> Vec<&dyn rusqlite::ToSql> {
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect()
        };
        let filtrados = filtrados_cte(&where_sql);
        // Composición das UTE dos contratos filtrados, co importe total da UTE
        // (suma das resolucións cuxo adxudicatario casa coa UTE vía cokey) e os
        // datos do contrato. Unha fila por membro; a clave do nó empresa é o CIF
        // do membro (ou a clave normalizada do nome se non hai CIF).
        let sql = format!(
            "WITH {filtrados}, \
             ute_imp AS (SELECT r.contract_id AS cid, cokey(r.adxudicatario) AS k, \
                                SUM(r.importe_resolucion_num) AS imp \
                         FROM contract_resolucion r GROUP BY r.contract_id, cokey(r.adxudicatario)) \
             SELECT um.contract_id, um.ute_key, um.ute_nome, \
                    COALESCE(NULLIF(TRIM(um.membro_cif),''), cokey(um.membro_nome)) AS ekey, \
                    um.membro_nome, COALESCE(c.asunto,''), COALESCE(c.data_publicacion,''), ui.imp \
             FROM ute_membro um \
             JOIN contracts c ON c.id = um.contract_id \
             LEFT JOIN ute_imp ui ON ui.cid = um.contract_id AND ui.k = um.ute_key \
             WHERE um.contract_id IN (SELECT id FROM filtrados) \
             ORDER BY um.contract_id, um.ute_key"
        );
        struct UteGrupo {
            contract_id: String,
            nome: String,
            asunto: String,
            data_iso: String,
            importe: f64,
            membros: Vec<(String, String)>, // (clave_empresa, nome)
        }
        let mut utes: Vec<UteGrupo> = Vec::new();
        {
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query(params().as_slice())?;
            let mut actual: Option<(String, String)> = None;
            while let Some(row) = rows.next()? {
                let cid: String = row.get(0)?;
                let ukey: String = row.get(1)?;
                if actual.as_ref() != Some(&(cid.clone(), ukey.clone())) {
                    actual = Some((cid.clone(), ukey.clone()));
                    utes.push(UteGrupo {
                        contract_id: cid,
                        nome: row.get(2)?,
                        asunto: row.get(5)?,
                        data_iso: row.get(6)?,
                        importe: row.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                        membros: Vec::new(),
                    });
                }
                let ekey: String = row.get(3)?;
                let mnome: String = row.get(4)?;
                utes.last_mut().unwrap().membros.push((ekey, mnome));
            }
        }

        // Só as UTE con ≥2 empresas membro distintas forman trama.
        utes.retain(|u| {
            let distintos: HashSet<&String> = u.membros.iter().map(|(k, _)| k).collect();
            distintos.len() >= 2
        });

        // Metadatos das empresas (nome representativo e nº de contratos distintos).
        let mut emp_nome: HashMap<String, String> = HashMap::new();
        let mut emp_contratos: HashMap<String, HashSet<String>> = HashMap::new();
        for u in &utes {
            for (ekey, nome) in &u.membros {
                emp_nome.entry(ekey.clone()).or_insert_with(|| nome.clone());
                emp_contratos
                    .entry(ekey.clone())
                    .or_default()
                    .insert(u.contract_id.clone());
            }
        }

        // Union-find sobre as empresas (nós = clave de empresa).
        let mut idx: HashMap<String, usize> = HashMap::new();
        let mut key_of = |k: &str| -> usize {
            let n = idx.len();
            *idx.entry(k.to_string()).or_insert(n)
        };
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for u in &utes {
            let base = key_of(&u.membros[0].0);
            for (ekey, _) in &u.membros[1..] {
                let e = key_of(ekey);
                edges.push((base, e));
            }
        }
        let mut parent: Vec<usize> = (0..idx.len()).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for &(a, b) in &edges {
            let ra = find(&mut parent, a);
            let rb = find(&mut parent, b);
            if ra != rb {
                parent[ra] = rb;
            }
        }

        #[derive(Default)]
        struct Build {
            empresas: HashMap<String, EmpresaNodo>,
            utes: Vec<UteRelacion>,
            contratos: Vec<ContratoAdxudicado>,
            contratos_vistos: HashSet<String>,
        }
        let mut grupos: HashMap<usize, Build> = HashMap::new();

        // Empresas: un nó por clave de empresa.
        for (ekey, &node) in &idx {
            let root = find(&mut parent, node);
            let nome = emp_nome.get(ekey).cloned().unwrap_or_else(|| ekey.clone());
            let num_contratos = emp_contratos.get(ekey).map(|s| s.len() as i64).unwrap_or(0);
            grupos.entry(root).or_default().empresas.insert(
                ekey.clone(),
                EmpresaNodo {
                    cif: ekey.clone(),
                    empresa_nome: nome,
                    num_contratos,
                },
            );
        }

        // UTE e contratos atribuídos ao grupo (un contrato unha soa vez por grupo).
        for u in &utes {
            let root = find(&mut parent, idx[&u.membros[0].0]);
            let b = grupos.entry(root).or_default();
            b.utes.push(UteRelacion {
                nome: u.nome.clone(),
                membros: u.membros.iter().map(|(_, n)| n.clone()).collect(),
            });
            if b.contratos_vistos.insert(u.contract_id.clone()) {
                b.contratos.push(ContratoAdxudicado {
                    contract_id: u.contract_id.clone(),
                    empresa_nome: u.nome.clone(),
                    asunto: u.asunto.clone(),
                    publicacion: format_data_gl(&u.data_iso),
                    importe_num: u.importe,
                    importe_txt: format_importe(u.importe),
                });
            }
        }

        // Materializar e ordenar (as tramas máis grandes primeiro).
        let mut out: Vec<GrupoRelacion> = grupos
            .into_values()
            .map(|b| {
                let mut empresas: Vec<EmpresaNodo> = b.empresas.into_values().collect();
                empresas.sort_by(|a, b| {
                    a.empresa_nome
                        .to_lowercase()
                        .cmp(&b.empresa_nome.to_lowercase())
                });
                let importe_total = b.contratos.iter().map(|c| c.importe_num).sum();
                GrupoRelacion {
                    empresas,
                    utes: b.utes,
                    contratos: b.contratos,
                    importe_total,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            let contratos =
                |g: &GrupoRelacion| g.empresas.iter().map(|e| e.num_contratos).sum::<i64>();
            b.empresas
                .len()
                .cmp(&a.empresas.len())
                .then_with(|| contratos(b).cmp(&contratos(a)))
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        id: &str,
        asunto: &str,
        organismo: &str,
        estado: &str,
        pub_: &str,
    ) -> ContractSummary {
        ContractSummary {
            id: id.into(),
            tipo: TipoContrato::Licitacion,
            referencia: format!("R{id}"),
            asunto: asunto.into(),
            importe: String::new(),
            estado: estado.into(),
            publicacion: pub_.into(),
            // Código derivado do nome (basta con que sexa estable e non baleiro
            // para que o JOIN con `organismos` devolva o nome).
            cod_organismo: format!("OR-{organismo}"),
            organismo: organismo.into(),
        }
    }

    // Carga progresiva: `count_local` + `query_local_page` deben repartir o mesmo
    // resultado (e na mesma orde) ca `query_local`, e `query_local_by_id` traer un.
    #[test]
    fn conta_e_paxina_o_listado_local() {
        let path = std::env::temp_dir().join("congal_test_paxina.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");
        let filas: Vec<ContractSummary> = (1..=5)
            .map(|i| {
                summary(
                    &i.to_string(),
                    &format!("obxecto {i}"),
                    "Org",
                    "Estado",
                    "0{i}-01-2025",
                )
            })
            .collect();
        db.upsert_summaries(&filas, "agora").expect("upsert");

        let f = LocalFilters::default();
        assert_eq!(db.count_local(&f).expect("count"), 5);

        let todo = db.query_local(&f).expect("todo");
        assert_eq!(todo.len(), 5);

        // As páxinas concatenadas reproducen a lista completa na mesma orde.
        let p1 = db.query_local_page(&f, 0, 2).expect("p1");
        let p2 = db.query_local_page(&f, 2, 2).expect("p2");
        let p3 = db.query_local_page(&f, 4, 2).expect("p3");
        assert_eq!(p1.len(), 2);
        assert_eq!(p2.len(), 2);
        assert_eq!(p3.len(), 1); // última páxina parcial
        let ids_paxinas: Vec<String> = p1
            .iter()
            .chain(&p2)
            .chain(&p3)
            .map(|r| r.id.clone())
            .collect();
        let ids_todo: Vec<String> = todo.iter().map(|r| r.id.clone()).collect();
        assert_eq!(ids_paxinas, ids_todo);

        assert!(db.query_local_by_id("3").expect("by id").is_some());
        assert!(db.query_local_by_id("nope").expect("by id").is_none());

        let _ = std::fs::remove_file(&path);
    }

    // Reproduce o fallo de nrm(NULL): un contrato sen resolución deixa
    // `adxudicatario` a NULL no LEFT JOIN; antes a consulta enteira fallaba.
    #[test]
    fn busca_por_adxudicatario_con_contratos_sen_resolucion() {
        let path = std::env::temp_dir().join("congal_test_nrm.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        db.upsert_summaries(
            &[
                summary("1", "obra", "Concello", "Adxudicado", "01/02/2025"),
                summary("2", "servizo", "Deputación", "Pendente", "03/04/2024"),
            ],
            "agora",
        )
        .expect("upsert summaries");

        // Só o contrato 1 ten adxudicatario; o 2 queda con adxudicatario NULL.
        let detail = ContractDetail {
            contract_id: "1".into(),
            ..Default::default()
        };
        let res = [Resolucion {
            adxudicatario: "Empresa Técnica SL".into(),
            ..Default::default()
        }];
        db.upsert_detail(&detail, &res).expect("upsert detail");

        // Busca insensible a acentos sobre adxudicatario, ignorando os NULL.
        let f = LocalFilters {
            adxudicatario: "tecnica".into(),
            ..Default::default()
        };
        let rows = db
            .query_local(&f)
            .expect("a consulta non debe fallar con adxudicatario NULL");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "1");

        let _ = std::fs::remove_file(&path);
    }

    // A vista de empresas agrupa por NIF, conta os contratos distintos e suma os
    // importes adxudicados a cada empresa, ordenando por importe descendente.
    #[test]
    fn empresas_agrega_contratos_e_importes() {
        let path = std::env::temp_dir().join("congal_test_empresas.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");
        db.upsert_summaries(
            &[
                summary("1", "a", "Org", "Adxudicado", "01/02/2025"),
                summary("2", "b", "Org", "Adxudicado", "02/02/2025"),
            ],
            "agora",
        )
        .expect("sum");
        // Empresa A adxudicataria nos dous contratos (mesmo NIF, distinta grafía).
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "Empresa A SL".into(),
                nif: "B111".into(),
                importe_num: Some(1000.0),
                ..Default::default()
            }],
        )
        .expect("d1");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "2".into(),
                ..Default::default()
            },
            &[
                Resolucion {
                    adxudicatario: "EMPRESA A, S.L.".into(),
                    nif: "B111".into(),
                    importe_num: Some(500.0),
                    ..Default::default()
                },
                Resolucion {
                    adxudicatario: "Empresa B SL".into(),
                    nif: "B222".into(),
                    importe_num: Some(200.0),
                    ..Default::default()
                },
            ],
        )
        .expect("d2");

        let empresas = db
            .query_empresas_page(&LocalFilters::default(), 0, 1000)
            .expect("empresas");
        assert_eq!(empresas.len(), 2);
        // Orde por importe total descendente: A (1500) antes de B (200).
        assert_eq!(empresas[0].nif, "B111");
        assert_eq!(empresas[0].num_contratos, 2);
        assert_eq!(empresas[0].importe_total, 1500.0);
        assert_eq!(empresas[1].nif, "B222");
        assert_eq!(empresas[1].num_contratos, 1);
        assert_eq!(empresas[1].importe_total, 200.0);

        // O resumo (nº de empresas + importe total) cóntase na BD sobre toda a busca.
        let (n, total) = db
            .empresas_resumo(&LocalFilters::default())
            .expect("resumo");
        assert_eq!(n, 2);
        assert_eq!(total, 1700.0);

        // Paxinación: cada páxina devolve as filas na mesma orde estable.
        let pax0 = db
            .query_empresas_page(&LocalFilters::default(), 0, 1)
            .expect("pax0");
        let pax1 = db
            .query_empresas_page(&LocalFilters::default(), 1, 1)
            .expect("pax1");
        assert_eq!(pax0.len(), 1);
        assert_eq!(pax1.len(), 1);
        assert_eq!(pax0[0].nif, "B111");
        assert_eq!(pax1[0].nif, "B222");

        // O filtro por organismo (panel lateral) aplícase tamén a esta vista.
        let f = LocalFilters {
            organismo: "inexistente".into(),
            ..Default::default()
        };
        assert!(
            db.query_empresas_page(&f, 0, 1000)
                .expect("empresas filtr")
                .is_empty(),
            "un filtro que non casa con ningún contrato non devolve empresas"
        );
        assert_eq!(db.empresas_resumo(&f).expect("resumo filtr"), (0, 0.0));

        let _ = std::fs::remove_file(&path);
    }

    // Só se marca o contrato no que TODOS os lotes constan cun único participante.
    #[test]
    fn marca_contratos_de_participante_unico() {
        let path = std::env::temp_dir().join("congal_test_part_unico.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");
        db.upsert_summaries(
            &[
                summary("1", "a", "Concello", "Adxudicado", "01/02/2025"),
                summary("2", "b", "Concello", "Adxudicado", "01/02/2025"),
                summary("3", "c", "Concello", "Adxudicado", "01/02/2025"),
                summary("4", "d", "Concello", "Adxudicado", "01/02/2025"),
            ],
            "agora",
        )
        .expect("sum");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                participacion: "1".into(),
                adxudicatario: "X SL".into(),
                ..Default::default()
            }],
        )
        .expect("d1");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "2".into(),
                ..Default::default()
            },
            &[Resolucion {
                participacion: "3".into(),
                adxudicatario: "Y SL".into(),
                ..Default::default()
            }],
        )
        .expect("d2");
        // O contrato 4 ten dous lotes: un cun único participante e outro con
        // participación descoñecida (baleira). Non debe marcarse.
        db.upsert_detail(
            &ContractDetail {
                contract_id: "4".into(),
                ..Default::default()
            },
            &[
                Resolucion {
                    participacion: "1".into(),
                    adxudicatario: "Z SL".into(),
                    ..Default::default()
                },
                Resolucion {
                    participacion: "".into(),
                    adxudicatario: "W SL".into(),
                    ..Default::default()
                },
            ],
        )
        .expect("d4");
        // O contrato 3 queda sen detalle (sen resolución).

        let rows = db.query_local(&LocalFilters::default()).expect("query");
        let unico = |id: &str| rows.iter().find(|r| r.id == id).unwrap().participante_unico;
        assert!(unico("1"), "1 participante → marcado");
        assert!(!unico("2"), "3 participantes → non marcado");
        assert!(!unico("3"), "sen resolución → non marcado");
        assert!(
            !unico("4"),
            "lote con participación descoñecida → non marcado"
        );

        let _ = std::fs::remove_file(&path);
    }

    // O listado ordénase pola columna escollida: o ID por valor numérico (non
    // lexicográfico) e a data cronoloxicamente (orde ISO, non a visual).
    #[test]
    fn ordena_o_listado_por_columna() {
        use crate::model::SortColumn;
        let path = std::env::temp_dir().join("congal_test_sort.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");
        db.upsert_summaries(
            &[
                summary("10", "a", "Org", "Adxudicado", "01/02/2025"),
                summary("2", "b", "Org", "Adxudicado", "03/02/2025"),
                summary("100", "c", "Org", "Adxudicado", "02/02/2025"),
            ],
            "agora",
        )
        .expect("sum");
        let ids = |f: &LocalFilters| -> Vec<String> {
            db.query_local(f)
                .unwrap()
                .into_iter()
                .map(|r| r.id)
                .collect()
        };

        let mut f = LocalFilters {
            sort_col: SortColumn::Id,
            sort_asc: true,
            ..Default::default()
        };
        assert_eq!(ids(&f), ["2", "10", "100"], "ID ascendente numérico");
        f.sort_asc = false;
        assert_eq!(ids(&f), ["100", "10", "2"], "ID descendente numérico");

        f.sort_col = SortColumn::Data;
        f.sort_asc = true;
        assert_eq!(
            ids(&f),
            ["10", "100", "2"],
            "data cronolóxica (01/02, 02/02, 03/02)"
        );

        let _ = std::fs::remove_file(&path);
    }

    // Tras normalizar: o nome do organismo cárgase vía JOIN, a data gárdase en
    // ISO e amósase como DD/MM/YYYY, e o importe da fila vén de `importe_num`.
    #[test]
    fn normalizacion_organismo_data_e_importe() {
        let path = std::env::temp_dir().join("congal_test_norm.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        let s = ContractSummary {
            id: "100".into(),
            tipo: TipoContrato::Licitacion,
            referencia: "R100".into(),
            asunto: "obra".into(),
            importe: "1.234,56 €".into(),
            estado: "Adxudicado".into(),
            publicacion: "15-03-2025".into(),
            cod_organismo: "ORG1".into(),
            organismo: "Concello da Coruña".into(),
        };
        db.upsert_summaries(&[s], "2025-03-16 10:00:00")
            .expect("upsert");

        // O organismo quedou na súa táboa e aparece nas opcións locais.
        let opts = db.local_options().expect("options");
        assert_eq!(opts.organismos, vec!["Concello da Coruña".to_string()]);

        let rows = db.query_local(&LocalFilters::default()).expect("query");
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.organismo, "Concello da Coruña"); // vía JOIN
        assert_eq!(r.publicacion, "15/03/2025"); // ISO -> presentación
        assert_eq!(r.importe_txt, "1.234,56 €"); // formatado desde importe_num

        // O filtro por organismo (insensible a acentos) atopa o contrato.
        let f = LocalFilters {
            organismo: "coruna".into(),
            ..Default::default()
        };
        assert_eq!(db.query_local(&f).expect("query org").len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    // Tras normalizar o estado: gárdase nunha táboa propia (un `cod_estado` por
    // nome distinto), o nome cárgase vía JOIN, aparece nas opcións locais e o
    // filtro por estado segue a funcionar. `estado_previo` segue devolvendo o
    // texto (do que depende a sincronización incremental).
    #[test]
    fn normalizacion_estado() {
        let path = std::env::temp_dir().join("congal_test_estado.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        db.upsert_summaries(
            &[
                summary("1", "obra", "Concello", "Formalizado", "01/02/2025"),
                summary(
                    "2",
                    "servizo",
                    "Deputación",
                    "Pendente de adxudicar",
                    "03/04/2024",
                ),
                // Mesmo estado que o 1: comparte `cod_estado`, non duplica fila.
                summary(
                    "3",
                    "subministración",
                    "Concello",
                    "Formalizado",
                    "05/06/2025",
                ),
            ],
            "agora",
        )
        .expect("upsert summaries");

        // O nome do estado cárgase vía JOIN na consulta local.
        let rows = db.query_local(&LocalFilters::default()).expect("query");
        let r1 = rows.iter().find(|r| r.id == "1").expect("fila 1");
        assert_eq!(r1.estado, "Formalizado");

        // `estado_previo` devolve o texto (úsao a sync para `is_estado_terminal`).
        assert_eq!(
            db.estado_previo("2").expect("previo").as_deref(),
            Some("Pendente de adxudicar")
        );
        assert_eq!(db.estado_previo("descoñecido").expect("previo"), None);

        let _ = std::fs::remove_file(&path);
    }

    // Unha UTE adxudicataria relaciona as súas empresas membro; o contrato e o
    // importe cóntanse unha soa vez. Constrúese só con datos do contrato.
    #[test]
    fn ute_relaciona_os_membros() {
        use crate::model::{Ute, UteMembro};
        let path = std::env::temp_dir().join("congal_test_ute.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        db.upsert_summaries(
            &[summary(
                "1",
                "obra",
                "Concello",
                "Formalizado",
                "01/02/2025",
            )],
            "agora",
        )
        .expect("summaries");
        // O adxudicatario é a UTE.
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "UTE A - B".into(),
                importe_num: Some(1000.0),
                ..Default::default()
            }],
        )
        .expect("detail");
        // Composición da UTE (dato do propio contrato, sen fontes externas).
        db.upsert_utes(
            "1",
            &[Ute {
                nome: "UTE A - B".into(),
                nif: "U12345678".into(),
                membros: vec![
                    UteMembro {
                        cif: "A11111111".into(),
                        nome: "EMPRESA A".into(),
                    },
                    UteMembro {
                        cif: "B22222222".into(),
                        nome: "EMPRESA B".into(),
                    },
                ],
            }],
        )
        .expect("utes");

        let rel = db
            .relacions_compartidas(&LocalFilters::default())
            .expect("rel");
        assert_eq!(rel.len(), 1, "un grupo coa UTE");
        let g = &rel[0];
        assert_eq!(g.empresas.len(), 2, "os dous membros");
        assert_eq!(g.utes.len(), 1);
        assert!(g.utes[0].nome.contains("UTE A"));
        assert_eq!(g.utes[0].membros.len(), 2);
        // O contrato e o importe cóntanse unha soa vez (non por cada membro).
        assert_eq!(g.contratos.len(), 1);
        assert_eq!(g.importe_total, 1000.0);

        let _ = std::fs::remove_file(&path);
    }

    // Os filtros do panel lateral tamén limitan a vista de relacións: ao filtrar
    // por un ano, só contan as UTE dos contratos dese ano.
    #[test]
    fn filtros_aplicanse_a_relacions() {
        use crate::model::{Ute, UteMembro};
        let path = std::env::temp_dir().join("congal_test_rel_filtros.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        // Dúas UTE adxudicatarias en anos distintos.
        db.upsert_summaries(
            &[
                summary("1", "obra", "Concello", "Formalizado", "01/02/2024"),
                summary("2", "servizo", "Concello", "Formalizado", "03/04/2025"),
            ],
            "agora",
        )
        .expect("summaries");
        for (cid, ute_nome, m1, m2) in [
            ("1", "UTE A - B", "EMPRESA A", "EMPRESA B"),
            ("2", "UTE C - D", "EMPRESA C", "EMPRESA D"),
        ] {
            db.upsert_detail(
                &ContractDetail {
                    contract_id: cid.into(),
                    ..Default::default()
                },
                &[Resolucion {
                    adxudicatario: ute_nome.into(),
                    importe_num: Some(100.0),
                    ..Default::default()
                }],
            )
            .expect("detail");
            db.upsert_utes(
                cid,
                &[Ute {
                    nome: ute_nome.into(),
                    nif: format!("U{cid}0000000"),
                    membros: vec![
                        UteMembro {
                            cif: format!("A{cid}111111"),
                            nome: m1.into(),
                        },
                        UteMembro {
                            cif: format!("B{cid}222222"),
                            nome: m2.into(),
                        },
                    ],
                }],
            )
            .expect("utes");
        }

        // Sen filtros: as dúas tramas (unha por UTE).
        let rel = db
            .relacions_compartidas(&LocalFilters::default())
            .expect("relacions");
        assert_eq!(rel.len(), 2, "sen filtros hai dúas tramas");

        // Filtrando por 2024 só conta a UTE do contrato dese ano.
        let filtros = LocalFilters {
            year: "2024".into(),
            ..Default::default()
        };
        let rel = db
            .relacions_compartidas(&filtros)
            .expect("relacions filtradas");
        assert_eq!(rel.len(), 1, "co filtro de ano só queda unha trama");
        assert_eq!(rel[0].empresas.len(), 2);

        let _ = std::fs::remove_file(&path);
    }

    // Un contrato con varios lotes debe aparecer UNHA soa vez na consulta local;
    // os adxudicatarios agréganse e os importes súmanse.
    #[test]
    fn contrato_con_varios_lotes_aparece_unha_vez() {
        let path = std::env::temp_dir().join("congal_test_lotes.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        db.upsert_summaries(
            &[summary(
                "10",
                "obra con lotes",
                "Concello",
                "Formalizado",
                "01/02/2025",
            )],
            "agora",
        )
        .expect("upsert summaries");

        let detail = ContractDetail {
            contract_id: "10".into(),
            ..Default::default()
        };
        let res = [
            Resolucion {
                lote: "1".into(),
                adxudicatario: "Empresa A SL".into(),
                importe_num: Some(1000.0),
                importe_txt: "1.000,00 €".into(),
                ..Default::default()
            },
            Resolucion {
                lote: "2".into(),
                adxudicatario: "Empresa B SL".into(),
                importe_num: Some(2500.5),
                importe_txt: "2.500,50 €".into(),
                ..Default::default()
            },
        ];
        db.upsert_detail(&detail, &res).expect("upsert detail");

        let rows = db.query_local(&LocalFilters::default()).expect("query");
        assert_eq!(rows.len(), 1, "o contrato debe aparecer unha soa vez");
        let r = &rows[0];
        assert_eq!(r.id, "10");
        // Os dous adxudicatarios distintos aparecen agregados.
        assert!(r.adxudicatario.contains("Empresa A SL"));
        assert!(r.adxudicatario.contains("Empresa B SL"));
        // Importe total = suma dos lotes, formatado en galego.
        assert_eq!(r.importe_resolucion_txt, "3.500,50 €");

        // Filtrar por un adxudicatario segue a devolver o contrato (unha vez).
        let f = LocalFilters {
            adxudicatario: "empresa b".into(),
            ..Default::default()
        };
        let rows = db.query_local(&f).expect("query filtrada");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "10");

        let _ = std::fs::remove_file(&path);
    }
}
