//! Modelos de datos da aplicación.

use chrono::{Datelike, Local};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Ano actual segundo o reloxo do sistema.
pub fn current_year() -> i32 {
    Local::now().year()
}

/// Grupos de estado tal e como os amosa a web (catro caixas de selección),
/// cada un mapeado aos códigos numéricos que entende `resultadoIndex.jsp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoGroup {
    EnPrazo,
    Pendente,
    Resoltos,
    Suspendidos,
}

impl EstadoGroup {
    /// Códigos numéricos que representan este grupo no parámetro `ESTADO`.
    pub fn codes(self) -> &'static str {
        match self {
            EstadoGroup::EnPrazo => "1",
            EstadoGroup::Pendente => "2,3",
            EstadoGroup::Resoltos => "4,5,6,8",
            EstadoGroup::Suspendidos => "7",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EstadoGroup::EnPrazo => "En prazo de presentación de ofertas",
            EstadoGroup::Pendente => "Pendente de adxudicar",
            EstadoGroup::Resoltos => "Resoltos",
            EstadoGroup::Suspendidos => "Suspendidos por recurso",
        }
    }

    pub const ALL: [EstadoGroup; 4] = [
        EstadoGroup::EnPrazo,
        EstadoGroup::Pendente,
        EstadoGroup::Resoltos,
        EstadoGroup::Suspendidos,
    ];
}

/// Filtros da busca/scraping, equivalentes ao formulario web de licitacións.
#[derive(Debug, Clone)]
pub struct Filters {
    pub estados: Vec<EstadoGroup>,
    pub year: String,
    pub organo: String,           // código OR
    pub asunto: String,           // ASUNTO_Lic (busca textual)
    pub tipo_contrato: String,    // TC
    pub tipo_procedemento: String, // TP
    pub tipo_tramitacion: String, // TT
    pub sistema: String,          // SC
    pub materia: String,          // CPV
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            estados: EstadoGroup::ALL.to_vec(),
            year: current_year().to_string(),
            organo: String::new(),
            asunto: String::new(),
            tipo_contrato: String::new(),
            tipo_procedemento: String::new(),
            tipo_tramitacion: String::new(),
            sistema: String::new(),
            materia: String::new(),
        }
    }
}

impl Filters {
    /// Constrúe o valor do parámetro `ESTADO`. Se non hai ningún grupo
    /// seleccionado, devólvense todos os códigos (1..8).
    pub fn estado_param(&self) -> String {
        if self.estados.is_empty() {
            return "1,2,3,4,5,6,7,8".to_string();
        }
        self.estados
            .iter()
            .map(|e| e.codes())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Rexistro do listado, tal e como vén no JSON oculto `#resSearch`.
#[derive(Debug, Clone, Deserialize)]
pub struct ContractSummary {
    pub id: String,
    #[serde(default)]
    pub referencia: String,
    #[serde(default)]
    pub asunto: String,
    #[serde(default)]
    pub importe: String,
    #[serde(default)]
    pub estado: String,
    #[serde(default)]
    pub publicacion: String,
    #[serde(rename = "codOrganismo", default)]
    pub cod_organismo: String,
    #[serde(default)]
    pub organismo: String,
}

/// Datos do detalle dun contrato (páxina `licitacion?N=...`).
#[derive(Debug, Clone, Default)]
pub struct ContractDetail {
    pub contract_id: String,
    pub referencia: String,
    pub obxecto: String,
    pub tipo_tramitacion: String,
    pub tipo_procedemento: String,
    pub tipo_contrato: String,
    pub orzamento_base: String,
    pub valor_estimado: String,
    pub num_lotes: String,
    pub sistema_contratacion: String,
    pub observacions: String,
    pub data_difusion: String,
    pub sara: String,
    pub centralizada: String,
    pub lei_aplicacion: String,
    /// URL pública onde se consulta a resolución do contrato.
    pub enlace_resolucion: String,
    /// Resto de pares `dt/dd` non mapeados a columnas tipadas.
    pub extra: BTreeMap<String, String>,
}

/// Unha liña da táboa "Datos da resolución do procedemento" (unha por lote).
#[derive(Debug, Clone, Default)]
pub struct Resolucion {
    pub lote: String,
    pub participacion: String,
    pub estado_resolucion: String,
    pub adxudicatario: String,
    pub importe_txt: String,
    pub importe_num: Option<f64>,
    pub data_difusion: String,
    pub prazo_execucion: String,
    pub recurso: String,
}

/// Opcións dos despregables de filtro extraídas de `portada.jsp`.
#[derive(Debug, Clone, Default)]
pub struct FilterOptions {
    pub organos: Vec<(String, String)>,
    pub materias: Vec<(String, String)>,
    pub tipos_contrato: Vec<(String, String)>,
    pub tipos_procedemento: Vec<(String, String)>,
    pub tipos_tramitacion: Vec<(String, String)>,
    pub sistemas: Vec<(String, String)>,
}

/// Indica se un estado é terminal (resolto) e, polo tanto, non hai que
/// volver descargar o seu detalle nunha sincronización incremental.
pub fn is_estado_terminal(estado: &str) -> bool {
    let e = estado.to_lowercase();
    const TERMINAIS: [&str; 5] = ["formaliz", "deserto", "anulado", "desestim", "resolto"];
    TERMINAIS.iter().any(|t| e.contains(t))
}

/// Normaliza texto para buscas: minúsculas e sen diacríticos (acentos, til do
/// ñ, diérese…), de xeito que a busca sexa insensible a maiúsculas e acentos.
pub fn normalize_search(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// Normaliza un importe en formato galego/español (`1.000.000,00 €`) a `f64`.
pub fn parse_importe(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.' || *c == '-')
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    // Quitar separadores de millares (.) e usar . como decimal.
    let normalized = cleaned.replace('.', "").replace(',', ".");
    normalized.parse::<f64>().ok()
}

/// Formatea un importe `f64` ao formato galego/español (`1.234.567,89 €`),
/// con punto como separador de millares e coma como decimal. Inverso de
/// [`parse_importe`].
pub fn format_importe(v: f64) -> String {
    let neg = v < 0.0;
    let cents = (v.abs() * 100.0).round() as u64;
    let euros = cents / 100;
    let dec = cents % 100;

    // Agrupar os enteiros en grupos de tres díxitos cun punto.
    let digits = euros.to_string();
    let mut enteiro = String::new();
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            enteiro.push('.');
        }
        enteiro.push(ch);
    }
    format!("{}{enteiro},{dec:02} €", if neg { "-" } else { "" })
}

/// Fila combinada para a táboa de resultados locais.
#[derive(Debug, Clone, Default)]
pub struct LocalRow {
    pub id: String,
    pub referencia: String,
    pub asunto: String,
    pub importe_txt: String,
    pub estado: String,
    pub publicacion: String,
    pub organismo: String,
    pub adxudicatario: String,
    pub importe_resolucion_txt: String,
    pub enlace_resolucion: String,
}

/// Filtros aplicados localmente sobre a base de datos (sen rede).
#[derive(Debug, Clone, Default)]
pub struct LocalFilters {
    pub texto: String,        // sobre asunto/referencia
    pub organismo: String,    // subcadea sobre nome do organismo
    pub estado: String,       // subcadea sobre estado
    pub year: String,         // ano de publicación
    pub adxudicatario: String, // subcadea sobre adxudicatario
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importe_galego_a_float() {
        assert_eq!(parse_importe("1.000.000,00"), Some(1_000_000.0));
        assert_eq!(parse_importe("3.502,22 €"), Some(3502.22));
        assert_eq!(parse_importe("500,00"), Some(500.0));
        assert_eq!(parse_importe("_"), None);
        assert_eq!(parse_importe(""), None);
    }

    #[test]
    fn formatea_importe_galego() {
        assert_eq!(format_importe(1_000_000.0), "1.000.000,00 €");
        assert_eq!(format_importe(3502.22), "3.502,22 €");
        assert_eq!(format_importe(500.0), "500,00 €");
        assert_eq!(format_importe(0.0), "0,00 €");
        assert_eq!(format_importe(3500.5), "3.500,50 €");
        // Ida e volta: o que formatea debe poder reanalizarse.
        assert_eq!(parse_importe(&format_importe(1234.56)), Some(1234.56));
    }

    #[test]
    fn normalizacion_busca() {
        assert_eq!(normalize_search("Concello da Coruña"), "concello da coruna");
        assert_eq!(normalize_search("ÓRGANO"), "organo");
        assert_eq!(normalize_search("Educación"), "educacion");
        // Insensible a maiúsculas e acentos: a consulta normalizada atópase no texto.
        assert!(normalize_search("Vehículos eléctricos").contains(&normalize_search("ELÉCTRIC")));
        assert!(normalize_search("Adxudicación").contains(&normalize_search("dicacion")));
    }

    #[test]
    fn estados_terminais() {
        assert!(is_estado_terminal("Formalizado"));
        assert!(is_estado_terminal("Anulado/Desestimado"));
        assert!(is_estado_terminal("Deserto"));
        assert!(!is_estado_terminal("Pendente de adxudicar"));
        assert!(!is_estado_terminal("Adxudicado"));
        assert!(!is_estado_terminal("En prazo de presentación de ofertas"));
    }

    #[test]
    fn estado_param_por_defecto_todos() {
        let f = Filters::default();
        let p = f.estado_param();
        for c in ["1", "2", "3", "4", "5", "6", "7", "8"] {
            assert!(p.contains(c), "falta o código {c} en {p}");
        }
    }
}
