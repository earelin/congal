//! Cliente das APIs JSON de datoscif.es: busca de entidades e descarga de cargos
//! (administradores, apoderados…) dunha empresa.
//!
//! A diferenza de contratosdegalicia.gal (ISO-8859-1 + HTML), datoscif responde
//! **JSON en UTF-8**, así que aquí NON se usa `decode_bytes`.

use crate::model::{CargoRow, EmpresaInfo, Suggestion, normalize_busca_datoscif, parse_data};
use crate::scraper::{Client, clean_text};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

pub const DATOSCIF_BASE: &str = "https://www.datoscif.es";

/// Tope de páxinas de cargos por entidade (rede de seguridade fronte a bucles).
const MAX_PAGINAS: i64 = 50;

/// Codifica un valor de formulario en `application/x-www-form-urlencoded` usando
/// **ISO-8859-1** (datoscif espera o ñ como `%F1`, non `%C3%91` de UTF-8). Os
/// puntos de código fóra de Latin-1 (raros tras normalizar) omítense.
fn form_encode_latin1(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => out.push('+'),
            c if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') => out.push(c),
            c if (c as u32) <= 0xFF => out.push_str(&format!("%{:02X}", c as u32)),
            _ => {}
        }
    }
    out
}

/// Busca entidades (empresas e persoas) por nome en datoscif. A busca está
/// anclada ao inicio (prefixo) e dobra os acentos agudos pero conserva o ñ; ademais
/// o parámetro `nombre` debe ir codificado en **ISO-8859-1** (ñ = `%F1`, non
/// `%C3%91`). Normalizamos co criterio de datoscif e enviamos o corpo en Latin-1.
pub fn search_entities(client: &Client, termo: &str) -> Result<Vec<Suggestion>> {
    let termo = normalize_busca_datoscif(termo);
    if termo.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!("{DATOSCIF_BASE}/sugerencias.ajax");
    let body = format!("nombre={}&tipo=autocompletar", form_encode_latin1(&termo));
    let resp = client
        .http()
        .post(&url)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .with_context(|| format!("POST sugerencias '{termo}'"))?;
    let body = resp.text().with_context(|| "lendo sugerencias")?;
    // O servidor pode devolver unha cadea baleira ou "[]" se non hai resultados.
    // Cando unha entidade foi renomeada, a suxestión trae `url_new`/`nombre_new`
    // e a ficha co CIF e cargos vive no slug NOVO: seguímolo aquí.
    let raw: Vec<SuggestionRaw> = serde_json::from_str(&body).unwrap_or_default();
    let suggestions = raw
        .into_iter()
        .map(|r| {
            let renomeada = !r.url_new.trim().is_empty();
            let ruta = if r.tipo_entidad == 2 {
                "directivo"
            } else {
                "empresa"
            };
            if renomeada {
                let url = r.url_new;
                Suggestion {
                    nombre: if r.nombre_new.trim().is_empty() {
                        r.nombre
                    } else {
                        r.nombre_new
                    },
                    uri: format!("/{ruta}/{url}"),
                    url,
                    tipo_entidad: r.tipo_entidad,
                }
            } else {
                Suggestion {
                    nombre: r.nombre,
                    url: r.url,
                    uri: r.uri,
                    tipo_entidad: r.tipo_entidad,
                }
            }
        })
        .collect();
    Ok(suggestions)
}

/// Fila crúa de `sugerencias.ajax`. Inclúe os campos `*_new` que datoscif engade
/// cando a entidade foi renomeada (apunta ao slug actual).
#[derive(Debug, Clone, Deserialize, Default)]
struct SuggestionRaw {
    #[serde(default)]
    nombre: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    uri: String,
    #[serde(default)]
    tipo_entidad: i64,
    #[serde(default)]
    url_new: String,
    #[serde(default)]
    nombre_new: String,
}

/// Estrutura crúa dunha fila de cargo en `/filtros.ajax`.
#[derive(Debug, Clone, Deserialize)]
struct CargoRaw {
    #[serde(default)]
    cargo: String,
    #[serde(default)]
    persona_url: String,
    #[serde(default)]
    persona_nombre: String,
    #[serde(default)]
    desde: String,
    #[serde(default)]
    hasta: String,
}

/// Resposta paxinada de `/filtros.ajax` (tipo=cargos).
#[derive(Debug, Clone, Deserialize)]
struct CargosPage {
    #[serde(default)]
    datos: Vec<CargoRaw>,
    #[serde(default)]
    num_paginas: i64,
}

/// Descarga TODOS os cargos (actuais e antigos) dunha empresa, paxinando ata
/// esgotar os resultados.
pub fn fetch_cargos(client: &Client, slug: &str) -> Result<Vec<CargoRow>> {
    let url = format!("{DATOSCIF_BASE}/filtros.ajax");
    let mut out = Vec::new();
    let mut pagina = 1;
    loop {
        let pag = pagina.to_string();
        let resp = client
            .http()
            .post(&url)
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&[
                ("tipo", "cargos"),
                ("activo", "2"), // 2 = ver todos (actuais + antigos)
                ("url", slug),
                ("pagina", pag.as_str()),
                ("cargo", ""),
                ("nombre", ""),
                ("tipo_entidad", "1"),
            ])
            .send()
            .with_context(|| format!("POST cargos {slug} páxina {pagina}"))?;
        let body = resp.text().with_context(|| "lendo cargos")?;
        let page: CargosPage = serde_json::from_str(&body).unwrap_or(CargosPage {
            datos: Vec::new(),
            num_paginas: 0,
        });
        for r in page.datos {
            if r.persona_url.trim().is_empty() || es_cargo_excluido(&r.cargo) {
                continue;
            }
            let activo = r.hasta.trim().is_empty();
            out.push(CargoRow {
                persona_url: r.persona_url,
                persona_nome: r.persona_nombre,
                cargo: r.cargo,
                desde: parse_data(&r.desde).unwrap_or(r.desde),
                hasta: parse_data(&r.hasta).unwrap_or(r.hasta),
                activo,
            });
        }
        if pagina >= page.num_paginas || pagina >= MAX_PAGINAS {
            break;
        }
        pagina += 1;
    }
    Ok(out)
}

/// Cargos que non interesan para as relacións (p.ex. auditores: non implican
/// control nin propiedade da empresa).
fn es_cargo_excluido(cargo: &str) -> bool {
    cargo.to_lowercase().contains("auditor")
}

/// Captura o valor dun `<span itemprop="NOME">…</span>` (microdata) da páxina.
fn itemprop_re(prop: &str) -> Regex {
    Regex::new(&format!(r#"(?is)itemprop="{prop}"[^>]*>(.*?)</span>"#)).unwrap()
}

/// Descarga e analiza a ficha HTML dunha empresa (CIF, domicilio, municipio,
/// provincia…). Os datos veñen en microdata schema.org (`itemprop`).
pub fn fetch_empresa_info(client: &Client, slug: &str) -> Result<EmpresaInfo> {
    let url = format!("{DATOSCIF_BASE}/empresa/{slug}");
    let resp = client
        .http()
        .get(&url)
        .send()
        .with_context(|| format!("GET ficha {slug}"))?;
    // datoscif é UTF-8: lemos o corpo como texto directamente.
    let html = resp.text().with_context(|| "lendo ficha empresa")?;
    Ok(parse_empresa_info(&html))
}

pub(crate) fn parse_empresa_info(html: &str) -> EmpresaInfo {
    let get = |prop: &str| -> String {
        itemprop_re(prop)
            .captures(html)
            .map(|c| clean_text(&c[1]))
            .unwrap_or_default()
    };
    EmpresaInfo {
        cif: get("taxID"),
        domicilio: get("streetAddress"),
        cod_postal: get("postalCode"),
        municipio: get("addressLocality"),
        provincia: get("addressRegion"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // O parámetro `nombre` codifícase en ISO-8859-1 (ñ → %F1, espazo → +).
    #[test]
    fn codifica_form_en_latin1() {
        assert_eq!(
            form_encode_latin1("club basquet coruña sad"),
            "club+basquet+coru%F1a+sad"
        );
    }

    // Verifica o parsing das suxestións contra unha resposta real capturada.
    #[test]
    fn parsea_sugerencias() {
        let body = include_str!("../../tests/fixtures/datoscif_sugerencias_inditex.json");
        let s: Vec<Suggestion> = serde_json::from_str(body).expect("json");
        assert!(!s.is_empty());
        let inditex = s
            .iter()
            .find(|x| x.url == "inditex-sa")
            .expect("debe estar INDITEX SA");
        assert_eq!(inditex.nombre, "INDITEX SA");
        assert_eq!(inditex.tipo_entidad, 1);
    }

    // Verifica o parsing dunha páxina de cargos real.
    #[test]
    fn parsea_cargos() {
        let body = include_str!("../../tests/fixtures/datoscif_cargos_inditex.json");
        let page: CargosPage = serde_json::from_str(body).expect("json");
        assert_eq!(page.num_paginas, 10);
        assert_eq!(page.datos.len(), 10);
        let primeiro = &page.datos[0];
        assert!(!primeiro.persona_url.is_empty());
        assert!(!primeiro.persona_nombre.is_empty());
        assert!(!primeiro.cargo.is_empty());
    }

    // Extracción do enderezo (municipio, provincia, CP, domicilio) do microdata.
    #[test]
    fn parsea_enderezo_empresa() {
        let html = include_str!("../../tests/fixtures/datoscif_empresa_caru.html");
        let info = parse_empresa_info(html);
        assert_eq!(info.municipio, "Santander");
        assert_eq!(info.provincia, "Cantabria"); // o espazo inicial límpase
        assert_eq!(info.cod_postal, "39001");
        assert!(info.domicilio.contains("Cardenal Cisneros"));
    }

    // Extracción do CIF (non todas as fichas o teñen; INDITEX si).
    #[test]
    fn parsea_cif_empresa() {
        let html = include_str!("../../tests/fixtures/datoscif_empresa_inditex.html");
        let info = parse_empresa_info(html);
        assert_eq!(info.cif, "A28601094");
    }

    // Os auditores exclúense; os cargos de control/propiedade consérvanse.
    #[test]
    fn exclue_auditores() {
        assert!(es_cargo_excluido("Auditor"));
        assert!(es_cargo_excluido("Auditor de Cuentas"));
        assert!(es_cargo_excluido("Auditor Suplente"));
        assert!(!es_cargo_excluido("Administrador Único"));
        assert!(!es_cargo_excluido("Apoderado"));
        assert!(!es_cargo_excluido("Socio Unico"));
    }

    // Proba en vivo contra datoscif.es. Executar con:
    //   cargo test --release -- --ignored --nocapture live_datoscif
    #[test]
    #[ignore]
    fn live_datoscif() {
        let client = Client::new().expect("cliente");

        let sug = search_entities(&client, "inditex").expect("busca");
        println!("sugerencias inditex: {}", sug.len());
        let inditex = sug
            .iter()
            .find(|s| s.url == "inditex-sa")
            .expect("debe atoparse inditex-sa");
        assert_eq!(inditex.tipo_entidad, 1);

        // Membro de UTE renomeado: busca polo nome (truncado/antigo) → segue
        // `url_new` ao slug actual, e a ficha trae o CIF do contrato (A85788073).
        let sug =
            search_entities(&client, "AQUATEC PROYECTOS PARA EL SECTOR DEL").expect("busca membro");
        let m = sug.first().expect("debe haber suxestión");
        println!("membro UTE → slug actual: {}", m.url);
        let info = fetch_empresa_info(&client, &m.url).expect("ficha membro");
        assert_eq!(
            info.cif, "A85788073",
            "o CIF do contrato debe casar coa ficha nova"
        );

        // Nome con ñ e acento: debe atoparse (codificación ISO-8859-1 + ñ mantido).
        let sug = search_entities(&client, "CLUB BÁSQUET CORUÑA, S.A.D").expect("busca ñ");
        println!("sugerencias club básquet coruña: {}", sug.len());
        assert!(
            sug.iter().any(|s| s.url == "club-basquet-coruna-sad"),
            "debe atoparse club-basquet-coruna-sad cun nome acentuado/con ñ"
        );

        let cargos = fetch_cargos(&client, "inditex-sa").expect("cargos");
        println!("cargos inditex-sa: {}", cargos.len());
        // INDITEX SA ten decenas de cargos (actuais + antigos) e varias páxinas.
        assert!(
            cargos.len() > 20,
            "esperábanse moitos cargos, hai {}",
            cargos.len()
        );
        assert!(cargos.iter().all(|c| !c.persona_url.is_empty()));

        let info = fetch_empresa_info(&client, "inditex-sa").expect("ficha");
        println!(
            "CIF={} municipio={} provincia={}",
            info.cif, info.municipio, info.provincia
        );
        assert_eq!(info.cif, "A28601094");
    }
}
