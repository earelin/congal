//! Busca de licitacións por organismo + ano: `POST resultadoIndex.jsp` e
//! extracción do JSON oculto `#resSearch`. A API do perfil do contratante non
//! filtra licitacións por ano, así que para iso seguimos usando o buscador, que
//! si filtra no servidor (parámetros `OR` = organismo e `YEAR` = ano).

use crate::model::{ContractSummary, TipoContrato};
use crate::scraper::{BASE, Client, decode_bytes};
use anyhow::{Context, Result};
use scraper::{Html, Selector};
use serde::Deserialize;
use std::collections::HashSet;

/// Fila crúa do JSON oculto `#resSearch`.
#[derive(Debug, Clone, Deserialize)]
struct SearchRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    referencia: String,
    #[serde(default)]
    asunto: String,
    #[serde(default)]
    importe: String,
    #[serde(default)]
    estado: String,
    #[serde(default)]
    publicacion: String,
}

/// Busca as licitacións dun organismo nun ano (filtro no servidor) e devólveas
/// como `ContractSummary` (tipo licitación). O `cod_organismo`/`organismo`
/// fíxanse aos da importación (o id de `organoL`, o mesmo que usan os menores),
/// e NON ao `codOrganismo` do JSON, que pertence a outro espazo de códigos.
/// Se `year` está baleiro, o servidor devolve as licitacións de todos os anos.
pub fn search_licitaciones(
    client: &Client,
    org_id: &str,
    org_nome: &str,
    year: &str,
) -> Result<Vec<ContractSummary>> {
    let form = [
        ("TC", ""),
        ("TP", ""),
        ("TT", ""),
        ("SC", ""),
        ("ESTADO", "1,2,3,4,5,6,7,8"),
        ("CPV", ""),
        ("YEAR", year),
        ("OR", org_id),
        ("SO", "1"),
        ("FE", "1"),
    ];

    let resp = client
        .http()
        .post(format!("{BASE}/resultadoIndex.jsp?lang=gl"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&form)
        .send()
        .context("POST resultadoIndex.jsp")?;
    let html = decode_bytes(&resp.bytes()?);

    let mut rows = parse_results(&html)?;
    // O servidor pode repetir un contrato (unha entrada por lote); `id`
    // identifícao univocamente, así que deduplicamos conservando o primeiro.
    let mut vistos = HashSet::new();
    rows.retain(|r| vistos.insert(r.id.clone()));

    Ok(rows
        .into_iter()
        .map(|r| ContractSummary {
            id: r.id,
            tipo: TipoContrato::Licitacion,
            referencia: r.referencia,
            asunto: r.asunto,
            importe: r.importe,
            estado: r.estado,
            publicacion: r.publicacion,
            cod_organismo: org_id.to_string(),
            organismo: org_nome.to_string(),
        })
        .collect())
}

/// Extrae o array JSON do input oculto `#resSearch`.
fn parse_results(html: &str) -> Result<Vec<SearchRow>> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("#resSearch").unwrap();
    let Some(el) = doc.select(&sel).next() else {
        return Ok(Vec::new());
    };
    let val = el.value().attr("value").unwrap_or("").trim();
    if val.is_empty() || val == "[]" {
        return Ok(Vec::new());
    }
    serde_json::from_str(val).context("parsing resSearch JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<html><body>
      <input type="hidden" id="resSearch" value="[{&quot;id&quot;:&quot;1&quot;,&quot;referencia&quot;:&quot;R1&quot;,&quot;asunto&quot;:&quot;Obra do hospital de Lugo&quot;,&quot;importe&quot;:&quot;1.000,00&quot;,&quot;estado&quot;:&quot;Formalizado&quot;,&quot;publicacion&quot;:&quot;01-01-2025&quot;,&quot;codOrganismo&quot;:&quot;10&quot;,&quot;organismo&quot;:&quot;SERGAS&quot;},{&quot;id&quot;:&quot;2&quot;,&quot;referencia&quot;:&quot;R2&quot;,&quot;asunto&quot;:&quot;Subministración de papel&quot;,&quot;importe&quot;:&quot;500,00&quot;,&quot;estado&quot;:&quot;Adxudicado&quot;,&quot;publicacion&quot;:&quot;02-02-2025&quot;,&quot;codOrganismo&quot;:&quot;11&quot;,&quot;organismo&quot;:&quot;Concello&quot;}]">
    </body></html>"#;

    #[test]
    fn parsea_dous_rexistros_do_ressearch() {
        let rows = parse_results(SAMPLE).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "1");
        assert_eq!(rows[0].asunto, "Obra do hospital de Lugo");
        assert_eq!(rows[1].asunto, "Subministración de papel");
    }

    #[test]
    fn ressearch_baleiro_devolve_lista_baleira() {
        let html = r#"<input id="resSearch" value="">"#;
        assert!(parse_results(html).unwrap().is_empty());
    }

    // O id identifica univocamente cada contrato: se o servidor repite un id
    // (unha entrada por lote), tras deduplicar debe quedar unha soa fila.
    #[test]
    fn deduplica_contratos_repetidos_polo_id() {
        let html = r#"<input id="resSearch" value="[{&quot;id&quot;:&quot;1&quot;,&quot;asunto&quot;:&quot;Lote A&quot;},{&quot;id&quot;:&quot;1&quot;,&quot;asunto&quot;:&quot;Lote B&quot;},{&quot;id&quot;:&quot;2&quot;,&quot;asunto&quot;:&quot;Outro&quot;}]">"#;
        let mut rows = parse_results(html).unwrap();
        assert_eq!(rows.len(), 3);
        let mut vistos = HashSet::new();
        rows.retain(|r| vistos.insert(r.id.clone()));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].asunto, "Lote A"); // consérvase a primeira aparición
    }
}
