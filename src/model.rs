//! Modelos de datos da aplicación.

use chrono::{Datelike, Local, NaiveDate};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Ano actual segundo o reloxo do sistema.
pub fn current_year() -> i32 {
    Local::now().year()
}

/// Tipo de contrato: licitación (procedemento ordinario) ou contrato menor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TipoContrato {
    #[default]
    Licitacion,
    Menor,
}

impl TipoContrato {
    /// Valor gardado na BD (columna `contracts.tipo`).
    pub fn as_str(self) -> &'static str {
        match self {
            TipoContrato::Licitacion => "licitacion",
            TipoContrato::Menor => "menor",
        }
    }

    /// Interpreta o texto gardado na BD.
    pub fn from_db(s: &str) -> TipoContrato {
        match s {
            "menor" => TipoContrato::Menor,
            _ => TipoContrato::Licitacion,
        }
    }

    /// Etiqueta para a interface (sub-pestanas).
    pub fn etiqueta(self) -> &'static str {
        match self {
            TipoContrato::Licitacion => "Licitacións",
            TipoContrato::Menor => "Contratos menores",
        }
    }
}

/// Parámetros dunha importación: un organismo (o `org_id` é o valor de `organoL`,
/// que coincide co id da API) e o ano que acouta a busca de contratos menores
/// (as licitacións báixanse enteiras).
#[derive(Debug, Clone, Default)]
pub struct ImportParams {
    pub org_id: String,
    pub org_nome: String,
    pub ano: String,
}

/// Fila do listado de contratos menores da API `.../contratosmenores/table`.
/// Inclúe xa o adxudicatario (nome + NIF), polo que non precisa páxina de detalle.
/// Os campos de texto poden vir con espazos de recheo → hai que facer `trim`.
#[derive(Debug, Clone, Deserialize)]
pub struct MenorRow {
    pub id: i64,
    #[serde(default)]
    pub publicado: String,
    #[serde(default)]
    pub objeto: String,
    #[serde(default)]
    pub importe: Option<f64>,
    #[serde(default)]
    pub nif: String,
    #[serde(default)]
    pub adjudicatario: String,
    #[serde(default)]
    pub duracion: String,
}

/// Rexistro dun contrato para gardar no listado (DTO de escritura cara a
/// `contracts`). Antes deserializábase do JSON oculto do listado web; agora
/// constrúese a partir das filas da API (licitacións) ou nos tests.
#[derive(Debug, Clone, Default)]
pub struct ContractSummary {
    pub id: String,
    pub tipo: TipoContrato,
    pub referencia: String,
    pub asunto: String,
    pub importe: String,
    pub estado: String,
    pub publicacion: String,
    pub cod_organismo: String,
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
    /// Orzamento base de licitación, **con IVE** (só o valor numérico).
    pub orzamento_base: Option<f64>,
    /// Valor estimado do contrato, **sen IVE** (só o valor numérico).
    pub valor_estimado: Option<f64>,
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
    /// NIF/CIF do adxudicatario (extraído das táboas ocultas de licitadores/
    /// formalización; a táboa principal de resolución só trae o nome). Baleiro
    /// se non se atopou.
    pub nif: String,
    pub importe_txt: String,
    pub importe_num: Option<f64>,
    pub data_difusion: String,
    pub prazo_execucion: String,
    pub recurso: String,
}

/// Opcións do despregable de órgano de contratación extraídas de `portada.jsp`.
/// O formulario de importación só filtra por órgano (e ano), así que non se
/// conservan as demais listas (materias, tipos…).
#[derive(Debug, Clone, Default)]
pub struct FilterOptions {
    pub organos: Vec<(String, String)>,
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

/// Normaliza unha data do portal a ISO 8601 (`YYYY-MM-DD`). Acepta os formatos
/// que devolve o sitio (`DD-MM-YYYY`, `DD/MM/YYYY`) e tamén o propio ISO; ignora
/// unha posible hora ao final. Devolve `None` se non se pode interpretar.
pub fn parse_data(s: &str) -> Option<String> {
    let token = s.split_whitespace().next().unwrap_or("");
    if token.is_empty() {
        return None;
    }
    for fmt in ["%d-%m-%Y", "%d/%m/%Y", "%Y-%m-%d", "%Y/%m/%d"] {
        if let Ok(d) = NaiveDate::parse_from_str(token, fmt) {
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }
    None
}

/// Formatea unha data ISO (`YYYY-MM-DD`) para presentación en galego
/// (`DD/MM/YYYY`). Se non é unha data ISO válida, devolve a entrada sen tocar.
pub fn format_data_gl(iso: &str) -> String {
    NaiveDate::parse_from_str(iso.trim(), "%Y-%m-%d")
        .map(|d| d.format("%d/%m/%Y").to_string())
        .unwrap_or_else(|_| iso.to_string())
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
        if i > 0 && (len - i).is_multiple_of(3) {
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
    pub tipo: TipoContrato,
    pub referencia: String,
    pub asunto: String,
    pub importe_txt: String,
    pub estado: String,
    pub publicacion: String,
    pub organismo: String,
    pub adxudicatario: String,
    /// NIF/CIF do adxudicatario (relevante sobre todo nos contratos menores, que
    /// xa o traen no listado).
    pub nif: String,
    /// Duración do contrato menor (baleiro nas licitacións).
    pub duracion: String,
    pub importe_resolucion_txt: String,
    pub enlace_resolucion: String,
    /// `true` se en TODOS os lotes da resolución consta un único participante
    /// (posible indicio de que algo non é regular). Derívase de que tanto o
    /// mínimo coma o máximo de `participacion` sexan `1`; un lote con
    /// participación descoñecida/baleira non se marca.
    pub participante_unico: bool,
}

// ───────────────────────── UTEs e relacións ────────────────────────────────

/// Unha empresa membro dunha UTE, tal como vén no popup de licitadores
/// (`CIF - NOME`). O CIF é completo e fiable; o nome pode vir truncado.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UteMembro {
    pub cif: String,
    pub nome: String,
}

/// Unha Unión Temporal de Empresas (UTE) presentada nun contrato: o seu nome, o
/// seu NIF (empeza por `U`, ou un código `TEMP-` provisional) e as empresas que
/// a compoñen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ute {
    pub nome: String,
    pub nif: String,
    pub membros: Vec<UteMembro>,
}

/// Unha razón social adxudicataria de contratos, agregada para a vista de
/// empresas: o seu nome representativo, o NIF, o número de contratos distintos
/// nos que é adxudicataria e a suma dos importes adxudicados. Constrúese só con
/// datos de contratosdegalicia.gal (a táboa `contract_resolucion`).
#[derive(Debug, Clone)]
pub struct EmpresaContratos {
    /// NIF/CIF da empresa (baleiro se non se coñece).
    pub nif: String,
    pub nome: String,
    pub num_contratos: i64,
    pub importe_total: f64,
}

/// Unha razón social que forma parte dun grupo por compartir UTE con outras.
#[derive(Debug, Clone)]
pub struct EmpresaNodo {
    /// CIF da empresa (ou, se non se coñece, a clave normalizada do nome).
    pub cif: String,
    pub empresa_nome: String,
    pub num_contratos: i64,
}

/// Un contrato adxudicado a unha das razóns sociais dun grupo. Úsase no
/// despregable de contratos de cada trama na vista de relacións.
#[derive(Debug, Clone)]
pub struct ContratoAdxudicado {
    pub contract_id: String,
    /// Razón social do grupo á que se lle adxudicou este contrato.
    pub empresa_nome: String,
    pub asunto: String,
    /// Data de publicación formatada como DD/MM/YYYY.
    pub publicacion: String,
    pub importe_num: f64,
    pub importe_txt: String,
}

/// Unha UTE que conecta varias razóns sociais do grupo (o vínculo que define a
/// trama: empresas que concorreron xuntas nunha mesma UTE adxudicataria).
#[derive(Debug, Clone)]
pub struct UteRelacion {
    pub nome: String,
    /// Nomes das empresas membros da UTE.
    pub membros: Vec<String>,
}

/// Un grupo (trama) de razóns sociais interconectadas por pertencer a unha mesma
/// UTE adxudicataria: unha compoñente conexa do grafo empresa↔empresa onde as
/// arestas son a coparticipación nunha UTE.
#[derive(Debug, Clone)]
pub struct GrupoRelacion {
    pub empresas: Vec<EmpresaNodo>,
    /// UTE que vinculan as razóns sociais deste grupo.
    pub utes: Vec<UteRelacion>,
    /// Contratos nos que as razóns sociais do grupo son adxudicatarias,
    /// ordenados por data descendente. Limitados polos filtros do panel.
    pub contratos: Vec<ContratoAdxudicado>,
    /// Suma dos importes de adxudicación de `contratos`.
    pub importe_total: f64,
}

/// Clave de comparación dun nome: minúsculas, sen acentos, **sen puntos/comas**
/// (S.L. → sl) e co resto de signos convertidos en espazo. Base do fuzzy match.
pub fn company_key(s: &str) -> String {
    let lowered = normalize_search(s);
    let mut buf = String::with_capacity(lowered.len());
    for c in lowered.chars() {
        match c {
            // Puntos, comas e apóstrofos elimínanse sen deixar oco (S.L. → sl).
            '.' | ',' | '\'' | '"' | '`' | '·' => {}
            c if c.is_alphanumeric() || c == ' ' => buf.push(c),
            // Guións, barras, &, paréntese… sepáranse como espazo.
            _ => buf.push(' '),
        }
    }
    buf.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normaliza un NIF/CIF para comparar: en maiúsculas e só alfanuméricos
/// (elimina puntos, guións e espazos). «b-36.881.415» → «B36881415».
pub fn normalize_nif(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// Columna pola que se ordena o listado de contratos (clic na cabeceira).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    Id,
    #[default]
    Data,
    Obxecto,
    Importe,
    Estado,
    Organismo,
    Adxudicatario,
    ImporteResolucion,
}

impl SortColumn {
    /// Expresión SQL pola que ordenar. Son fragmentos fixos (sen entrada do
    /// usuario): non hai risco de inxección.
    pub fn order_sql(self) -> &'static str {
        match self {
            SortColumn::Id => "CAST(c.id AS INTEGER)",
            SortColumn::Data => "c.data_publicacion",
            SortColumn::Obxecto => "c.asunto COLLATE NOCASE",
            SortColumn::Importe => "c.importe_num",
            SortColumn::Estado => "e.nome COLLATE NOCASE",
            SortColumn::Organismo => "o.nome COLLATE NOCASE",
            SortColumn::Adxudicatario => "r.adxudicatarios COLLATE NOCASE",
            SortColumn::ImporteResolucion => "r.importe_total",
        }
    }

    /// Sentido por defecto ao premer por primeira vez nunha columna: as numéricas e
    /// de data ordénanse descendente (o máis recente/grande primeiro); as de texto,
    /// ascendente (alfabético A→Z).
    pub fn default_asc(self) -> bool {
        match self {
            SortColumn::Id
            | SortColumn::Data
            | SortColumn::Importe
            | SortColumn::ImporteResolucion => false,
            SortColumn::Obxecto
            | SortColumn::Estado
            | SortColumn::Organismo
            | SortColumn::Adxudicatario => true,
        }
    }
}

/// Filtros aplicados localmente sobre a base de datos (sen rede).
#[derive(Debug, Clone, Default)]
pub struct LocalFilters {
    pub texto: String,         // sobre asunto/referencia
    pub organismo: String,     // subcadea sobre nome do organismo
    pub year: String,          // ano de publicación
    pub adxudicatario: String, // subcadea sobre adxudicatario
    /// Tipo de contrato amosado (sub-pestana activa). Só o usa `query_local`; a
    /// vista de relacións ignórao (considera os dous tipos).
    pub tipo: TipoContrato,
    /// Orde do listado (columna + ascendente). Por defecto: data descendente.
    pub sort_col: SortColumn,
    pub sort_asc: bool,
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
    fn parsea_datas_a_iso() {
        assert_eq!(parse_data("01-02-2025").as_deref(), Some("2025-02-01"));
        assert_eq!(parse_data("1/2/2025").as_deref(), Some("2025-02-01"));
        assert_eq!(parse_data("2025-02-01").as_deref(), Some("2025-02-01"));
        // Ignórase a hora final.
        assert_eq!(
            parse_data("01-02-2025 13:45").as_deref(),
            Some("2025-02-01")
        );
        assert_eq!(parse_data(""), None);
        assert_eq!(parse_data("sen data"), None);
    }

    #[test]
    fn formatea_data_para_presentacion() {
        assert_eq!(format_data_gl("2025-02-01"), "01/02/2025");
        // Entrada non ISO: devólvese tal cal.
        assert_eq!(format_data_gl("sen data"), "sen data");
        // Ida e volta.
        assert_eq!(
            parse_data(&format_data_gl("2024-12-31")).as_deref(),
            Some("2024-12-31")
        );
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
    fn clave_empresa_quita_puntos_e_acentos() {
        // Base da clave que usa `upsert_utes`/`cokey` para atribuír contratos ás UTE.
        assert_eq!(company_key("Construccións S.L."), "construccions sl");
        assert_eq!(
            company_key("Obras, Pinturas y Más S.A."),
            "obras pinturas y mas sa"
        );
        // O ñ dóbrase a n (igual ca `normalize_search`), en ambos lados da comparación.
        assert_eq!(company_key("CORUÑA SAD"), company_key("CORUNA SAD"));
    }
}
