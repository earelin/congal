//! Enriquecemento dos adxudicatarios con datoscif.es: busca a entidade de cada
//! adxudicatario, vincúlaa con alta confianza e descarga os cargos da empresa.

use crate::db::Db;
use crate::model::{DatosCifEntidade, EstadoMatch, best_match, search_variants};
use crate::scraper::{self, Client};
use crate::worker::Event;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Resumo dun enriquecemento.
#[derive(Debug, Clone, Default)]
pub struct EnrichResult {
    pub procesados: usize,
    pub vinculados: usize,
    pub sen_match: usize,
    pub empresas_con_cargos: usize,
    pub cargos: usize,
    pub erros: usize,
}

/// Pausa de cortesía entre peticións a datoscif.
const THROTTLE: Duration = Duration::from_millis(350);

pub fn run_enrich(
    client: &Client,
    db: &mut Db,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
) -> Result<EnrichResult> {
    let _ = tx.send(Event::SyncProgress {
        done: 0,
        total: 0,
        msg: "Buscando adxudicatarios sen vincular…".into(),
    });

    let pendentes = db.adxudicatarios_pendentes()?;
    let total = pendentes.len();
    let mut result = EnrichResult::default();
    let now = crate::now_string();

    let _ = tx.send(Event::SyncProgress {
        done: 0,
        total,
        msg: format!("{total} adxudicatarios a vincular con datoscif"),
    });

    for (i, adx) in pendentes.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Event::Log("Enriquecemento cancelado.".into()));
            break;
        }

        // Buscar coas variantes do nome (reordenación para persoas), parando en
        // canto haxa un emparellamento fiable.
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();
        let mut decided = None;
        for (vi, variant) in search_variants(adx).into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if vi > 0 {
                std::thread::sleep(THROTTLE);
            }
            match scraper::search_entities(client, &variant) {
                Ok(found) => {
                    for s in found {
                        if seen.insert(s.url.clone()) {
                            suggestions.push(s);
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Event::Log(format!("Erro buscando «{variant}»: {e}")));
                }
            }
            let (url, conf) = best_match(adx, &suggestions);
            if conf.is_auto() {
                decided = Some((url, conf));
                break;
            }
        }
        let (url, conf) = decided.unwrap_or_else(|| best_match(adx, &suggestions));

        match (url, conf.is_auto()) {
            (Some(url), true) => {
                let sug = suggestions.iter().find(|s| s.url == url);
                if let Some(sug) = sug {
                    let mut ent = DatosCifEntidade {
                        url: sug.url.clone(),
                        nome: sug.nombre.clone(),
                        tipo_entidad: sug.tipo_entidad,
                        uri: sug.uri.clone(),
                        ..Default::default()
                    };
                    // Descargar a ficha (CIF, municipio…) só das empresas.
                    if ent.is_empresa() {
                        std::thread::sleep(THROTTLE);
                        match scraper::fetch_empresa_info(client, &ent.url) {
                            Ok(info) => {
                                ent.cif = info.cif;
                                ent.domicilio = info.domicilio;
                                ent.cod_postal = info.cod_postal;
                                ent.municipio = info.municipio;
                                ent.provincia = info.provincia;
                            }
                            Err(e) => {
                                let _ = tx.send(Event::Log(format!(
                                    "Erro descargando a ficha de {}: {e}",
                                    ent.url
                                )));
                            }
                        }
                    }
                    // A entidade débese gardar ANTES dos cargos: as FK de
                    // `datoscif_cargo` apuntan a `datoscif_entidade`.
                    db.upsert_datoscif_entidade(&ent, false, &now)?;

                    // Cargos (e propietarios) das empresas.
                    if ent.is_empresa() {
                        std::thread::sleep(THROTTLE);
                        match scraper::fetch_cargos(client, &ent.url) {
                            Ok(cargos) => {
                                if let Err(e) = db.upsert_cargos(&ent.url, &cargos, &now) {
                                    result.erros += 1;
                                    let _ = tx.send(Event::Log(format!(
                                        "Erro gardando cargos de {}: {e}",
                                        ent.url
                                    )));
                                } else {
                                    result.cargos += cargos.len();
                                    result.empresas_con_cargos += 1;
                                    // Marcar a entidade como xa procesada (flag pegañento).
                                    db.upsert_datoscif_entidade(&ent, true, &now)?;
                                }
                            }
                            Err(e) => {
                                result.erros += 1;
                                let _ = tx.send(Event::Log(format!(
                                    "Erro descargando cargos de {}: {e}",
                                    ent.url
                                )));
                            }
                        }
                    }
                    db.upsert_match(adx, Some(&ent.url), conf.as_str(), EstadoMatch::Auto.as_str(), &now)?;
                    result.vinculados += 1;
                } else {
                    db.upsert_match(adx, None, conf.as_str(), EstadoMatch::Pendente.as_str(), &now)?;
                    result.sen_match += 1;
                }
            }
            _ => {
                db.upsert_match(adx, None, conf.as_str(), EstadoMatch::Pendente.as_str(), &now)?;
                result.sen_match += 1;
            }
        }

        result.procesados += 1;
        let _ = tx.send(Event::SyncProgress {
            done: i + 1,
            total,
            msg: format!(
                "Vinculando {}/{} · {} vinculados · {} sen match",
                i + 1,
                total,
                result.vinculados,
                result.sen_match
            ),
        });
        std::thread::sleep(THROTTLE);
    }

    Ok(result)
}
