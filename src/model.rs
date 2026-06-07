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

/// Normaliza un nome para a busca en datoscif. A diferenza de [`normalize_search`],
/// **mantén o ñ**: datoscif dobra os acentos agudos (á→a) pero CONSERVA o ñ no seu
/// índice, e a busca está anclada ao inicio (prefixo). Os puntos elimínanse sen
/// oco (S.A.D → sad) e o resto da puntuación convértese en espazo.
///
/// Ademais, a petición a datoscif debe ir codificada en **ISO-8859-1** (o ñ vai
/// como `%F1`, non `%C3%91`); diso encárgase `scraper::search_entities`.
pub fn normalize_busca_datoscif(s: &str) -> String {
    let mut buf = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => buf.push('a'),
            'é' | 'è' | 'ë' | 'ê' => buf.push('e'),
            'í' | 'ì' | 'ï' | 'î' => buf.push('i'),
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => buf.push('o'),
            'ú' | 'ù' | 'ü' | 'û' => buf.push('u'),
            'ç' => buf.push('c'),
            // O ñ NON se dobra: cae no caso alfanumérico e mantense.
            '.' | ',' | '\'' | '"' | '`' | '·' => {}
            c if c.is_alphanumeric() || c == ' ' => buf.push(c),
            _ => buf.push(' '),
        }
    }
    buf.split_whitespace().collect::<Vec<_>>().join(" ")
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
    /// `true` se na resolución consta un único participante (posible indicio de
    /// que algo non é regular). Derívase de `MAX(participacion) == 1`.
    pub participante_unico: bool,
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
    /// `true` se algunha persoa do grupo ten un cargo **vixente** nesta empresa.
    pub activa: bool,
    /// `true` se a empresa ten algún vínculo por cargo (vixente ou cesado). Se é
    /// `false` pero está no grupo, conéctaa só unha UTE (non un administrador).
    pub con_cargos: bool,
}

/// Unha persoa (administrador/apoderado) que conecta razóns sociais dun grupo.
#[derive(Debug, Clone)]
pub struct PersoaNodo {
    pub persona_url: String,
    pub persona_nome: String,
    /// Cantas razóns sociais do grupo controla esta persoa.
    pub num_empresas: usize,
    /// Razóns sociais do grupo onde o seu cargo está vixente.
    pub empresas_activas: usize,
    /// Razóns sociais do grupo onde o seu cargo xa foi cesado (histórico).
    pub empresas_pasadas: usize,
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
/// Unha UTE que conecta varias razóns sociais do grupo (vínculo «UTE», distinto
/// do vínculo por administrador compartido).
#[derive(Debug, Clone)]
pub struct UteRelacion {
    pub nome: String,
    /// Nomes das empresas membros (as que se puideron emparellar con datoscif).
    pub membros: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GrupoRelacion {
    pub persoas: Vec<PersoaNodo>,
    pub empresas: Vec<EmpresaNodo>,
    /// UTE que vinculan razóns sociais deste grupo (poden estar baleiras).
    pub utes: Vec<UteRelacion>,
    /// Contratos nos que as razóns sociais do grupo son adxudicatarias,
    /// ordenados por data descendente. Limitados polos filtros do panel.
    pub contratos: Vec<ContratoAdxudicado>,
    /// Suma dos importes de adxudicación de `contratos`.
    pub importe_total: f64,
}

/// Nivel de confianza do emparellamento adxudicatario ↔ entidade de datoscif.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confianza {
    /// O CIF da ficha de datoscif coincide co NIF do adxudicatario: o sinal máis
    /// fiable, mesmo cando os nomes difiren.
    Cif,
    /// Nome normalizado idéntico.
    Exacta,
    /// Idéntico tras eliminar o sufixo de razón social (SL, SA…).
    Nucleo,
    /// Persoa: mesmos tokens de nome sen importar a orde.
    Tokens,
    /// Varios candidatos igual de bos: non se vincula automaticamente.
    Ambigua,
    /// Ningún candidato casa.
    SenMatch,
}

impl Confianza {
    pub fn as_str(self) -> &'static str {
        match self {
            Confianza::Cif => "cif",
            Confianza::Exacta => "exacta",
            Confianza::Nucleo => "nucleo",
            Confianza::Tokens => "tokens",
            Confianza::Ambigua => "ambigua",
            Confianza::SenMatch => "sen_match",
        }
    }
}

/// Estado de revisión dun emparellamento adxudicatario ↔ datoscif.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoMatch {
    /// Vinculado automaticamente por alta confianza.
    Auto,
    /// Sen candidatos plausibles: nada que revisar.
    Pendente,
    /// Hai candidatos plausibles pero sen confianza dabondo: agarda confirmación
    /// da persoa usuaria na pestana de revisión.
    Revisar,
    /// Vinculado a man pola persoa usuaria.
    Manual,
    /// A persoa usuaria revisou e descartou todos os candidatos (sen vínculo).
    Descartado,
}

impl EstadoMatch {
    pub fn as_str(self) -> &'static str {
        match self {
            EstadoMatch::Auto => "auto",
            EstadoMatch::Pendente => "pendente",
            EstadoMatch::Revisar => "revisar",
            EstadoMatch::Manual => "manual",
            EstadoMatch::Descartado => "descartado",
        }
    }
}

/// Un caso pendente de revisión manual: un adxudicatario sen vínculo fiable, co
/// seu NIF (se se coñece) e a lista de candidatos de datoscif a escoller.
#[derive(Debug, Clone)]
pub struct CasoRevision {
    pub adx_nome: String,
    /// NIF/CIF do adxudicatario, se algunha resolución o trae (baleiro se non).
    pub nif: String,
    pub candidatos: Vec<Suggestion>,
}

/// Sufixos de razón social (xa normalizados, sen puntos) que se eliminan ao
/// comparar nomes de empresa, xa que poden non coincidir entre as dúas fontes.
const LEGAL_SUFFIXES: &[&str] = &[
    "slu", "slne", "sll", "slp", "sl", "srl", "srlu", "sau", "sal", "sad", "sa",
    "scoop", "coop", "scp", "sc", "aie", "ute", "cb", "sociedad", "limitada",
    "anonima", "deportiva",
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

/// Termo de busca de reserva: o núcleo do nome (sen sufixo de razón social) e
/// sen as palabras moi curtas (1-2 letras), que adoitan ser ruído. Úsase para
/// reintentar a busca en datoscif cando o nome completo non dá resultados.
pub fn fallback_search_term(s: &str) -> String {
    // Mantense o ñ (normalización datoscif) e quítase o sufixo de razón social.
    strip_legal_suffix(&normalize_busca_datoscif(s))
        .split_whitespace()
        .filter(|t| t.chars().count() > 2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normaliza un NIF/CIF para comparar: en maiúsculas e só alfanuméricos
/// (elimina puntos, guións e espazos). «b-36.881.415» → «B36881415».
pub fn normalize_nif(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// Tipo de entidade deducido do formato do NIF/CIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NifKind {
    /// CIF de persoa xurídica (empresa): letra inicial + 7 díxitos + control.
    Empresa,
    /// NIF/NIE de persoa física: 8 díxitos + letra, ou NIE [XYZ]+7+letra.
    Persoa,
}

/// Deduce se un NIF/CIF é de empresa ou de persoa física polo seu formato.
/// Devolve `None` se non encaixa en ningún patrón coñecido.
pub fn nif_kind(nif: &str) -> Option<NifKind> {
    let n = normalize_nif(nif);
    let b = n.as_bytes();
    if b.len() != 9 {
        return None;
    }
    let is_digit = |c: u8| c.is_ascii_digit();
    // CIF empresa: [ABCDEFGHJNPQRSUVW] + 7 díxitos + (díxito ou letra de control).
    if b"ABCDEFGHJNPQRSUVW".contains(&b[0])
        && b[1..8].iter().all(|&c| is_digit(c))
        && (is_digit(b[8]) || b[8].is_ascii_alphabetic())
    {
        return Some(NifKind::Empresa);
    }
    // NIF persoa: 8 díxitos + letra.
    if b[..8].iter().all(|&c| is_digit(c)) && b[8].is_ascii_alphabetic() {
        return Some(NifKind::Persoa);
    }
    // NIE: [XYZ] + 7 díxitos + letra.
    if b"XYZ".contains(&b[0]) && b[1..8].iter().all(|&c| is_digit(c)) && b[8].is_ascii_alphabetic() {
        return Some(NifKind::Persoa);
    }
    None
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

/// Resultado de emparellar por nome un adxudicatario coas suxestións de datoscif.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchResult {
    /// Un único gañador claro no mellor nivel de confianza.
    Unico(String, Confianza),
    /// Empate no mellor nivel: varios candidatos (slugs) igual de bos. Resólvese
    /// despois validando por CIF ou pedindo confirmación.
    Ambiguo(Vec<String>),
    /// Ningún candidato casa por nome.
    Ningun,
}

/// Empareja por nome un adxudicatario coas suxestións, con prioridade
/// Exacta > Núcleo (empresa) > Tokens (persoa). No mellor nivel non baleiro,
/// devolve [`MatchResult::Unico`] se hai un só gañador ou [`MatchResult::Ambiguo`]
/// coa lista de candidatos se hai empate.
pub fn match_suggestions(adx_nome: &str, suggestions: &[Suggestion]) -> MatchResult {
    let akey = company_key(adx_nome);
    if akey.is_empty() {
        return MatchResult::Ningun;
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
            Pick::One(url) => return MatchResult::Unico(url, conf),
            Pick::Many => {
                pool.sort();
                pool.dedup();
                return MatchResult::Ambiguo(pool);
            }
            Pick::Empty => {}
        }
    }
    MatchResult::Ningun
}

/// Candidatos plausibles para revisión manual cando non houbo vínculo fiable.
/// Inclúe os empatados nun nivel de confianza (`Ambiguo`) e mais as suxestións
/// que comparten algunha palabra distintiva (>2 letras) co núcleo do nome. Ordena
/// os empatados primeiro, deduplica por slug e limita o número (evita ruído).
pub fn review_candidates(adx_nome: &str, suggestions: &[Suggestion]) -> Vec<Suggestion> {
    const MAX: usize = 8;
    let tied: Vec<String> = match match_suggestions(adx_nome, suggestions) {
        MatchResult::Ambiguo(urls) => urls,
        _ => Vec::new(),
    };
    let core_tokens: std::collections::HashSet<String> = company_core(adx_nome)
        .split_whitespace()
        .filter(|t| t.chars().count() > 2)
        .map(str::to_string)
        .collect();

    let comparte_token = |nome: &str| -> bool {
        company_core(nome)
            .split_whitespace()
            .any(|t| t.chars().count() > 2 && core_tokens.contains(t))
    };

    let mut out: Vec<Suggestion> = Vec::new();
    let mut vistos = std::collections::HashSet::new();
    // Primeiro os empatados (na orde estable dos slugs), logo o resto plausible.
    for url in &tied {
        if let Some(s) = suggestions.iter().find(|s| &s.url == url) {
            if vistos.insert(s.url.clone()) {
                out.push(s.clone());
            }
        }
    }
    for s in suggestions {
        if out.len() >= MAX {
            break;
        }
        if vistos.contains(&s.url) {
            continue;
        }
        if comparte_token(&s.nombre) && vistos.insert(s.url.clone()) {
            out.push(s.clone());
        }
    }
    out.truncate(MAX);
    out
}

/// Variantes do nome dun adxudicatario coas que buscar en datoscif. Para persoas
/// («NOME APELIDO1 APELIDO2») reordénase movendo o primeiro token ao final para
/// casar co patrón de datoscif («APELIDO1 APELIDO2 NOME»), xa que a busca é por
/// subcadea sobre o nome gardado.
pub fn search_variants(adx_nome: &str) -> Vec<String> {
    // Normalización «datoscif» (mantén o ñ): a busca está anclada ao inicio e o
    // nome completo casa como prefixo.
    let key = normalize_busca_datoscif(adx_nome);
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
}

/// Filtros aplicados localmente sobre a base de datos (sen rede).
#[derive(Debug, Clone, Default)]
pub struct LocalFilters {
    pub texto: String,        // sobre asunto/referencia
    pub organismo: String,    // subcadea sobre nome do organismo
    pub estado: String,       // subcadea sobre estado
    pub year: String,         // ano de publicación
    pub adxudicatario: String, // subcadea sobre adxudicatario
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
        assert_eq!(
            match_suggestions("Inditex Moda, S.L.", &cands),
            MatchResult::Unico("inditex-moda-sl".into(), Confianza::Exacta)
        );
        // Núcleo: o adxudicatario trae outro sufixo (SLU) pero o núcleo casa.
        assert_eq!(
            match_suggestions("INDITEX MODA SLU", &cands),
            MatchResult::Unico("inditex-moda-sl".into(), Confianza::Nucleo)
        );
    }

    #[test]
    fn match_persoa_reordenada_por_tokens() {
        // Contratos: «nome apelido1 apelido2»; datoscif: «apelido1 apelido2 nome».
        let cands = [sug("Garcia Lopez Manuel", "garcia-lopez-manuel", 2)];
        assert_eq!(
            match_suggestions("MANUEL GARCÍA LÓPEZ", &cands),
            MatchResult::Unico("garcia-lopez-manuel".into(), Confianza::Tokens)
        );
    }

    #[test]
    fn match_ambiguo_non_vincula() {
        // Dúas empresas distintas co mesmo núcleo: empate → ambiguo (candidatos).
        let cands = [sug("Foo SL", "foo-sl", 1), sug("Foo SA", "foo-sa", 1)];
        assert_eq!(
            match_suggestions("FOO", &cands),
            MatchResult::Ambiguo(vec!["foo-sa".into(), "foo-sl".into()])
        );
    }

    #[test]
    fn match_sen_candidatos() {
        assert_eq!(match_suggestions("Empresa Inexistente SL", &[]), MatchResult::Ningun);
    }

    #[test]
    fn candidatos_de_revision_prioriza_empatados_e_filtra_ruido() {
        let cands = [
            sug("Talleres O Rosal SL", "talleres-o-rosal-sl", 1),
            sug("Talleres O Rosal SA", "talleres-o-rosal-sa", 1),
            sug("Panadería Lonxe SL", "panaderia-lonxe-sl", 1), // sen tokens comúns
        ];
        let r = review_candidates("Talleres O Rosal", &cands);
        let urls: Vec<&str> = r.iter().map(|s| s.url.as_str()).collect();
        // Os dous «rosal» (empatados por núcleo) entran; o ruído queda fóra.
        assert!(urls.contains(&"talleres-o-rosal-sl"));
        assert!(urls.contains(&"talleres-o-rosal-sa"));
        assert!(!urls.contains(&"panaderia-lonxe-sl"));
    }

    #[test]
    fn nif_kind_distingue_empresa_e_persoa() {
        assert_eq!(nif_kind("B36881415"), Some(NifKind::Empresa));
        assert_eq!(nif_kind("A28601094"), Some(NifKind::Empresa));
        assert_eq!(nif_kind("12345678Z"), Some(NifKind::Persoa));
        assert_eq!(nif_kind("X1234567L"), Some(NifKind::Persoa));
        assert_eq!(nif_kind("b-36.881.415"), Some(NifKind::Empresa)); // normalízase
        assert_eq!(nif_kind("LIXO"), None);
    }

    #[test]
    fn termo_de_reserva_quita_sufixo_e_palabras_curtas() {
        // Sufixo de razón social e palabras de 1-2 letras fóra.
        assert_eq!(fallback_search_term("Talleres O Rosal, S.L."), "talleres rosal");
        assert_eq!(fallback_search_term("INDITEX MODA SL"), "inditex moda");
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

    #[test]
    fn busca_datoscif_mantena_n_tilde_e_dobra_acentos() {
        // Acentos agudos dóbranse; o ñ consérvase; S.A.D → sad.
        assert_eq!(
            normalize_busca_datoscif("CLUB BÁSQUET CORUÑA, S.A.D"),
            "club basquet coruña sad"
        );
        // O sufixo SAD recoñécese; o núcleo conserva o ñ.
        assert_eq!(
            fallback_search_term("CLUB BÁSQUET CORUÑA, S.A.D"),
            "club basquet coruña"
        );
    }

    #[test]
    fn club_sad_casa_coa_suxestion_de_datoscif() {
        // O termo de busca conserva o ñ (para que datoscif o atope en Latin-1).
        assert!(
            search_variants("CLUB BÁSQUET CORUÑA, S.A.D")
                .contains(&"club basquet coruña sad".to_string())
        );
        // E o emparellamento local casa (company_key dobra ñ→n en ambos lados).
        let cands = [sug("CLUB BASQUET CORUÑA SAD", "club-basquet-coruna-sad", 1)];
        assert_eq!(
            match_suggestions("CLUB BÁSQUET CORUÑA, S.A.D", &cands),
            MatchResult::Unico("club-basquet-coruna-sad".into(), Confianza::Exacta)
        );
    }
}
