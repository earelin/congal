//! Extracción das opcións dos despregables de filtro desde `portada.jsp`.

use crate::model::FilterOptions;
use crate::scraper::{BASE, Client, clean_text, decode_bytes};
use anyhow::{Context, Result};
use regex::Regex;
use std::sync::OnceLock;

/// Descarga a portada e extrae as opcións dos selects de filtro.
pub fn load_filter_options(client: &Client) -> Result<FilterOptions> {
    let resp = client
        .http()
        .get(format!("{BASE}/portada.jsp?lang=gl"))
        .send()
        .context("GET portada.jsp")?;
    let html = decode_bytes(&resp.bytes()?);

    Ok(FilterOptions {
        organos: select_options(&html, "organoL"),
    })
}

/// Extrae os pares `(value, etiqueta)` dun `<select id="...">`.
fn select_options(html: &str, id: &str) -> Vec<(String, String)> {
    static OPTION: OnceLock<Regex> = OnceLock::new();
    let option_re = OPTION.get_or_init(|| {
        // O valor pode vir con comiñas dobres ou simples (organoL usa simples).
        Regex::new(r#"(?is)<option[^>]*\bvalue=["']([^"']*)["'][^>]*>(.*?)</option>"#).unwrap()
    });

    let pattern = format!(
        r#"(?is)<select[^>]*\bid="{}"[^>]*>(.*?)</select>"#,
        regex::escape(id)
    );
    let select_re = Regex::new(&pattern).unwrap();

    let Some(cap) = select_re.captures(html) else {
        return Vec::new();
    };
    let inner = &cap[1];
    let mut out = Vec::new();
    for o in option_re.captures_iter(inner) {
        let value = o[1].trim().to_string();
        let label = clean_text(&o[2]);
        if value.is_empty() {
            continue; // opción "Seleccione..." sen valor
        }
        out.push((value, label));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_opcions_de_filtro_da_portada() {
        let bytes = include_bytes!("../../tests/fixtures/portada.html");
        let html = crate::scraper::decode_bytes(bytes);

        let organos = select_options(&html, "organoL");
        assert!(organos.len() > 300, "organos={}", organos.len());
        assert!(
            organos
                .iter()
                .any(|(c, l)| c == "48" && l.contains("AGASP"))
        );

        assert_eq!(select_options(&html, "tcLMultiSelect").len(), 7);
        assert_eq!(select_options(&html, "scLMultiSelect").len(), 4);
        assert!(select_options(&html, "materiaMultiSelect").len() >= 40);
    }
}
