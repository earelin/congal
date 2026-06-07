//! Sincronización incremental: busca + descarga de detalle só do necesario.

use crate::db::Db;
use crate::model::{Filters, is_estado_terminal};
use crate::scraper::{self, Client};
use crate::worker::Event;
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Resumo dunha sincronización.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    pub atopados: usize,
    pub novos: usize,
    pub actualizados: usize,
    pub saltados: usize,
    pub detalles_descargados: usize,
    pub erros: usize,
}

/// Pausa de cortesía entre peticións de detalle.
const THROTTLE: Duration = Duration::from_millis(350);

pub fn run_sync(
    client: &Client,
    db: &mut Db,
    filters: &Filters,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
) -> Result<SyncResult> {
    let _ = tx.send(Event::SyncProgress {
        done: 0,
        total: 0,
        msg: "Buscando no servidor…".into(),
    });

    let summaries = scraper::search(client, filters)?;
    let mut result = SyncResult {
        atopados: summaries.len(),
        ..Default::default()
    };

    // Clasificar: que gardar e de que descargar o detalle.
    let mut to_upsert = Vec::new();
    let mut to_detail = Vec::new();
    for s in &summaries {
        match db.estado_previo(&s.id)? {
            None => {
                result.novos += 1;
                to_upsert.push(s.clone());
                to_detail.push(s.id.clone());
            }
            Some(prev) if !is_estado_terminal(&prev) => {
                result.actualizados += 1;
                to_upsert.push(s.clone());
                to_detail.push(s.id.clone());
            }
            Some(_) => {
                result.saltados += 1;
            }
        }
    }

    let now = crate::now_string();
    db.upsert_summaries(&to_upsert, &now)?;

    let total = to_detail.len();
    let _ = tx.send(Event::SyncProgress {
        done: 0,
        total,
        msg: format!(
            "{} atopados · {} novos · {} a actualizar · {} resoltos saltados",
            result.atopados, result.novos, result.actualizados, result.saltados
        ),
    });

    for (i, id) in to_detail.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Event::Log("Sincronización cancelada.".into()));
            break;
        }
        match scraper::fetch_detail(client, id) {
            Ok((detail, resolucions, utes)) => {
                let r = db
                    .upsert_detail(&detail, &resolucions)
                    .and_then(|_| db.upsert_utes(&detail.contract_id, &utes));
                if let Err(e) = r {
                    result.erros += 1;
                    let _ = tx.send(Event::Log(format!("Erro gardando {id}: {e}")));
                } else {
                    result.detalles_descargados += 1;
                }
            }
            Err(e) => {
                result.erros += 1;
                let _ = tx.send(Event::Log(format!("Erro descargando {id}: {e}")));
            }
        }
        let _ = tx.send(Event::SyncProgress {
            done: i + 1,
            total,
            msg: format!("Descargando detalle {}/{}", i + 1, total),
        });
        std::thread::sleep(THROTTLE);
    }

    db.set_meta("ultima_sync", &now)?;
    Ok(result)
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::db::Db;
    use crate::model::{Filters, LocalFilters};

    // Proba en vivo contra o servidor real. Executar con:
    //   cargo test --release -- --ignored --nocapture live_end_to_end
    #[test]
    #[ignore]
    fn live_end_to_end() {
        let client = Client::new().expect("cliente");

        // 1) Detalle dun contrato coñecido con adxudicatario.
        let (d, res, _utes) = scraper::fetch_detail(&client, "824418").expect("detalle");
        println!("referencia={} lotes_res={}", d.referencia, res.len());
        assert!(res.iter().any(|r| r.adxudicatario.contains("SOLTEC")));

        // 2) Busca acoutada (un ano) e persistencia en BD temporal.
        let filters = Filters {
            year: "2025".into(),
            ..Default::default()
        };
        let rows = scraper::search(&client, &filters).expect("busca");
        println!("busca 2025: {} rexistros", rows.len());
        assert!(rows.len() > 100);

        let tmp = std::env::temp_dir().join("cg_live_test.sqlite");
        let _ = std::fs::remove_file(&tmp);
        let mut db = Db::open(&tmp).expect("db");
        db.upsert_summaries(&rows[..50.min(rows.len())], &crate::now_string())
            .expect("upsert");
        db.upsert_detail(&d, &res).expect("upsert detail");

        // 3) Consulta local por adxudicatario + exportación ODS.
        let lf = LocalFilters {
            adxudicatario: "SOLTEC".into(),
            ..Default::default()
        };
        let found = db.query_local(&lf).expect("query");
        println!("busca local SOLTEC: {} filas", found.len());
        assert!(found.iter().any(|r| r.adxudicatario.contains("SOLTEC")));

        let ods = std::env::temp_dir().join("cg_live_test.ods");
        crate::export::export_ods(&ods, &found).expect("export");
        assert!(std::fs::metadata(&ods).unwrap().len() > 0);
        println!("ODS escrito en {}", ods.display());
    }
}
