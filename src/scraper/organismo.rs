//! Acceso á API JSON do perfil do contratante de contratosdegalicia.gal.
//!
//! As táboas do perfil dun organismo aliméntanse de dúas rutas REST que
//! devolven **JSON en UTF-8** (a diferenza do resto do sitio, que é HTML
//! ISO-8859-1), polo que aquí parséase directamente con `serde_json`:
//!   - `api/v1/organismos/{id}/licitaciones/table`    (todas as licitacións)
//!   - `api/v1/organismos/{id}/contratosmenores/table` (só por ventás de datas)
//!
//! O `{id}` é o valor do despregable `organoL` da portada (o que xa parsea
//! `options.rs`). As respostas son do formato DataTables:
//! `{recordsTotal, recordsFiltered, data:[…]}`.

use crate::model::MenorRow;
use crate::scraper::{BASE, Client};
use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Tamaño de páxina ao paxinar unha táboa. O servidor rexeita (HTTP 500) os
/// tamaños grandes; 100 é o máximo admitido.
const PAGE: usize = 100;

/// Ancho máximo (en días) dunha ventá de contratos menores. O servidor rexeita
/// (HTTP 500) os intervalos longos; ~3 meses funcionan, así que usamos 80 días.
const WINDOW_DAYS: i64 = 80;

/// Resposta paxinada estilo DataTables.
#[derive(Debug, Deserialize)]
struct TablePage<T> {
    #[serde(rename = "recordsFiltered", default)]
    records_filtered: i64,
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

/// GET dunha ruta da API cos parámetros dados, esixindo status 2xx e devolvendo
/// o corpo como texto (UTF-8). Os valores son sempre ASCII seguro (ids, datas,
/// números, `gl`), así que se concatenan directamente á URL.
fn get_json(client: &Client, base_url: &str, query: &[(&str, &str)]) -> Result<String> {
    let qs = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let url = format!("{base_url}?{qs}");
    let resp = client
        .http()
        .get(&url)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Accept", "application/json")
        .send()
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("GET {url}"))?;
    resp.text().with_context(|| format!("lendo {url}"))
}

/// Analiza a resposta de `contratosmenores/table`, recortando os espazos de
/// recheo dos campos de texto. Devolve as filas e o total filtrado pola ventá.
pub(crate) fn parse_contratos_menores(json: &str) -> Result<(Vec<MenorRow>, i64)> {
    let mut page: TablePage<MenorRow> = serde_json::from_str(json)?;
    for r in &mut page.data {
        r.publicado = r.publicado.trim().to_string();
        r.objeto = r.objeto.trim().to_string();
        r.nif = r.nif.trim().to_string();
        r.adjudicatario = r.adjudicatario.trim().to_string();
        r.duracion = r.duracion.trim().to_string();
    }
    Ok((page.data, page.records_filtered))
}

/// Descarga os contratos menores dun organismo para un ano completo. Como o
/// servidor só admite ventás curtas de datas, itéranse ventás de [`WINDOW_DAYS`]
/// días (acoutando ao día de hoxe), paxinando cada unha e deduplicando por id.
/// `progress(acumulados)` chámase ao rematar cada ventá; respéctase `cancel`.
pub fn fetch_contratos_menores(
    client: &Client,
    org_id: &str,
    ano: i32,
    cancel: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(usize),
) -> Result<Vec<MenorRow>> {
    let url = format!("{BASE}/api/v1/organismos/{org_id}/contratosmenores/table");
    let mut out = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();

    let inicio = NaiveDate::from_ymd_opt(ano, 1, 1).context("ano inválido")?;
    let fin_ano = NaiveDate::from_ymd_opt(ano, 12, 31).context("ano inválido")?;
    // Non ten sentido pedir datas futuras (o servidor limita ata hoxe).
    let hoxe = Local::now().date_naive();
    let fin = fin_ano.min(hoxe);
    if inicio > fin {
        return Ok(out);
    }

    let mut ws = inicio;
    while ws <= fin {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let we = (ws + Duration::days(WINDOW_DAYS - 1)).min(fin);
        let (ds, de) = (ws.to_string(), we.to_string());
        let mut start = 0usize;
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let body = get_json(
                client,
                &url,
                &[
                    ("idioma", "gl"),
                    ("draw", "1"),
                    ("start", &start.to_string()),
                    ("length", &PAGE.to_string()),
                    ("datestart", &ds),
                    ("dateend", &de),
                ],
            )?;
            let (rows, filtered) = parse_contratos_menores(&body)?;
            let n = rows.len();
            for r in rows {
                if seen.insert(r.id) {
                    out.push(r);
                }
            }
            start += n;
            if n == 0 || start as i64 >= filtered {
                break;
            }
        }
        progress(out.len());
        ws = we + Duration::days(1);
    }
    Ok(out)
}

/// Ano (i32) a partir do texto do filtro; `None` se non é un ano válido.
pub fn parse_ano(s: &str) -> Option<i32> {
    s.trim()
        .parse::<i32>()
        .ok()
        .filter(|y| (2000..=2100).contains(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_contratos_menores_e_recorta_espazos() {
        let json = include_str!("../../tests/fixtures/api_contratosmenores_agasp.json");
        let (rows, filtered) = parse_contratos_menores(json).expect("parse");
        assert_eq!(filtered, rows.len() as i64);
        let r = &rows[0];
        assert_eq!(r.id, 1428752);
        // Os campos veñen con recheo de espazos: deben quedar limpos.
        assert_eq!(r.nif, r.nif.trim());
        assert_eq!(r.adjudicatario, r.adjudicatario.trim());
        assert!(!r.nif.contains(' ') || r.nif.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(!r.adjudicatario.is_empty());
        assert!(!r.duracion.is_empty());
    }

    #[test]
    fn ano_valido() {
        assert_eq!(parse_ano("2024"), Some(2024));
        assert_eq!(parse_ano(" 2008 "), Some(2008));
        assert_eq!(parse_ano(""), None);
        assert_eq!(parse_ano("abc"), None);
    }
}
