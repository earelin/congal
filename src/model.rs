//! Modelos de datos da aplicación.

use chrono::{Datelike, Local, NaiveDate};
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

/// Normaliza unha data do portal a ISO 8601 (`YYYY-MM-DD`). Acepta os formatos
/// que devolve o sitio (`DD-MM-YYYY`, `DD/MM/YYYY`) e tamén o propio ISO; ignora
/// unha posible hora ao final. Devolve `None` se non se pode interpretar.
pub fn parse_data(s: &str) -> Option<String> {
    let token = s.trim().split_whitespace().next().unwrap_or("");
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

// ───────────────────────── datoscif.es: entidades e cargos ─────────────────

/// Suxestión devolta polo buscador de datoscif (`/sugerencias.ajax`).
#[derive(Debug, Clone, Deserialize)]
pub struct Suggestion {
    pub nombre: String,
    /// Slug canónico da entidade (p.ex. `inditex-sa`), clave estable en datoscif.
    pub url: String,
    #[serde(default)]
    pub uri: String,
    /// 1 = empresa, 2 = persoa.
    pub tipo_entidad: i64,
}

/// Entidade de datoscif (empresa ou persoa) tal como a gardamos.
#[derive(Debug, Clone, Default)]
pub struct DatosCifEntidade {
    pub url: String,
    pub nome: String,
    pub tipo_entidad: i64,
    pub uri: String,
    // Datos da persoa xurídica (só empresas; baleiro nas persoas).
    pub cif: String,
    pub domicilio: String,
    pub cod_postal: String,
    pub municipio: String,
    pub provincia: String,
}

impl DatosCifEntidade {
    pub fn is_empresa(&self) -> bool {
        self.tipo_entidad == 1
    }
}

/// Ficha da persoa xurídica extraída da páxina HTML da empresa en datoscif.
#[derive(Debug, Clone, Default)]
pub struct EmpresaInfo {
    pub cif: String,
    pub domicilio: String,
    pub cod_postal: String,
    pub municipio: String,
    pub provincia: String,
}

/// Un cargo (relación persoa→empresa) para amosar na vista de detalle.
#[derive(Debug, Clone, Default)]
pub struct CargoRow {
    pub persona_url: String,
    pub persona_nome: String,
    pub cargo: String,
    pub desde: String,
    pub hasta: String,
    pub activo: bool,
}

/// Unha razón social que aparece como adxudicataria e forma parte dun grupo.
#[derive(Debug, Clone)]
pub struct EmpresaNodo {
    pub empresa_url: String,
    pub empresa_nome: String,
    pub provincia: String,
    pub num_contratos: i64,
}

/// Unha persoa (administrador/apoderado) que conecta razóns sociais dun grupo.
#[derive(Debug, Clone)]
pub struct PersoaNodo {
    pub persona_url: String,
    pub persona_nome: String,
    /// Cantas razóns sociais do grupo controla esta persoa.
    pub num_empresas: usize,
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

/// Un grupo (trama) de razóns sociais interconectadas: unha compoñente conexa
/// do grafo persoa↔empresa, onde as persoas comparten cargo en varias das
/// empresas adxudicatarias. Substitúe a vista de «unha persoa por tarxeta»,
/// fusionando os casos onde varias persoas controlan as mesmas empresas.
#[derive(Debug, Clone)]
pub struct GrupoRelacion {
    pub persoas: Vec<PersoaNodo>,
    pub empresas: Vec<EmpresaNodo>,
    /// Contratos nos que as razóns sociais do grupo son adxudicatarias,
    /// ordenados por data descendente. Limitados polos filtros do panel.
    pub contratos: Vec<ContratoAdxudicado>,
    /// Suma dos importes de adxudicación de `contratos`.
    pub importe_total: f64,
}

/// Nivel de confianza do emparellamento adxudicatario ↔ entidade de datoscif.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confianza {
    /// Nome normalizado idéntico.
    Exacta,
    /// Idéntico tras eliminar o sufixo de razón social (SL, SA…).
    Nucleo,
    /// Persoa: mesmos tokens de nome sen importar a orde.
    Tokens,
    /// Varios candidatos igual de bos: non se vincula.
    Ambigua,
    /// Ningún candidato casa.
    SenMatch,
}

impl Confianza {
    pub fn as_str(self) -> &'static str {
        match self {
            Confianza::Exacta => "exacta",
            Confianza::Nucleo => "nucleo",
            Confianza::Tokens => "tokens",
            Confianza::Ambigua => "ambigua",
            Confianza::SenMatch => "sen_match",
        }
    }

    /// Indica se o emparellamento é dabondo fiable como para vincularse só.
    pub fn is_auto(self) -> bool {
        matches!(self, Confianza::Exacta | Confianza::Nucleo | Confianza::Tokens)
    }
}

/// Estado de revisión dun emparellamento (a revisión manual queda para o futuro).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoMatch {
    /// Vinculado automaticamente por alta confianza.
    Auto,
    /// Sen vínculo: pendente de revisión manual.
    Pendente,
}

impl EstadoMatch {
    pub fn as_str(self) -> &'static str {
        match self {
            EstadoMatch::Auto => "auto",
            EstadoMatch::Pendente => "pendente",
        }
    }
}

/// Sufixos de razón social (xa normalizados, sen puntos) que se eliminan ao
/// comparar nomes de empresa, xa que poden non coincidir entre as dúas fontes.
const LEGAL_SUFFIXES: &[&str] = &[
    "slu", "slne", "sll", "slp", "sl", "slu", "srl", "srlu", "sau", "sal", "sa",
    "scoop", "coop", "scp", "sc", "aie", "ute", "cb", "sociedad", "limitada",
    "anonima",
];

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

/// Elimina os sufixos de razón social finais dunha clave xa normalizada.
pub fn strip_legal_suffix(key: &str) -> String {
    let mut tokens: Vec<&str> = key.split_whitespace().collect();
    while tokens.len() > 1 && LEGAL_SUFFIXES.contains(tokens.last().unwrap()) {
        tokens.pop();
    }
    tokens.join(" ")
}

/// Núcleo dun nome de empresa: clave sen o sufixo de razón social.
pub fn company_core(s: &str) -> String {
    strip_legal_suffix(&company_key(s))
}

/// Multiset ordenado de tokens dunha clave (para comparar nomes de persoa sen
/// importar a orde: «nome apelido1 apelido2» vs «apelido1 apelido2 nome»).
fn token_multiset(key: &str) -> Vec<String> {
    let mut t: Vec<String> = key.split_whitespace().map(str::to_string).collect();
    t.sort();
    t
}

/// Resultado de escoller un candidato dentro dun nivel de confianza.
enum Pick {
    Empty,
    One(String),
    Many,
}

/// Deduplica un grupo de slugs candidatos e decide se hai un único gañador.
fn pick(pool: &mut Vec<String>) -> Pick {
    pool.sort();
    pool.dedup();
    match pool.len() {
        0 => Pick::Empty,
        1 => Pick::One(pool.remove(0)),
        _ => Pick::Many,
    }
}

/// Escolle a mellor entidade de datoscif para un adxudicatario, con prioridade
/// Exacta > Núcleo (empresa) > Tokens (persoa). Só devolve `Some(url)` se hai un
/// único gañador no mellor nivel; se hai empate devolve `(None, Ambigua)`.
pub fn best_match(adx_nome: &str, suggestions: &[Suggestion]) -> (Option<String>, Confianza) {
    let akey = company_key(adx_nome);
    if akey.is_empty() {
        return (None, Confianza::SenMatch);
    }
    let acore = company_core(adx_nome);
    let atoks = token_multiset(&akey);

    let mut exacta: Vec<String> = Vec::new();
    let mut nucleo: Vec<String> = Vec::new();
    let mut tokens: Vec<String> = Vec::new();
    for s in suggestions {
        let ckey = company_key(&s.nombre);
        if ckey.is_empty() {
            continue;
        }
        if ckey == akey {
            exacta.push(s.url.clone());
        } else if s.tipo_entidad == 1 {
            if !acore.is_empty() && strip_legal_suffix(&ckey) == acore {
                nucleo.push(s.url.clone());
            }
        } else if s.tipo_entidad == 2 && atoks.len() >= 2 && token_multiset(&ckey) == atoks {
            tokens.push(s.url.clone());
        }
    }

    for (mut pool, conf) in [
        (exacta, Confianza::Exacta),
        (nucleo, Confianza::Nucleo),
        (tokens, Confianza::Tokens),
    ] {
        match pick(&mut pool) {
            Pick::One(url) => return (Some(url), conf),
            Pick::Many => return (None, Confianza::Ambigua),
            Pick::Empty => {}
        }
    }
    (None, Confianza::SenMatch)
}

/// Variantes do nome dun adxudicatario coas que buscar en datoscif. Para persoas
/// («NOME APELIDO1 APELIDO2») reordénase movendo o primeiro token ao final para
/// casar co patrón de datoscif («APELIDO1 APELIDO2 NOME»), xa que a busca é por
/// subcadea sobre o nome gardado.
pub fn search_variants(adx_nome: &str) -> Vec<String> {
    let key = company_key(adx_nome);
    let mut out = vec![key.clone()];
    let toks: Vec<&str> = key.split_whitespace().collect();
    // Só ten sentido reordenar cando semella unha persoa: 2-4 tokens e sen
    // sufixo de razón social.
    let semella_persoa = (2..=4).contains(&toks.len()) && strip_legal_suffix(&key) == key;
    if semella_persoa {
        // Primeiro token ao final: «nome a1 a2» → «a1 a2 nome».
        let mut rot = toks[1..].to_vec();
        rot.push(toks[0]);
        out.push(rot.join(" "));
        // Só os apelidos (tokens 2..n), que adoitan ser máis distintivos.
        if toks.len() >= 3 {
            out.push(toks[1..].join(" "));
        }
    }
    out.dedup();
    out
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
    fn parsea_datas_a_iso() {
        assert_eq!(parse_data("01-02-2025").as_deref(), Some("2025-02-01"));
        assert_eq!(parse_data("1/2/2025").as_deref(), Some("2025-02-01"));
        assert_eq!(parse_data("2025-02-01").as_deref(), Some("2025-02-01"));
        // Ignórase a hora final.
        assert_eq!(parse_data("01-02-2025 13:45").as_deref(), Some("2025-02-01"));
        assert_eq!(parse_data(""), None);
        assert_eq!(parse_data("sen data"), None);
    }

    #[test]
    fn formatea_data_para_presentacion() {
        assert_eq!(format_data_gl("2025-02-01"), "01/02/2025");
        // Entrada non ISO: devólvese tal cal.
        assert_eq!(format_data_gl("sen data"), "sen data");
        // Ida e volta.
        assert_eq!(parse_data(&format_data_gl("2024-12-31")).as_deref(), Some("2024-12-31"));
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

    fn sug(nombre: &str, url: &str, tipo: i64) -> Suggestion {
        Suggestion {
            nombre: nombre.into(),
            url: url.into(),
            uri: String::new(),
            tipo_entidad: tipo,
        }
    }

    #[test]
    fn clave_empresa_quita_puntos_e_acentos() {
        assert_eq!(company_key("Construccións S.L."), "construccions sl");
        assert_eq!(company_key("Obras, Pinturas y Más S.A."), "obras pinturas y mas sa");
        assert_eq!(company_core("INDITEX MODA S.L."), "inditex moda");
        // O sufixo non importa: mesmo núcleo con SL ou SA.
        assert_eq!(company_core("Foo SL"), company_core("FOO, S.A."));
    }

    #[test]
    fn match_empresa_exacta_e_por_nucleo() {
        let cands = [
            sug("INDITEX MODA SL", "inditex-moda-sl", 1),
            sug("INDITEX SA", "inditex-sa", 1),
        ];
        // Exacta tras normalizar puntuación.
        let (url, c) = best_match("Inditex Moda, S.L.", &cands);
        assert_eq!(url.as_deref(), Some("inditex-moda-sl"));
        assert_eq!(c, Confianza::Exacta);
        // Núcleo: o adxudicatario trae outro sufixo (SLU) pero o núcleo casa.
        let (url, c) = best_match("INDITEX MODA SLU", &cands);
        assert_eq!(url.as_deref(), Some("inditex-moda-sl"));
        assert_eq!(c, Confianza::Nucleo);
    }

    #[test]
    fn match_persoa_reordenada_por_tokens() {
        // Contratos: «nome apelido1 apelido2»; datoscif: «apelido1 apelido2 nome».
        let cands = [sug("Garcia Lopez Manuel", "garcia-lopez-manuel", 2)];
        let (url, c) = best_match("MANUEL GARCÍA LÓPEZ", &cands);
        assert_eq!(url.as_deref(), Some("garcia-lopez-manuel"));
        assert_eq!(c, Confianza::Tokens);
    }

    #[test]
    fn match_ambiguo_non_vincula() {
        // Dúas empresas distintas co mesmo núcleo: empate → ambigua, sen vínculo.
        let cands = [
            sug("Foo SL", "foo-sl", 1),
            sug("Foo SA", "foo-sa", 1),
        ];
        let (url, c) = best_match("FOO", &cands);
        assert_eq!(url, None);
        assert_eq!(c, Confianza::Ambigua);
    }

    #[test]
    fn match_sen_candidatos() {
        let (url, c) = best_match("Empresa Inexistente SL", &[]);
        assert_eq!(url, None);
        assert_eq!(c, Confianza::SenMatch);
    }

    #[test]
    fn variantes_de_busca_para_persoa() {
        let v = search_variants("MANUEL GARCÍA LÓPEZ");
        assert!(v.contains(&"manuel garcia lopez".to_string()));
        assert!(v.contains(&"garcia lopez manuel".to_string())); // reordenada
        assert!(v.contains(&"garcia lopez".to_string())); // só apelidos
        // Unha empresa con sufixo non se reordena.
        let v = search_variants("Inditex Moda SL");
        assert_eq!(v, vec!["inditex moda sl".to_string()]);
    }
}
