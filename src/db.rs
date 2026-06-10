//! Persistencia en SQLite (rusqlite, bundled).

use crate::model::{
    CargoRow, CasoRevision, ContractDetail, ContractSummary, ContratoAdxudicado, DatosCifEntidade,
    EmpresaNodo, GrupoRelacion, LocalFilters, LocalRow, PersoaNodo, Resolucion, Suggestion, Ute,
    UteRelacion, company_key, format_data_gl, format_importe, normalize_search, parse_data,
    parse_importe,
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
    pub estados: Vec<String>,
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
    if !f.estado.trim().is_empty() {
        sql.push_str(" AND nrm(e.nome) LIKE ?");
        args.push(format!("%{}%", normalize_search(&f.estado)));
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
        // (minúsculas, sen acentos nin puntuación), para unir os adxudicatarios
        // dos contratos coa táboa `adxudicatario_match`.
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
                referencia        TEXT,
                asunto            TEXT,
                importe_num       REAL,
                cod_estado        INTEGER REFERENCES estados(cod_estado),
                data_publicacion  TEXT,   -- ISO 8601 'YYYY-MM-DD'
                cod_organismo     TEXT REFERENCES organismos(cod_organismo),
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
            CREATE INDEX IF NOT EXISTS idx_contracts_estado ON contracts(cod_estado);
            CREATE INDEX IF NOT EXISTS idx_contracts_data  ON contracts(data_publicacion);
            CREATE INDEX IF NOT EXISTS idx_res_contract    ON contract_resolucion(contract_id);
            CREATE INDEX IF NOT EXISTS idx_res_adx         ON contract_resolucion(adxudicatario);

            CREATE TABLE IF NOT EXISTS meta (
                clave TEXT PRIMARY KEY,
                valor TEXT
            );

            -- ───────── Enriquecemento con datoscif.es ─────────
            -- Entidades (empresas e persoas) identificadas polo seu slug canónico.
            CREATE TABLE IF NOT EXISTS datoscif_entidade (
                url                TEXT PRIMARY KEY,   -- slug, p.ex. 'inditex-sa'
                nome               TEXT NOT NULL,
                tipo_entidad       INTEGER NOT NULL,   -- 1=empresa, 2=persoa
                uri                TEXT,
                cif                TEXT,               -- só empresas
                domicilio          TEXT,
                cod_postal         TEXT,
                municipio          TEXT,
                provincia          TEXT,
                cargos_descargados INTEGER NOT NULL DEFAULT 0,
                actualizado_en     TEXT
            );

            -- Cargos: relación persoa→empresa cun rol e vixencia.
            CREATE TABLE IF NOT EXISTS datoscif_cargo (
                empresa_url TEXT NOT NULL REFERENCES datoscif_entidade(url) ON DELETE CASCADE,
                persona_url TEXT NOT NULL REFERENCES datoscif_entidade(url) ON DELETE CASCADE,
                cargo       TEXT,
                activo      INTEGER,   -- 1 se segue vixente (sen data 'hasta')
                desde       TEXT,      -- ISO 8601
                hasta       TEXT,      -- ISO 8601
                PRIMARY KEY (empresa_url, persona_url, cargo, desde)
            );

            -- Vínculo entre o nome dun adxudicatario dos contratos e unha entidade
            -- de datoscif. A clave é o nome normalizado (company_key), que une cos
            -- contratos vía a función SQL cokey(adxudicatario).
            CREATE TABLE IF NOT EXISTS adxudicatario_match (
                adx_key        TEXT PRIMARY KEY,
                adx_nome       TEXT NOT NULL,
                datoscif_url   TEXT REFERENCES datoscif_entidade(url),  -- NULL se sen vínculo
                confianza      TEXT NOT NULL,
                estado         TEXT NOT NULL,
                actualizado_en TEXT
            );

            -- Composición das UTE adxudicatarias: unha fila por empresa membro.
            -- A resolución do membro a datoscif faise por CIF contra
            -- `datoscif_entidade.cif` (non se garda aquí).
            CREATE TABLE IF NOT EXISTS ute_membro (
                contract_id TEXT NOT NULL REFERENCES contracts(id) ON DELETE CASCADE,
                ute_key     TEXT NOT NULL,   -- company_key(nome da UTE)
                ute_nome    TEXT NOT NULL,
                ute_nif     TEXT,
                membro_cif  TEXT NOT NULL,
                membro_nome TEXT NOT NULL,
                PRIMARY KEY (contract_id, ute_key, membro_cif)
            );

            -- Candidatos de datoscif gardados para os casos en estado 'revisar',
            -- a escoller a man na pestana de revisión.
            CREATE TABLE IF NOT EXISTS adxudicatario_candidato (
                adx_key      TEXT NOT NULL,
                datoscif_url TEXT NOT NULL,
                nome         TEXT NOT NULL,
                tipo_entidad INTEGER NOT NULL,
                uri          TEXT,
                orde         INTEGER NOT NULL,
                PRIMARY KEY (adx_key, datoscif_url)
            );

            CREATE INDEX IF NOT EXISTS idx_cargo_persona ON datoscif_cargo(persona_url);
            CREATE INDEX IF NOT EXISTS idx_cargo_empresa ON datoscif_cargo(empresa_url);
            CREATE INDEX IF NOT EXISTS idx_match_url     ON adxudicatario_match(datoscif_url);
            "#,
        )?;
        // Migración aditiva para bases de datos que xa tiñan `datoscif_entidade`
        // sen as columnas da ficha. SQLite non ten "ADD COLUMN IF NOT EXISTS";
        // ignórase o erro de columna duplicada.
        for col in ["cif", "domicilio", "cod_postal", "municipio", "provincia"] {
            let _ = self.conn.execute(
                &format!("ALTER TABLE datoscif_entidade ADD COLUMN {col} TEXT"),
                [],
            );
        }
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
                    (id, referencia, asunto, importe_num, cod_estado,
                     data_publicacion, cod_organismo, actualizado_en)
                   VALUES (?1,?2,?3,?4,
                     (SELECT cod_estado FROM estados WHERE nome=?5),
                     ?6,?7,?8)
                   ON CONFLICT(id) DO UPDATE SET
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
        let mut sql = String::from(
            r#"SELECT c.id, COALESCE(c.referencia,''), COALESCE(c.asunto,''),
                      c.importe_num, COALESCE(e.nome,''),
                      COALESCE(c.data_publicacion,''), COALESCE(o.nome,''),
                      COALESCE(r.adxudicatarios,''), r.importe_total,
                      COALESCE(d.enlace_resolucion,''), r.max_part, r.min_part
               FROM contracts c
               LEFT JOIN organismos o ON o.cod_organismo = c.cod_organismo
               LEFT JOIN estados e ON e.cod_estado = c.cod_estado
               LEFT JOIN contract_detail d ON d.contract_id = c.id
               LEFT JOIN (
                   SELECT contract_id,
                          GROUP_CONCAT(DISTINCT NULLIF(TRIM(adxudicatario),'')) AS adxudicatarios,
                          SUM(importe_resolucion_num) AS importe_total,
                          MAX(CAST(participacion AS INTEGER)) AS max_part,
                          MIN(CAST(participacion AS INTEGER)) AS min_part
                   FROM contract_resolucion
                   GROUP BY contract_id
               ) r ON r.contract_id = c.id
               WHERE 1=1"#,
        );
        let (where_sql, args) = local_where(f);
        sql.push_str(&where_sql);
        // Orde escollida na cabeceira (expresión fixa, sen entrada do usuario);
        // os valores baleiros/NULL van ao final, e o id (numérico) desempata de
        // xeito estable.
        let dir = if f.sort_asc { "ASC" } else { "DESC" };
        sql.push_str(&format!(
            " ORDER BY {} {dir} NULLS LAST, CAST(c.id AS INTEGER) DESC LIMIT 5000",
            f.sort_col.order_sql()
        ));

        let mut stmt = self.conn.prepare(&sql)?;
        let params_dyn: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params_dyn.as_slice(), |row| {
            let importe_num: Option<f64> = row.get(3)?;
            let data_iso: String = row.get(5)?;
            let importe_total: Option<f64> = row.get(8)?;
            let max_part: Option<i64> = row.get(10)?;
            let min_part: Option<i64> = row.get(11)?;
            Ok(LocalRow {
                id: row.get(0)?,
                referencia: row.get(1)?,
                asunto: row.get(2)?,
                importe_txt: importe_num.map(format_importe).unwrap_or_default(),
                estado: row.get(4)?,
                publicacion: format_data_gl(&data_iso),
                organismo: row.get(6)?,
                adxudicatario: row.get(7)?,
                importe_resolucion_txt: importe_total.map(format_importe).unwrap_or_default(),
                enlace_resolucion: row.get(9)?,
                // Único participante só se TODOS os lotes constan cun único
                // participante: se algún ten participación descoñecida/baleira
                // (CAST → 0) ou maior, non se marca.
                participante_unico: max_part == Some(1) && min_part == Some(1),
            })
        })?;
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
        let estados = self.distinct(
            "SELECT nome FROM estados \
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
            estados,
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

    // ───────────────────── Enriquecemento con datoscif.es ─────────────────────

    /// Adxudicatarios distintos (por clave normalizada) que aínda non teñen
    /// ningún rexistro en `adxudicatario_match`; devólvese un nome representativo.
    pub fn adxudicatarios_pendentes(&self) -> Result<Vec<String>> {
        // Exclúense as UTE: non se buscan en datoscif (só os seus membros). Unha
        // UTE recoñécese porque foi decomposta en `ute_membro` ou porque o seu
        // NIF é de UTE (empeza por «U» seguido de díxito).
        self.distinct(
            "SELECT MIN(TRIM(adxudicatario)) FROM contract_resolucion \
             WHERE TRIM(COALESCE(adxudicatario,'')) <> '' \
               AND COALESCE(nif,'') NOT GLOB 'U[0-9]*' \
               AND cokey(adxudicatario) NOT IN (SELECT adx_key FROM adxudicatario_match) \
               AND cokey(adxudicatario) NOT IN (SELECT DISTINCT ute_key FROM ute_membro) \
             GROUP BY cokey(adxudicatario) \
             ORDER BY 1 COLLATE NOCASE",
        )
    }

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
        // Unha UTE non se empareja con datoscif: límpanse as filas que puidese ter
        // deixado un enrich anterior (antes de tela como UTE).
        {
            let mut del_m = tx.prepare("DELETE FROM adxudicatario_match WHERE adx_key = ?1")?;
            let mut del_c = tx.prepare("DELETE FROM adxudicatario_candidato WHERE adx_key = ?1")?;
            for u in utes {
                let key = company_key(&u.nome);
                del_m.execute(params![key])?;
                del_c.execute(params![key])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Membros de UTE aínda sen entidade de datoscif (a súa empresa non está
    /// gardada por CIF). Devolve `(membro_cif, membro_nome)` distintos.
    pub fn ute_membros_pendentes(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT membro_cif, MIN(membro_nome)
               FROM ute_membro
               WHERE membro_cif NOT IN
                     (SELECT cif FROM datoscif_entidade WHERE TRIM(COALESCE(cif,'')) <> '')
               GROUP BY membro_cif
               ORDER BY 2 COLLATE NOCASE"#,
        )?;
        let out = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// NIF/CIF coñecido dun adxudicatario (por clave normalizada do nome), se
    /// algunha resolución o trae. Úsao `enrich` para validar/desambiguar por CIF.
    pub fn nif_de_adxudicatario(&self, adx_nome: &str) -> Result<Option<String>> {
        let key = company_key(adx_nome);
        let nif = self
            .conn
            .query_row(
                "SELECT nif FROM contract_resolucion \
                 WHERE cokey(adxudicatario) = ?1 AND TRIM(COALESCE(nif,'')) <> '' \
                 LIMIT 1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .ok();
        Ok(nif.filter(|s| !s.trim().is_empty()))
    }

    /// Garda (ou actualiza) o resultado dun emparellamento. `datoscif_url` é
    /// `None` cando non houbo vínculo fiable.
    pub fn upsert_match(
        &self,
        adx_nome: &str,
        datoscif_url: Option<&str>,
        confianza: &str,
        estado: &str,
        now: &str,
    ) -> Result<()> {
        let key = company_key(adx_nome);
        self.conn.execute(
            r#"INSERT INTO adxudicatario_match
                (adx_key, adx_nome, datoscif_url, confianza, estado, actualizado_en)
               VALUES (?1,?2,?3,?4,?5,?6)
               ON CONFLICT(adx_key) DO UPDATE SET
                 adx_nome=excluded.adx_nome, datoscif_url=excluded.datoscif_url,
                 confianza=excluded.confianza, estado=excluded.estado,
                 actualizado_en=excluded.actualizado_en"#,
            params![key, adx_nome, datoscif_url, confianza, estado, now],
        )?;
        Ok(())
    }

    /// Substitúe os candidatos de revisión dun adxudicatario polos dados (na orde
    /// recibida). Bórraos se a lista está baleira.
    pub fn upsert_candidatos(&mut self, adx_nome: &str, candidatos: &[Suggestion]) -> Result<()> {
        let key = company_key(adx_nome);
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM adxudicatario_candidato WHERE adx_key = ?1",
            params![key],
        )?;
        {
            let mut stmt = tx.prepare(
                r#"INSERT OR REPLACE INTO adxudicatario_candidato
                    (adx_key, datoscif_url, nome, tipo_entidad, uri, orde)
                   VALUES (?1,?2,?3,?4,?5,?6)"#,
            )?;
            for (i, c) in candidatos.iter().enumerate() {
                stmt.execute(params![
                    key,
                    c.url,
                    c.nombre,
                    c.tipo_entidad,
                    c.uri,
                    i as i64
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Elimina os candidatos de revisión dun adxudicatario (ao resolver o caso).
    pub fn delete_candidatos(&self, adx_nome: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM adxudicatario_candidato WHERE adx_key = ?1",
            params![company_key(adx_nome)],
        )?;
        Ok(())
    }

    /// Casos pendentes de revisión manual (estado 'revisar'), con NIF e candidatos.
    pub fn casos_para_revisar(&self) -> Result<Vec<CasoRevision>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT m.adx_key, m.adx_nome,
                      COALESCE((SELECT r.nif FROM contract_resolucion r
                                WHERE cokey(r.adxudicatario) = m.adx_key
                                  AND TRIM(COALESCE(r.nif,'')) <> '' LIMIT 1), '')
               FROM adxudicatario_match m
               WHERE m.estado = 'revisar'
                 AND m.adx_key NOT IN (SELECT DISTINCT ute_key FROM ute_membro)
               ORDER BY m.adx_nome COLLATE NOCASE"#,
        )?;
        let casos = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut cand_stmt = self.conn.prepare(
            r#"SELECT datoscif_url, nome, tipo_entidad, COALESCE(uri,'')
               FROM adxudicatario_candidato WHERE adx_key = ?1 ORDER BY orde"#,
        )?;
        let mut out = Vec::with_capacity(casos.len());
        for (key, adx_nome, nif) in casos {
            let candidatos = cand_stmt
                .query_map(params![key], |row| {
                    Ok(Suggestion {
                        url: row.get(0)?,
                        nombre: row.get(1)?,
                        tipo_entidad: row.get(2)?,
                        uri: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.push(CasoRevision {
                adx_nome,
                nif,
                candidatos,
            });
        }
        Ok(out)
    }

    /// Adxudicatarios sen correspondencia en datoscif (estado 'pendente'): non se
    /// atopou candidato ningún. Resólvense co proceso asistido (busca manual).
    pub fn casos_sen_match(&self) -> Result<Vec<CasoRevision>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT m.adx_nome,
                      COALESCE((SELECT r.nif FROM contract_resolucion r
                                WHERE cokey(r.adxudicatario) = m.adx_key
                                  AND TRIM(COALESCE(r.nif,'')) <> '' LIMIT 1), '')
               FROM adxudicatario_match m
               WHERE m.estado = 'pendente'
                 AND m.adx_key NOT IN (SELECT DISTINCT ute_key FROM ute_membro)
               ORDER BY m.adx_nome COLLATE NOCASE"#,
        )?;
        let out = stmt
            .query_map([], |row| {
                Ok(CasoRevision {
                    adx_nome: row.get(0)?,
                    nif: row.get(1)?,
                    candidatos: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Inserta/actualiza unha entidade de datoscif. `cargos_descargados` é
    /// "pegañento": unha vez a 1 non volve a 0.
    pub fn upsert_datoscif_entidade(
        &self,
        ent: &DatosCifEntidade,
        cargos_descargados: bool,
        now: &str,
    ) -> Result<()> {
        self.conn.execute(
            r#"INSERT INTO datoscif_entidade
                (url, nome, tipo_entidad, uri, cif, domicilio, cod_postal, municipio,
                 provincia, cargos_descargados, actualizado_en)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
               ON CONFLICT(url) DO UPDATE SET
                 nome=excluded.nome, tipo_entidad=excluded.tipo_entidad,
                 uri=excluded.uri, cif=excluded.cif, domicilio=excluded.domicilio,
                 cod_postal=excluded.cod_postal, municipio=excluded.municipio,
                 provincia=excluded.provincia,
                 cargos_descargados=MAX(datoscif_entidade.cargos_descargados, excluded.cargos_descargados),
                 actualizado_en=excluded.actualizado_en"#,
            params![
                ent.url,
                ent.nome,
                ent.tipo_entidad,
                ent.uri,
                ent.cif,
                ent.domicilio,
                ent.cod_postal,
                ent.municipio,
                ent.provincia,
                cargos_descargados as i64,
                now
            ],
        )?;
        Ok(())
    }

    /// Substitúe os cargos dunha empresa e dá de alta as persoas implicadas.
    /// Garante que existe a fila da empresa en `datoscif_entidade` (as FK de
    /// `datoscif_cargo` apuntan a ela), sen pisar os datos se xa existe.
    pub fn upsert_cargos(
        &mut self,
        empresa_url: &str,
        cargos: &[CargoRow],
        now: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        // A empresa pode aínda non estar gardada; créase cun nome provisional
        // (o `enrich` actualízao despois cos datos reais).
        tx.execute(
            r#"INSERT OR IGNORE INTO datoscif_entidade (url, nome, tipo_entidad, uri, actualizado_en)
               VALUES (?1, ?1, 1, ?2, ?3)"#,
            params![empresa_url, format!("/empresa/{empresa_url}"), now],
        )?;
        tx.execute(
            "DELETE FROM datoscif_cargo WHERE empresa_url = ?1",
            params![empresa_url],
        )?;
        {
            // Alta da persoa (sen pisar cargos_descargados se xa existise).
            let mut per_stmt = tx.prepare(
                r#"INSERT INTO datoscif_entidade (url, nome, tipo_entidad, uri, actualizado_en)
                   VALUES (?1,?2,2,?3,?4)
                   ON CONFLICT(url) DO UPDATE SET nome=excluded.nome, uri=excluded.uri"#,
            )?;
            let mut car_stmt = tx.prepare(
                r#"INSERT OR IGNORE INTO datoscif_cargo
                    (empresa_url, persona_url, cargo, activo, desde, hasta)
                   VALUES (?1,?2,?3,?4,?5,?6)"#,
            )?;
            for c in cargos {
                let uri = format!("/directivo/{}", c.persona_url);
                per_stmt.execute(params![c.persona_url, c.persona_nome, uri, now])?;
                car_stmt.execute(params![
                    empresa_url,
                    c.persona_url,
                    c.cargo,
                    c.activo as i64,
                    c.desde,
                    c.hasta,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Entidade de datoscif vinculada a un adxudicatario, se a hai.
    pub fn entidade_de_adxudicatario(&self, adx_nome: &str) -> Result<Option<DatosCifEntidade>> {
        let key = company_key(adx_nome);
        let ent = self
            .conn
            .query_row(
                r#"SELECT e.url, e.nome, e.tipo_entidad, COALESCE(e.uri,''),
                          COALESCE(e.cif,''), COALESCE(e.domicilio,''),
                          COALESCE(e.cod_postal,''), COALESCE(e.municipio,''),
                          COALESCE(e.provincia,'')
                   FROM adxudicatario_match m
                   JOIN datoscif_entidade e ON e.url = m.datoscif_url
                   WHERE m.adx_key = ?1"#,
                params![key],
                |row| {
                    Ok(DatosCifEntidade {
                        url: row.get(0)?,
                        nome: row.get(1)?,
                        tipo_entidad: row.get(2)?,
                        uri: row.get(3)?,
                        cif: row.get(4)?,
                        domicilio: row.get(5)?,
                        cod_postal: row.get(6)?,
                        municipio: row.get(7)?,
                        provincia: row.get(8)?,
                    })
                },
            )
            .ok();
        Ok(ent)
    }

    /// Empresas (tipo_entidad=1) xa presentes en datoscif: as razóns sociais
    /// adxudicatarias vinculadas das que se descargou (ou se pode redescargar) a
    /// ficha e os cargos. Úsao a reimportación para refrescar os datos.
    pub fn empresas_vinculadas(&self) -> Result<Vec<DatosCifEntidade>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT url, nome, tipo_entidad, COALESCE(uri,''),
                      COALESCE(cif,''), COALESCE(domicilio,''),
                      COALESCE(cod_postal,''), COALESCE(municipio,''),
                      COALESCE(provincia,'')
               FROM datoscif_entidade
               WHERE tipo_entidad = 1
               ORDER BY nome COLLATE NOCASE"#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DatosCifEntidade {
                url: row.get(0)?,
                nome: row.get(1)?,
                tipo_entidad: row.get(2)?,
                uri: row.get(3)?,
                cif: row.get(4)?,
                domicilio: row.get(5)?,
                cod_postal: row.get(6)?,
                municipio: row.get(7)?,
                provincia: row.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Entidade de datoscif cun CIF dado (para resolver membros de UTE por CIF).
    pub fn entidade_por_cif(&self, cif: &str) -> Result<Option<DatosCifEntidade>> {
        if cif.trim().is_empty() {
            return Ok(None);
        }
        let ent = self
            .conn
            .query_row(
                r#"SELECT url, nome, tipo_entidad, COALESCE(uri,''), COALESCE(cif,''),
                          COALESCE(domicilio,''), COALESCE(cod_postal,''),
                          COALESCE(municipio,''), COALESCE(provincia,'')
                   FROM datoscif_entidade WHERE cif = ?1 LIMIT 1"#,
                params![cif],
                |row| {
                    Ok(DatosCifEntidade {
                        url: row.get(0)?,
                        nome: row.get(1)?,
                        tipo_entidad: row.get(2)?,
                        uri: row.get(3)?,
                        cif: row.get(4)?,
                        domicilio: row.get(5)?,
                        cod_postal: row.get(6)?,
                        municipio: row.get(7)?,
                        provincia: row.get(8)?,
                    })
                },
            )
            .ok();
        Ok(ent)
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

    /// Cargos dunha empresa (activos primeiro), co nome da persoa vía JOIN.
    pub fn cargos_de_empresa(&self, empresa_url: &str) -> Result<Vec<CargoRow>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT c.persona_url, COALESCE(e.nome,''), COALESCE(c.cargo,''),
                      COALESCE(c.desde,''), COALESCE(c.hasta,''), COALESCE(c.activo,0)
               FROM datoscif_cargo c
               LEFT JOIN datoscif_entidade e ON e.url = c.persona_url
               WHERE c.empresa_url = ?1
               ORDER BY c.activo DESC, e.nome COLLATE NOCASE"#,
        )?;
        let rows = stmt.query_map(params![empresa_url], |row| {
            Ok(CargoRow {
                persona_url: row.get(0)?,
                persona_nome: row.get(1)?,
                cargo: row.get(2)?,
                desde: row.get(3)?,
                hasta: row.get(4)?,
                activo: row.get::<_, i64>(5)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Grupos (tramas) de razóns sociais interconectadas: compoñentes conexas
    /// do grafo persoa↔empresa, onde unha ou varias persoas teñen cargo en ≥2
    /// razóns sociais que ademais aparecen como adxudicatarias nos contratos.
    /// É o cerne da detección de "a mesma man detrás de varias empresas".
    ///
    /// Só as persoas que conectan ≥2 empresas forman arestas do grafo: así, dous
    /// administradores que controlan as mesmas empresas caen no mesmo grupo en
    /// vez de aparecer como dúas tarxetas case idénticas.
    ///
    /// Os `filtros` (os mesmos do panel lateral) limitan os contratos tidos en
    /// conta: só contan as razóns sociais con polo menos un contrato que casa cos
    /// filtros, de xeito que a vista de relacións reflicte o listado actual.
    pub fn relacions_compartidas(&self, filtros: &LocalFilters) -> Result<Vec<GrupoRelacion>> {
        use std::collections::{HashMap, HashSet};
        let (where_sql, args) = local_where(filtros);
        let params = || -> Vec<&dyn rusqlite::ToSql> {
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect()
        };
        // Subconsulta dos contratos que casan cos filtros do panel. As empresas
        // contratadas inclúen tanto os adxudicatarios normais como os MEMBROS das
        // UTE adxudicatarias (resoltos por CIF contra `datoscif_entidade.cif`).
        let filtrados = format!(
            "filtrados AS (SELECT c.id FROM contracts c \
               LEFT JOIN organismos o ON o.cod_organismo = c.cod_organismo \
               LEFT JOIN estados e ON e.cod_estado = c.cod_estado \
               WHERE 1=1{where_sql})"
        );
        let ec_pairs = "ec_pairs AS ( \
              SELECT m.datoscif_url AS empresa_url, r.contract_id AS contract_id \
              FROM adxudicatario_match m \
              JOIN contract_resolucion r ON cokey(r.adxudicatario) = m.adx_key \
              WHERE m.datoscif_url IS NOT NULL AND r.contract_id IN (SELECT id FROM filtrados) \
              UNION \
              SELECT de.url, um.contract_id FROM ute_membro um \
              JOIN datoscif_entidade de ON de.cif = um.membro_cif AND TRIM(COALESCE(de.cif,'')) <> '' \
              WHERE um.contract_id IN (SELECT id FROM filtrados))";

        // 1) Metadatos das empresas contratadas (nome, provincia, nº contratos).
        let emp_sql = format!(
            "WITH {filtrados}, {ec_pairs} \
             SELECT ec.empresa_url, COALESCE(de.nome,''), COALESCE(de.provincia,''), \
                    COUNT(DISTINCT ec.contract_id) \
             FROM ec_pairs ec LEFT JOIN datoscif_entidade de ON de.url = ec.empresa_url \
             GROUP BY ec.empresa_url"
        );
        let mut emp_meta: HashMap<String, (String, String, i64)> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(&emp_sql)?;
            let mut rows = stmt.query(params().as_slice())?;
            while let Some(row) = rows.next()? {
                emp_meta.insert(row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?));
            }
        }

        // 2) Arestas persoa↔empresa (persoas cun cargo en ≥2 das empresas).
        let arestas_sql = format!(
            "WITH {filtrados}, {ec_pairs}, \
             empresa_contratos AS (SELECT empresa_url FROM ec_pairs GROUP BY empresa_url), \
             persoa_empresas AS ( \
                SELECT c.persona_url, c.empresa_url, MAX(COALESCE(c.activo,0)) AS activo \
                FROM datoscif_cargo c JOIN empresa_contratos ec ON ec.empresa_url = c.empresa_url \
                GROUP BY c.persona_url, c.empresa_url), \
             persoas_multi AS (SELECT persona_url FROM persoa_empresas \
                GROUP BY persona_url HAVING COUNT(DISTINCT empresa_url) >= 2) \
             SELECT pe.persona_url, COALESCE(de.nome,''), pe.empresa_url, pe.activo \
             FROM persoa_empresas pe \
             JOIN persoas_multi pm ON pm.persona_url = pe.persona_url \
             LEFT JOIN datoscif_entidade de ON de.url = pe.persona_url"
        );
        struct Aresta {
            persona_url: String,
            persona_nome: String,
            empresa_url: String,
            activo: bool,
        }
        let arestas: Vec<Aresta> = {
            let mut stmt = self.conn.prepare(&arestas_sql)?;
            stmt.query_map(params().as_slice(), |row| {
                Ok(Aresta {
                    persona_url: row.get(0)?,
                    persona_nome: row.get(1)?,
                    empresa_url: row.get(2)?,
                    activo: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<rusqlite::Result<_>>()?
        };

        // 3) UTE adxudicatarias e os seus membros resoltos (≥2 para formar trama).
        let ute_sql = format!(
            "WITH {filtrados} \
             SELECT um.contract_id, um.ute_key, um.ute_nome, de.url, COALESCE(de.nome,'') \
             FROM ute_membro um \
             JOIN datoscif_entidade de ON de.cif = um.membro_cif AND TRIM(COALESCE(de.cif,'')) <> '' \
             WHERE um.contract_id IN (SELECT id FROM filtrados) \
             ORDER BY um.contract_id, um.ute_key"
        );
        struct UteGrupo {
            nome: String,
            membros: Vec<(String, String)>, // (empresa_url, nome)
        }
        let mut utes: Vec<UteGrupo> = Vec::new();
        {
            let mut stmt = self.conn.prepare(&ute_sql)?;
            let mut rows = stmt.query(params().as_slice())?;
            let mut actual: Option<(String, String)> = None; // (contract_id, ute_key)
            while let Some(row) = rows.next()? {
                let cid: String = row.get(0)?;
                let ukey: String = row.get(1)?;
                let nome: String = row.get(2)?;
                let murl: String = row.get(3)?;
                let mnome: String = row.get(4)?;
                if actual.as_ref() != Some(&(cid.clone(), ukey.clone())) {
                    actual = Some((cid.clone(), ukey.clone()));
                    utes.push(UteGrupo {
                        nome,
                        membros: Vec::new(),
                    });
                }
                utes.last_mut().unwrap().membros.push((murl, mnome));
            }
        }

        // Union-find sobre os nós (persoas e empresas), prefixo "P:"/"E:".
        let mut idx: HashMap<String, usize> = HashMap::new();
        let mut key_of = |prefix: char, url: &str| -> usize {
            let k = format!("{prefix}:{url}");
            let n = idx.len();
            *idx.entry(k).or_insert(n)
        };
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for a in &arestas {
            let p = key_of('P', &a.persona_url);
            let e = key_of('E', &a.empresa_url);
            edges.push((p, e));
        }
        // Arestas empresa↔empresa por pertenza á mesma UTE (≥2 membros resoltos).
        for u in &utes {
            if u.membros.len() < 2 {
                continue;
            }
            let base = key_of('E', &u.membros[0].0);
            for m in &u.membros[1..] {
                let e = key_of('E', &m.0);
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
        for &(p, e) in &edges {
            let rp = find(&mut parent, p);
            let re = find(&mut parent, e);
            if rp != re {
                parent[rp] = re;
            }
        }

        // Flags de cargo por empresa (para distinguir vínculo histórico de UTE).
        let mut emp_cargo: HashMap<String, (bool, bool)> = HashMap::new(); // (con_cargos, activa)
        for a in &arestas {
            let f = emp_cargo
                .entry(a.empresa_url.clone())
                .or_insert((false, false));
            f.0 = true;
            f.1 |= a.activo;
        }

        #[derive(Default)]
        struct Build {
            persoas: HashMap<String, PersoaNodo>,
            empresas: HashMap<String, EmpresaNodo>,
            utes: Vec<UteRelacion>,
            contratos: Vec<ContratoAdxudicado>,
            contratos_vistos: HashSet<String>,
        }
        let mut grupos: HashMap<usize, Build> = HashMap::new();

        // Persoas.
        for a in &arestas {
            let root = find(&mut parent, idx[&format!("P:{}", a.persona_url)]);
            let b = grupos.entry(root).or_default();
            let persoa = b
                .persoas
                .entry(a.persona_url.clone())
                .or_insert_with(|| PersoaNodo {
                    persona_url: a.persona_url.clone(),
                    persona_nome: a.persona_nome.clone(),
                    num_empresas: 0,
                    empresas_activas: 0,
                    empresas_pasadas: 0,
                });
            persoa.num_empresas += 1;
            if a.activo {
                persoa.empresas_activas += 1;
            } else {
                persoa.empresas_pasadas += 1;
            }
        }

        // Empresas: todo nó "E:" que apareza nalgunha aresta (persoa ou UTE).
        let empresa_nodes: Vec<String> = idx
            .keys()
            .filter_map(|k| k.strip_prefix("E:").map(str::to_string))
            .collect();
        for url in empresa_nodes {
            let root = find(&mut parent, idx[&format!("E:{url}")]);
            let (nome, provincia, num_contratos) = emp_meta
                .get(&url)
                .cloned()
                .unwrap_or_else(|| (url.clone(), String::new(), 0));
            let (con_cargos, activa) = emp_cargo.get(&url).copied().unwrap_or((false, false));
            grupos.entry(root).or_default().empresas.insert(
                url.clone(),
                EmpresaNodo {
                    empresa_url: url,
                    empresa_nome: nome,
                    provincia,
                    num_contratos,
                    activa,
                    con_cargos,
                },
            );
        }

        // UTE como vínculo do grupo (clase distinta do administrador compartido).
        for u in &utes {
            if u.membros.len() < 2 {
                continue;
            }
            let root = find(&mut parent, idx[&format!("E:{}", u.membros[0].0)]);
            grupos.entry(root).or_default().utes.push(UteRelacion {
                nome: u.nome.clone(),
                membros: u.membros.iter().map(|(_, n)| n.clone()).collect(),
            });
        }

        // Contratos adxudicados ás empresas do grupo (normais + UTE), un por
        // contrato e grupo (a UTE atribúese unha soa vez, co seu importe total).
        let contratos_sql = format!(
            "WITH {filtrados}, \
             ute_imp AS (SELECT r.contract_id AS cid, cokey(r.adxudicatario) AS ute_key, \
                                SUM(r.importe_resolucion_num) AS imp \
                         FROM contract_resolucion r GROUP BY r.contract_id, cokey(r.adxudicatario)) \
             SELECT empresa_url, contract_id, rotulo, asunto, data_publicacion, importe FROM ( \
                SELECT m.datoscif_url AS empresa_url, r.contract_id AS contract_id, \
                       COALESCE(de.nome,'') AS rotulo, COALESCE(c.asunto,'') AS asunto, \
                       COALESCE(c.data_publicacion,'') AS data_publicacion, \
                       SUM(r.importe_resolucion_num) AS importe \
                FROM adxudicatario_match m \
                JOIN contract_resolucion r ON cokey(r.adxudicatario) = m.adx_key \
                JOIN contracts c ON c.id = r.contract_id \
                LEFT JOIN datoscif_entidade de ON de.url = m.datoscif_url \
                WHERE m.datoscif_url IS NOT NULL AND r.contract_id IN (SELECT id FROM filtrados) \
                GROUP BY m.datoscif_url, r.contract_id \
                UNION ALL \
                SELECT de.url AS empresa_url, um.contract_id, um.ute_nome AS rotulo, \
                       COALESCE(c.asunto,''), COALESCE(c.data_publicacion,''), ui.imp \
                FROM ute_membro um \
                JOIN datoscif_entidade de ON de.cif = um.membro_cif AND TRIM(COALESCE(de.cif,'')) <> '' \
                JOIN contracts c ON c.id = um.contract_id \
                LEFT JOIN ute_imp ui ON ui.cid = um.contract_id AND ui.ute_key = um.ute_key \
                WHERE um.contract_id IN (SELECT id FROM filtrados)) \
             ORDER BY data_publicacion DESC, contract_id DESC"
        );
        {
            let mut stmt = self.conn.prepare(&contratos_sql)?;
            let mut filas = stmt.query(params().as_slice())?;
            while let Some(row) = filas.next()? {
                let empresa_url: String = row.get(0)?;
                let Some(&node) = idx.get(&format!("E:{empresa_url}")) else {
                    continue;
                };
                let root = find(&mut parent, node);
                let Some(b) = grupos.get_mut(&root) else {
                    continue;
                };
                let contract_id: String = row.get(1)?;
                // Un contrato cóntase unha soa vez por grupo (a UTE ten varios membros).
                if !b.contratos_vistos.insert(contract_id.clone()) {
                    continue;
                }
                let importe_num: f64 = row.get::<_, Option<f64>>(5)?.unwrap_or(0.0);
                let data_iso: String = row.get(4)?;
                b.contratos.push(ContratoAdxudicado {
                    contract_id,
                    empresa_nome: row.get(2)?,
                    asunto: row.get(3)?,
                    publicacion: format_data_gl(&data_iso),
                    importe_num,
                    importe_txt: format_importe(importe_num),
                });
            }
        }

        // Materializar e ordenar.
        let mut out: Vec<GrupoRelacion> = grupos
            .into_values()
            .map(|b| {
                let mut persoas: Vec<PersoaNodo> = b.persoas.into_values().collect();
                persoas.sort_by(|a, b| {
                    a.persona_nome
                        .to_lowercase()
                        .cmp(&b.persona_nome.to_lowercase())
                });
                let mut empresas: Vec<EmpresaNodo> = b.empresas.into_values().collect();
                empresas.sort_by(|a, b| {
                    a.empresa_nome
                        .to_lowercase()
                        .cmp(&b.empresa_nome.to_lowercase())
                });
                let importe_total = b.contratos.iter().map(|c| c.importe_num).sum();
                GrupoRelacion {
                    persoas,
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

        // Só dous estados distintos quedan na táboa `estados`.
        let opts = db.local_options().expect("options");
        assert_eq!(
            opts.estados,
            vec![
                "Formalizado".to_string(),
                "Pendente de adxudicar".to_string()
            ]
        );

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

        // Filtro por estado, insensible a maiúsculas/acentos.
        let f = LocalFilters {
            estado: "pendente".into(),
            ..Default::default()
        };
        let filtradas = db.query_local(&f).expect("query estado");
        assert_eq!(filtradas.len(), 1);
        assert_eq!(filtradas[0].id, "2");

        let _ = std::fs::remove_file(&path);
    }

    // Regresión: gardar cargos dunha empresa que aínda non está en
    // `datoscif_entidade` non debe violar a FK (créase a fila pai soa).
    #[test]
    fn upsert_cargos_crea_empresa_se_non_existe() {
        use crate::model::CargoRow;
        let path = std::env::temp_dir().join("congal_test_fk_cargos.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        // Sen ter inserido a empresa antes: non debe fallar a FK.
        db.upsert_cargos(
            "academia-lorca-institute-sl",
            &[CargoRow {
                persona_url: "garcia-lorca-federico".into(),
                persona_nome: "Garcia Lorca Federico".into(),
                cargo: "Administrador Único".into(),
                activo: true,
                ..Default::default()
            }],
            "agora",
        )
        .expect("upsert_cargos non debe violar a FK");

        let cargos = db
            .cargos_de_empresa("academia-lorca-institute-sl")
            .expect("cargos");
        assert_eq!(cargos.len(), 1);
        assert_eq!(cargos[0].persona_nome, "Garcia Lorca Federico");

        let _ = std::fs::remove_file(&path);
    }

    // A cola de revisión garda candidatos co NIF do contrato e baléirase ao
    // resolver o caso (cambiar de estado e borrar os candidatos).
    #[test]
    fn cola_de_revision_garda_e_resolve() {
        use crate::model::{DatosCifEntidade, EstadoMatch, Suggestion};
        let path = std::env::temp_dir().join("congal_test_revision.sqlite");
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
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "Talleres O Rosal SL".into(),
                nif: "B36881415".into(),
                ..Default::default()
            }],
        )
        .expect("detail");

        // Caso en revisión con dous candidatos.
        db.upsert_match(
            "Talleres O Rosal SL",
            None,
            "ambigua",
            EstadoMatch::Revisar.as_str(),
            "agora",
        )
        .expect("match");
        db.upsert_candidatos(
            "Talleres O Rosal SL",
            &[
                Suggestion {
                    nombre: "TALLERES O ROSAL SL".into(),
                    url: "talleres-o-rosal-sl".into(),
                    uri: "/empresa/talleres-o-rosal-sl".into(),
                    tipo_entidad: 1,
                },
                Suggestion {
                    nombre: "TALLERES O ROSAL SA".into(),
                    url: "talleres-o-rosal-sa".into(),
                    uri: "/empresa/talleres-o-rosal-sa".into(),
                    tipo_entidad: 1,
                },
            ],
        )
        .expect("candidatos");

        let casos = db.casos_para_revisar().expect("casos");
        assert_eq!(casos.len(), 1);
        assert_eq!(casos[0].adx_nome, "Talleres O Rosal SL");
        assert_eq!(casos[0].nif, "B36881415"); // tómase da resolución
        assert_eq!(casos[0].candidatos.len(), 2);
        assert_eq!(casos[0].candidatos[0].url, "talleres-o-rosal-sl"); // respéctase a orde

        // Resolver a man: dar de alta a entidade, vincular e borrar candidatos.
        db.upsert_datoscif_entidade(
            &DatosCifEntidade {
                url: "talleres-o-rosal-sl".into(),
                nome: "TALLERES O ROSAL SL".into(),
                tipo_entidad: 1,
                ..Default::default()
            },
            false,
            "agora",
        )
        .expect("entidade");
        db.upsert_match(
            "Talleres O Rosal SL",
            Some("talleres-o-rosal-sl"),
            "manual",
            EstadoMatch::Manual.as_str(),
            "agora",
        )
        .expect("match2");
        db.delete_candidatos("Talleres O Rosal SL").expect("delete");

        assert!(db.casos_para_revisar().expect("casos2").is_empty());

        let _ = std::fs::remove_file(&path);
    }

    // Os casos 'pendente' (sen candidatos) aparecen en casos_sen_match, non na
    // cola de revisión, para resolvelos co proceso asistido.
    #[test]
    fn casos_sen_match_lista_pendentes() {
        use crate::model::EstadoMatch;
        let path = std::env::temp_dir().join("congal_test_senmatch.sqlite");
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
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "Empresa Rara SL".into(),
                nif: "B11111111".into(),
                ..Default::default()
            }],
        )
        .expect("detail");

        db.upsert_match(
            "Empresa Rara SL",
            None,
            "sen_match",
            EstadoMatch::Pendente.as_str(),
            "agora",
        )
        .expect("match");

        let sen = db.casos_sen_match().expect("sen_match");
        assert_eq!(sen.len(), 1);
        assert_eq!(sen[0].adx_nome, "Empresa Rara SL");
        assert_eq!(sen[0].nif, "B11111111");
        assert!(sen[0].candidatos.is_empty());
        // Non está na cola de revisión (esa é só para 'revisar').
        assert!(db.casos_para_revisar().expect("rev").is_empty());

        let _ = std::fs::remove_file(&path);
    }

    // Dúas razóns sociais distintas, ambas adxudicatarias en contratos e ambas
    // co mesmo administrador (persona_url): debe detectarse a relación.
    #[test]
    fn detecta_persoa_con_varias_razons_sociais() {
        use crate::model::{CargoRow, DatosCifEntidade};
        let path = std::env::temp_dir().join("congal_test_relacions.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        // Dous contratos, cada un cun adxudicatario distinto.
        db.upsert_summaries(
            &[
                summary("1", "obra", "Concello", "Formalizado", "01/02/2025"),
                summary("2", "servizo", "Concello", "Formalizado", "03/04/2025"),
            ],
            "agora",
        )
        .expect("summaries");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "Empresa A, S.L.".into(),
                importe_num: Some(1000.0),
                ..Default::default()
            }],
        )
        .expect("detail 1");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "2".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "EMPRESA B SL".into(),
                importe_num: Some(2500.5),
                ..Default::default()
            }],
        )
        .expect("detail 2");

        // Entidades de datoscif e vínculos (alta confianza).
        for (url, nome, adx) in [
            ("empresa-a-sl", "EMPRESA A SL", "Empresa A, S.L."),
            ("empresa-b-sl", "EMPRESA B SL", "EMPRESA B SL"),
        ] {
            db.upsert_datoscif_entidade(
                &DatosCifEntidade {
                    url: url.into(),
                    nome: nome.into(),
                    tipo_entidad: 1,
                    uri: format!("/empresa/{url}"),
                    municipio: "Santiago".into(),
                    provincia: "A Coruña".into(),
                    ..Default::default()
                },
                true,
                "agora",
            )
            .expect("entidade");
            db.upsert_match(adx, Some(url), "exacta", "auto", "agora")
                .expect("match");
            // A mesma persoa administra ambas empresas.
            db.upsert_cargos(
                url,
                &[CargoRow {
                    persona_url: "perez-perez-xan".into(),
                    persona_nome: "Perez Perez Xan".into(),
                    cargo: "Administrador Único".into(),
                    activo: true,
                    ..Default::default()
                }],
                "agora",
            )
            .expect("cargos");
        }

        let rel = db
            .relacions_compartidas(&LocalFilters::default())
            .expect("relacions");
        assert_eq!(rel.len(), 1, "debe haber un grupo relacionado");
        assert_eq!(rel[0].persoas.len(), 1, "unha soa persoa conecta o grupo");
        assert_eq!(rel[0].persoas[0].persona_url, "perez-perez-xan");
        assert_eq!(
            rel[0].persoas[0].num_empresas, 2,
            "controla dúas razóns sociais"
        );
        assert_eq!(rel[0].empresas.len(), 2, "dúas razóns sociais no grupo");
        assert!(rel[0].empresas.iter().all(|e| e.num_contratos == 1));
        // O despregable de contratos do grupo reúne ambas adxudicacións e suma os importes.
        assert_eq!(rel[0].contratos.len(), 2, "os dous contratos do grupo");
        assert_eq!(
            rel[0].importe_total, 3500.5,
            "suma dos importes adxudicados"
        );
        let ids: std::collections::HashSet<&str> = rel[0]
            .contratos
            .iter()
            .map(|c| c.contract_id.as_str())
            .collect();
        assert_eq!(ids, std::collections::HashSet::from(["1", "2"]));

        // E a entidade recupérase desde o nome do adxudicatario (insensible a puntuación).
        let ent = db
            .entidade_de_adxudicatario("Empresa A SL")
            .expect("query")
            .expect("debe atoparse");
        assert_eq!(ent.url, "empresa-a-sl");

        let _ = std::fs::remove_file(&path);
    }

    // Un administrador conecta dúas empresas pero foi cesado nunha: a relación
    // mantense (indicio) pero debe marcarse como histórica nesa empresa.
    #[test]
    fn cargo_pasado_marca_vinculo_historico() {
        use crate::model::{CargoRow, DatosCifEntidade};
        let path = std::env::temp_dir().join("congal_test_historico.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        db.upsert_summaries(
            &[
                summary("1", "obra", "Concello", "Formalizado", "01/02/2025"),
                summary("2", "servizo", "Concello", "Formalizado", "03/04/2025"),
            ],
            "agora",
        )
        .expect("summaries");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "Empresa A, S.L.".into(),
                ..Default::default()
            }],
        )
        .expect("detail 1");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "2".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "EMPRESA B SL".into(),
                ..Default::default()
            }],
        )
        .expect("detail 2");

        // Empresa A: cargo VIXENTE. Empresa B: cargo CESADO (histórico).
        for (url, nome, adx, activo) in [
            ("empresa-a-sl", "EMPRESA A SL", "Empresa A, S.L.", true),
            ("empresa-b-sl", "EMPRESA B SL", "EMPRESA B SL", false),
        ] {
            db.upsert_datoscif_entidade(
                &DatosCifEntidade {
                    url: url.into(),
                    nome: nome.into(),
                    tipo_entidad: 1,
                    ..Default::default()
                },
                true,
                "agora",
            )
            .expect("entidade");
            db.upsert_match(adx, Some(url), "exacta", "auto", "agora")
                .expect("match");
            db.upsert_cargos(
                url,
                &[CargoRow {
                    persona_url: "perez-perez-xan".into(),
                    persona_nome: "Perez Perez Xan".into(),
                    cargo: "Administrador Único".into(),
                    activo,
                    hasta: if activo {
                        String::new()
                    } else {
                        "2020-01-01".into()
                    },
                    ..Default::default()
                }],
                "agora",
            )
            .expect("cargos");
        }

        let rel = db
            .relacions_compartidas(&LocalFilters::default())
            .expect("relacions");
        assert_eq!(rel.len(), 1);
        let g = &rel[0];
        // A persoa ten un vínculo actual e outro pasado.
        assert_eq!(g.persoas[0].empresas_activas, 1);
        assert_eq!(g.persoas[0].empresas_pasadas, 1);
        // A empresa A está activa; a B só ten cargos pasados.
        let a = g
            .empresas
            .iter()
            .find(|e| e.empresa_url == "empresa-a-sl")
            .unwrap();
        let b = g
            .empresas
            .iter()
            .find(|e| e.empresa_url == "empresa-b-sl")
            .unwrap();
        assert!(a.activa, "A ten cargo vixente");
        assert!(!b.activa, "B só ten cargo cesado (histórico)");

        let _ = std::fs::remove_file(&path);
    }

    // Unha UTE adxudicataria relaciona as súas empresas membro (sen necesidade de
    // administrador compartido); o contrato e o importe cóntanse unha soa vez.
    #[test]
    fn ute_relaciona_os_membros() {
        use crate::model::{DatosCifEntidade, Ute, UteMembro};
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
        // As dúas empresas membro, xa con ficha en datoscif (CIF).
        for (url, nome, cif) in [
            ("empresa-a-sl", "EMPRESA A SL", "A11111111"),
            ("empresa-b-sl", "EMPRESA B SL", "B22222222"),
        ] {
            db.upsert_datoscif_entidade(
                &DatosCifEntidade {
                    url: url.into(),
                    nome: nome.into(),
                    tipo_entidad: 1,
                    cif: cif.into(),
                    ..Default::default()
                },
                true,
                "agora",
            )
            .expect("entidade");
        }
        // Composición da UTE.
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
        assert!(g.persoas.is_empty(), "sen administrador compartido");
        assert_eq!(g.utes.len(), 1);
        assert!(g.utes[0].nome.contains("UTE A"));
        assert_eq!(g.utes[0].membros.len(), 2);
        // O contrato e o importe cóntanse unha soa vez (non por cada membro).
        assert_eq!(g.contratos.len(), 1);
        assert_eq!(g.importe_total, 1000.0);
        // As empresas non levan marca de «só cargos pasados» (entran por UTE).
        assert!(g.empresas.iter().all(|e| !e.con_cargos));

        // A propia UTE NON se busca en datoscif nin aparece na cola de revisión.
        assert!(
            !db.adxudicatarios_pendentes()
                .unwrap()
                .iter()
                .any(|a| a.contains("UTE")),
            "o nome da UTE non debe estar entre os adxudicatarios a vincular"
        );
        assert!(
            db.casos_sen_match()
                .unwrap()
                .iter()
                .all(|c| !c.adx_nome.contains("UTE"))
        );

        let _ = std::fs::remove_file(&path);
    }

    // Dous administradores distintos que controlan AMBOS as mesmas dúas empresas
    // deben caer nun único grupo (compoñente conexa), non en dúas tarxetas.
    #[test]
    fn dous_administradores_mesmas_empresas_un_so_grupo() {
        use crate::model::{CargoRow, DatosCifEntidade};
        let path = std::env::temp_dir().join("congal_test_trama.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        db.upsert_summaries(
            &[
                summary("1", "obra", "Concello", "Formalizado", "01/02/2025"),
                summary("2", "servizo", "Concello", "Formalizado", "03/04/2025"),
            ],
            "agora",
        )
        .expect("summaries");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "Empresa A, S.L.".into(),
                ..Default::default()
            }],
        )
        .expect("detail 1");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "2".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "EMPRESA B SL".into(),
                ..Default::default()
            }],
        )
        .expect("detail 2");

        for (url, nome, adx) in [
            ("empresa-a-sl", "EMPRESA A SL", "Empresa A, S.L."),
            ("empresa-b-sl", "EMPRESA B SL", "EMPRESA B SL"),
        ] {
            db.upsert_datoscif_entidade(
                &DatosCifEntidade {
                    url: url.into(),
                    nome: nome.into(),
                    tipo_entidad: 1,
                    uri: format!("/empresa/{url}"),
                    ..Default::default()
                },
                true,
                "agora",
            )
            .expect("entidade");
            db.upsert_match(adx, Some(url), "exacta", "auto", "agora")
                .expect("match");
            // Dúas persoas distintas administran AMBAS empresas.
            db.upsert_cargos(
                url,
                &[
                    CargoRow {
                        persona_url: "xan".into(),
                        persona_nome: "Xan".into(),
                        cargo: "Administrador".into(),
                        activo: true,
                        ..Default::default()
                    },
                    CargoRow {
                        persona_url: "maria".into(),
                        persona_nome: "Maria".into(),
                        cargo: "Administradora".into(),
                        activo: true,
                        ..Default::default()
                    },
                ],
                "agora",
            )
            .expect("cargos");
        }

        let rel = db
            .relacions_compartidas(&LocalFilters::default())
            .expect("relacions");
        assert_eq!(rel.len(), 1, "ambos administradores forman un único grupo");
        assert_eq!(rel[0].persoas.len(), 2, "as dúas persoas no mesmo grupo");
        assert_eq!(rel[0].empresas.len(), 2, "as dúas razóns sociais no grupo");
        assert!(rel[0].persoas.iter().all(|p| p.num_empresas == 2));

        let _ = std::fs::remove_file(&path);
    }

    // Os filtros do panel lateral tamén limitan a vista de relacións: ao filtrar
    // por un ano que só inclúe un dos contratos, a outra empresa deixa de contar
    // e a relación (que precisaba ≥2 empresas) desaparece.
    #[test]
    fn filtros_aplicanse_a_relacions() {
        use crate::model::{CargoRow, DatosCifEntidade};
        let path = std::env::temp_dir().join("congal_test_rel_filtros.sqlite");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).expect("db");

        // Dous contratos en anos distintos, un por empresa.
        db.upsert_summaries(
            &[
                summary("1", "obra", "Concello", "Formalizado", "01/02/2024"),
                summary("2", "servizo", "Concello", "Formalizado", "03/04/2025"),
            ],
            "agora",
        )
        .expect("summaries");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "1".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "Empresa A, S.L.".into(),
                ..Default::default()
            }],
        )
        .expect("detail 1");
        db.upsert_detail(
            &ContractDetail {
                contract_id: "2".into(),
                ..Default::default()
            },
            &[Resolucion {
                adxudicatario: "EMPRESA B SL".into(),
                ..Default::default()
            }],
        )
        .expect("detail 2");

        for (url, nome, adx) in [
            ("empresa-a-sl", "EMPRESA A SL", "Empresa A, S.L."),
            ("empresa-b-sl", "EMPRESA B SL", "EMPRESA B SL"),
        ] {
            db.upsert_datoscif_entidade(
                &DatosCifEntidade {
                    url: url.into(),
                    nome: nome.into(),
                    tipo_entidad: 1,
                    uri: format!("/empresa/{url}"),
                    ..Default::default()
                },
                true,
                "agora",
            )
            .expect("entidade");
            db.upsert_match(adx, Some(url), "exacta", "auto", "agora")
                .expect("match");
            db.upsert_cargos(
                url,
                &[CargoRow {
                    persona_url: "xan".into(),
                    persona_nome: "Xan".into(),
                    cargo: "Administrador".into(),
                    activo: true,
                    ..Default::default()
                }],
                "agora",
            )
            .expect("cargos");
        }

        // Sen filtros: detéctase a relación (a persoa controla as dúas empresas).
        let rel = db
            .relacions_compartidas(&LocalFilters::default())
            .expect("relacions");
        assert_eq!(rel.len(), 1, "sen filtros hai unha relación");

        // Filtrando por 2024 só conta a empresa A → xa non hai relación.
        let filtros = LocalFilters {
            year: "2024".into(),
            ..Default::default()
        };
        let rel = db
            .relacions_compartidas(&filtros)
            .expect("relacions filtradas");
        assert!(rel.is_empty(), "co filtro de ano a relación desaparece");

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
