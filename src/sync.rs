//! Importación por organismo: licitacións (con detalle) e contratos menores,
//! a partir das APIs do perfil do contratante.

use crate::db::Db;
use crate::model::{ImportParams, is_estado_terminal};
use crate::scraper::{self, Client};
use crate::worker::Event;
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Resumo dunha importación.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    /// Licitacións atopadas no organismo.
    pub licitacions: usize,
    pub novos: usize,
    pub actualizados: usize,
    pub saltados: usize,
    pub detalles_descargados: usize,
    /// Contratos menores importados (do ano escollido).
    pub menores: usize,
    pub erros: usize,
}

/// Pausa de cortesía entre peticións de detalle.
const THROTTLE: Duration = Duration::from_millis(350);

/// Importa un organismo: as súas licitacións do ano (descargando o detalle das
/// novas ou aínda non terminais) e os contratos menores do mesmo ano.
pub fn run_import(
    client: &Client,
    db: &mut Db,
    p: &ImportParams,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
) -> Result<SyncResult> {
    let now = crate::now_string();
    let mut result = SyncResult::default();

    // ───────── Licitacións (do ano, filtradas no servidor) ─────────
    let _ = tx.send(Event::SyncProgress {
        done: 0,
        total: 0,
        msg: format!("Descargando licitacións de {}…", p.org_nome),
    });
    let summaries = scraper::search_licitaciones(client, &p.org_id, &p.org_nome, &p.ano)?;
    result.licitacions = summaries.len();

    // Decidir de cales descargar o detalle (novas ou aínda non terminais).
    let mut to_detail = Vec::new();
    for s in &summaries {
        match db.estado_previo(&s.id)? {
            None => {
                result.novos += 1;
                to_detail.push(s.id.clone());
            }
            Some(prev) if !is_estado_terminal(&prev) => {
                result.actualizados += 1;
                to_detail.push(s.id.clone());
            }
            Some(_) => result.saltados += 1,
        }
    }
    db.upsert_summaries(&summaries, &now)?;

    let total = to_detail.len();
    let _ = tx.send(Event::SyncProgress {
        done: 0,
        total,
        msg: format!(
            "{} licitacións · {} novas · {} a actualizar · {} terminais saltadas",
            result.licitacions, result.novos, result.actualizados, result.saltados
        ),
    });
    for (i, id) in to_detail.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Event::Log("Importación cancelada.".into()));
            db.set_meta("ultima_sync", &now)?;
            return Ok(result);
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
            msg: format!("Descargando detalle de licitacións {}/{}", i + 1, total),
        });
        std::thread::sleep(THROTTLE);
    }

    // ───────── Contratos menores (do ano) ─────────
    if !cancel.load(Ordering::Relaxed)
        && let Some(ano) = scraper::parse_ano(&p.ano)
    {
        let _ = tx.send(Event::SyncProgress {
            done: 0,
            total: 0,
            msg: format!("Descargando contratos menores de {ano}…"),
        });
        let mut progress = |n: usize| {
            let _ = tx.send(Event::SyncProgress {
                done: n,
                total: 0,
                msg: format!("Contratos menores: {n} descargados…"),
            });
        };
        let menores =
            scraper::fetch_contratos_menores(client, &p.org_id, ano, cancel, &mut progress)?;
        result.menores = menores.len();
        db.upsert_menores(&p.org_id, &p.org_nome, &menores, &now)?;
    }

    db.set_meta("ultima_sync", &now)?;
    Ok(result)
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::db::Db;
    use crate::model::{LocalFilters, TipoContrato};

    // Proba en vivo contra o servidor real. Executar con:
    //   cargo test --release -- --ignored --nocapture live_end_to_end
    #[test]
    #[ignore]
    fn live_end_to_end() {
        let client = Client::new().expect("cliente");

        // 1) Licitacións dun organismo pequeno (AGASP, id 48) vía resultadoIndex
        // (sen filtro de ano → todas).
        let lics = scraper::search_licitaciones(&client, "48", "AGASP", "").expect("licitacións");
        println!("AGASP licitacións: {}", lics.len());
        assert!(lics.len() > 10);
        assert!(lics.iter().all(|l| l.cod_organismo == "48"));

        // 2) Detalle dunha licitación coñecida con adxudicatario.
        let (d, res, _utes) = scraper::fetch_detail(&client, "824418").expect("detalle");
        println!("referencia={} lotes_res={}", d.referencia, res.len());
        assert!(res.iter().any(|r| r.adxudicatario.contains("SOLTEC")));

        // 3) Contratos menores dun ano (iteración de ventás).
        let cancel = Arc::new(AtomicBool::new(false));
        let mut noop = |_: usize| {};
        let menores =
            scraper::fetch_contratos_menores(&client, "48", 2024, &cancel, &mut noop).expect("CM");
        println!("AGASP contratos menores 2024: {}", menores.len());
        assert!(!menores.is_empty());

        // 4) Persistencia e consulta local.
        let tmp = std::env::temp_dir().join("cg_live_test.sqlite");
        let _ = std::fs::remove_file(&tmp);
        let mut db = Db::open(&tmp).expect("db");
        db.upsert_detail(&d, &res).expect("upsert detail");
        db.upsert_menores("48", "AGASP", &menores, &crate::now_string())
            .expect("upsert menores");

        let lf = LocalFilters {
            tipo: TipoContrato::Menor,
            ..Default::default()
        };
        let found = db.query_local(&lf).expect("query");
        println!("menores en BD: {} filas", found.len());
        assert!(!found.is_empty());
        assert!(found.iter().all(|r| r.tipo == TipoContrato::Menor));
    }
}
