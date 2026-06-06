//! Interface gráfica (egui): listado de contratos importados como pantalla
//! principal, e importación de datos en diálogos modais.

use crate::db::{Db, DbStats, LocalOptions};
use crate::model::{
    ContractDetail, EstadoGroup, FilterOptions, Filters, LocalFilters, LocalRow, Resolucion,
};
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
    status: String,
    logs: Vec<String>,
    stats: DbStats,

    local: LocalFilters,
    local_options: LocalOptions,
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
            status: "Listo.".to_string(),
            logs: Vec::new(),
            stats,
            local: LocalFilters::default(),
            local_options,
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
                    self.local_options = self.db.local_options().unwrap_or_default();
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
        self.main_view(ui);
        if self.show_import_dialog {
            self.import_dialog(&ctx);
        }
        if self.show_progress_dialog {
            self.progress_dialog(&ctx);
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
            ui.heading("Importación de datos");
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

/// Campo de texto dunha liña con padding interior e ancho completo.
fn text_input(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .margin(egui::Margin::symmetric(8, 6))
            .desired_width(f32::INFINITY),
    )
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
