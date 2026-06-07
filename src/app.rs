//! Interface gráfica (egui): listado de contratos importados como pantalla
//! principal, e importación de datos en diálogos modais.

use crate::db::{Db, DbStats, LocalOptions};
use crate::model::{
    CargoRow, ContractDetail, DatosCifEntidade, EstadoGroup, FilterOptions, Filters, LocalFilters,
    GrupoRelacion, LocalRow, Resolucion,
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

    /// Pestana activa da vista principal.
    tab: Tab,
    /// Vista de relacións: grupos (tramas) de razóns sociais interconectadas.
    relacions: Vec<GrupoRelacion>,
    relacions_loaded: bool,
}

/// Pestanas da vista principal. As dúas comparten os filtros do panel lateral.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// Listado de contratos (e o detalle dun contrato seleccionado).
    Contratos,
    /// Tramas de razóns sociais relacionadas entre si.
    Relacions,
}

/// Datos de datoscif asociados a un adxudicatario dun contrato.
struct AdxDatosCif {
    adx_nome: String,
    entidade: DatosCifEntidade,
    cargos: Vec<CargoRow>,
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
            tab: Tab::Contratos,
            relacions: Vec::new(),
            relacions_loaded: false,
        };
        app.refresh_local();
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
                    self.status = format!(
                        "Vinculación rematada · {} procesados · {} vinculados · {} sen match · {} empresas con cargos · {} cargos · {} erros",
                        r.procesados, r.vinculados, r.sen_match, r.empresas_con_cargos, r.cargos, r.erros
                    );
                    // Os vínculos cambiaron: invalidar a vista de relacións e o
                    // detalle aberto, e refrescar a táboa.
                    self.relacions_loaded = false;
                    self.need_query = true;
                    if let Some(row) = self.selected_row.clone() {
                        self.select_contract(row);
                    }
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
        if self.need_query {
            self.refresh_local();
            self.need_query = false;
        }
        if self.busy {
            ctx.request_repaint();
        }
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
        self.selected = Some(row.id.clone());
        self.selected_row = Some(row);
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
    }

    fn refresh_relacions(&mut self) {
        self.relacions = self.db.relacions_compartidas(&self.local).unwrap_or_default();
        self.relacions_loaded = true;
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
                            "Importar datos",
                            Color32::WHITE,
                        ))
                        .fill(theme::accent(self.dark)),
                    );
                    if importar.clicked() {
                        self.show_import_dialog = true;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let icon = if self.dark { "☀ Claro" } else { "🌙 Escuro" };
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
            ui.set_width(720.0);
            ui.heading("Importar datos");
            ui.label(
                RichText::new(
                    "Escolle os filtros dos contratos a descargar. Os contratos xa resoltos \
                     non se volven descargar; só se actualizan os que seguían en proceso e os novos.",
                )
                .small()
                .color(Color32::GRAY),
            );
            ui.add_space(10.0);

            ui.columns(2, |cols| {
                // Columna esquerda: estado, ano e busca textual.
                let ui = &mut cols[0];
                ui.label(RichText::new("Estado").strong());
                for g in EstadoGroup::ALL {
                    let mut on = self.filters.estados.contains(&g);
                    if ui.checkbox(&mut on, g.label()).changed() {
                        if on {
                            self.filters.estados.push(g);
                        } else {
                            self.filters.estados.retain(|x| *x != g);
                        }
                    }
                }
                ui.add_space(8.0);

                ui.label(RichText::new("Ano").strong());
                year_combo(ui, &mut self.filters.year);
                ui.add_space(8.0);

                ui.label(RichText::new("Busca textual (obxecto)").strong());
                text_input(ui, &mut self.filters.asunto);
                ui.add_space(8.0);

                ui.label(RichText::new("Órgano de contratación").strong());
                combo_codigo(
                    ui,
                    "organo",
                    &mut self.filters.organo,
                    &self.options.organos,
                    self.combo_filtros.entry("organo".into()).or_default(),
                );

                // Columna dereita: clasificacións do contrato.
                let ui = &mut cols[1];
                ui.label(RichText::new("Tipo de contrato").strong());
                combo_codigo(ui, "tc", &mut self.filters.tipo_contrato, &self.options.tipos_contrato, self.combo_filtros.entry("tc".into()).or_default());
                ui.add_space(6.0);
                ui.label(RichText::new("Tipo de procedemento").strong());
                combo_codigo(ui, "tp", &mut self.filters.tipo_procedemento, &self.options.tipos_procedemento, self.combo_filtros.entry("tp".into()).or_default());
                ui.add_space(6.0);
                ui.label(RichText::new("Tipo de tramitación").strong());
                combo_codigo(ui, "tt", &mut self.filters.tipo_tramitacion, &self.options.tipos_tramitacion, self.combo_filtros.entry("tt".into()).or_default());
                ui.add_space(6.0);
                ui.label(RichText::new("Sistema de contratación").strong());
                combo_codigo(ui, "sc", &mut self.filters.sistema, &self.options.sistemas, self.combo_filtros.entry("sc".into()).or_default());
                ui.add_space(6.0);
                ui.label(RichText::new("Materia (CPV)").strong());
                combo_codigo(ui, "cpv", &mut self.filters.materia, &self.options.materias, self.combo_filtros.entry("cpv".into()).or_default());
            });

            if !self.options_loaded {
                ui.add_space(6.0);
                ui.label(RichText::new("Cargando opcións de filtro…").small().italics());
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let importar = ui.add_enabled(
                    !self.busy,
                    egui::Button::new(RichText::new("Importar").color(Color32::WHITE))
                        .fill(theme::accent(self.dark)),
                );
                if importar.clicked() {
                    self.busy = true;
                    self.progress = None;
                    self.progress_titulo = "Importación de contratos";
                    self.logs.clear();
                    self.status = "Iniciando importación…".into();
                    self.worker.send(Command::Sync(self.filters.clone()));
                    self.show_import_dialog = false;
                    self.show_progress_dialog = true;
                }
                if ui.button("Limpar filtros").clicked() {
                    self.filters = Filters::default();
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
                                egui::Button::new(
                                    RichText::new("Pechar").color(Color32::WHITE),
                                )
                                .fill(theme::accent(self.dark)),
                            )
                            .clicked()
                        {
                            self.show_progress_dialog = false;
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
                    !self.busy && !self.rows.is_empty(),
                    egui::Button::new(RichText::new("Exportar a ODS").color(Color32::WHITE))
                        .fill(theme::accent(self.dark)),
                );
                if export.clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("OpenDocument Spreadsheet", &["ods"])
                        .set_file_name("contratos.ods")
                        .save_file()
                    {
                        self.busy = true;
                        self.status = "Exportando…".into();
                        self.worker.send(Command::Export {
                            path,
                            filters: self.local.clone(),
                        });
                    }
                }
                ui.label(
                    RichText::new(format!("{} filas no resultado", self.rows.len()))
                        .small()
                        .color(Color32::GRAY),
                );

                ui.add_space(10.0);
                ui.separator();
                ui.label(RichText::new("Empresas").strong());
                let vincular = ui.add_enabled(
                    !self.busy,
                    egui::Button::new("Vincular con datoscif"),
                );
                if vincular.clicked() {
                    self.busy = true;
                    self.progress = None;
                    self.progress_titulo = "Vinculación con datoscif (relacións)";
                    self.logs.clear();
                    self.status = "Iniciando vinculación con datoscif…".into();
                    self.worker.send(Command::Enrich);
                    self.show_progress_dialog = true;
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
            }
        });
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        let mut clicked: Option<LocalRow> = None;
        let selected = self.selected.clone();
        // Texto non seleccionable nas celas: así o cursor non entra en modo
        // inserción de texto e o clic chega á fila enteira (sense ::click).
        ui.style_mut().interaction.selectable_labels = false;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(70.0).at_least(56.0))
            .column(Column::initial(90.0))
            // Columnas de texto: reparten o espazo sobrante a partes iguais, así ao
            // agrandar a ventá medran as tres e amosan máis información. O `at_least`
            // fixa a proporción mínima (Obxecto algo máis ancho que as outras).
            //
            // Teñen que ser `.resizable(false)`: en egui_extras unha columna `remainder`
            // só segue ocupando o espazo sobrante en cada fotograma se NON é redimensionable;
            // se o é, queda fixada co ancho do primeiro fotograma e non medra ao agrandar a ventá.
            .column(Column::remainder().at_least(200.0).clip(true).resizable(false))
            .column(Column::initial(110.0))
            .column(Column::initial(130.0))
            .column(Column::remainder().at_least(150.0).clip(true).resizable(false))
            .column(Column::remainder().at_least(150.0).clip(true).resizable(false))
            .column(Column::initial(120.0))
            .header(24.0, |mut h| {
                for t in [
                    "ID", "Data", "Obxecto", "Importe", "Estado", "Organismo",
                    "Adxudicatario", "Imp. resolución",
                ] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|mut body| {
                for r in &self.rows {
                    let is_sel = selected.as_deref() == Some(r.id.as_str());
                    body.row(22.0, |mut row| {
                        row.set_selected(is_sel);
                        row.col(|ui| {
                            ui.label(&r.id);
                        });
                        row.col(|ui| {
                            ui.label(&r.publicacion);
                        });
                        row.col(|ui| {
                            ui.label(&r.asunto);
                        });
                        row.col(|ui| {
                            ui.label(&r.importe_txt);
                        });
                        row.col(|ui| {
                            ui.label(&r.estado);
                        });
                        row.col(|ui| {
                            ui.label(&r.organismo);
                        });
                        row.col(|ui| {
                            ui.label(&r.adxudicatario);
                        });
                        row.col(|ui| {
                            ui.label(&r.importe_resolucion_txt);
                        });
                        let resp = row.response();
                        resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            clicked = Some(r.clone());
                        }
                    });
                }
            });
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
                 persoas dunha mesma trama.",
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

        ScrollArea::vertical().show(ui, |ui| {
            for g in &self.relacions {
                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    // Persoas que conectan o grupo.
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Persoas:").small().color(Color32::GRAY));
                        for p in &g.persoas {
                            ui.hyperlink_to(
                                RichText::new(&p.persona_nome).strong(),
                                format!("{DATOSCIF_BASE}/directivo/{}", p.persona_url),
                            );
                            ui.label(
                                RichText::new(format!("({} empresas)", p.num_empresas))
                                    .small()
                                    .color(Color32::GRAY),
                            );
                        }
                    });
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
                        });
                    }
                });
            }
        });
    }
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
                    for c in &a.cargos {
                        ui.horizontal_wrapped(|ui| {
                            let punto = if c.activo { "●" } else { "○" };
                            ui.label(RichText::new(punto).small());
                            ui.label(RichText::new(&c.persona_nome).strong().small());
                            if !c.cargo.is_empty() {
                                ui.label(RichText::new(format!("— {}", c.cargo)).small());
                            }
                        });
                    }
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
            if ui.selectable_label(selected.is_empty(), "(todos)").clicked() {
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
            if ui.selectable_label(selected.is_empty(), "(todos)").clicked() {
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
) {
    let actual = options
        .iter()
        .find(|(c, _)| c == selected)
        .map(|(_, l)| l.as_str())
        .unwrap_or("(todos)");

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
            let filas = (options.len() + 1).min(10); // +1 pola opción "(todos)"
            ui.set_min_height(filas as f32 * row_h);
            // Ao seleccionar, limpamos o texto de busca e pechamos o popup.
            if ui.selectable_label(selected.is_empty(), "(todos)").clicked() {
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
