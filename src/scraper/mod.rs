//! Cliente HTTP e funcións de extracción contra contratosdegalicia.gal.

mod detail;
mod options;
mod search;

pub use detail::fetch_detail;
pub use options::load_filter_options;
pub use search::search;

use anyhow::Result;
use std::time::Duration;

pub const BASE: &str = "https://www.contratosdegalicia.gal";

/// Cliente con cookie de sesión reutilizable.
pub struct Client {
    http: reqwest::blocking::Client,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .cookie_store(true)
            .user_agent(
                "Mozilla/5.0 (compatible; congal/0.1; +https://www.contratosdegalicia.gal)",
            )
            .gzip(true)
            .timeout(Duration::from_secs(180))
            .build()?;
        // Primeira chamada para obter cookie de sesión.
        let _ = http.get(format!("{BASE}/portada.jsp?lang=gl")).send();
        Ok(Client { http })
    }

    pub(crate) fn http(&self) -> &reqwest::blocking::Client {
        &self.http
    }
}

/// Decodifica bytes do servidor (ISO-8859-1 / Windows-1252) a `String`.
pub(crate) fn decode_bytes(bytes: &[u8]) -> String {
    let (cow, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
    cow.into_owned()
}

/// Elimina etiquetas HTML, decodifica entidades e colapsa espazos en branco.
pub(crate) fn clean_text(fragment: &str) -> String {
    // Quitar etiquetas.
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for c in fragment.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = html_escape::decode_html_entities(&out);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}
