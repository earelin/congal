//! Extracción do detalle dun contrato e dos datos da súa resolución.

use crate::model::{ContractDetail, Resolucion, parse_importe};
use crate::scraper::{BASE, Client, clean_text, decode_bytes};
use anyhow::{Context, Result};
use regex::Regex;
use std::sync::OnceLock;

/// Descarga e analiza a páxina de detalle dun contrato.
pub fn fetch_detail(client: &Client, id: &str) -> Result<(ContractDetail, Vec<Resolucion>)> {
    let url = format!("{BASE}/licitacion?OP=50&N={id}&lang=gl");
    let resp = client
        .http()
        .get(&url)
        .send()
        .with_context(|| format!("GET detalle {id}"))?;
    let html = decode_bytes(&resp.bytes()?);
    Ok(parse_detail(id, &html))
}

fn dt_dd_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?is)<dt[^>]*>(.*?)</dt>\s*<dd[^>]*>(.*?)</dd>").unwrap()
    })
}

fn tr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<tr[^>]*>(.*?)</tr>").unwrap())
}

fn cell_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<t[dh][^>]*>(.*?)</t[dh]>").unwrap())
}

pub(crate) fn parse_detail(id: &str, html: &str) -> (ContractDetail, Vec<Resolucion>) {
    let mut d = ContractDetail {
        contract_id: id.to_string(),
        enlace_resolucion: format!("{BASE}/licitacion?N={id}"),
        ..Default::default()
    };

    for cap in dt_dd_re().captures_iter(html) {
        let label = clean_text(&cap[1]);
        let value = clean_text(&cap[2]);
        if label.is_empty() {
            continue;
        }
        let l = label.to_lowercase();
        let set = |field: &mut String, v: &str| {
            if field.is_empty() && !v.is_empty() {
                *field = v.to_string();
            }
        };
        if l.contains("referencia") {
            set(&mut d.referencia, &value);
        } else if l.contains("obxecto") {
            set(&mut d.obxecto, &value);
        } else if l.contains("tipo de tramitación") {
            set(&mut d.tipo_tramitacion, &value);
        } else if l.contains("tipo de procedemento") {
            set(&mut d.tipo_procedemento, &value);
        } else if l.contains("tipo de contrato") {
            set(&mut d.tipo_contrato, &value);
        } else if l.contains("orzamento base") {
            set(&mut d.orzamento_base, &value);
        } else if l.contains("valor estimado") {
            set(&mut d.valor_estimado, &value);
        } else if l.contains("lotes") {
            set(&mut d.num_lotes, &value);
        } else if l.contains("sistema de contratación") {
            set(&mut d.sistema_contratacion, &value);
        } else if l.contains("observaci") {
            set(&mut d.observacions, &value);
        } else if l.contains("data de difusión") {
            set(&mut d.data_difusion, &value);
        } else if l.contains("sara") {
            set(&mut d.sara, &value);
        } else if l.contains("centralizada") {
            set(&mut d.centralizada, &value);
        } else if l.contains("lei nacional") {
            set(&mut d.lei_aplicacion, &value);
        } else if !value.is_empty() {
            d.extra.entry(label).or_insert(value);
        }
    }

    let resolucions = parse_resolucions(html);
    (d, resolucions)
}

/// Analiza a táboa "Datos da resolución do procedemento" (unha fila por lote).
fn parse_resolucions(html: &str) -> Vec<Resolucion> {
    let Some(start) = html.find("Datos da resolución") else {
        return Vec::new();
    };
    let rest = &html[start..];
    // Acoutar á primeira táboa que segue ao título.
    let Some(tstart) = rest.find("<table") else {
        return Vec::new();
    };
    let tend = rest[tstart..]
        .find("</table>")
        .map(|e| tstart + e + "</table>".len())
        .unwrap_or(rest.len());
    let table = &rest[tstart..tend];

    let mut out = Vec::new();
    for (i, tr) in tr_re().captures_iter(table).enumerate() {
        // Saltar a fila de cabeceira.
        let row_html = &tr[1];
        if i == 0 || row_html.to_lowercase().contains("<th") {
            continue;
        }
        let cells: Vec<String> = cell_re()
            .captures_iter(row_html)
            .map(|c| clean_text(&c[1]))
            .collect();
        if cells.iter().all(|c| c.is_empty()) {
            continue;
        }
        let get = |idx: usize| cells.get(idx).cloned().unwrap_or_default();
        let importe_txt = get(4);
        let r = Resolucion {
            lote: get(0),
            participacion: get(1),
            estado_resolucion: get(2),
            adxudicatario: get(3),
            importe_num: parse_importe(&importe_txt),
            importe_txt,
            data_difusion: get(5),
            prazo_execucion: get(6),
            recurso: get(7),
        };
        // Ignorar filas baleiras de contido relevante.
        if r.estado_resolucion.is_empty() && r.adxudicatario.is_empty() {
            continue;
        }
        out.push(r);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrae_adxudicatario_e_importe_da_resolucion() {
        let bytes = include_bytes!("../../tests/fixtures/detalle_824418.html");
        let html = crate::scraper::decode_bytes(bytes);
        let (d, res) = parse_detail("824418", &html);

        assert_eq!(d.contract_id, "824418");
        assert!(d.enlace_resolucion.contains("N=824418"));
        assert!(!d.referencia.is_empty(), "referencia baleira");
        assert!(!res.is_empty(), "debe haber polo menos unha resolución");

        let soltec = res
            .iter()
            .find(|r| r.adxudicatario.contains("SOLTEC"))
            .expect("debe atoparse o adxudicatario SOLTEC");
        assert!(soltec.importe_txt.contains("3.502"), "importe={}", soltec.importe_txt);
        let n = soltec.importe_num.expect("importe numérico");
        assert!((3502.0..3503.0).contains(&n), "importe_num={n}");
        assert!(soltec.estado_resolucion.to_lowercase().contains("adxudic"));
    }

    #[test]
    fn detalle_anulado_ten_campos_pero_sen_adxudicatario() {
        let bytes = include_bytes!("../../tests/fixtures/detalle_824580.html");
        let html = crate::scraper::decode_bytes(bytes);
        let (d, res) = parse_detail("824580", &html);

        assert_eq!(d.referencia, "2024-142");
        assert!(d.tipo_contrato.to_lowercase().contains("serviz"));
        assert!(!d.orzamento_base.is_empty());
        assert!(
            res.iter().all(|r| !r.adxudicatario.contains("S.L.")),
            "non debería haber adxudicatario real nun contrato anulado"
        );
    }
}
