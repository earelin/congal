//! Persistencia en SQLite (rusqlite, bundled).

use crate::model::{
    ContractDetail, ContractSummary, LocalFilters, LocalRow, Resolucion, normalize_search,
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
        let db = Db { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS contracts (
                id                TEXT PRIMARY KEY,
                referencia        TEXT,
                asunto            TEXT,
                importe_num       REAL,
                importe_txt       TEXT,
                estado            TEXT,
                data_publicacion  TEXT,
                cod_organismo     TEXT,
                organismo         TEXT,
                detalle_descargado INTEGER NOT NULL DEFAULT 0,
                actualizado_en    TEXT
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
            CREATE INDEX IF NOT EXISTS idx_contracts_estado ON contracts(estado);
            CREATE INDEX IF NOT EXISTS idx_contracts_data  ON contracts(data_publicacion);
            CREATE INDEX IF NOT EXISTS idx_res_contract    ON contract_resolucion(contract_id);
            CREATE INDEX IF NOT EXISTS idx_res_adx         ON contract_resolucion(adxudicatario);

            CREATE TABLE IF NOT EXISTS meta (
                clave TEXT PRIMARY KEY,
                valor TEXT
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
                "SELECT estado FROM contracts WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .ok();
        Ok(r)
    }

    /// Inserta/actualiza un lote de rexistros do listado.
    pub fn upsert_summaries(&mut self, rows: &[ContractSummary], now: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                r#"INSERT INTO contracts
                    (id, referencia, asunto, importe_num, importe_txt, estado,
                     data_publicacion, cod_organismo, organismo, actualizado_en)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                   ON CONFLICT(id) DO UPDATE SET
                     referencia=excluded.referencia,
                     asunto=excluded.asunto,
                     importe_num=excluded.importe_num,
                     importe_txt=excluded.importe_txt,
                     estado=excluded.estado,
                     data_publicacion=excluded.data_publicacion,
                     cod_organismo=excluded.cod_organismo,
                     organismo=excluded.organismo,
                     actualizado_en=excluded.actualizado_en"#,
            )?;
            for r in rows {
                stmt.execute(params![
                    r.id,
                    r.referencia,
                    r.asunto,
                    parse_importe(&r.importe),
                    r.importe,
                    r.estado,
                    r.publicacion,
                    r.cod_organismo,
                    r.organismo,
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

    /// Consulta local con filtros, devolvendo filas combinadas (un contrato pode
    /// aparecer varias veces se ten varios lotes con adxudicatario distinto).
    pub fn query_local(&self, f: &LocalFilters) -> Result<Vec<LocalRow>> {
        let mut sql = String::from(
            r#"SELECT c.id, COALESCE(c.referencia,''), COALESCE(c.asunto,''),
                      COALESCE(c.importe_txt,''), COALESCE(c.estado,''),
                      COALESCE(c.data_publicacion,''), COALESCE(c.organismo,''),
                      COALESCE(r.adxudicatario,''), COALESCE(r.importe_resolucion_txt,''),
                      COALESCE(d.enlace_resolucion,'')
               FROM contracts c
               LEFT JOIN contract_detail d ON d.contract_id = c.id
               LEFT JOIN contract_resolucion r ON r.contract_id = c.id
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
            sql.push_str(" AND nrm(c.organismo) LIKE ?");
            args.push(format!("%{}%", normalize_search(&f.organismo)));
        }
        if !f.estado.trim().is_empty() {
            sql.push_str(" AND nrm(c.estado) LIKE ?");
            args.push(format!("%{}%", normalize_search(&f.estado)));
        }
        if !f.year.trim().is_empty() {
            sql.push_str(" AND c.data_publicacion LIKE ?");
            args.push(format!("%{}%", f.year.trim()));
        }
        if !f.adxudicatario.trim().is_empty() {
            sql.push_str(" AND nrm(r.adxudicatario) LIKE ?");
            args.push(format!("%{}%", normalize_search(&f.adxudicatario)));
        }
        sql.push_str(" ORDER BY c.data_publicacion DESC, c.id DESC LIMIT 5000");

        let mut stmt = self.conn.prepare(&sql)?;
        let params_dyn: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params_dyn.as_slice(), |row| {
            Ok(LocalRow {
                id: row.get(0)?,
                referencia: row.get(1)?,
                asunto: row.get(2)?,
                importe_txt: row.get(3)?,
                estado: row.get(4)?,
                publicacion: row.get(5)?,
                organismo: row.get(6)?,
                adxudicatario: row.get(7)?,
                importe_resolucion_txt: row.get(8)?,
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
            "SELECT DISTINCT organismo FROM contracts \
             WHERE TRIM(COALESCE(organismo,'')) <> '' ORDER BY organismo COLLATE NOCASE",
        )?;
        let estados = self.distinct(
            "SELECT DISTINCT estado FROM contracts \
             WHERE TRIM(COALESCE(estado,'')) <> '' ORDER BY estado COLLATE NOCASE",
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
            cod_organismo: String::new(),
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
}
