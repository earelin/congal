//! Busca de licitacións: POST a `resultadoIndex.jsp` e extracción do JSON oculto.

use crate::model::{ContractSummary, Filters};
use crate::scraper::{BASE, Client, decode_bytes};
use anyhow::{Context, Result};
use scraper::{Html, Selector};

/// Executa a busca cos filtros dados e devolve os rexistros do listado.
///
/// O filtro textual (`asunto`) aplícase localmente sobre o resultado, xa que o
/// buscador do servidor non o trata de forma fiable.
pub fn search(client: &Client, filters: &Filters) -> Result<Vec<ContractSummary>> {
    let estado = filters.estado_param();
    let form = [
        ("TC", filters.tipo_contrato.as_str()),
        ("TP", filters.tipo_procedemento.as_str()),
        ("TT", filters.tipo_tramitacion.as_str()),
        ("SC", filters.sistema.as_str()),
        ("ESTADO", estado.as_str()),
        ("CPV", filters.materia.as_str()),
        ("YEAR", filters.year.as_str()),
        ("OR", filters.organo.as_str()),
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

    if !filters.asunto.trim().is_empty() {
        let q = filters.asunto.to_lowercase();
        rows.retain(|r| {
            r.asunto.to_lowercase().contains(&q) || r.referencia.to_lowercase().contains(&q)
        });
    }
    Ok(rows)
}

/// Extrae o array JSON do input oculto `#resSearch`.
fn parse_results(html: &str) -> Result<Vec<ContractSummary>> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("#resSearch").unwrap();
    let Some(el) = doc.select(&sel).next() else {
        return Ok(Vec::new());
    };
    let val = el.value().attr("value").unwrap_or("").trim();
    if val.is_empty() || val == "[]" {
        return Ok(Vec::new());
    }
    let parsed: Vec<ContractSummary> =
        serde_json::from_str(val).context("parsing resSearch JSON")?;
    Ok(parsed)
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
        assert_eq!(rows[0].organismo, "SERGAS");
        assert_eq!(rows[1].asunto, "Subministración de papel");
    }

    #[test]
    fn ressearch_baleiro_devolve_lista_baleira() {
        let html = r#"<input id="resSearch" value="">"#;
        assert!(parse_results(html).unwrap().is_empty());
    }
}
