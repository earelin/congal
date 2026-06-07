//! Interface gráfica (egui): listado de contratos importados como pantalla
//! principal, e importación de datos en diálogos modais.

use crate::db::{Db, DbStats, LocalOptions};
use crate::enrich::EnrichMode;
use crate::model::{
    CargoRow, CasoRevision, ContractDetail, DatosCifEntidade, FilterOptions, Filters,
    GrupoRelacion, LocalFilters, LocalRow, Resolucion, SortColumn, Suggestion, format_data_gl,
    format_importe,
};
use crate::scraper::DATOSCIF_BASE;
use crate::theme;
use crate::worker::{Command, Event, Worker};
use egui::{Align, Color32, Layout, RichText, ScrollArea};
use egui_extras::{Column, TableBuilder};

pub struct App {
    worker: Worker,
    db: Db,
    dark: bool,

    options: FilterOptions,
    options_loaded: bool,
    /// Texto de busca de cada ComboBox, indexado polo seu id.
    combo_filtros: std::collections::HashMap<String, String>,

    /// Filtros de importación (formulario do modal).
    filters: Filters,
    /// Visibilidade do modal de selección de filtros de importación.
    show_import_dialog: bool,
    /// Visibilidade do modal de progreso da importación.
    show_progress_dialog: bool,

    busy: bool,
    progress: Option<(usize, usize)>,
    /// Título do modal de progreso segundo a operación en curso.
    progress_titulo: &'static str,
    /// `true` cando a última operación rematada foi unha vinculación/reimportación
    /// con datoscif; habilita o botón «Reimportar datos das empresas» no resultado.
    enrich_finished: bool,
    status: String,
    logs: Vec<String>,
    stats: DbStats,

    local: LocalFilters,
    local_options: LocalOptions,
    rows: Vec<LocalRow>,
    need_query: bool,
    /// Id do contrato seleccionado (resáltase na táboa e ábrese o seu diálogo).
    selected: Option<String>,
    /// Fila de resumo do contrato seleccionado (info do listado).
    selected_row: Option<LocalRow>,
    /// Detalle descargado do contrato seleccionado, se o hai.
    selected_detail: Option<(ContractDetail, Vec<Resolucion>)>,
    /// Info de datoscif (entidade + cargos) por adxudicatario do contrato aberto.
    selected_datoscif: Vec<AdxDatosCif>,
    /// Composición das UTE adxudicatarias do contrato aberto.
    selected_utes: Vec<UteDetalle>,

    /// Pestana activa da vista principal.
    tab: Tab,
    /// Vista de relacións: grupos (tramas) de razóns sociais interconectadas.
    relacions: Vec<GrupoRelacion>,
    relacions_loaded: bool,

    /// Cola de revisión manual: casos sen vínculo fiable con candidatos a escoller.
    revisions: Vec<CasoRevision>,
    /// Casos sen correspondencia en datoscif (resólvense coa busca asistida).
    sen_match: Vec<CasoRevision>,
    revisions_loaded: bool,
    /// Texto de busca asistida por caso (clave = nome do adxudicatario).
    rev_search: std::collections::HashMap<String, String>,
    /// Resultados da última busca asistida por caso.
    rev_results: std::collections::HashMap<String, Vec<Suggestion>>,
    /// Caso cuxa busca asistida está en curso (para amosar o spinner).
    rev_searching: Option<String>,

    /// Canle pola que o fío do diálogo nativo «Gardar como» devolve o destino
    /// escollido (`None` se o usuario cancela). Está presente mentres o diálogo
    /// está aberto; así o diálogo non bloquea o fío da interface.
    export_rx: Option<std::sync::mpsc::Receiver<Option<std::path::PathBuf>>>,
}

/// Pestanas da vista principal. As dúas comparten os filtros do panel lateral.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// Listado de contratos (e o detalle dun contrato seleccionado).
    Contratos,
    /// Tramas de razóns sociais relacionadas entre si.
    Relacions,
    /// Cola de revisión manual dos emparellamentos dubidosos.
    Revision,
}

/// Acción escollida nun caso da pestana de revisión (procésase fóra do bucle de
/// pintado para non chocar cos préstamos de `self`).
enum RevAction {
    /// Vincular o adxudicatario á entidade (`Some`) ou descartalo (`None`).
    Resolve(String, Option<Suggestion>),
    /// Lanzar a busca asistida en datoscif para o caso co termo dado.
    Search(String, String),
}

/// Datos de datoscif asociados a un adxudicatario dun contrato.
struct AdxDatosCif {
    adx_nome: String,
    entidade: DatosCifEntidade,
    cargos: Vec<CargoRow>,
}

/// Unha empresa membro dunha UTE para a ficha do contrato.
struct UteMembroVista {
    nome: String,
    cif: String,
    /// Entidade de datoscif do membro (por CIF), se está vinculada.
    entidade: Option<DatosCifEntidade>,
    cargos: Vec<CargoRow>,
}

/// A composición dunha UTE adxudicataria, para amosar na ficha do contrato.
struct UteDetalle {
    ute_nome: String,
    membros: Vec<UteMembroVista>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let dark = crate::system_dark();
        theme::apply(&cc.egui_ctx, dark);

        let db_path = crate::db_path();
        let db = Db::open(&db_path).expect("non se puido abrir a base de datos");
        let stats = db.stats().unwrap_or_default();
        let local_options = db.local_options().unwrap_or_default();

        let worker = Worker::spawn(cc.egui_ctx.clone(), db_path);
        worker.send(Command::LoadOptions);

        let mut app = App {
            worker,
            db,
            dark,
            options: FilterOptions::default(),
            options_loaded: false,
            combo_filtros: std::collections::HashMap::new(),
            filters: Filters::default(),
            show_import_dialog: false,
            show_progress_dialog: false,
            busy: false,
            progress: None,
            progress_titulo: "Progreso",
            enrich_finished: false,
            status: "Listo.".to_string(),
            logs: Vec::new(),
            stats,
            local: LocalFilters::default(),
            local_options,
            rows: Vec::new(),
            need_query: true,
            selected: None,
            selected_row: None,
            selected_detail: None,
            selected_datoscif: Vec::new(),
            selected_utes: Vec::new(),
            tab: Tab::Contratos,
            relacions: Vec::new(),
            relacions_loaded: false,
            revisions: Vec::new(),
            sen_match: Vec::new(),
            revisions_loaded: false,
            rev_search: std::collections::HashMap::new(),
            rev_results: std::collections::HashMap::new(),
            rev_searching: None,
            export_rx: None,
        };
        app.refresh_local();
        app.refresh_revision();
        app
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.worker.rx.try_recv() {
            match ev {
                Event::Options(o) => {
                    self.options = o;
                    self.options_loaded = true;
                }
                Event::SyncProgress { done, total, msg } => {
                    self.progress = if total > 0 { Some((done, total)) } else { None };
                    self.status = msg;
                    self.busy = true;
                }
                Event::SyncDone(r) => {
                    self.busy = false;
                    self.progress = None;
                    self.status = format!(
                        "Sincronización rematada · {} novos · {} actualizados · {} resoltos saltados · {} detalles · {} erros",
                        r.novos, r.actualizados, r.saltados, r.detalles_descargados, r.erros
                    );
                    self.stats = self.db.stats().unwrap_or_default();
                    self.local_options = self.db.local_options().unwrap_or_default();
                    self.need_query = true;
                }
                Event::EnrichDone(r) => {
                    self.busy = false;
                    self.progress = None;
                    self.enrich_finished = true;
                    self.status = if r.reimport {
                        format!(
                            "Reimportación rematada · {} empresas · {} con cargos · {} cargos · {} erros",
                            r.procesados, r.empresas_con_cargos, r.cargos, r.erros
                        )
                    } else {
                        format!(
                            "Vinculación rematada · {} procesados · {} vinculados · {} membros UTE · {} a revisar · {} sen match · {} empresas con cargos · {} cargos · {} erros",
                            r.procesados,
                            r.vinculados,
                            r.ute_membros,
                            r.a_revisar,
                            r.sen_match,
                            r.empresas_con_cargos,
                            r.cargos,
                            r.erros
                        )
                    };
                    // Os vínculos cambiaron: invalidar a vista de relacións, a cola
                    // de revisión e o detalle aberto, e refrescar a táboa.
                    self.relacions_loaded = false;
                    self.revisions_loaded = false;
                    self.need_query = true;
                    if let Some(row) = self.selected_row.clone() {
                        self.select_contract(row);
                    }
                }
                Event::RevisionResolved {
                    adx_nome,
                    vinculado,
                } => {
                    self.busy = false;
                    self.progress = None;
                    self.status = if vinculado {
                        format!("«{adx_nome}» vinculado.")
                    } else {
                        format!("«{adx_nome}» descartado.")
                    };
                    // Limpar o estado de busca asistida do caso resolto.
                    self.rev_search.remove(&adx_nome);
                    self.rev_results.remove(&adx_nome);
                    if self.rev_searching.as_deref() == Some(adx_nome.as_str()) {
                        self.rev_searching = None;
                    }
                    // O caso resolto sae da cola; recalcular relacións e detalle.
                    self.revisions_loaded = false;
                    self.relacions_loaded = false;
                    self.need_query = true;
                    if let Some(row) = self.selected_row.clone() {
                        self.select_contract(row);
                    }
                }
                Event::DatoscifResults {
                    adx_nome,
                    suggestions,
                } => {
                    if self.rev_searching.as_deref() == Some(adx_nome.as_str()) {
                        self.rev_searching = None;
                    }
                    self.rev_results.insert(adx_nome, suggestions);
                }
                Event::Exported(path, n) => {
                    self.busy = false;
                    self.status = format!("Exportadas {n} filas a {}", path.display());
                }
                Event::Error(e) => {
                    self.busy = false;
                    self.progress = None;
                    self.status = format!("⚠ {e}");
                    self.logs.push(e);
                }
                Event::Log(l) => self.logs.push(l),
            }
        }
        // Resultado do diálogo nativo «Gardar como» (noutro fío para non
        // bloquear a interface). Cando chega, arrincamos a exportación real.
        let picked = self.export_rx.as_ref().map(|rx| rx.try_recv());
        match picked {
            Some(Ok(escolla)) => {
                self.export_rx = None;
                if let Some(path) = escolla {
                    self.start_export(path);
                }
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => self.export_rx = None,
            // Aínda non escolleu: seguimos pintando para non perder o resultado.
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) => ctx.request_repaint(),
            None => {}
        }

        if self.need_query {
            self.refresh_local();
            self.need_query = false;
        }
        if !self.revisions_loaded {
            self.refresh_revision();
        }
        if self.busy {
            ctx.request_repaint();
        }
    }

    /// Lanza unha vinculación con datoscif (novos ou reimportación) e amosa o
    /// diálogo de progreso.
    fn start_enrich(&mut self, mode: EnrichMode) {
        self.busy = true;
        self.progress = None;
        self.enrich_finished = false;
        let (titulo, status): (&'static str, &str) = match mode {
            EnrichMode::Novos => (
                "Vinculación con datoscif (relacións)",
                "Iniciando vinculación con datoscif…",
            ),
            EnrichMode::Reimportar => (
                "Reimportación de datos das empresas",
                "Iniciando reimportación de datos das empresas…",
            ),
        };
        self.progress_titulo = titulo;
        self.logs.clear();
        self.status = status.into();
        self.worker.send(Command::Enrich(mode));
        self.show_progress_dialog = true;
    }

    /// Lanza a exportación a ODS no fío traballador e amosa o diálogo de progreso.
    fn start_export(&mut self, path: std::path::PathBuf) {
        self.busy = true;
        self.progress = None;
        self.enrich_finished = false;
        self.progress_titulo = "Exportación a ODS";
        self.logs.clear();
        self.status = "Exportando a ODS…".into();
        self.show_progress_dialog = true;
        self.worker.send(Command::Export {
            path,
            filters: self.local.clone(),
        });
    }

    fn refresh_local(&mut self) {
        match self.db.query_local(&self.local) {
            Ok(rows) => self.rows = rows,
            Err(e) => self.status = format!("⚠ consulta local: {e}"),
        }
    }

    fn select_contract(&mut self, row: LocalRow) {
        self.selected_detail = self.db.load_detail(&row.id).ok().flatten();
        self.selected_datoscif = self.build_datoscif_info();
        self.selected_utes = self.build_utes_info(&row.id);
        self.selected = Some(row.id.clone());
        self.selected_row = Some(row);
    }

    /// Composición das UTE adxudicatarias do contrato: cada membro coa súa entidade
    /// de datoscif (por CIF) e os seus cargos, se está vinculada.
    fn build_utes_info(&self, contract_id: &str) -> Vec<UteDetalle> {
        let mut out: Vec<UteDetalle> = Vec::new();
        for (ute_nome, nome, cif) in self
            .db
            .ute_membros_de_contrato(contract_id)
            .unwrap_or_default()
        {
            let entidade = self.db.entidade_por_cif(&cif).ok().flatten();
            let cargos = entidade
                .as_ref()
                .filter(|e| e.is_empresa())
                .map(|e| self.db.cargos_de_empresa(&e.url).unwrap_or_default())
                .unwrap_or_default();
            let membro = UteMembroVista {
                nome,
                cif,
                entidade,
                cargos,
            };
            match out.iter_mut().find(|u| u.ute_nome == ute_nome) {
                Some(u) => u.membros.push(membro),
                None => out.push(UteDetalle {
                    ute_nome,
                    membros: vec![membro],
                }),
            }
        }
        out
    }

    /// Reúne, para cada adxudicatario distinto do contrato aberto, a entidade de
    /// datoscif vinculada (se a hai) e os seus cargos (só empresas).
    fn build_datoscif_info(&self) -> Vec<AdxDatosCif> {
        let mut out = Vec::new();
        let mut vistos = std::collections::HashSet::new();
        if let Some((_, resolucions)) = &self.selected_detail {
            for r in resolucions {
                let adx = r.adxudicatario.trim();
                if adx.is_empty() || !vistos.insert(adx.to_string()) {
                    continue;
                }
                if let Ok(Some(ent)) = self.db.entidade_de_adxudicatario(adx) {
                    let cargos = if ent.is_empresa() {
                        self.db.cargos_de_empresa(&ent.url).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    out.push(AdxDatosCif {
                        adx_nome: adx.to_string(),
                        entidade: ent,
                        cargos,
                    });
                }
            }
        }
        out
    }

    fn close_detail(&mut self) {
        self.selected = None;
        self.selected_row = None;
        self.selected_detail = None;
        self.selected_datoscif.clear();
        self.selected_utes.clear();
    }

    fn refresh_relacions(&mut self) {
        self.relacions = self
            .db
            .relacions_compartidas(&self.local)
            .unwrap_or_default();
        self.relacions_loaded = true;
    }

    fn refresh_revision(&mut self) {
        self.revisions = self.db.casos_para_revisar().unwrap_or_default();
        self.sen_match = self.db.casos_sen_match().unwrap_or_default();
        self.revisions_loaded = true;
    }

    /// Lanza unha busca asistida en datoscif para un caso (non bloquea a interface;
    /// os resultados chegan por `Event::DatoscifResults`).
    fn start_search(&mut self, adx_nome: String, termo: String) {
        if termo.trim().is_empty() {
            return;
        }
        self.rev_searching = Some(adx_nome.clone());
        self.worker
            .send(Command::SearchDatoscif { adx_nome, termo });
    }

    /// Envía ao worker a resolución dun caso (vincular a `escolla` ou descartar)
    /// e amosa o diálogo de progreso.
    fn start_resolve(&mut self, adx_nome: String, escolla: Option<Suggestion>) {
        self.busy = true;
        self.progress = None;
        self.enrich_finished = false;
        self.progress_titulo = "Resolución manual";
        self.logs.clear();
        self.status = if escolla.is_some() {
            format!("Vinculando «{adx_nome}»…")
        } else {
            format!("Descartando «{adx_nome}»…")
        };
        self.worker
            .send(Command::ResolveMatch { adx_nome, escolla });
        self.show_progress_dialog = true;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.top_bar(ui);
        self.main_view(ui);
        if self.show_import_dialog {
            self.import_dialog(&ctx);
        }
        if self.show_progress_dialog {
            self.progress_dialog(&ctx);
        }
    }

    /// Chámase unha soa vez ao pechar (tamén ao pechar a ventá desde o sistema).
    /// Pecha primeiro o fío traballador (e a súa conexión SQLite) e logo fai o
    /// checkpoint da conexión principal para non deixar conexións sen pechar.
    fn on_exit(&mut self) {
        self.worker.shutdown();
        self.db.checkpoint();
    }

    /// Só queremos persistir a posición e o tamaño da ventá (a través da
    /// característica `persistence` de eframe e `persist_window`, activa por
    /// defecto). Non persistimos o estado interno de egui para que a interface
    /// arranque sempre limpa (combos pechados, scroll ao inicio, tema do sistema).
    fn persist_egui_memory(&self) -> bool {
        false
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("topbar")
            .exact_size(54.0)
            .show_inside(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.heading("Congal");
                    ui.add_space(16.0);

                    let importar = ui.add_enabled(
                        !self.busy,
                        egui::Button::new(theme::icon_label(
                            ui,
                            theme::icons::ARROW_BIG_DOWN,
                            "Importar contratos",
                            Color32::WHITE,
                        ))
                        .fill(theme::accent(self.dark)),
                    );
                    if importar.clicked() {
                        self.show_import_dialog = true;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let icon = if self.dark {
                            "☀ Claro"
                        } else {
                            "🌙 Escuro"
                        };
                        if ui.button(icon).clicked() {
                            self.dark = !self.dark;
                            theme::apply(ui.ctx(), self.dark);
                        }
                        ui.separator();
                        let mut info = format!(
                            "BD: {} contratos · {} con detalle",
                            self.stats.total, self.stats.con_detalle
                        );
                        if let Some(u) = &self.stats.ultima_sync {
                            info.push_str(&format!(" · última importación: {u}"));
                        }
                        ui.label(RichText::new(info).small().color(Color32::GRAY));
                    });
                });
            });
    }

    /// Modal de selección dos filtros de importación.
    fn import_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::new("import_dialog")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("Importar contratos");
            ui.label(
                RichText::new(
                    "Escolle o órgano de contratación (obrigatorio) e, opcionalmente, o ano. \
                     Os contratos xa resoltos non se volven descargar; só se actualizan os que \
                     seguían en proceso e os novos.",
                )
                .small()
                .color(Color32::GRAY),
            );
            ui.add_space(12.0);

            ui.label(RichText::new("Órgano de contratación").strong());
            combo_codigo(
                ui,
                "organo",
                &mut self.filters.organo,
                &self.options.organos,
                self.combo_filtros.entry("organo".into()).or_default(),
                false,
            );
            ui.add_space(10.0);

            ui.label(RichText::new("Ano").strong());
            year_combo(ui, &mut self.filters.year);

            if !self.options_loaded {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Cargando opcións de filtro…")
                        .small()
                        .italics(),
                );
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                // O órgano é obrigatorio para evitar importacións masivas.
                let pode_importar = !self.busy && !self.filters.organo.trim().is_empty();
                let importar = ui.add_enabled(
                    pode_importar,
                    egui::Button::new(RichText::new("Importar").color(Color32::WHITE))
                        .fill(theme::accent(self.dark)),
                );
                if importar.clicked() {
                    self.busy = true;
                    self.progress = None;
                    self.enrich_finished = false;
                    self.progress_titulo = "Importación de contratos";
                    self.logs.clear();
                    self.status = "Iniciando importación…".into();
                    self.worker.send(Command::Sync(self.filters.clone()));
                    self.show_import_dialog = false;
                    self.show_progress_dialog = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Cancelar").clicked() {
                        self.show_import_dialog = false;
                    }
                });
            });
        });

        // O modal só se pecha co botón "Cancelar" (ou ao iniciar a importación),
        // non ao premer fóra nin con Escape.
        let _ = modal;
    }

    /// Modal coa barra de progreso da importación en curso.
    fn progress_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::new("progress_dialog")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(self.progress_titulo);
            ui.add_space(10.0);

            if self.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&self.status);
                });
                ui.add_space(8.0);
                let frac = match self.progress {
                    Some((done, total)) => done as f32 / total.max(1) as f32,
                    None => 0.0,
                };
                let bar = egui::ProgressBar::new(frac).desired_width(ui.available_width());
                let bar = match self.progress {
                    Some((done, total)) => bar.text(format!("{done}/{total}")),
                    None => bar.animate(true),
                };
                ui.add(bar);
            } else {
                ui.label(RichText::new(&self.status).strong());
            }

            if !self.logs.is_empty() {
                ui.add_space(10.0);
                ui.collapsing("Rexistro", |ui| {
                    ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for l in self.logs.iter().rev().take(200) {
                            ui.label(RichText::new(l).small().monospace());
                        }
                    });
                });
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if self.busy {
                    if ui.button("Cancelar").clicked() {
                        self.worker.request_cancel();
                    }
                } else {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("Pechar").color(Color32::WHITE))
                                    .fill(theme::accent(self.dark)),
                            )
                            .clicked()
                        {
                            self.show_progress_dialog = false;
                        }
                        // Tras unha vinculación, ofrécese redescargar a ficha e os
                        // cargos de todas as empresas xa vinculadas (volver a importar
                        // «Importar relacións» non fai nada se non hai novos
                        // adxudicatarios).
                        if self.enrich_finished
                            && ui.button("Reimportar datos das empresas").clicked()
                        {
                            self.start_enrich(EnrichMode::Reimportar);
                        }
                    });
                }
            });
        });

        // Mentres a importación está en curso non se pode pechar facendo clic fóra.
        if !self.busy && modal.should_close() {
            self.show_progress_dialog = false;
        }
    }

    fn main_view(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("filtros_local")
            .resizable(true)
            .default_size(300.0)
            .show_inside(ui, |ui| {
                ui.add_space(6.0);
                ui.heading("Busca local");
                ui.add_space(6.0);
                let mut changed = false;
                ui.label(RichText::new("Texto (obxecto/referencia)").strong());
                changed |= text_input(ui, &mut self.local.texto).changed();
                ui.add_space(6.0);
                ui.label(RichText::new("Adxudicatario").strong());
                changed |= combo_valor(
                    ui,
                    "loc_adx",
                    &mut self.local.adxudicatario,
                    &self.local_options.adxudicatarios,
                    self.combo_filtros.entry("loc_adx".into()).or_default(),
                );
                ui.add_space(6.0);
                ui.label(RichText::new("Organismo").strong());
                changed |= combo_valor(
                    ui,
                    "loc_org",
                    &mut self.local.organismo,
                    &self.local_options.organismos,
                    self.combo_filtros.entry("loc_org".into()).or_default(),
                );
                ui.add_space(6.0);
                ui.label(RichText::new("Estado").strong());
                changed |= combo_valor(
                    ui,
                    "loc_est",
                    &mut self.local.estado,
                    &self.local_options.estados,
                    self.combo_filtros.entry("loc_est".into()).or_default(),
                );
                ui.add_space(6.0);
                ui.label(RichText::new("Ano").strong());
                changed |= combo_valor(
                    ui,
                    "loc_ano",
                    &mut self.local.year,
                    &self.local_options.anos,
                    self.combo_filtros.entry("loc_ano".into()).or_default(),
                );

                ui.add_space(12.0);
                if ui.button("Limpar").clicked() {
                    self.local = LocalFilters::default();
                    for k in ["loc_adx", "loc_org", "loc_est", "loc_ano"] {
                        self.combo_filtros.remove(k);
                    }
                    changed = true;
                }
                // A busca execútase automaticamente cando cambia calquera filtro.
                // Os filtros aplícanse ás dúas pestanas, así que invalidamos tamén
                // a vista de relacións para que se recalcule co novo subconxunto.
                if changed {
                    self.need_query = true;
                    self.relacions_loaded = false;
                }

                ui.add_space(10.0);
                ui.separator();
                let export = ui.add_enabled(
                    !self.busy && !self.rows.is_empty() && self.export_rx.is_none(),
                    egui::Button::new(RichText::new("Exportar a ODS").color(Color32::WHITE))
                        .fill(theme::accent(self.dark)),
                );
                if export.clicked() {
                    // O diálogo nativo «Gardar como» execútase noutro fío: se se
                    // chamase aquí (no fío da interface) bloquearíao e o sistema
                    // marcaría a app como «non responde». O destino devólvese pola
                    // canle e recóllese en `drain_events`.
                    let (tx, rx) = std::sync::mpsc::channel();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let path = rfd::FileDialog::new()
                            .add_filter("OpenDocument Spreadsheet", &["ods"])
                            .set_file_name("contratos.ods")
                            .save_file();
                        let _ = tx.send(path);
                        ctx.request_repaint();
                    });
                    self.export_rx = Some(rx);
                }
                ui.label(
                    RichText::new(format!("{} filas no resultado", self.rows.len()))
                        .small()
                        .color(Color32::GRAY),
                );

                ui.add_space(10.0);
                ui.separator();
                ui.label(RichText::new("Empresas").strong());
                let vincular = ui.add_enabled(!self.busy, egui::Button::new("Importar relacións"));
                if vincular.clicked() {
                    self.start_enrich(EnrichMode::Novos);
                }
                ui.label(
                    RichText::new(
                        "Busca cada adxudicatario en datoscif.es e garda os seus \
                         administradores e apoderados.",
                    )
                    .small()
                    .color(Color32::GRAY),
                );
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.tab == Tab::Contratos, "📄 Contratos")
                    .clicked()
                {
                    self.tab = Tab::Contratos;
                }
                if ui
                    .selectable_label(self.tab == Tab::Relacions, "🔗 Relacións")
                    .clicked()
                {
                    self.tab = Tab::Relacions;
                }
                let rev_label = if self.revisions.is_empty() {
                    "📝 Revisión".to_string()
                } else {
                    format!("📝 Revisión ({})", self.revisions.len())
                };
                if ui
                    .selectable_label(self.tab == Tab::Revision, rev_label)
                    .clicked()
                {
                    self.tab = Tab::Revision;
                }
            });
            ui.add_space(4.0);
            ui.separator();

            match self.tab {
                Tab::Contratos => {
                    if self.selected.is_some() {
                        self.detail_view(ui);
                    } else {
                        self.results_table(ui);
                    }
                }
                Tab::Relacions => self.relacions_view(ui),
                Tab::Revision => self.revision_view(ui),
            }
        });
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        let mut clicked: Option<LocalRow> = None;
        let selected = self.selected.clone();
        // Cor de resalte para os contratos cun único participante.
        let aviso = if self.dark {
            Color32::from_rgb(0xF0, 0xB0, 0x30)
        } else {
            Color32::from_rgb(0xB5, 0x6A, 0x00)
        };
        // Orde actual (cópiase para usala dentro do peche da cabeceira sen tomar
        // prestado `self`); o clic recóllese e aplícase tras a táboa.
        let (cur_col, cur_asc) = (self.local.sort_col, self.local.sort_asc);
        let accent = theme::accent(self.dark);
        let mut clicked_header: Option<SortColumn> = None;
        // egui_extras só debuxa a liña de redimensión nas columnas redimensionables. As
        // columnas `remainder` (Obxecto, Organismo, Adxudicatario) non o son, así que se
        // recolle o x do bordo dereito de cada unha na cabeceira e píntanse a man esas
        // liñas de alto completo trala táboa, co mesmo trazo e posición que as de egui_extras.
        let mut sep_xs: Vec<f32> = Vec::new();
        let spacing_x = ui.spacing().item_spacing.x;
        let sep_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        let table_top = ui.cursor().top();
        // Texto non seleccionable nas celas: así o cursor non entra en modo
        // inserción de texto e o clic chega á fila enteira (sense ::click).
        ui.style_mut().interaction.selectable_labels = false;
        let out = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            // As columnas numéricas/de data son redimensionables cun ancho propio. As de
            // texto (Obxecto, Organismo, Adxudicatario) son `remainder`: reparten o espazo
            // sobrante e medran ao agrandar a ventá (Obxecto algo máis ancha polo `at_least`).
            // Teñen que ser `.resizable(false)`: en egui_extras unha columna `remainder` só
            // segue ocupando o espazo sobrante en cada fotograma se NON é redimensionable;
            // se o é, queda fixada co ancho do primeiro fotograma.
            .column(Column::initial(70.0).at_least(56.0))
            .column(Column::initial(90.0).at_least(70.0))
            .column(
                Column::remainder()
                    .at_least(200.0)
                    .clip(true)
                    .resizable(false),
            )
            .column(Column::initial(110.0).at_least(90.0))
            .column(Column::initial(120.0).at_least(80.0).clip(true))
            .column(
                Column::remainder()
                    .at_least(150.0)
                    .clip(true)
                    .resizable(false),
            )
            .column(
                Column::remainder()
                    .at_least(150.0)
                    .clip(true)
                    .resizable(false),
            )
            .column(Column::initial(120.0).at_least(90.0))
            .header(24.0, |mut h| {
                // (título, columna de orde). Premer ordena; volver premer inverte. A
                // columna activa marca cor de acento e frecha. Os títulos van centrados
                // e a etiqueta enche a cela (todo o ancho é clicable). Úsanse etiquetas
                // clicables (non `selectable_label`) para que o fondo do hover non quede
                // recortado polas columnas con `clip`.
                for (t, col) in [
                    ("ID", SortColumn::Id),
                    ("Data", SortColumn::Data),
                    ("Obxecto", SortColumn::Obxecto),
                    ("Importe", SortColumn::Importe),
                    ("Estado", SortColumn::Estado),
                    ("Organismo", SortColumn::Organismo),
                    ("Adxudicatario", SortColumn::Adxudicatario),
                    ("Imp. resolución", SortColumn::ImporteResolucion),
                ] {
                    let activa = cur_col == col;
                    let etiqueta = if activa {
                        format!("{t} {}", if cur_asc { "▲" } else { "▼" })
                    } else {
                        t.to_string()
                    };
                    let mut txt = RichText::new(etiqueta).strong();
                    if activa {
                        txt = txt.color(accent);
                    }
                    h.col(|ui| {
                        // Recóllese o bordo dereito (antes da marxe) das columnas `remainder`
                        // para pintarlles a man a liña separadora que egui_extras non debuxa.
                        if matches!(
                            col,
                            SortColumn::Obxecto | SortColumn::Organismo | SortColumn::Adxudicatario
                        ) {
                            sep_xs.push(ui.max_rect().right() + spacing_x);
                        }
                        pad_cela(ui);
                        let lab = egui::Label::new(txt).sense(egui::Sense::click());
                        let resp = ui
                            .with_layout(
                                Layout::centered_and_justified(egui::Direction::LeftToRight),
                                |ui| ui.add(lab),
                            )
                            .inner;
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            clicked_header = Some(col);
                        }
                    });
                }
            })
            .body(|mut body| {
                for r in &self.rows {
                    let is_sel = selected.as_deref() == Some(r.id.as_str());
                    body.row(22.0, |mut row| {
                        row.set_selected(is_sel);
                        // Icona de aviso á esquerda; ID á dereita (números aliñados).
                        row.col(|ui| {
                            pad_cela(ui);
                            // Icona de aviso á esquerda; ID á dereita. Faise nun único
                            // `right_to_left` (centrado en vertical coma o resto de celas)
                            // para que o texto do ID quede á mesma altura; o aviso métese
                            // ao final (esquerda) cun `left_to_right` que ocupa o resto.
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(&r.id);
                                if r.participante_unico {
                                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                        ui.label(RichText::new("⚠").color(aviso))
                                            .on_hover_text("Un só participante presentado");
                                    });
                                }
                            });
                        });
                        row.col(|ui| {
                            pad_cela(ui);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(&r.publicacion);
                            });
                        });
                        row.col(|ui| {
                            pad_cela(ui);
                            ui.label(&r.asunto);
                        });
                        row.col(|ui| {
                            pad_cela(ui);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(&r.importe_txt);
                            });
                        });
                        row.col(|ui| {
                            pad_cela(ui);
                            ui.label(&r.estado);
                        });
                        row.col(|ui| {
                            pad_cela(ui);
                            ui.label(&r.organismo);
                        });
                        row.col(|ui| {
                            pad_cela(ui);
                            ui.label(&r.adxudicatario);
                        });
                        row.col(|ui| {
                            pad_cela(ui);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(&r.importe_resolucion_txt);
                            });
                        });
                        let resp = row.response();
                        resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            clicked = Some(r.clone());
                        }
                    });
                }
            });
        // Liñas separadoras das columnas `remainder` non redimensionables: de alto
        // completo (da cabeceira ata o fondo do contido, recortado ao viewport), para
        // que se vexan continuas e aliñen coas que debuxa egui_extras nas demais.
        let bottom = (out.inner_rect.top() + out.content_size.y).min(out.inner_rect.bottom());
        for x in sep_xs {
            ui.painter()
                .vline(x, egui::Rangef::new(table_top, bottom), sep_stroke);
        }
        if let Some(col) = clicked_header {
            // Mesma columna: inverte; nova columna: sentido por defecto segundo o tipo
            // (numéricas/data → desc; texto → asc).
            if self.local.sort_col == col {
                self.local.sort_asc = !self.local.sort_asc;
            } else {
                self.local.sort_col = col;
                self.local.sort_asc = col.default_asc();
            }
            self.need_query = true;
        }
        if let Some(row) = clicked {
            self.select_contract(row);
        }
    }

    /// Vista de detalle do contrato seleccionado. Ocupa o espazo da táboa ata
    /// que se volve á lista co botón "← Volver".
    fn detail_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("← Volver á lista").clicked() {
                self.close_detail();
            }
            ui.add_space(8.0);
            ui.heading("Detalle do contrato");
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        // Se xa se pechou neste fotograma evitamos pintar o resto.
        if self.selected.is_none() {
            return;
        }

        ScrollArea::vertical().show(ui, |ui| {
            // Resumo do listado (sempre dispoñible, mesmo sen detalle descargado).
            if let Some(r) = &self.selected_row {
                if !r.asunto.trim().is_empty() {
                    ui.label(RichText::new(&r.asunto).strong().size(16.0));
                    ui.add_space(8.0);
                }
                kv(ui, "ID", &r.id);
                kv(ui, "Referencia", &r.referencia);
                kv(ui, "Data de publicación", &r.publicacion);
                kv(ui, "Estado", &r.estado);
                kv(ui, "Importe", &r.importe_txt);
                kv(ui, "Organismo", &r.organismo);
            }

            match &self.selected_detail {
                None => {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Sen detalle descargado para este contrato.")
                            .italics()
                            .color(Color32::GRAY),
                    );
                }
                Some((d, resolucions)) => {
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label(RichText::new("Datos do contrato").strong());
                    ui.add_space(6.0);
                    kv(ui, "Obxecto", &d.obxecto);
                    kv(ui, "Tipo de contrato", &d.tipo_contrato);
                    kv(ui, "Tipo de procedemento", &d.tipo_procedemento);
                    kv(ui, "Tipo de tramitación", &d.tipo_tramitacion);
                    kv(ui, "Orzamento base", &d.orzamento_base);
                    kv(ui, "Valor estimado", &d.valor_estimado);
                    kv(ui, "Nº lotes", &d.num_lotes);
                    kv(ui, "Sistema de contratación", &d.sistema_contratacion);
                    kv(ui, "Data de difusión", &d.data_difusion);
                    kv(ui, "Observacións", &d.observacions);

                    if !d.enlace_resolucion.is_empty() {
                        ui.add_space(8.0);
                        ui.hyperlink_to("🔗 Abrir resolución na web", &d.enlace_resolucion);
                    }

                    if !resolucions.is_empty() {
                        ui.add_space(12.0);
                        ui.label(RichText::new("Resolucións / adxudicacións").strong());
                        for r in resolucions {
                            ui.add_space(6.0);
                            ui.group(|ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    RichText::new(format!(
                                        "Lote {} · {}",
                                        if r.lote.is_empty() { "—" } else { &r.lote },
                                        r.estado_resolucion
                                    ))
                                    .strong(),
                                );
                                field(ui, "Adxudicatario", &r.adxudicatario);
                                field(ui, "NIF", &r.nif);
                                field(ui, "Importe", &r.importe_txt);
                                field(ui, "Data difusión", &r.data_difusion);
                                field(ui, "Prazo execución", &r.prazo_execucion);
                            });
                        }
                    }

                    if !d.extra.is_empty() {
                        ui.add_space(12.0);
                        ui.collapsing("Outros campos", |ui| {
                            for (k, v) in &d.extra {
                                kv(ui, k, v);
                            }
                        });
                    }
                }
            }

            // Información de datoscif: quen está detrás de cada adxudicatario.
            render_datoscif(ui, &self.selected_datoscif);
            // Composición das UTE adxudicatarias (empresas membro).
            render_utes(ui, &self.selected_utes);
        });
    }

    /// Vista de relacións: persoas que controlan varias razóns sociais que
    /// aparecen como adxudicatarias nos contratos.
    fn relacions_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Relacións entre razóns sociais");
        ui.label(
            RichText::new(
                "Grupos de razóns sociais conectadas entre si por persoas \
                 (administradores/apoderados) cun cargo en dúas ou máis das empresas \
                 adxudicatarias dos teus contratos. Cada grupo reúne todas as empresas e \
                 persoas dunha mesma trama. Os vínculos por cargos xa cesados (pasados) \
                 resáltanse: son indicio de relación pero NON son actuais.",
            )
            .small()
            .color(Color32::GRAY),
        );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if !self.relacions_loaded {
            self.refresh_relacions();
        }
        if self.relacions.is_empty() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "Aínda non se atoparon relacións. Vincula primeiro os adxudicatarios \
                     con datoscif (botón «Vincular con datoscif»).",
                )
                .italics()
                .color(Color32::GRAY),
            );
            return;
        }

        // Cor de resalte para os vínculos históricos (cargos cesados). Cáptase
        // antes do préstamo inmutable de `self.relacions`.
        let historico = if self.dark {
            Color32::from_rgb(0xF0, 0xB0, 0x30)
        } else {
            Color32::from_rgb(0xB5, 0x6A, 0x00)
        };
        // Cor do vínculo por UTE (distinta do histórico e do administrador).
        let ute_color = if self.dark {
            Color32::from_rgb(0x5A, 0xC8, 0xE0)
        } else {
            Color32::from_rgb(0x0A, 0x84, 0xA5)
        };

        // Id do contrato premido nun despregable; trátase tras a ScrollArea
        // (fóra do préstamo inmutable de `self.relacions`) para saltar á súa ficha.
        let mut jump: Option<String> = None;
        ScrollArea::vertical().show(ui, |ui| {
            for g in &self.relacions {
                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    // Persoas que conectan o grupo (pode non habelas: trama só por UTE).
                    if !g.persoas.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Persoas:").small().color(Color32::GRAY));
                            for p in &g.persoas {
                                ui.hyperlink_to(
                                    RichText::new(&p.persona_nome).strong(),
                                    format!("{DATOSCIF_BASE}/directivo/{}", p.persona_url),
                                );
                                // Resumo dos vínculos da persoa: cantos actuais e cantos
                                // xa cesados. Se hai algún pasado, resáltase.
                                let (txt, color) = if p.empresas_pasadas == 0 {
                                    (format!("({} empresas)", p.num_empresas), Color32::GRAY)
                                } else if p.empresas_activas == 0 {
                                    (
                                        format!("({} empresas, todas pasadas)", p.num_empresas),
                                        historico,
                                    )
                                } else {
                                    (
                                        format!(
                                            "({} actuais · {} pasada{})",
                                            p.empresas_activas,
                                            p.empresas_pasadas,
                                            if p.empresas_pasadas == 1 { "" } else { "s" },
                                        ),
                                        historico,
                                    )
                                };
                                ui.label(RichText::new(txt).small().color(color));
                            }
                        });
                    }

                    // UTE: empresas que concorreron xuntas (vínculo distinto do
                    // administrador compartido).
                    for u in &g.utes {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("🤝 UTE:").small().strong().color(ute_color));
                            ui.label(RichText::new(&u.nome).small().strong());
                            if !u.membros.is_empty() {
                                ui.label(
                                    RichText::new(format!("— {}", u.membros.join(" + ")))
                                        .small()
                                        .color(Color32::GRAY),
                                );
                            }
                        });
                    }

                    // Razóns sociais do grupo.
                    ui.add_space(4.0);
                    for e in &g.empresas {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("•");
                            ui.hyperlink_to(
                                RichText::new(&e.empresa_nome).strong(),
                                format!("{DATOSCIF_BASE}/empresa/{}", e.empresa_url),
                            );
                            let mut extra = format!("· {} contratos", e.num_contratos);
                            if !e.provincia.is_empty() {
                                extra = format!("· {} {}", e.provincia, extra);
                            }
                            ui.label(RichText::new(extra).small().color(Color32::GRAY));
                            // Vínculo só por cargos cesados (non se marca se a empresa
                            // entra no grupo por UTE, non por administrador).
                            if e.con_cargos && !e.activa {
                                ui.label(
                                    RichText::new("⏱ só cargos pasados")
                                        .small()
                                        .strong()
                                        .color(historico),
                                );
                            }
                        });
                    }

                    // Despregable cos contratos adxudicados ás empresas do grupo,
                    // coa suma dos importes no encabezado.
                    if !g.contratos.is_empty() {
                        ui.add_space(6.0);
                        let titulo = format!(
                            "Contratos adxudicados ({}) · {} en total",
                            g.contratos.len(),
                            format_importe(g.importe_total),
                        );
                        let id = egui::Id::new(("contratos_grupo", &g.empresas[0].empresa_url));
                        egui::CollapsingHeader::new(RichText::new(titulo).strong())
                            .id_salt(id)
                            .show(ui, |ui| {
                                // As celas non capturan o clic: así o cursor non entra
                                // en modo selección e o clic chega á fila enteira.
                                ui.style_mut().interaction.selectable_labels = false;
                                for c in &g.contratos {
                                    let resp = ui
                                        .horizontal_wrapped(|ui| {
                                            ui.label(
                                                RichText::new(format!("#{}", c.contract_id))
                                                    .small()
                                                    .monospace()
                                                    .color(Color32::GRAY),
                                            );
                                            ui.label(
                                                RichText::new(&c.publicacion)
                                                    .small()
                                                    .monospace()
                                                    .color(Color32::GRAY),
                                            );
                                            ui.label(
                                                RichText::new(&c.importe_txt).small().strong(),
                                            );
                                            ui.label(
                                                RichText::new(format!("· {}", c.empresa_nome))
                                                    .small()
                                                    .color(Color32::GRAY),
                                            );
                                            if !c.asunto.is_empty() {
                                                ui.label(
                                                    RichText::new(format!("— {}", c.asunto))
                                                        .small(),
                                                );
                                            }
                                        })
                                        .response
                                        .interact(egui::Sense::click());
                                    if resp.hovered() {
                                        resp.clone()
                                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    }
                                    if resp.clicked() {
                                        jump = Some(c.contract_id.clone());
                                    }
                                }
                            });
                    }
                });
            }
        });

        // Saltar á ficha do contrato premido: selecciónase e cámbiase á pestana
        // Contratos, que pasará a amosar a vista de detalle no seguinte fotograma.
        if let Some(id) = jump
            && let Some(row) = self.rows.iter().find(|r| r.id == id).cloned()
        {
            self.select_contract(row);
            self.tab = Tab::Contratos;
        }
    }

    /// Cola de revisión manual + proceso asistido para os casos sen correspondencia.
    fn revision_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Revisión de vínculos");
        ui.label(
            RichText::new(
                "Adxudicatarios que datoscif non puido vincular con confianza dabondo. \
                 Escolle a entidade correcta entre os candidatos ou búscaa a man en datoscif; \
                 descártaos se ningún coincide. O NIF do contrato (cando se coñece) axuda a decidir.",
            )
            .small()
            .color(Color32::GRAY),
        );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if self.revisions.is_empty() && self.sen_match.is_empty() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "Non hai casos pendentes. Importa relacións para xerar vínculos a revisar.",
                )
                .italics()
                .color(Color32::GRAY),
            );
            return;
        }

        let busy = self.busy;
        // Sácanse as listas para poder mutar o estado de busca (`self.rev_*`)
        // mentres se pintan os casos.
        let revisar = std::mem::take(&mut self.revisions);
        let sen_match = std::mem::take(&mut self.sen_match);
        let mut accion: Option<RevAction> = None;

        ScrollArea::vertical().show(ui, |ui| {
            if !revisar.is_empty() {
                ui.label(
                    RichText::new(format!("Con candidatos suxeridos ({})", revisar.len())).strong(),
                );
                for caso in &revisar {
                    if let Some(a) = self.render_caso(ui, caso, busy) {
                        accion = Some(a);
                    }
                }
            }
            if !sen_match.is_empty() {
                ui.add_space(8.0);
                egui::CollapsingHeader::new(
                    RichText::new(format!(
                        "Sen correspondencia en datoscif ({})",
                        sen_match.len()
                    ))
                    .strong(),
                )
                .id_salt("sen_match")
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "Non se atopou candidato automaticamente. Busca a man axustando o \
                             termo (quita acentos ou sufixos, ou usa só parte do nome).",
                        )
                        .small()
                        .color(Color32::GRAY),
                    );
                    for caso in &sen_match {
                        if let Some(a) = self.render_caso(ui, caso, busy) {
                            accion = Some(a);
                        }
                    }
                });
            }
        });

        self.revisions = revisar;
        self.sen_match = sen_match;

        match accion {
            Some(RevAction::Resolve(adx_nome, escolla)) => self.start_resolve(adx_nome, escolla),
            Some(RevAction::Search(adx_nome, termo)) => self.start_search(adx_nome, termo),
            None => {}
        }
    }

    /// Pinta un caso de revisión: candidatos suxeridos (se os hai), busca asistida
    /// en datoscif e descartar. Devolve a acción escollida, se a hai.
    fn render_caso(
        &mut self,
        ui: &mut egui::Ui,
        caso: &CasoRevision,
        busy: bool,
    ) -> Option<RevAction> {
        // Estado de busca deste caso (a locais; escríbese de volta tras pintar).
        let mut termo = self
            .rev_search
            .get(&caso.adx_nome)
            .cloned()
            .unwrap_or_else(|| caso.adx_nome.clone());
        let searching = self.rev_searching.as_deref() == Some(caso.adx_nome.as_str());
        let resultados = self.rev_results.get(&caso.adx_nome).cloned();
        let mut accion: Option<RevAction> = None;

        ui.add_space(6.0);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&caso.adx_nome).strong().size(15.0));
                if !caso.nif.is_empty() {
                    ui.label(
                        RichText::new(format!("NIF {}", caso.nif))
                            .small()
                            .monospace()
                            .color(Color32::GRAY),
                    );
                }
            });

            // Candidatos suxeridos automaticamente.
            if !caso.candidatos.is_empty() {
                ui.add_space(4.0);
                for c in &caso.candidatos {
                    if candidato_row(ui, c, busy) {
                        accion = Some(RevAction::Resolve(caso.adx_nome.clone(), Some(c.clone())));
                    }
                }
            }

            // Busca asistida en datoscif.
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Buscar en datoscif:")
                        .small()
                        .color(Color32::GRAY),
                );
                let resp = ui.add_enabled(
                    !busy && !searching,
                    egui::TextEdit::singleline(&mut termo)
                        .desired_width(220.0)
                        .hint_text("nome a buscar"),
                );
                let premido_enter =
                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let buscar = ui.add_enabled(!busy && !searching, egui::Button::new("Buscar"));
                if searching {
                    ui.spinner();
                }
                if (buscar.clicked() || premido_enter) && !termo.trim().is_empty() {
                    accion = Some(RevAction::Search(caso.adx_nome.clone(), termo.clone()));
                }
            });

            // Resultados da busca asistida.
            if let Some(res) = &resultados {
                if res.is_empty() {
                    ui.label(
                        RichText::new("Sen resultados. Proba a axustar o termo.")
                            .small()
                            .italics()
                            .color(Color32::GRAY),
                    );
                } else {
                    for c in res.iter().take(15) {
                        if candidato_row(ui, c, busy) {
                            accion =
                                Some(RevAction::Resolve(caso.adx_nome.clone(), Some(c.clone())));
                        }
                    }
                }
            }

            ui.add_space(4.0);
            if ui
                .add_enabled(!busy, egui::Button::new("Ningún — descartar"))
                .clicked()
            {
                accion = Some(RevAction::Resolve(caso.adx_nome.clone(), None));
            }
        });

        self.rev_search.insert(caso.adx_nome.clone(), termo);
        accion
    }
}

/// Pinta unha fila de candidato co botón «Vincular». Devolve `true` se se premeu.
fn candidato_row(ui: &mut egui::Ui, c: &Suggestion, busy: bool) -> bool {
    let es_empresa = c.tipo_entidad == 1;
    let ruta = if es_empresa { "empresa" } else { "directivo" };
    let mut vincular = false;
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!busy, egui::Button::new("Vincular"))
            .clicked()
        {
            vincular = true;
        }
        ui.label(if es_empresa { "🏢" } else { "👤" });
        ui.label(RichText::new(&c.nombre).strong());
        ui.hyperlink_to("↗ datoscif", format!("{DATOSCIF_BASE}/{ruta}/{}", c.url));
    });
    vincular
}

/// Renderiza, na vista de detalle, a entidade de datoscif e os seus cargos para
/// cada adxudicatario do contrato.
fn render_datoscif(ui: &mut egui::Ui, info: &[AdxDatosCif]) {
    if info.is_empty() {
        return;
    }
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);
    ui.label(RichText::new("Quen está detrás (datoscif)").strong());
    for a in info {
        ui.add_space(6.0);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&a.entidade.nome).strong());
                let ruta = if a.entidade.is_empresa() {
                    "empresa"
                } else {
                    "directivo"
                };
                ui.hyperlink_to(
                    "↗ datoscif",
                    format!("{DATOSCIF_BASE}/{ruta}/{}", a.entidade.url),
                );
            });
            if a.adx_nome != a.entidade.nome {
                ui.label(
                    RichText::new(format!("adxudicatario: {}", a.adx_nome))
                        .small()
                        .color(Color32::GRAY),
                );
            }
            // Datos da persoa xurídica (CIF, localización).
            let e = &a.entidade;
            if !e.cif.is_empty() {
                ui.label(RichText::new(format!("CIF: {}", e.cif)).small());
            }
            let lugar = match (e.municipio.is_empty(), e.provincia.is_empty()) {
                (false, false) => format!("{} ({})", e.municipio, e.provincia),
                (false, true) => e.municipio.clone(),
                (true, false) => e.provincia.clone(),
                (true, true) => String::new(),
            };
            if !lugar.is_empty() {
                let mut txt = lugar;
                if !e.domicilio.is_empty() {
                    txt = format!("{} · {}", e.domicilio, txt);
                }
                ui.label(RichText::new(txt).small().color(Color32::GRAY));
            }
            if a.entidade.is_empresa() {
                if a.cargos.is_empty() {
                    ui.label(
                        RichText::new("Sen cargos rexistrados.")
                            .small()
                            .italics()
                            .color(Color32::GRAY),
                    );
                } else {
                    ui.add_space(4.0);
                    render_cargos(ui, &a.cargos);
                }
            }
        });
    }
}

/// Reserva unha marxe interior de 4px a ambos lados da cela para que o contido
/// (sobre todo os números aliñados á dereita e o texto recortado) non toque os
/// bordos da columna.
fn pad_cela(ui: &mut egui::Ui) {
    const PAD: f32 = 4.0;
    let ancho = ui.available_width();
    ui.add_space(PAD);
    ui.set_max_width((ancho - 2.0 * PAD).max(0.0));
}

/// Renderiza a lista de cargos dunha empresa: activos con ●; cesados con ○,
/// atenuados e marcados como históricos (cor ámbar) coa data de cese.
fn render_cargos(ui: &mut egui::Ui, cargos: &[CargoRow]) {
    let historico = if ui.visuals().dark_mode {
        Color32::from_rgb(0xF0, 0xB0, 0x30)
    } else {
        Color32::from_rgb(0xB5, 0x6A, 0x00)
    };
    for c in cargos {
        ui.horizontal_wrapped(|ui| {
            if c.activo {
                ui.label(RichText::new("●").small());
                ui.label(RichText::new(&c.persona_nome).strong().small());
                if !c.cargo.is_empty() {
                    ui.label(RichText::new(format!("— {}", c.cargo)).small());
                }
            } else {
                ui.label(RichText::new("○").small().color(historico));
                ui.label(RichText::new(&c.persona_nome).small().color(Color32::GRAY));
                if !c.cargo.is_empty() {
                    ui.label(
                        RichText::new(format!("— {}", c.cargo))
                            .small()
                            .color(Color32::GRAY),
                    );
                }
                let marca = if c.hasta.is_empty() {
                    "· cesado".to_string()
                } else {
                    format!("· cesado o {}", format_data_gl(&c.hasta))
                };
                ui.label(RichText::new(marca).small().strong().color(historico));
            }
        });
    }
}

/// Renderiza, na ficha do contrato, a composición das UTE adxudicatarias: cada
/// empresa membro co seu CIF, e (se está vinculada) a súa ficha de datoscif e cargos.
fn render_utes(ui: &mut egui::Ui, utes: &[UteDetalle]) {
    if utes.is_empty() {
        return;
    }
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);
    ui.label(RichText::new("Composición da UTE").strong());
    for u in utes {
        ui.add_space(6.0);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(&u.ute_nome).strong());
            for m in &u.membros {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("🏢");
                    ui.label(RichText::new(&m.nome).strong());
                    if !m.cif.is_empty() {
                        ui.label(
                            RichText::new(format!("· {}", m.cif))
                                .small()
                                .monospace()
                                .color(Color32::GRAY),
                        );
                    }
                    match &m.entidade {
                        Some(e) if !e.url.is_empty() => {
                            ui.hyperlink_to(
                                "↗ datoscif",
                                format!("{DATOSCIF_BASE}/empresa/{}", e.url),
                            );
                        }
                        _ => {
                            ui.label(
                                RichText::new("(sen vincular)")
                                    .small()
                                    .italics()
                                    .color(Color32::GRAY),
                            );
                        }
                    }
                });
                if !m.cargos.is_empty() {
                    render_cargos(ui, &m.cargos);
                }
            }
        });
    }
}

/// Campo de texto dunha liña con padding interior e ancho completo.
fn text_input(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .margin(egui::Margin::symmetric(8, 6))
            .desired_width(f32::INFINITY),
    )
}

/// Fila «etiqueta + valor» para a vista de detalle. A etiqueta ocupa unha
/// columna fixa á esquerda e o valor axústase a varias liñas (wrap) para que o
/// texto longo non se saia do marco da vista. Omítese se o valor está baleiro.
fn kv(ui: &mut egui::Ui, label: &str, value: &str) {
    if value.trim().is_empty() || value == "_" {
        return;
    }
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(180.0, 0.0),
            Layout::left_to_right(Align::TOP),
            |ui| ui.label(RichText::new(label).strong()),
        );
        // `wrap()` fai que o valor se reparta en varias liñas dentro do ancho
        // restante en lugar de desbordar a vista.
        ui.add(egui::Label::new(value).wrap());
    });
    ui.add_space(4.0);
}

/// Mostra unha etiqueta + valor se o valor non está baleiro.
fn field(ui: &mut egui::Ui, label: &str, value: &str) {
    if value.trim().is_empty() || value == "_" {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{label}: ")).strong().small());
        ui.label(RichText::new(value).small());
    });
}

/// Dropdown de anos (descendente, dende o ano actual) cunha opción "(todos)".
fn year_combo(ui: &mut egui::Ui, selected: &mut String) {
    let current = crate::model::current_year();
    let actual = if selected.is_empty() {
        "(todos)".to_string()
    } else {
        selected.clone()
    };
    egui::ComboBox::from_id_salt("ano")
        .selected_text(actual)
        .width(ui.available_width().min(280.0))
        .height(320.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(selected.is_empty(), "(todos)")
                .clicked()
            {
                selected.clear();
            }
            for y in (2008..=current).rev() {
                let ys = y.to_string();
                if ui.selectable_label(*selected == ys, &ys).clicked() {
                    *selected = ys;
                }
            }
        });
}

/// Dropdown de selección dun valor exacto dunha lista (co buscador integrado).
/// Inclúe a opción "(todos)" que limpa o filtro. Devolve `true` se cambiou a selección.
fn combo_valor(
    ui: &mut egui::Ui,
    id: &str,
    selected: &mut String,
    values: &[String],
    filtro: &mut String,
) -> bool {
    let actual = if selected.is_empty() {
        "(todos)"
    } else {
        selected.as_str()
    };

    let open_id = egui::Id::new(("combo_open", id));
    let was_open: bool = ui.data(|d| d.get_temp(open_id).unwrap_or(false));
    let mut is_open = false;
    let mut closing = false;
    let mut changed = false;

    egui::ComboBox::from_id_salt(id)
        .selected_text(actual)
        .width(ui.available_width().min(280.0))
        .height(320.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            is_open = true;
            // Cada vez que se abre o popup (transición pechado→aberto) reiniciamos
            // a busca: así o listado amósase completo e non filtrado pola última
            // busca. Faise antes de pintar o buscador e de filtrar a lista.
            if !was_open {
                filtro.clear();
            }

            if values.len() > 12 {
                let resp = text_input(ui, filtro);
                // Foco no buscador ao abrir o popup.
                if !was_open {
                    resp.request_focus();
                }
            }

            // Altura ESTABLE do popup: calcúlase a partir do total de entradas,
            // non das visibles tras o filtro. Se dependese do filtro, ao reducir
            // a poucos resultados o popup encollería; e como o `Area` do ComboBox
            // queda fixado co tamaño do frame anterior e o seu ScrollArea limítase
            // ao espazo dispoñible (`available.at_most(max_height)`), ese tamaño
            // pequeno perpetúase: ao reabrir (ou ao limpar o filtro) o listado
            // completo amosaríase nun oco dunha soa fila. Cunha altura constante
            // o popup nunca se contrae e o problema desaparece.
            let row_h = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
            let filas = (values.len() + 1).min(10); // +1 pola opción "(todos)"
            ui.set_min_height(filas as f32 * row_h);

            let f = crate::model::normalize_search(filtro);
            if ui
                .selectable_label(selected.is_empty(), "(todos)")
                .clicked()
            {
                selected.clear();
                changed = true;
                closing = true;
                ui.close();
            }
            for v in values {
                if !f.is_empty() && !crate::model::normalize_search(v).contains(&f) {
                    continue;
                }
                if ui.selectable_label(selected == v, v).clicked() {
                    *selected = v.clone();
                    changed = true;
                    closing = true;
                    ui.close();
                }
            }
        });

    // Se pechamos por selección, gardamos estado "pechado" para que a próxima
    // apertura se detecte como transición e se volva pedir o foco do buscador.
    ui.data_mut(|d| d.insert_temp(open_id, is_open && !closing));
    changed
}

/// ComboBox que selecciona un código a partir dunha lista `(código, etiqueta)`.
/// Inclúe unha opción baleira "(todos)". `filtro` permite filtrar listas longas.
fn combo_codigo(
    ui: &mut egui::Ui,
    id: &str,
    selected: &mut String,
    options: &[(String, String)],
    filtro: &mut String,
    permitir_todos: bool,
) {
    let placeholder = if permitir_todos {
        "(todos)"
    } else {
        "(escolle…)"
    };
    let actual = options
        .iter()
        .find(|(c, _)| c == selected)
        .map(|(_, l)| l.as_str())
        .unwrap_or(placeholder);

    // Flag en memoria para saber se o popup xa estaba aberto no frame anterior,
    // e así enfocar o campo de busca só no momento de abrilo.
    let open_id = egui::Id::new(("combo_open", id));
    let was_open: bool = ui.data(|d| d.get_temp(open_id).unwrap_or(false));
    let mut is_open = false;
    let mut closing = false;

    egui::ComboBox::from_id_salt(id)
        .selected_text(actual)
        .width(ui.available_width().min(280.0))
        // Altura do popup: o ComboBox xa envolve o contido nun ScrollArea propio,
        // que encolle se hai poucos resultados e fai scroll se hai moitos.
        .height(320.0)
        // Por defecto o popup péchase ao premer dentro (incluído o campo de busca);
        // mantémolo aberto e pechámolo manualmente ao seleccionar unha opción.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            is_open = true;
            if options.len() > 12 {
                let resp = text_input(ui, filtro);
                // Ao abrir o popup, dirixir o teclado ao campo de busca.
                if !was_open {
                    resp.request_focus();
                }
            }
            // Altura estable a partir do total de opcións (ver nota en `combo_valor`):
            // evita que o popup quede fixado nun tamaño pequeno tras filtrar.
            let row_h = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
            let extra = if permitir_todos { 1 } else { 0 }; // +1 pola opción "(todos)"
            let filas = (options.len() + extra).min(10);
            ui.set_min_height(filas as f32 * row_h);
            // Ao seleccionar, limpamos o texto de busca e pechamos o popup. A opción
            // "(todos)" só se ofrece cando se permiten filtros baleiros.
            if permitir_todos
                && ui
                    .selectable_label(selected.is_empty(), "(todos)")
                    .clicked()
            {
                selected.clear();
                filtro.clear();
                closing = true;
                ui.close();
            }
            let f = crate::model::normalize_search(filtro);
            for (code, label) in options {
                if !f.is_empty() && !crate::model::normalize_search(label).contains(&f) {
                    continue;
                }
                if ui.selectable_label(selected == code, label).clicked() {
                    *selected = code.clone();
                    filtro.clear();
                    closing = true;
                    ui.close();
                }
            }
        });

    // Tras pechar por selección, gardamos "pechado" para que a próxima apertura
    // se detecte como transición e se volva pedir o foco do buscador.
    ui.data_mut(|d| d.insert_temp(open_id, is_open && !closing));
}
