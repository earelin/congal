//! Interface gráfica (egui) con dúas áreas: scraper e traballo local.

use crate::db::{Db, DbStats};
use crate::model::{
    ContractDetail, EstadoGroup, FilterOptions, Filters, LocalFilters, LocalRow, Resolucion,
};
use crate::theme;
use crate::worker::{Command, Event, Worker};
use egui::{Align, Color32, Layout, RichText, ScrollArea};
use egui_extras::{Column, TableBuilder};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Scraper,
    Local,
}

pub struct App {
    worker: Worker,
    db: Db,
    dark: bool,
    tab: Tab,

    options: FilterOptions,
    options_loaded: bool,
    organo_filtro: String,

    filters: Filters,

    busy: bool,
    progress: Option<(usize, usize)>,
    status: String,
    logs: Vec<String>,
    stats: DbStats,

    local: LocalFilters,
    rows: Vec<LocalRow>,
    need_query: bool,
    selected: Option<String>,
    selected_detail: Option<(ContractDetail, Vec<Resolucion>)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let dark = crate::system_dark();
        theme::apply(&cc.egui_ctx, dark);

        let db_path = crate::db_path();
        let db = Db::open(&db_path).expect("non se puido abrir a base de datos");
        let stats = db.stats().unwrap_or_default();

        let worker = Worker::spawn(cc.egui_ctx.clone(), db_path);
        worker.send(Command::LoadOptions);

        let mut app = App {
            worker,
            db,
            dark,
            tab: Tab::Scraper,
            options: FilterOptions::default(),
            options_loaded: false,
            organo_filtro: String::new(),
            filters: Filters::default(),
            busy: false,
            progress: None,
            status: "Listo.".to_string(),
            logs: Vec::new(),
            stats,
            local: LocalFilters::default(),
            rows: Vec::new(),
            need_query: true,
            selected: None,
            selected_detail: None,
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
                    self.need_query = true;
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

    fn select_contract(&mut self, id: String) {
        self.selected_detail = self.db.load_detail(&id).ok().flatten();
        self.selected = Some(id);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.top_bar(ui);
        self.status_bar(ui);
        match self.tab {
            Tab::Scraper => self.scraper_tab(ui),
            Tab::Local => self.local_tab(ui),
        }
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
                    ui.heading("Contratos Públicos de Galicia");
                    ui.add_space(16.0);

                    // Segmented control.
                    ui.scope(|ui| {
                        let seg = |ui: &mut egui::Ui, label: &str, tab: Tab, cur: &mut Tab| {
                            if ui.selectable_label(*cur == tab, RichText::new(label)).clicked() {
                                *cur = tab;
                            }
                        };
                        seg(ui, "  Scraper  ", Tab::Scraper, &mut self.tab);
                        seg(ui, "  Traballo local  ", Tab::Local, &mut self.tab);
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let icon = if self.dark { "☀ Claro" } else { "🌙 Escuro" };
                        if ui.button(icon).clicked() {
                            self.dark = !self.dark;
                            theme::apply(ui.ctx(), self.dark);
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "BD: {} contratos · {} con detalle",
                                self.stats.total, self.stats.con_detalle
                            ))
                            .small()
                            .color(Color32::GRAY),
                        );
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("statusbar")
            .exact_size(30.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.busy {
                        ui.spinner();
                    }
                    ui.label(RichText::new(&self.status).small());
                    if let Some((done, total)) = self.progress {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                                    .desired_width(220.0)
                                    .text(format!("{done}/{total}")),
                            );
                        });
                    }
                });
            });
    }

    fn scraper_tab(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("filtros")
            .resizable(true)
            .default_size(320.0)
            .show_inside(ui, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(6.0);
                    ui.heading("Filtros");
                    ui.add_space(6.0);

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
                    ui.text_edit_singleline(&mut self.filters.year);
                    ui.add_space(8.0);

                    ui.label(RichText::new("Busca textual (obxecto)").strong());
                    ui.text_edit_singleline(&mut self.filters.asunto);
                    ui.add_space(8.0);

                    ui.label(RichText::new("Órgano de contratación").strong());
                    combo_codigo(
                        ui,
                        "organo",
                        &mut self.filters.organo,
                        &self.options.organos,
                        &mut self.organo_filtro,
                    );
                    ui.add_space(8.0);

                    ui.label(RichText::new("Tipo de contrato").strong());
                    combo_codigo(ui, "tc", &mut self.filters.tipo_contrato, &self.options.tipos_contrato, &mut String::new());
                    ui.add_space(4.0);
                    ui.label(RichText::new("Tipo de procedemento").strong());
                    combo_codigo(ui, "tp", &mut self.filters.tipo_procedemento, &self.options.tipos_procedemento, &mut String::new());
                    ui.add_space(4.0);
                    ui.label(RichText::new("Tipo de tramitación").strong());
                    combo_codigo(ui, "tt", &mut self.filters.tipo_tramitacion, &self.options.tipos_tramitacion, &mut String::new());
                    ui.add_space(4.0);
                    ui.label(RichText::new("Sistema de contratación").strong());
                    combo_codigo(ui, "sc", &mut self.filters.sistema, &self.options.sistemas, &mut String::new());
                    ui.add_space(4.0);
                    ui.label(RichText::new("Materia (CPV)").strong());
                    combo_codigo(ui, "cpv", &mut self.filters.materia, &self.options.materias, &mut String::new());

                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let sync = ui.add_enabled(
                            !self.busy,
                            egui::Button::new(RichText::new("Sincronizar").color(Color32::WHITE))
                                .fill(theme::accent(self.dark)),
                        );
                        if sync.clicked() {
                            self.busy = true;
                            self.status = "Iniciando sincronización…".into();
                            self.worker.send(Command::Sync(self.filters.clone()));
                        }
                        if self.busy && ui.button("Cancelar").clicked() {
                            self.worker.request_cancel();
                        }
                        if ui.button("Limpar").clicked() {
                            self.filters = Filters::default();
                        }
                    });

                    if !self.options_loaded {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Cargando opcións de filtro…").small().italics());
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(6.0);
            ui.heading("Sincronización");
            ui.label(
                "Descarga e actualiza contratos. Os contratos xa resoltos non se volven \
                 descargar; só se actualizan os que seguían en proceso e os novos.",
            );
            ui.add_space(8.0);
            if let Some(u) = &self.stats.ultima_sync {
                ui.label(format!("Última sincronización: {u}"));
            }
            ui.separator();
            ui.label(RichText::new("Rexistro").strong());
            ScrollArea::vertical().show(ui, |ui| {
                for l in self.logs.iter().rev().take(200) {
                    ui.label(RichText::new(l).small().monospace());
                }
            });
        });
    }

    fn local_tab(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("filtros_local")
            .resizable(true)
            .default_size(300.0)
            .show_inside(ui, |ui| {
                ui.add_space(6.0);
                ui.heading("Busca local");
                ui.add_space(6.0);
                let mut changed = false;
                ui.label(RichText::new("Texto (obxecto/referencia)").strong());
                changed |= ui.text_edit_singleline(&mut self.local.texto).changed();
                ui.add_space(6.0);
                ui.label(RichText::new("Adxudicatario").strong());
                changed |= ui.text_edit_singleline(&mut self.local.adxudicatario).changed();
                ui.add_space(6.0);
                ui.label(RichText::new("Organismo").strong());
                changed |= ui.text_edit_singleline(&mut self.local.organismo).changed();
                ui.add_space(6.0);
                ui.label(RichText::new("Estado").strong());
                changed |= ui.text_edit_singleline(&mut self.local.estado).changed();
                ui.add_space(6.0);
                ui.label(RichText::new("Ano").strong());
                changed |= ui.text_edit_singleline(&mut self.local.year).changed();

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Buscar").clicked() {
                        changed = true;
                    }
                    if ui.button("Limpar").clicked() {
                        self.local = LocalFilters::default();
                        changed = true;
                    }
                });
                if changed {
                    self.need_query = true;
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
            });

        if self.selected.is_some() {
            self.detail_panel(ui);
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.results_table(ui);
        });
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        let mut clicked: Option<String> = None;
        let selected = self.selected.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(70.0).at_least(56.0))
            .column(Column::initial(90.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::initial(110.0))
            .column(Column::initial(130.0))
            .column(Column::initial(180.0).clip(true))
            .column(Column::initial(180.0).clip(true))
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
                        if row.response().clicked() {
                            clicked = Some(r.id.clone());
                        }
                    });
                }
            });
        if let Some(id) = clicked {
            self.select_contract(id);
        }
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("detalle")
            .resizable(true)
            .default_size(380.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Detalle");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("✕").clicked() {
                            self.selected = None;
                            self.selected_detail = None;
                        }
                    });
                });
                ui.separator();
                let Some((d, resolucions)) = &self.selected_detail else {
                    ui.label("Sen detalle descargado para este contrato.");
                    return;
                };
                ScrollArea::vertical().show(ui, |ui| {
                    field(ui, "ID", &d.contract_id);
                    field(ui, "Referencia", &d.referencia);
                    field(ui, "Obxecto", &d.obxecto);
                    field(ui, "Tipo de contrato", &d.tipo_contrato);
                    field(ui, "Tipo de procedemento", &d.tipo_procedemento);
                    field(ui, "Tipo de tramitación", &d.tipo_tramitacion);
                    field(ui, "Orzamento base", &d.orzamento_base);
                    field(ui, "Valor estimado", &d.valor_estimado);
                    field(ui, "Nº lotes", &d.num_lotes);
                    field(ui, "Sistema de contratación", &d.sistema_contratacion);
                    field(ui, "Data de difusión", &d.data_difusion);
                    field(ui, "Observacións", &d.observacions);

                    if !d.enlace_resolucion.is_empty() {
                        ui.add_space(4.0);
                        ui.hyperlink_to("🔗 Abrir resolución na web", &d.enlace_resolucion);
                    }

                    if !resolucions.is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new("Resolucións / adxudicacións").strong());
                        for (i, r) in resolucions.iter().enumerate() {
                            ui.add_space(4.0);
                            ui.group(|ui| {
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
                                let _ = i;
                            });
                        }
                    }

                    if !d.extra.is_empty() {
                        ui.add_space(10.0);
                        ui.collapsing("Outros campos", |ui| {
                            for (k, v) in &d.extra {
                                field(ui, k, v);
                            }
                        });
                    }
                });
            });
    }
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

    egui::ComboBox::from_id_salt(id)
        .selected_text(actual)
        .width(ui.available_width().min(280.0))
        .show_ui(ui, |ui| {
            if options.len() > 12 {
                ui.text_edit_singleline(filtro);
            }
            if ui.selectable_label(selected.is_empty(), "(todos)").clicked() {
                selected.clear();
            }
            let f = filtro.to_lowercase();
            ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (code, label) in options {
                    if !f.is_empty() && !label.to_lowercase().contains(&f) {
                        continue;
                    }
                    if ui
                        .selectable_label(selected == code, label)
                        .clicked()
                    {
                        *selected = code.clone();
                    }
                }
            });
        });
}
