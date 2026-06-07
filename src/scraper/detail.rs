//! Extracción do detalle dun contrato e dos datos da súa resolución.

use crate::model::{
    ContractDetail, Resolucion, Ute, UteMembro, company_key, normalize_nif, parse_importe,
};
use crate::scraper::{BASE, Client, clean_text, decode_bytes};
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Descarga e analiza a páxina de detalle dun contrato.
pub fn fetch_detail(
    client: &Client,
    id: &str,
) -> Result<(ContractDetail, Vec<Resolucion>, Vec<Ute>)> {
    let url = format!("{BASE}/licitacion?OP=50&N={id}&lang=gl");
    let resp = client
        .http()
        .get(&url)
        .send()
        .with_context(|| format!("GET detalle {id}"))?;
    let html = decode_bytes(&resp.bytes()?);
    let (detail, resolucions) = parse_detail(id, &html);
    let utes = parse_utes(&html, &resolucions);
    Ok((detail, resolucions, utes))
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

/// Token NIF/CIF español: CIF empresa, NIF persoa (8 díxitos + letra) ou NIE.
fn nif_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b([ABCDEFGHJNPQRSUVW][0-9]{7}[0-9A-J]|[XYZ][0-9]{7}[A-Z]|[0-9]{8}[A-Z])\b")
            .unwrap()
    })
}

/// Filas da táboa oculta de licitadores (`<tr class="… filasLicitadores …">`).
fn filaslic_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<tr[^>]*filasLicitadores[^>]*>(.*?)</tr>").unwrap())
}

/// Cela «Nome <br/> NIF» (táboa de formalización «Contratista»): captura o texto
/// antes do `<br>` (nome) e o que vén despois ata a seguinte etiqueta.
fn name_br_nif_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)([^<>]{4,})<br\s*/?>([^<]{4,40})").unwrap())
}

fn li_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<li[^>]*>(.*?)</li>").unwrap())
}

/// Extrae as UTE **adxudicatarias** do popup de licitadores. Cada licitador é un
/// `<li>` dentro dun `<ul class='list-unstyled'>`; unha UTE distínguese por ter un
/// `<ul>` aniñado co `<li>CIF - NOME</li>` de cada empresa membro. Só se devolven
/// as UTE cuxo nome coincide cun adxudicatario (gañador) do contrato.
pub(crate) fn parse_utes(html: &str, resolucions: &[Resolucion]) -> Vec<Ute> {
    use std::collections::HashSet;
    let ganadores: HashSet<String> = resolucions
        .iter()
        .map(|r| company_key(&r.adxudicatario))
        .filter(|k| !k.is_empty())
        .collect();

    let mut out = Vec::new();
    for tr in filaslic_re().captures_iter(html) {
        let cells: Vec<&str> = cell_re()
            .captures_iter(&tr[1])
            .map(|c| c.get(1).map_or("", |m| m.as_str()))
            .collect();
        if cells.len() < 3 {
            continue;
        }
        let nome_raw = cells[2];

        // O <ul> aniñado é o segundo <ul> da cela; sen el non é unha UTE.
        let Some(first_ul) = nome_raw.find("<ul") else { continue };
        let Some(inner_ul) = nome_raw[first_ul + 3..].find("<ul").map(|p| first_ul + 3 + p) else {
            continue;
        };
        let Some(inner_open_end) = nome_raw[inner_ul..].find('>').map(|p| inner_ul + p + 1) else {
            continue;
        };
        let inner_close = nome_raw[inner_open_end..]
            .find("</ul>")
            .map(|p| inner_open_end + p)
            .unwrap_or(nome_raw.len());
        let inner = &nome_raw[inner_open_end..inner_close];

        // Nome da UTE: o primeiro <li> (antes da ul aniñada).
        let ute_nome = li_re()
            .captures(&nome_raw[..inner_ul])
            .map(|c| clean_text(&c[1]))
            .unwrap_or_default();
        // Só UTE adxudicatarias (o seu nome casa cun gañador).
        if ute_nome.is_empty() || !ganadores.contains(&company_key(&ute_nome)) {
            continue;
        }

        let mut membros = Vec::new();
        for li in li_re().captures_iter(inner) {
            let txt = clean_text(&li[1]);
            if let Some((cif, nome)) = txt.split_once(" - ") {
                let cif = cif.trim();
                if nif_token_re().is_match(cif) {
                    membros.push(UteMembro {
                        cif: normalize_nif(cif),
                        nome: nome.trim().to_string(),
                    });
                }
            }
        }
        if !membros.is_empty() {
            out.push(Ute {
                nome: ute_nome,
                nif: clean_text(cells[1]),
                membros,
            });
        }
    }
    out
}

/// Extrae un mapa «clave de empresa → NIF» a partir das táboas (ocultas) de
/// licitadores e de formalización do detalle. A táboa principal de resolución só
/// trae o nome do adxudicatario; o NIF aparece nesoutras dúas.
fn extract_nif_map(html: &str) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    let mut add = |nome: &str, nif: &str| {
        let key = company_key(nome);
        let nif = normalize_nif(nif);
        if !key.is_empty() && !nif.is_empty() {
            map.entry(key).or_insert(nif);
        }
    };

    // (a) Filas da táboa de licitadores: unha cela é o NIF e outra o nome.
    for tr in filaslic_re().captures_iter(html) {
        let cells: Vec<String> = cell_re()
            .captures_iter(&tr[1])
            .map(|c| clean_text(&c[1]))
            .collect();
        let Some(nif) = cells.iter().find(|c| nif_token_re().is_match(c)) else {
            continue;
        };
        let Some(m) = nif_token_re().find(nif) else { continue };
        let nif_val = m.as_str().to_string();
        // O nome é a cela con máis letras que non sexa o propio NIF.
        if let Some(nome) = cells
            .iter()
            .filter(|c| !nif_token_re().is_match(c))
            .max_by_key(|c| c.chars().filter(|ch| ch.is_alphabetic()).count())
        {
            if nome.chars().filter(|ch| ch.is_alphabetic()).count() >= 4 {
                add(nome, &nif_val);
            }
        }
    }

    // (b) Celas «Nome <br/> NIF» (táboa de formalización).
    for cap in name_br_nif_re().captures_iter(html) {
        let nome = clean_text(&cap[1]);
        let despois = clean_text(&cap[2]);
        if let Some(m) = nif_token_re().find(&despois) {
            add(&nome, m.as_str());
        }
    }

    map
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

    // O NIF do adxudicatario non está nesta táboa: búscase no mapa extraído das
    // táboas ocultas de licitadores/formalización de toda a páxina.
    let nif_map = extract_nif_map(html);

    let mut out = Vec::new();
    for (i, tr) in tr_re().captures_iter(table).enumerate() {
        // Saltar a fila de cabeceira.
        let row_html = &tr[1];
        if i == 0 || row_html.to_lowercase().contains("<th") {
            continue;
        }
        let raw_cells: Vec<&str> = cell_re()
            .captures_iter(row_html)
            .map(|c| c.get(1).map_or("", |m| m.as_str()))
            .collect();
        let cells: Vec<String> = raw_cells.iter().map(|c| clean_text(c)).collect();
        if cells.iter().all(|c| c.is_empty()) {
            continue;
        }
        let get = |idx: usize| cells.get(idx).cloned().unwrap_or_default();
        let importe_txt = get(4);
        let base = Resolucion {
            lote: get(0),
            participacion: get(1),
            estado_resolucion: get(2),
            adxudicatario: get(3),
            nif: String::new(),
            importe_num: parse_importe(&importe_txt),
            importe_txt,
            data_difusion: get(5),
            prazo_execucion: get(6),
            recurso: get(7),
        };
        // Ignorar filas baleiras de contido relevante.
        if base.estado_resolucion.is_empty() && base.adxudicatario.is_empty() {
            continue;
        }

        // «Múltiples adxudicatarios do procedemento»: varias empresas (sen ser unha
        // UTE) repartíronse o contrato. Expándese nunha fila por empresa real (que
        // veñen en inputs ocultos). NON implica relación entre elas. O importe déixase
        // en branco: o portal non o desagrega por empresa.
        let multiples = raw_cells.get(3).map(|c| parse_multiples_adx(c)).unwrap_or_default();
        if multiples.is_empty() {
            let nif = nif_map
                .get(&company_key(&base.adxudicatario))
                .cloned()
                .unwrap_or_default();
            out.push(Resolucion { nif, ..base });
        } else {
            for (nome, cif) in multiples {
                out.push(Resolucion {
                    adxudicatario: nome,
                    nif: cif,
                    importe_num: None,
                    importe_txt: String::new(),
                    ..base.clone()
                });
            }
        }
    }
    out
}

/// Inputs ocultos `ADX_NOM_*`/`ADX_CIF_*` cos adxudicatarios reais do placeholder.
fn input_adx_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)id="ADX_(?:NOM|CIF)_([0-9]+_[0-9]+_[0-9]+)"\s*value\s*=\s*"([^"]*)""#)
            .unwrap()
    })
}

/// Extrae os adxudicatarios reais do placeholder «Múltiples adxudicatarios»: veñen
/// en inputs ocultos `ADX_NOM_*` (que garda o CIF) e `ADX_CIF_*` (o nome) — o portal
/// trócaos, así que se asigna por patrón de NIF. Devolve `(nome, cif)` por orde.
fn parse_multiples_adx(cell: &str) -> Vec<(String, String)> {
    if !cell.contains("ADX_") {
        return Vec::new();
    }
    let mut orden: Vec<String> = Vec::new();
    let mut vals: HashMap<String, Vec<String>> = HashMap::new();
    for cap in input_adx_re().captures_iter(cell) {
        let suffix = cap[1].to_string();
        let entry = vals.entry(suffix.clone()).or_default();
        if entry.is_empty() {
            orden.push(suffix);
        }
        entry.push(clean_text(&cap[2]));
    }
    let mut out = Vec::new();
    for suffix in orden {
        let vs = &vals[&suffix];
        let cif = vs.iter().find(|v| nif_token_re().is_match(v)).cloned();
        let nome = vs.iter().find(|v| !nif_token_re().is_match(v)).cloned();
        if let (Some(nome), Some(cif)) = (nome, cif) {
            if !nome.trim().is_empty() {
                out.push((nome.trim().to_string(), normalize_nif(&cif)));
            }
        }
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
    fn extrae_nif_do_adxudicatario() {
        // O NIF non está na táboa principal de resolución, senón nas táboas
        // ocultas de licitadores/formalización; extráese e asóciase por nome.
        let bytes = include_bytes!("../../tests/fixtures/detalle_824418.html");
        let html = crate::scraper::decode_bytes(bytes);
        let (_d, res) = parse_detail("824418", &html);
        let soltec = res
            .iter()
            .find(|r| r.adxudicatario.contains("SOLTEC"))
            .expect("debe atoparse SOLTEC");
        assert_eq!(soltec.nif, "B36881415");
    }

    #[test]
    fn expande_multiples_adxudicatarios() {
        // «Múltiples adxudicatarios do procedemento» (sen UTE): expándese nas
        // empresas reais (inputs ocultos), sen implicar relación entre elas.
        let bytes = include_bytes!("../../tests/fixtures/detalle_multiples_827187.html");
        let html = crate::scraper::decode_bytes(bytes);
        let (_d, res) = parse_detail("827187", &html);

        assert!(
            !res.iter().any(|r| r.adxudicatario.contains("Múltiples")),
            "o placeholder non debe quedar"
        );
        assert_eq!(res.len(), 3, "tres adxudicatarios reais");
        let ingenio = res
            .iter()
            .find(|r| r.adxudicatario.contains("INGENIO MEDIA"))
            .expect("INGENIO MEDIA");
        assert_eq!(ingenio.nif, "B70212634");
        assert!(res.iter().any(|r| r.adxudicatario.contains("IMAXE INTERMEDIA") && r.nif == "A15764723"));
        assert!(res.iter().any(|r| r.adxudicatario.contains("AVANTE") && r.nif == "B70509971"));
        // O importe déixase en branco (o portal non o desagrega por empresa).
        assert!(res.iter().all(|r| r.importe_num.is_none() && r.importe_txt.is_empty()));
    }

    #[test]
    fn extrae_membros_de_ute_adxudicataria() {
        let bytes = include_bytes!("../../tests/fixtures/detalle_ute_822607.html");
        let html = crate::scraper::decode_bytes(bytes);
        // Simulamos que a UTE foi a adxudicataria para que pase o filtro.
        let res = vec![Resolucion {
            adxudicatario: "UTE AQUATEC - AIN ACTIVE".into(),
            ..Default::default()
        }];
        let utes = parse_utes(&html, &res);
        assert_eq!(utes.len(), 1, "debe extraerse unha UTE adxudicataria");
        let u = &utes[0];
        assert!(u.nome.contains("AQUATEC"));
        assert_eq!(u.membros.len(), 2, "dúas empresas membro");
        assert_eq!(u.membros[0].cif, "A85788073");
        assert_eq!(u.membros[1].cif, "B15903883");
        assert!(u.membros[1].nome.contains("AIN ACTIVE"));
    }

    #[test]
    fn ute_perdedora_non_se_extrae() {
        // A UTE non gañou (o adxudicatario é outra empresa) → non se extrae.
        let bytes = include_bytes!("../../tests/fixtures/detalle_ute_822607.html");
        let html = crate::scraper::decode_bytes(bytes);
        let res = vec![Resolucion {
            adxudicatario: "AQUATICA INGENIERIA CIVIL, SL".into(),
            ..Default::default()
        }];
        assert!(parse_utes(&html, &res).is_empty());
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
