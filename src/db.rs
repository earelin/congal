//! Persistencia en SQLite (rusqlite, bundled).

use crate::model::{
    CargoRow, ContractDetail, ContractSummary, DatosCifEntidade, EmpresaRelacionada, LocalFilters,
    LocalRow, PersoaRelacion, Resolucion, company_key, format_data_gl, format_importe,
    normalize_search, parse_data, parse_importe,
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
        if w.iter().all(|c| c.is_ascii_digit())
            && (w.starts_with(b"19") || w.starts_with(b"20"))
        {
            return Some(String::from_utf8_lossy(w).into_owned());
        }
    }
    None
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
        let _ = self
            .conn
            .pragma_update(None, "wal_checkpoint", "TRUNCATE");
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

            CREATE TABLE IF NOT EXISTS contract_detail (
                contract_id          TEXT PRIMARY KEY REFERENCES contracts(id) ON DELETE CASCADE,
                referencia           TEXT,
                obxecto              TEXT,
                tipo_tramitacion     TEXT,
                tipo_procedemento    TEXT,
                tipo_contrato        TEXT,
                orzamento_base       TEXT,
                valor_estimado       TEXT,
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

            CREATE TABLE IF NOT EXISTS contract_resolucion (
                contract_id            TEXT NOT NULL REFERENCES contracts(id) ON DELETE CASCADE,
                lote                   TEXT,
                participacion          TEXT,
                estado_resolucion      TEXT,
                adxudicatario          TEXT,
                importe_resolucion_num REAL,
                importe_resolucion_txt TEXT,
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
            let mut est_stmt =
                tx.prepare(r#"INSERT OR IGNORE INTO estados (nome) VALUES (?1)"#)?;
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
    pub fn upsert_detail(
        &mut self,
        d: &ContractDetail,
        resolucions: &[Resolucion],
    ) -> Result<()> {
        let extra_json = serde_json::to_string(&d.extra).unwrap_or_else(|_| "{}".to_string());
        let tx = self.conn.transaction()?;
        // Garantir que existe a fila do contrato (evita violar a FK se aínda non
        // se gardou o resumo, p.ex. ao actualizar só o detalle).
        tx.execute(
            "INSERT OR IGNORE INTO contracts(id) VALUES(?1)",
            params![d.contract_id],
        )?;
        tx.execute(
            r#"INSERT INTO contract_detail
                (contract_id, referencia, obxecto, tipo_tramitacion, tipo_procedemento,
                 tipo_contrato, orzamento_base, valor_estimado, num_lotes,
                 sistema_contratacion, observacions, data_difusion, sara, centralizada,
                 lei_aplicacion, enlace_resolucion, extra_json)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
               ON CONFLICT(contract_id) DO UPDATE SET
                 referencia=excluded.referencia, obxecto=excluded.obxecto,
                 tipo_tramitacion=excluded.tipo_tramitacion,
                 tipo_procedemento=excluded.tipo_procedemento,
                 tipo_contrato=excluded.tipo_contrato,
                 orzamento_base=excluded.orzamento_base,
                 valor_estimado=excluded.valor_estimado, num_lotes=excluded.num_lotes,
                 sistema_contratacion=excluded.sistema_contratacion,
                 observacions=excluded.observacions, data_difusion=excluded.data_difusion,
                 sara=excluded.sara, centralizada=excluded.centralizada,
                 lei_aplicacion=excluded.lei_aplicacion,
                 enlace_resolucion=excluded.enlace_resolucion, extra_json=excluded.extra_json"#,
            params![
                d.contract_id, d.referencia, d.obxecto, d.tipo_tramitacion,
                d.tipo_procedemento, d.tipo_contrato, d.orzamento_base, d.valor_estimado,
                d.num_lotes, d.sistema_contratacion, d.observacions, d.data_difusion,
                d.sara, d.centralizada, d.lei_aplicacion, d.enlace_resolucion, extra_json,
            ],
        )?;
        tx.execute(
            "DELETE FROM contract_resolucion WHERE contract_id = ?1",
            params![d.contract_id],
        )?;
        {
            let mut stmt = tx.prepare(
                r#"INSERT INTO contract_resolucion
                    (contract_id, lote, participacion, estado_resolucion, adxudicatario,
                     importe_resolucion_num, importe_resolucion_txt, data_difusion,
                     prazo_execucion, recurso)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"#,
            )?;
            for r in resolucions {
                stmt.execute(params![
                    d.contract_id,
                    r.lote,
                    r.participacion,
                    r.estado_resolucion,
                    r.adxudicatario,
                    r.importe_num,
                    r.importe_txt,
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
        let total: i64 =
            self.conn
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
                      COALESCE(d.enlace_resolucion,'')
               FROM contracts c
               LEFT JOIN organismos o ON o.cod_organismo = c.cod_organismo
               LEFT JOIN estados e ON e.cod_estado = c.cod_estado
               LEFT JOIN contract_detail d ON d.contract_id = c.id
               LEFT JOIN (
                   SELECT contract_id,
                          GROUP_CONCAT(DISTINCT NULLIF(TRIM(adxudicatario),'')) AS adxudicatarios,
                          SUM(importe_resolucion_num) AS importe_total
                   FROM contract_resolucion
                   GROUP BY contract_id
               ) r ON r.contract_id = c.id
               WHERE 1=1"#,
        );
        let mut args: Vec<String> = Vec::new();
        if !f.texto.trim().is_empty() {
            sql.push_str(" AND (nrm(c.asunto) LIKE ?  OR nrm(c.referencia) LIKE ?)");
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
            // O contrato inclúese se ALGÚN dos seus lotes casa co adxudicatario;
            // a fila segue amosando todos os adxudicatarios do contrato.
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM contract_resolucion x \
                   WHERE x.contract_id = c.id AND nrm(x.adxudicatario) LIKE ?)",
            );
            args.push(format!("%{}%", normalize_search(&f.adxudicatario)));
        }
        sql.push_str(" ORDER BY c.data_publicacion DESC, c.id DESC LIMIT 5000");

        let mut stmt = self.conn.prepare(&sql)?;
        let params_dyn: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params_dyn.as_slice(), |row| {
            let importe_num: Option<f64> = row.get(3)?;
            let data_iso: String = row.get(5)?;
            let importe_total: Option<f64> = row.get(8)?;
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
                r#"SELECT referencia, obxecto, tipo_tramitacion, tipo_procedemento,
                          tipo_contrato, orzamento_base, valor_estimado, num_lotes,
                          sistema_contratacion, observacions, data_difusion, sara,
                          centralizada, lei_aplicacion, enlace_resolucion, extra_json
                   FROM contract_detail WHERE contract_id = ?1"#,
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
            r#"SELECT lote, participacion, estado_resolucion, adxudicatario,
                      importe_resolucion_num, importe_resolucion_txt, data_difusion,
                      prazo_execucion, recurso
               FROM contract_resolucion WHERE contract_id = ?1"#,
        )?;
        let res = stmt.query_map(params![id], |row| {
            Ok(Resolucion {
                lote: row.get(0)?,
                participacion: row.get(1)?,
                estado_resolucion: row.get(2)?,
                adxudicatario: row.get(3)?,
                importe_num: row.get(4)?,
                importe_txt: row.get(5)?,
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
        self.distinct(
            "SELECT MIN(TRIM(adxudicatario)) FROM contract_resolucion \
             WHERE TRIM(COALESCE(adxudicatario,'')) <> '' \
               AND cokey(adxudicatario) NOT IN (SELECT adx_key FROM adxudicatario_match) \
             GROUP BY cokey(adxudicatario) \
             ORDER BY 1 COLLATE NOCASE",
        )
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
    pub fn upsert_cargos(&mut self, empresa_url: &str, cargos: &[CargoRow], now: &str) -> Result<()> {
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

    /// Persoas que teñen cargo en ≥2 razóns sociais distintas que ademais
    /// aparecen como adxudicatarias nos contratos. É o cerne da detección de
    /// "a mesma man detrás de varias empresas".
    pub fn relacions_compartidas(&self) -> Result<Vec<PersoaRelacion>> {
        let mut stmt = self.conn.prepare(
            r#"
            WITH empresa_contratos AS (
                SELECT m.datoscif_url AS empresa_url,
                       COUNT(DISTINCT r.contract_id) AS num_contratos
                FROM adxudicatario_match m
                JOIN contract_resolucion r ON cokey(r.adxudicatario) = m.adx_key
                WHERE m.datoscif_url IS NOT NULL
                GROUP BY m.datoscif_url
            ),
            persoa_empresas AS (
                SELECT c.persona_url, c.empresa_url, MIN(c.cargo) AS cargo
                FROM datoscif_cargo c
                JOIN empresa_contratos ec ON ec.empresa_url = c.empresa_url
                GROUP BY c.persona_url, c.empresa_url
            ),
            persoas_multi AS (
                SELECT persona_url FROM persoa_empresas
                GROUP BY persona_url HAVING COUNT(DISTINCT empresa_url) >= 2
            )
            SELECT pe.persona_url, COALESCE(per.nome,''),
                   pe.empresa_url, COALESCE(emp.nome,''), COALESCE(emp.provincia,''),
                   COALESCE(pe.cargo,''), ec.num_contratos
            FROM persoa_empresas pe
            JOIN persoas_multi pm ON pm.persona_url = pe.persona_url
            JOIN empresa_contratos ec ON ec.empresa_url = pe.empresa_url
            LEFT JOIN datoscif_entidade per ON per.url = pe.persona_url
            LEFT JOIN datoscif_entidade emp ON emp.url = pe.empresa_url
            ORDER BY per.nome COLLATE NOCASE, pe.persona_url, emp.nome COLLATE NOCASE
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                EmpresaRelacionada {
                    empresa_url: row.get(2)?,
                    empresa_nome: row.get(3)?,
                    provincia: row.get(4)?,
                    cargo: row.get(5)?,
                    num_contratos: row.get(6)?,
                },
            ))
        })?;

        // Agrupar as filas por persoa.
        let mut out: Vec<PersoaRelacion> = Vec::new();
        for r in rows {
            let (purl, pnome, emp) = r?;
            match out.last_mut() {
                Some(last) if last.persona_url == purl => last.empresas.push(emp),
                _ => out.push(PersoaRelacion {
                    persona_url: purl,
                    persona_nome: pnome,
                    empresas: vec![emp],
                }),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, asunto: &str, organismo: &str, estado: &str, pub_: &str) -> ContractSummary {
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
        db.upsert_summaries(&[s], "2025-03-16 10:00:00").expect("upsert");

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
                summary("2", "servizo", "Deputación", "Pendente de adxudicar", "03/04/2024"),
                // Mesmo estado que o 1: comparte `cod_estado`, non duplica fila.
                summary("3", "subministración", "Concello", "Formalizado", "05/06/2025"),
            ],
            "agora",
        )
        .expect("upsert summaries");

        // Só dous estados distintos quedan na táboa `estados`.
        let opts = db.local_options().expect("options");
        assert_eq!(
            opts.estados,
            vec!["Formalizado".to_string(), "Pendente de adxudicar".to_string()]
        );

        // O nome do estado cárgase vía JOIN na consulta local.
        let rows = db.query_local(&LocalFilters::default()).expect("query");
        let r1 = rows.iter().find(|r| r.id == "1").expect("fila 1");
        assert_eq!(r1.estado, "Formalizado");

        // `estado_previo` devolve o texto (úsao a sync para `is_estado_terminal`).
        assert_eq!(db.estado_previo("2").expect("previo").as_deref(), Some("Pendente de adxudicar"));
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
            &ContractDetail { contract_id: "1".into(), ..Default::default() },
            &[Resolucion { adxudicatario: "Empresa A, S.L.".into(), ..Default::default() }],
        )
        .expect("detail 1");
        db.upsert_detail(
            &ContractDetail { contract_id: "2".into(), ..Default::default() },
            &[Resolucion { adxudicatario: "EMPRESA B SL".into(), ..Default::default() }],
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
            db.upsert_match(adx, Some(url), "exacta", "auto", "agora").expect("match");
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

        let rel = db.relacions_compartidas().expect("relacions");
        assert_eq!(rel.len(), 1, "debe haber unha persoa relacionada");
        assert_eq!(rel[0].persona_url, "perez-perez-xan");
        assert_eq!(rel[0].empresas.len(), 2, "controla dúas razóns sociais");
        assert!(rel[0].empresas.iter().all(|e| e.num_contratos == 1));

        // E a entidade recupérase desde o nome do adxudicatario (insensible a puntuación).
        let ent = db
            .entidade_de_adxudicatario("Empresa A SL")
            .expect("query")
            .expect("debe atoparse");
        assert_eq!(ent.url, "empresa-a-sl");

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
            &[summary("10", "obra con lotes", "Concello", "Formalizado", "01/02/2025")],
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
