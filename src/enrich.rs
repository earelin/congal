//! Enriquecemento dos adxudicatarios con datoscif.es: busca a entidade de cada
//! adxudicatario, vincúlaa con alta confianza e descarga os cargos da empresa.

use crate::db::Db;
use crate::model::{
    Confianza, DatosCifEntidade, EstadoMatch, MatchResult, NifKind, Suggestion, company_key,
    fallback_search_term, match_suggestions, nif_kind, normalize_nif, review_candidates,
    search_variants,
};
use crate::scraper::{self, Client};
use crate::worker::Event;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Modo de enriquecemento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrichMode {
    /// Procesa só os adxudicatarios aínda sen emparellar (novos).
    Novos,
    /// Volve descargar a ficha e os cargos de TODAS as empresas xa vinculadas,
    /// para actualizar datos que puidesen cambiar (novos administradores…).
    Reimportar,
}

/// Resumo dun enriquecemento.
#[derive(Debug, Clone, Default)]
pub struct EnrichResult {
    pub procesados: usize,
    pub vinculados: usize,
    pub sen_match: usize,
    pub empresas_con_cargos: usize,
    pub cargos: usize,
    pub erros: usize,
    /// Empresas membro de UTE vinculadas (validadas por CIF).
    pub ute_membros: usize,
    /// Casos deixados para revisión manual (con candidatos plausibles).
    pub a_revisar: usize,
    /// `true` se o resultado é dunha reimportación (cambia o resumo na interface).
    pub reimport: bool,
}

/// Pausa de cortesía entre peticións a datoscif.
const THROTTLE: Duration = Duration::from_millis(350);

/// Erro que se devolve cando datoscif deixa de responder ou nos bloquea (502,
/// timeouts…): detense o proceso con limpeza en lugar de seguir martelando.
fn erro_datoscif_bloqueado(client: &Client) -> anyhow::Error {
    anyhow::anyhow!(
        "datoscif está a bloquear as peticións (probablemente pola IP): {} fallos \
         seguidos. Detívose o proceso; agarda un tempo antes de volver tentalo.",
        client.fallos_datoscif()
    )
}

pub fn run_enrich(
    client: &Client,
    db: &mut Db,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
    mode: EnrichMode,
) -> Result<EnrichResult> {
    match mode {
        EnrichMode::Novos => enrich_novos(client, db, tx, cancel),
        EnrichMode::Reimportar => reimport_empresas(client, db, tx, cancel),
    }
}

/// Descarga a ficha (CIF, municipio…) e os cargos dunha empresa e gárdaos.
/// A entidade gárdase ANTES dos cargos porque as FK de `datoscif_cargo` apuntan
/// a `datoscif_entidade`. Asume que a chamada xa fixo o THROTTLE previo.
fn refresh_empresa(
    client: &Client,
    db: &mut Db,
    tx: &Sender<Event>,
    ent: &mut DatosCifEntidade,
    now: &str,
    result: &mut EnrichResult,
) -> Result<()> {
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
    db.upsert_datoscif_entidade(ent, false, now)?;

    std::thread::sleep(THROTTLE);
    match scraper::fetch_cargos(client, &ent.url) {
        Ok(cargos) => {
            if let Err(e) = db.upsert_cargos(&ent.url, &cargos, now) {
                result.erros += 1;
                let _ = tx.send(Event::Log(format!(
                    "Erro gardando cargos de {}: {e}",
                    ent.url
                )));
            } else {
                result.cargos += cargos.len();
                result.empresas_con_cargos += 1;
                // Marcar a entidade como xa procesada (flag pegañento).
                db.upsert_datoscif_entidade(ent, true, now)?;
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
    Ok(())
}

/// Reimporta a ficha e os cargos de todas as empresas xa vinculadas, para
/// refrescar os datos (p.ex. novos administradores). Non busca emparellamentos
/// novos: iso é traballo de [`enrich_novos`].
/// Resolve a man un caso de revisión: vincula o adxudicatario á entidade escollida
/// (descargando a súa ficha e cargos se é empresa) ou descártao se `escolla` é
/// `None`. En ambos casos elimina os candidatos pendentes do caso.
pub fn vincular_manual(
    client: &Client,
    db: &mut Db,
    tx: &Sender<Event>,
    adx_nome: &str,
    escolla: Option<Suggestion>,
) -> Result<()> {
    let now = crate::now_string();
    match escolla {
        Some(sug) => {
            let mut ent = DatosCifEntidade {
                url: sug.url.clone(),
                nome: sug.nombre.clone(),
                tipo_entidad: sug.tipo_entidad,
                uri: sug.uri.clone(),
                ..Default::default()
            };
            let mut result = EnrichResult::default();
            if ent.is_empresa() {
                refresh_empresa(client, db, tx, &mut ent, &now, &mut result)?;
            } else {
                db.upsert_datoscif_entidade(&ent, false, &now)?;
            }
            // confianza="manual": vínculo confirmado pola persoa usuaria.
            db.upsert_match(
                adx_nome,
                Some(&ent.url),
                "manual",
                EstadoMatch::Manual.as_str(),
                &now,
            )?;
        }
        None => {
            db.upsert_match(
                adx_nome,
                None,
                Confianza::SenMatch.as_str(),
                EstadoMatch::Descartado.as_str(),
                &now,
            )?;
        }
    }
    db.delete_candidatos(adx_nome)?;
    Ok(())
}

fn reimport_empresas(
    client: &Client,
    db: &mut Db,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
) -> Result<EnrichResult> {
    let _ = tx.send(Event::SyncProgress {
        done: 0,
        total: 0,
        msg: "Buscando empresas vinculadas…".into(),
    });

    client.reset_datoscif();

    let empresas = db.empresas_vinculadas()?;
    let total = empresas.len();
    let mut result = EnrichResult {
        reimport: true,
        ..Default::default()
    };
    let now = crate::now_string();

    let _ = tx.send(Event::SyncProgress {
        done: 0,
        total,
        msg: format!("{total} empresas a reimportar dende datoscif"),
    });

    for (i, mut ent) in empresas.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Event::Log("Reimportación cancelada.".into()));
            break;
        }
        if client.datoscif_bloqueado() {
            return Err(erro_datoscif_bloqueado(client));
        }
        let nome = ent.nome.clone();
        std::thread::sleep(THROTTLE);
        refresh_empresa(client, db, tx, &mut ent, &now, &mut result)?;
        result.procesados += 1;
        let _ = tx.send(Event::SyncProgress {
            done: i + 1,
            total,
            msg: format!("Reimportando {}/{} · {nome}", i + 1, total),
        });
    }

    Ok(result)
}

/// Busca en datoscif por cada termo (con throttle e respectando o cancelamento)
/// e acumula as suxestións novas en `suggestions`.
fn collect_search(
    client: &Client,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
    termos: impl IntoIterator<Item = String>,
    suggestions: &mut Vec<Suggestion>,
    seen: &mut HashSet<String>,
) {
    for termo in termos {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if termo.trim().is_empty() {
            continue;
        }
        std::thread::sleep(THROTTLE);
        match scraper::search_entities(client, &termo) {
            Ok(found) => {
                for s in found {
                    if seen.insert(s.url.clone()) {
                        suggestions.push(s);
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(Event::Log(format!("Erro buscando «{termo}»: {e}")));
            }
        }
    }
}

/// Entre os slugs de empresa candidatos, devolve o primeiro cuxa ficha teña o
/// CIF igual a `nif`. Baixa como moito `MAX_FICHAS` fichas (rede de seguridade).
fn cif_validate(
    client: &Client,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
    suggestions: &[Suggestion],
    urls: &[String],
    nif: &str,
) -> Option<String> {
    const MAX_FICHAS: usize = 8;
    let nif = normalize_nif(nif);
    let mut comprobadas = 0;
    for url in urls {
        if cancel.load(Ordering::Relaxed) || comprobadas >= MAX_FICHAS {
            break;
        }
        // Só as empresas expoñen CIF; as persoas non se poden validar así.
        let es_empresa = suggestions
            .iter()
            .any(|s| s.url == *url && s.tipo_entidad == 1);
        if !es_empresa {
            continue;
        }
        comprobadas += 1;
        std::thread::sleep(THROTTLE);
        match scraper::fetch_empresa_info(client, url) {
            Ok(info) if !info.cif.is_empty() && normalize_nif(&info.cif) == nif => {
                return Some(url.clone());
            }
            Ok(_) => {}
            Err(e) => {
                let _ = tx.send(Event::Log(format!("Erro validando o CIF de {url}: {e}")));
            }
        }
    }
    None
}

/// Resultado da decisión de emparellamento dun adxudicatario.
enum Decision {
    /// Vínculo fiable (único por nome ou validado por CIF): vincular automaticamente.
    Auto(String, Confianza),
    /// Sen vínculo fiable pero con candidatos plausibles: deixar para revisión manual.
    Revisar(Confianza, Vec<Suggestion>),
    /// Nin vínculo nin candidatos plausibles.
    SenMatch,
}

/// Empareja unha empresa membro dunha UTE (nome posiblemente truncado + CIF
/// fiable) coa súa entidade de datoscif, **esixindo coincidencia de CIF**. Busca
/// polo nome (e por un termo reducido) e valida a ficha dos candidatos contra o CIF.
fn match_member(
    client: &Client,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
    nome: &str,
    cif: &str,
) -> Option<Suggestion> {
    let mut suggestions: Vec<Suggestion> = Vec::new();
    let mut seen = HashSet::new();
    collect_search(
        client,
        tx,
        cancel,
        search_variants(nome),
        &mut suggestions,
        &mut seen,
    );
    let termo = fallback_search_term(nome);
    if !termo.is_empty() {
        collect_search(client, tx, cancel, [termo], &mut suggestions, &mut seen);
    }
    let urls: Vec<String> = suggestions
        .iter()
        .filter(|s| s.tipo_entidad == 1)
        .map(|s| s.url.clone())
        .collect();
    let url = cif_validate(client, tx, cancel, &suggestions, &urls, cif)?;
    suggestions.into_iter().find(|s| s.url == url)
}

/// Decide a entidade de datoscif dun adxudicatario. Estratexia:
/// 1. Buscar polas variantes do nome; se hai gañador único por nome, vincular.
/// 2. Se non, e coñecemos o CIF (empresa): reintentar cun termo reducido (núcleo
///    sen sufixos nin palabras curtas) e **validar por CIF** a ficha dos
///    candidatos. Un CIF coincidente é o sinal máis fiable.
/// 3. Se nada resolve pero hai candidatos plausibles, deixar para revisión manual.
///
/// As suxestións acumúlanse en `suggestions` para que o chamante constrúa a entidade.
fn decide_match(
    client: &Client,
    tx: &Sender<Event>,
    cancel: &Arc<AtomicBool>,
    adx: &str,
    nif: Option<&str>,
    suggestions: &mut Vec<Suggestion>,
    seen: &mut HashSet<String>,
) -> Decision {
    // 1) Busca polas variantes do nome.
    collect_search(client, tx, cancel, search_variants(adx), suggestions, seen);
    if let MatchResult::Unico(url, conf) = match_suggestions(adx, suggestions) {
        return Decision::Auto(url, conf);
    }

    // 2) Con CIF de empresa: reintento reducido + validación por CIF.
    let nif_empresa = nif.filter(|n| nif_kind(n) == Some(NifKind::Empresa));
    if let Some(nif) = nif_empresa {
        let termo = fallback_search_term(adx);
        if !termo.is_empty() && termo != company_key(adx) {
            collect_search(client, tx, cancel, [termo], suggestions, seen);
            // Tras ampliar, quizais xa hai gañador único por nome.
            if let MatchResult::Unico(url, conf) = match_suggestions(adx, suggestions) {
                return Decision::Auto(url, conf);
            }
        }
        let urls: Vec<String> = suggestions
            .iter()
            .filter(|s| s.tipo_entidad == 1)
            .map(|s| s.url.clone())
            .collect();
        if let Some(url) = cif_validate(client, tx, cancel, suggestions, &urls, nif) {
            return Decision::Auto(url, Confianza::Cif);
        }
    }

    // 3) Sen resolver: deixar candidatos plausibles para revisión manual.
    let conf = match match_suggestions(adx, suggestions) {
        MatchResult::Ambiguo(_) => Confianza::Ambigua,
        _ => Confianza::SenMatch,
    };
    let candidatos = review_candidates(adx, suggestions);
    if candidatos.is_empty() {
        Decision::SenMatch
    } else {
        Decision::Revisar(conf, candidatos)
    }
}

fn enrich_novos(
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

    // Contador de bloqueo limpo: este proceso parte de cero.
    client.reset_datoscif();

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

        // Emparellar: por nome e, se cómpre, validando por CIF co NIF da resolución.
        let nif = db.nif_de_adxudicatario(adx)?;
        let mut suggestions: Vec<Suggestion> = Vec::new();
        let mut seen = HashSet::new();
        let fallos_antes = client.fallos_datoscif();
        let decision = decide_match(
            client,
            tx,
            cancel,
            adx,
            nif.as_deref(),
            &mut suggestions,
            &mut seen,
        );

        // datoscif caeu/bloqueounos: deter o proceso (a mensaxe é clara) en vez de
        // seguir e gravar falsos «sen match».
        if client.datoscif_bloqueado() {
            return Err(erro_datoscif_bloqueado(client));
        }
        // Un fallo de rede que deixou as buscas baleiras NON é «sen match» (sería
        // un falso negativo): nese caso sáltase a empresa sen gravar nada.
        let fallo_transitorio =
            client.fallos_datoscif() > fallos_antes && matches!(decision, Decision::SenMatch);
        if fallo_transitorio {
            let _ = tx.send(Event::Log(format!(
                "«{adx}» saltado temporalmente: datoscif non respondeu."
            )));
        } else {
            match decision {
                Decision::Auto(url, conf) => {
                    let sug = suggestions
                        .iter()
                        .find(|s| s.url == url)
                        .expect("o url vén das suxestións");
                    let mut ent = DatosCifEntidade {
                        url: sug.url.clone(),
                        nome: sug.nombre.clone(),
                        tipo_entidad: sug.tipo_entidad,
                        uri: sug.uri.clone(),
                        ..Default::default()
                    };
                    // Só as empresas teñen ficha (CIF, municipio…) e cargos; as
                    // persoas gárdanse tal cal coa suxestión.
                    if ent.is_empresa() {
                        std::thread::sleep(THROTTLE);
                        refresh_empresa(client, db, tx, &mut ent, &now, &mut result)?;
                    } else {
                        db.upsert_datoscif_entidade(&ent, false, &now)?;
                    }
                    db.upsert_match(
                        adx,
                        Some(&ent.url),
                        conf.as_str(),
                        EstadoMatch::Auto.as_str(),
                        &now,
                    )?;
                    result.vinculados += 1;
                }
                Decision::Revisar(conf, candidatos) => {
                    db.upsert_candidatos(adx, &candidatos)?;
                    db.upsert_match(
                        adx,
                        None,
                        conf.as_str(),
                        EstadoMatch::Revisar.as_str(),
                        &now,
                    )?;
                    result.a_revisar += 1;
                }
                Decision::SenMatch => {
                    db.upsert_match(
                        adx,
                        None,
                        Confianza::SenMatch.as_str(),
                        EstadoMatch::Pendente.as_str(),
                        &now,
                    )?;
                    result.sen_match += 1;
                }
            }
        }

        result.procesados += 1;
        let _ = tx.send(Event::SyncProgress {
            done: i + 1,
            total,
            msg: format!(
                "Vinculando {}/{} · {} vinculados · {} a revisar · {} sen match",
                i + 1,
                total,
                result.vinculados,
                result.a_revisar,
                result.sen_match
            ),
        });
    }

    // Empresas membro das UTE adxudicatarias: emparéllanse por CIF (fiable) e
    // gárdanse coa súa ficha e cargos, para integrarse nas tramas.
    let membros = db.ute_membros_pendentes()?;
    if !membros.is_empty() {
        let total_m = membros.len();
        let _ = tx.send(Event::SyncProgress {
            done: 0,
            total: total_m,
            msg: format!("{total_m} empresas membro de UTE a vincular"),
        });
        for (i, (cif, nome)) in membros.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(Event::Log(
                    "Vinculación de membros de UTE cancelada.".into(),
                ));
                break;
            }
            if client.datoscif_bloqueado() {
                return Err(erro_datoscif_bloqueado(client));
            }
            if let Some(sug) = match_member(client, tx, cancel, nome, cif) {
                let mut ent = DatosCifEntidade {
                    url: sug.url.clone(),
                    nome: sug.nombre.clone(),
                    tipo_entidad: sug.tipo_entidad,
                    uri: sug.uri.clone(),
                    ..Default::default()
                };
                std::thread::sleep(THROTTLE);
                refresh_empresa(client, db, tx, &mut ent, &now, &mut result)?;
                result.ute_membros += 1;
            }
            let _ = tx.send(Event::SyncProgress {
                done: i + 1,
                total: total_m,
                msg: format!(
                    "Membros de UTE {}/{} · {} vinculados",
                    i + 1,
                    total_m,
                    result.ute_membros
                ),
            });
        }
    }

    Ok(result)
}
