//! Interface gráfica (egui): listado de contratos importados como pantalla
//! principal, e importación de datos en diálogos modais.

use crate::db::{Db, DbStats, LocalOptions};
use crate::model::{
    ContractDetail, EmpresaContrato, EmpresaContratos, EmpresaDetalle, EmpresaSortColumn,
    EmpresaUteMembro, FilterOptions, GrupoRelacion, ImportParams, LocalFilters, LocalRow,
    Resolucion, SortColumn, TipoContrato, format_importe,
};
use crate::theme;
use crate::worker::{Command, Event, Worker};
use egui::{Align, Color32, Layout, RichText, ScrollArea};
use egui_extras::{Column, TableBuilder};

/// Tamaño de chunco da carga progresiva da táboa local (filas por consulta).
const CHUNK: usize = 8000;

pub struct App {
    worker: Worker,
    db: Db,
    dark: bool,

    options: FilterOptions,
    options_loaded: bool,
    /// Texto de busca de cada ComboBox, indexado polo seu id.
    combo_filtros: std::collections::HashMap<String, String>,

    /// Parámetros de importación (formulario do modal: organismo + ano).
    import: ImportParams,
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
    /// Nº total de filas que casan cos filtros actuais (para a táboa virtual).
    total_rows: usize,
    /// Caché de filas cargadas por chunco (clave = índice de chunco = fila/`CHUNK`).
    /// A táboa virtual carga progresivamente só os chuncos que entran en pantalla.
    row_cache: std::collections::HashMap<usize, Vec<LocalRow>>,
    need_query: bool,
    /// Id do contrato seleccionado (resáltase na táboa e ábrese o seu diálogo).
    selected: Option<String>,
    /// Fila de resumo do contrato seleccionado (info do listado).
    selected_row: Option<LocalRow>,
    /// Detalle descargado do contrato seleccionado, se o hai.
    selected_detail: Option<(ContractDetail, Vec<Resolucion>)>,
    /// Composición das UTE adxudicatarias do contrato aberto.
    selected_utes: Vec<UteDetalle>,

    /// Pestana activa da vista principal.
    tab: Tab,
    /// Sub-pestana activa na vista de contratos: licitacións ou contratos menores.
    sub_tab: TipoContrato,
    /// Vista de relacións: grupos (tramas) de razóns sociais interconectadas.
    relacions: Vec<GrupoRelacion>,
    relacions_loaded: bool,
    /// Vista de empresas: nº total de empresas que casan cos filtros actuais
    /// (tamaño da táboa virtual e do resumo).
    empresas_total: usize,
    /// Importe total adxudicado a todas as empresas filtradas (para o resumo);
    /// vén de `db::empresas_resumo`, non de sumar as filas cargadas en memoria.
    empresas_importe_total: f64,
    /// Caché de empresas cargadas por chunco (clave = índice de chunco = fila/`CHUNK`).
    /// A táboa de empresas carga progresivamente, igual ca a de contratos.
    empresas_cache: std::collections::HashMap<usize, Vec<EmpresaContratos>>,
    empresas_loaded: bool,
    /// Empresa seleccionada na pestana Empresas: a súa ficha (contratos directos
    /// separados por tipo + UTE nas que participa). `None` = amosar o listado.
    selected_empresa: Option<EmpresaDetalle>,

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
    /// Listado de empresas adxudicatarias coa súa contratación agregada.
    Empresas,
    /// Tramas de razóns sociais relacionadas entre si.
    Relacions,
}

/// Unha empresa membro dunha UTE para a ficha do contrato.
struct UteMembroVista {
    nome: String,
    cif: String,
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
        // Ao virtualizar a táboa (`body.rows`), egui_extras pode rexistrar
        // transitoriamente o mesmo id de cela en dous rectángulos nun fotograma
        // (durante as pasadas de medición), o que dispara o aviso visual de
        // colisión de ids de egui: un flash de rectángulos vermellos na táboa.
        // É inocuo (non afecta á selección nin aos clics); desactivamos ese aviso
        // de depuración para que non se vexa.
        cc.egui_ctx.options_mut(|o| o.warn_on_id_clash = false);

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
            import: ImportParams {
                ano: crate::model::current_year().to_string(),
                ..ImportParams::default()
            },
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
            total_rows: 0,
            row_cache: std::collections::HashMap::new(),
            need_query: true,
            selected: None,
            selected_row: None,
            selected_detail: None,
            selected_utes: Vec::new(),
            tab: Tab::Contratos,
            sub_tab: TipoContrato::Licitacion,
            relacions: Vec::new(),
            relacions_loaded: false,
            empresas_total: 0,
            empresas_importe_total: 0.0,
            empresas_cache: std::collections::HashMap::new(),
            empresas_loaded: false,
            selected_empresa: None,
            export_rx: None,
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
                        "Importación rematada · {} licitacións ({} novas · {} actualizadas · {} saltadas · {} detalles) · {} contratos menores · {} erros",
                        r.licitacions,
                        r.novos,
                        r.actualizados,
                        r.saltados,
                        r.detalles_descargados,
                        r.menores,
                        r.erros
                    );
                    self.stats = self.db.stats().unwrap_or_default();
                    self.local_options = self.db.local_options().unwrap_or_default();
                    self.need_query = true;
                    self.relacions_loaded = false;
                    self.empresas_loaded = false;
                    self.selected_empresa = None;
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
        if self.busy {
            ctx.request_repaint();
        }
    }

    /// Lanza a exportación a ODS no fío traballador e amosa o diálogo de progreso.
    fn start_export(&mut self, path: std::path::PathBuf) {
        self.busy = true;
        self.progress = None;
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
        // Carga progresiva: só contamos o total e baleiramos a caché de chuncos;
        // as filas visibles cárganse baixo demanda en `results_table`.
        self.row_cache.clear();
        match self.db.count_local(&self.local) {
            Ok(n) => self.total_rows = n,
            Err(e) => {
                self.total_rows = 0;
                self.status = format!("⚠ consulta local: {e}");
            }
        }
    }

    /// Garante que o chunco que contén `index` está na caché; cárgao da BD se non.
    fn ensure_chunk_loaded(&mut self, index: usize) {
        let chunk = index / CHUNK;
        if self.row_cache.contains_key(&chunk) {
            return;
        }
        match self.db.query_local_page(&self.local, chunk * CHUNK, CHUNK) {
            Ok(rows) => {
                self.row_cache.insert(chunk, rows);
            }
            Err(e) => {
                // Marcar o chunco como (baleiro) para non reintentar en bucle.
                self.row_cache.insert(chunk, Vec::new());
                self.status = format!("⚠ consulta local: {e}");
            }
        }
    }

    fn select_contract(&mut self, row: LocalRow) {
        self.selected_detail = self.db.load_detail(&row.id).ok().flatten();
        self.selected_utes = self.build_utes_info(&row.id);
        self.selected = Some(row.id.clone());
        self.selected_row = Some(row);
    }

    /// Composición das UTE adxudicatarias do contrato (nome e CIF de cada membro),
    /// tal como vén do propio contrato.
    fn build_utes_info(&self, contract_id: &str) -> Vec<UteDetalle> {
        let mut out: Vec<UteDetalle> = Vec::new();
        for (ute_nome, nome, cif) in self
            .db
            .ute_membros_de_contrato(contract_id)
            .unwrap_or_default()
        {
            let membro = UteMembroVista { nome, cif };
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

    fn close_detail(&mut self) {
        self.selected = None;
        self.selected_row = None;
        self.selected_detail = None;
        self.selected_utes.clear();
    }

    fn refresh_relacions(&mut self) {
        self.relacions = self
            .db
            .relacions_compartidas(&self.local)
            .unwrap_or_default();
        self.relacions_loaded = true;
    }

    fn refresh_empresas(&mut self) {
        // Carga progresiva: baleiramos a caché de chuncos e só calculamos o resumo
        // (nº de empresas e importe total); as filas visibles cárganse baixo demanda
        // en `empresas_view`.
        self.empresas_cache.clear();
        match self.db.empresas_resumo(&self.local) {
            Ok((n, total)) => {
                self.empresas_total = n;
                self.empresas_importe_total = total;
            }
            Err(e) => {
                self.empresas_total = 0;
                self.empresas_importe_total = 0.0;
                self.status = format!("⚠ consulta empresas: {e}");
            }
        }
        self.empresas_loaded = true;
    }

    /// Garante que o chunco de empresas que contén `index` está na caché; cárgao da
    /// BD se non. Análogo a [`Self::ensure_chunk_loaded`] pero para a táboa de empresas.
    fn ensure_empresas_chunk_loaded(&mut self, index: usize) {
        let chunk = index / CHUNK;
        if self.empresas_cache.contains_key(&chunk) {
            return;
        }
        match self
            .db
            .query_empresas_page(&self.local, chunk * CHUNK, CHUNK)
        {
            Ok(rows) => {
                self.empresas_cache.insert(chunk, rows);
            }
            Err(e) => {
                // Marcar o chunco como (baleiro) para non reintentar en bucle.
                self.empresas_cache.insert(chunk, Vec::new());
                self.status = format!("⚠ consulta empresas: {e}");
            }
        }
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
            ui.heading("Importar contratos dun organismo");
            ui.label(
                RichText::new(
                    "Escolle o organismo (obrigatorio). Báixanse TODAS as súas licitacións \
                     (as resoltas non se volven descargar; só as novas e as que seguían en \
                     proceso) e os CONTRATOS MENORES do ano indicado. Cada importación engádese \
                     ao teu conxunto local; podes importar varios organismos.",
                )
                .small()
                .color(Color32::GRAY),
            );
            ui.add_space(12.0);

            ui.label(RichText::new("Organismo").strong());
            combo_codigo(
                ui,
                "organo",
                &mut self.import.org_id,
                &self.options.organos,
                self.combo_filtros.entry("organo".into()).or_default(),
                false,
            );
            ui.add_space(10.0);

            ui.label(RichText::new("Ano (contratos menores)").strong());
            year_combo(ui, &mut self.import.ano);

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
                // O organismo é obrigatorio para evitar importacións masivas.
                let pode_importar = !self.busy && !self.import.org_id.trim().is_empty();
                let importar = ui.add_enabled(
                    pode_importar,
                    egui::Button::new(RichText::new("Importar").color(Color32::WHITE))
                        .fill(theme::accent(self.dark)),
                );
                if importar.clicked() {
                    // Resolver o nome do organismo a partir do código escollido.
                    self.import.org_nome = self
                        .options
                        .organos
                        .iter()
                        .find(|(c, _)| c == &self.import.org_id)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_default();
                    self.busy = true;
                    self.progress = None;
                    self.progress_titulo = "Importación de contratos";
                    self.logs.clear();
                    self.status = "Iniciando importación…".into();
                    self.worker.send(Command::Import(self.import.clone()));
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
                    // O cancelamento só se aplica entre peticións; mentres se agarda
                    // a que remate a que está en curso, deshabilitamos o botón e
                    // avisamos para que non pareza que a app quedou pillada.
                    let cancelando = self
                        .worker
                        .cancel
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if ui
                        .add_enabled(!cancelando, egui::Button::new("Cancelar"))
                        .clicked()
                    {
                        self.worker.request_cancel();
                        self.status = "Cancelando… agardando a que remate a \
                                       petición en curso."
                            .into();
                    }
                    if cancelando {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Cancelando…").italics());
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
                    // Conservar a sub-pestana activa ao limpar os filtros.
                    self.local = LocalFilters {
                        tipo: self.sub_tab,
                        ..LocalFilters::default()
                    };
                    for k in ["loc_adx", "loc_org", "loc_ano"] {
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
                    self.empresas_loaded = false;
                    self.selected_empresa = None;
                }

                ui.add_space(10.0);
                ui.separator();
                let export = ui.add_enabled(
                    !self.busy && self.total_rows > 0 && self.export_rx.is_none(),
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
                    RichText::new(format!("{} filas no resultado", self.total_rows))
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
                    .selectable_label(self.tab == Tab::Empresas, "🏢 Empresas")
                    .clicked()
                {
                    self.tab = Tab::Empresas;
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
                        self.contratos_subtabs(ui);
                        self.results_table(ui);
                    }
                }
                Tab::Empresas => {
                    if self.selected_empresa.is_some() {
                        self.empresa_detail_view(ui);
                    } else {
                        self.empresas_view(ui);
                    }
                }
                Tab::Relacions => self.relacions_view(ui),
            }
        });
    }

    /// Selector das dúas sub-pestanas (Licitacións / Contratos menores). Ao
    /// cambiar, fíxase o tipo do filtro e relánzase a consulta.
    fn contratos_subtabs(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for tipo in [TipoContrato::Licitacion, TipoContrato::Menor] {
                if ui
                    .selectable_label(self.sub_tab == tipo, tipo.etiqueta())
                    .clicked()
                    && self.sub_tab != tipo
                {
                    self.sub_tab = tipo;
                    self.local.tipo = tipo;
                    // A orde por Estado/Imp. resolución non aplica aos menores:
                    // ao trocar de sub-pestana volvemos á orde por data.
                    self.local.sort_col = SortColumn::Data;
                    self.local.sort_asc = SortColumn::Data.default_asc();
                    self.need_query = true;
                }
            }
        });
        ui.add_space(4.0);
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        let menor = self.sub_tab == TipoContrato::Menor;
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
        // Carga progresiva: `total_rows` é o total (da BD); `cache` ten os chuncos
        // xa cargados; `needed` recolle os que faltan polas filas visibles, para
        // pedilos despois de pintar (a táboa virtual só visita as filas á vista).
        let total_rows = self.total_rows;
        let cache = &self.row_cache;
        let mut needed: Vec<usize> = Vec::new();
        let out = TableBuilder::new(ui)
            // Id estable e propio (separa o estado —anchos de columna, scroll— do
            // doutras táboas e fixa a clave entre fotogramas).
            .id_salt("contratos_table")
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
                // As columnas 5 e 8 cambian segundo a sub-pestana: nas licitacións
                // amósanse Estado e Imp. resolución (ordenables); nos contratos
                // menores, NIF e Duración (non ordenables → `None`).
                let cols: [(&str, Option<SortColumn>); 8] = [
                    ("ID", Some(SortColumn::Id)),
                    ("Data", Some(SortColumn::Data)),
                    ("Obxecto", Some(SortColumn::Obxecto)),
                    ("Importe", Some(SortColumn::Importe)),
                    if menor {
                        ("NIF", None)
                    } else {
                        ("Estado", Some(SortColumn::Estado))
                    },
                    ("Organismo", Some(SortColumn::Organismo)),
                    ("Adxudicatario", Some(SortColumn::Adxudicatario)),
                    if menor {
                        ("Duración", None)
                    } else {
                        ("Imp. resolución", Some(SortColumn::ImporteResolucion))
                    },
                ];
                for (t, col) in cols {
                    let activa = col == Some(cur_col);
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
                            Some(
                                SortColumn::Obxecto
                                    | SortColumn::Organismo
                                    | SortColumn::Adxudicatario
                            )
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
                        // Só as columnas con `SortColumn` reaccionan ao clic.
                        if let Some(c) = col
                            && resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                        {
                            clicked_header = Some(c);
                        }
                    });
                }
            })
            .body(|body| {
                // Virtualización + carga progresiva: `rows` só constrúe as filas
                // visibles, e os datos cárganse por chuncos baixo demanda. Unha fila
                // aínda non cargada píntase como «…» e o seu chunco márcase para pedir.
                body.rows(22.0, total_rows, |mut row| {
                    let i = row.index();
                    let chunk = i / CHUNK;
                    let Some(r) = cache.get(&chunk).and_then(|rows| rows.get(i % CHUNK)) else {
                        if !needed.contains(&chunk) {
                            needed.push(chunk);
                        }
                        for _ in 0..8 {
                            row.col(|ui| {
                                pad_cela(ui);
                                ui.label(RichText::new("…").weak());
                            });
                        }
                        return;
                    };
                    let is_sel = selected.as_deref() == Some(r.id.as_str());
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
                    // Columna 5: Estado (licitacións) ou NIF (menores).
                    row.col(|ui| {
                        pad_cela(ui);
                        if menor {
                            ui.label(&r.nif);
                        } else {
                            ui.label(&r.estado);
                        }
                    });
                    row.col(|ui| {
                        pad_cela(ui);
                        ui.label(&r.organismo);
                    });
                    row.col(|ui| {
                        pad_cela(ui);
                        ui.label(&r.adxudicatario);
                    });
                    // Columna 8: Imp. resolución (licitacións) ou Duración (menores).
                    row.col(|ui| {
                        pad_cela(ui);
                        if menor {
                            ui.label(&r.duracion);
                        } else {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(&r.importe_resolucion_txt);
                            });
                        }
                    });
                    let resp = row.response();
                    resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        clicked = Some(r.clone());
                    }
                });
            });
        // Liñas separadoras das columnas `remainder` non redimensionables: de alto
        // completo (da cabeceira ata o fondo do contido, recortado ao viewport), para
        // que se vexan continuas e aliñen coas que debuxa egui_extras nas demais.
        let bottom = (out.inner_rect.top() + out.content_size.y).min(out.inner_rect.bottom());
        for x in sep_xs {
            ui.painter()
                .vline(x, egui::Rangef::new(table_top, bottom), sep_stroke);
        }
        // Cargar os chuncos que faltaban polas filas que se acaban de ver e pedir
        // un repintado para que aparezan (substituíndo os «…»). Como tras cargalos
        // quedan na caché, no seguinte fotograma xa non se volven pedir.
        if !needed.is_empty() {
            for chunk in needed {
                self.ensure_chunk_loaded(chunk * CHUNK);
            }
            ui.ctx().request_repaint();
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
            let es_menor = self
                .selected_row
                .as_ref()
                .map(|r| r.tipo == TipoContrato::Menor)
                .unwrap_or(false);
            if let Some(r) = &self.selected_row {
                if !r.asunto.trim().is_empty() {
                    ui.label(RichText::new(&r.asunto).strong().size(16.0));
                    ui.add_space(8.0);
                }
                kv(ui, "Tipo", r.tipo.etiqueta());
                kv(ui, "ID", &r.id);
                kv(ui, "Referencia", &r.referencia);
                kv(ui, "Data de publicación", &r.publicacion);
                kv(ui, "Estado", &r.estado);
                kv(ui, "Importe", &r.importe_txt);
                kv(ui, "Organismo", &r.organismo);
                // Os contratos menores non teñen páxina de detalle: o adxudicatario,
                // o NIF e a duración veñen xa no listado.
                if es_menor {
                    kv(ui, "Adxudicatario", &r.adxudicatario);
                    kv(ui, "NIF", &r.nif);
                    kv(ui, "Duración", &r.duracion);
                }
            }

            match &self.selected_detail {
                // Os contratos menores non descargan detalle: o resumo de arriba
                // xa amosa todo o que hai.
                None if es_menor => {}
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
                    section_header(ui, "Datos do contrato");
                    ui.add_space(8.0);
                    kv(ui, "Obxecto", &d.obxecto);
                    kv(ui, "Tipo de contrato", &d.tipo_contrato);
                    kv(ui, "Tipo de procedemento", &d.tipo_procedemento);
                    kv(ui, "Tipo de tramitación", &d.tipo_tramitacion);
                    // Orzamento base é con IVE e valor estimado sen el; o
                    // matiz amósase na etiqueta xa que só gardamos o número.
                    kv(
                        ui,
                        "Orzamento base (con IVE)",
                        &d.orzamento_base.map(format_importe).unwrap_or_default(),
                    );
                    kv(
                        ui,
                        "Valor estimado (sen IVE)",
                        &d.valor_estimado.map(format_importe).unwrap_or_default(),
                    );
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
                        section_header(ui, "Resolucións / adxudicacións");
                        // Só ten sentido referenciar o lote cando hai máis dun.
                        let varios_lotes = resolucions.len() > 1;
                        for r in resolucions {
                            ui.add_space(6.0);
                            ui.group(|ui| {
                                ui.set_width(ui.available_width());
                                let cabeceira = if varios_lotes && !r.lote.is_empty() {
                                    format!("Lote {} · {}", r.lote, r.estado_resolucion)
                                } else {
                                    r.estado_resolucion.clone()
                                };
                                ui.label(RichText::new(cabeceira).strong());
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

            // Composición das UTE adxudicatarias (empresas membro).
            render_utes(ui, &self.selected_utes);
        });
    }

    /// Vista de empresas: listado de todas as razóns sociais adxudicatarias na
    /// busca actual, co seu NIF, número de contratos e importe total adxudicado.
    fn empresas_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Empresas adxudicatarias");
        ui.label(
            RichText::new(
                "Todas as razóns sociais ás que se lles adxudicou algún contrato na busca \
                 actual (licitacións e contratos menores). Para cada unha, o seu NIF, o número \
                 de contratos nos que é adxudicataria e a suma dos importes adxudicados.",
            )
            .small()
            .color(Color32::GRAY),
        );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if !self.empresas_loaded {
            self.refresh_empresas();
        }
        if self.empresas_total == 0 {
            ui.add_space(10.0);
            ui.label(
                RichText::new("Non hai empresas adxudicatarias na busca actual.")
                    .italics()
                    .color(Color32::GRAY),
            );
            return;
        }

        // Resumo: nº de empresas e importe total adxudicado (de `empresas_resumo`,
        // calculado na BD sobre toda a busca, non sobre as filas cargadas).
        ui.label(
            RichText::new(format!(
                "{} empresas · {} adxudicado en total",
                self.empresas_total,
                format_importe(self.empresas_importe_total),
            ))
            .small()
            .color(Color32::GRAY),
        );
        ui.add_space(4.0);

        // Carga progresiva por chuncos: a táboa virtual pide só os chuncos visibles.
        let total = self.empresas_total;
        let cache = &self.empresas_cache;
        let mut needed: Vec<usize> = Vec::new();
        // Orde actual (cópiase para usala dentro do peche da cabeceira sen tomar
        // prestado `self`); o clic recóllese e aplícase tras a táboa, igual ca os contratos.
        let (cur_col, cur_asc) = (self.local.empresas_sort_col, self.local.empresas_sort_asc);
        let accent = theme::accent(self.dark);
        let mut clicked_header: Option<EmpresaSortColumn> = None;
        // Empresa premida: ábrese a súa ficha tras a táboa (fóra do préstamo de `cache`).
        let mut clicked_empresa: Option<EmpresaContratos> = None;
        // As celas non capturan o clic; texto non seleccionable como na táboa de
        // contratos, para un aspecto coherente. A fila enteira é clicable (`sense`).
        ui.style_mut().interaction.selectable_labels = false;
        TableBuilder::new(ui)
            .id_salt("empresas_table")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().at_least(220.0).clip(true))
            .column(Column::initial(120.0).at_least(90.0).clip(true))
            .column(Column::initial(100.0).at_least(80.0))
            .column(Column::initial(140.0).at_least(110.0))
            .header(24.0, |mut h| {
                // (título, columna de orde, numérica). Premer ordena; volver premer inverte.
                // A columna activa marca cor de acento e frecha. As numéricas aliñan á dereita.
                for (t, col, num) in [
                    ("Empresa", EmpresaSortColumn::Nome, false),
                    ("NIF", EmpresaSortColumn::Nif, false),
                    ("Nº contratos", EmpresaSortColumn::NumContratos, true),
                    ("Importe total", EmpresaSortColumn::ImporteTotal, true),
                ] {
                    let activa = col == cur_col;
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
                        pad_cela(ui);
                        let lab = egui::Label::new(txt).sense(egui::Sense::click());
                        let resp = if num {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.add(lab))
                                .inner
                        } else {
                            ui.add(lab)
                        };
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            clicked_header = Some(col);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(22.0, total, |mut row| {
                    let i = row.index();
                    let chunk = i / CHUNK;
                    // Fila aínda non cargada: píntase «…» e o seu chunco márcase para pedir.
                    let Some(e) = cache.get(&chunk).and_then(|rows| rows.get(i % CHUNK)) else {
                        if !needed.contains(&chunk) {
                            needed.push(chunk);
                        }
                        for _ in 0..4 {
                            row.col(|ui| {
                                pad_cela(ui);
                                ui.label(RichText::new("…").weak());
                            });
                        }
                        return;
                    };
                    row.col(|ui| {
                        pad_cela(ui);
                        ui.label(&e.nome);
                    });
                    row.col(|ui| {
                        pad_cela(ui);
                        ui.label(RichText::new(&e.nif).monospace());
                    });
                    row.col(|ui| {
                        pad_cela(ui);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(e.num_contratos.to_string());
                        });
                    });
                    row.col(|ui| {
                        pad_cela(ui);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(format_importe(e.importe_total));
                        });
                    });
                    let resp = row.response();
                    resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        clicked_empresa = Some(e.clone());
                    }
                });
            });

        // Cargar os chuncos que faltaban e repintar para que enchan o «…».
        if !needed.is_empty() {
            for chunk in needed {
                self.ensure_empresas_chunk_loaded(chunk * CHUNK);
            }
            ui.ctx().request_repaint();
        }

        // Aplicar o clic na cabeceira: mesma columna inverte; nova columna usa o seu
        // sentido por defecto. `empresas_loaded = false` fai que `refresh_empresas`
        // limpe a caché e recargue os chuncos coa nova orde.
        if let Some(col) = clicked_header {
            if self.local.empresas_sort_col == col {
                self.local.empresas_sort_asc = !self.local.empresas_sort_asc;
            } else {
                self.local.empresas_sort_col = col;
                self.local.empresas_sort_asc = col.default_asc();
            }
            self.empresas_loaded = false;
        }

        // Abrir a ficha da empresa premida (carga os seus contratos e UTE).
        if let Some(e) = clicked_empresa {
            self.open_empresa(&e.nif, &e.nome);
        }
    }

    /// Carga a ficha dunha empresa (contratos directos + UTE nas que participa,
    /// acoutados á busca actual) e amósaa na vista de detalle da pestana Empresas.
    /// Identifícase pola mesma clave que o listado (NIF, ou clave do nome se non o
    /// ten); por iso abonda co NIF e o nome, que tamén traen os membros das UTE.
    fn open_empresa(&mut self, nif: &str, nome: &str) {
        match self.db.empresa_detalle(&self.local, nif, nome) {
            Ok(d) => self.selected_empresa = Some(d),
            Err(err) => self.status = format!("⚠ ficha empresa: {err}"),
        }
    }

    /// Ficha da empresa seleccionada: os seus contratos adxudicados directamente
    /// (separados en licitacións e contratos menores) e as UTE nas que participa
    /// cos seus contratos. Cada contrato é clicable e salta á súa ficha. Ocupa o
    /// espazo do listado ata que se volve con "← Volver".
    fn empresa_detail_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("← Volver á lista").clicked() {
                self.selected_empresa = None;
            }
            ui.add_space(8.0);
            ui.heading("Ficha de empresa");
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if self.selected_empresa.is_none() {
            return;
        }

        let dark = self.dark;
        // Cor do vínculo por UTE (a mesma que na vista de relacións).
        let ute_color = if dark {
            Color32::from_rgb(0x5A, 0xC8, 0xE0)
        } else {
            Color32::from_rgb(0x0A, 0x84, 0xA5)
        };
        // Accións premidas dentro da ScrollArea; aplícanse despois (fóra do préstamo
        // inmutable de `self.selected_empresa`): `jump` salta á ficha dun contrato;
        // `open_membro` (NIF, nome) abre a ficha doutra empresa relacionada.
        let mut jump: Option<String> = None;
        let mut open_membro: Option<(String, String)> = None;
        let emp = self.selected_empresa.as_ref().unwrap();
        ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new(&emp.nome).strong().size(16.0));
            if !emp.nif.trim().is_empty() {
                ui.label(
                    RichText::new(&emp.nif)
                        .monospace()
                        .small()
                        .color(Color32::GRAY),
                );
            }
            ui.add_space(4.0);
            let total: f64 = emp.contratos.iter().map(|c| c.importe_num).sum();
            ui.label(
                RichText::new(format!(
                    "{} contratos adxudicados directamente · {} en total",
                    emp.contratos.len(),
                    format_importe(total),
                ))
                .small()
                .color(Color32::GRAY),
            );

            // Contratos directos, separados por tipo (licitacións / menores).
            let licit: Vec<&EmpresaContrato> = emp
                .contratos
                .iter()
                .filter(|c| c.tipo == TipoContrato::Licitacion)
                .collect();
            let menores: Vec<&EmpresaContrato> = emp
                .contratos
                .iter()
                .filter(|c| c.tipo == TipoContrato::Menor)
                .collect();
            if !licit.is_empty() {
                ui.add_space(12.0);
                section_header(ui, &format!("Licitacións ({})", licit.len()));
                ui.add_space(4.0);
                if let Some(id) = empresa_contratos_taboa(ui, "ficha_licit", &licit) {
                    jump = Some(id);
                }
            }
            if !menores.is_empty() {
                ui.add_space(12.0);
                section_header(ui, &format!("Contratos menores ({})", menores.len()));
                ui.add_space(4.0);
                if let Some(id) = empresa_contratos_taboa(ui, "ficha_menores", &menores) {
                    jump = Some(id);
                }
            }
            if emp.contratos.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Sen contratos adxudicados directamente nesta busca.")
                        .italics()
                        .color(Color32::GRAY),
                );
            }

            // UTE nas que participa a empresa, cos seus contratos.
            if !emp.utes.is_empty() {
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Preme nunha UTE para ver as empresas relacionadas e os seus contratos.",
                    )
                    .small()
                    .color(Color32::GRAY),
                );
                for (idx, u) in emp.utes.iter().enumerate() {
                    ui.add_space(6.0);
                    // As empresas relacionadas son os demais membros da UTE (todos
                    // menos a propia empresa da ficha).
                    let relacionadas: Vec<&EmpresaUteMembro> =
                        u.membros.iter().filter(|m| !m.propia).collect();
                    let titulo = RichText::new(format!("🤝 {}", u.nome))
                        .strong()
                        .color(ute_color);
                    egui::CollapsingHeader::new(titulo)
                        .id_salt(egui::Id::new(("ficha_ute", idx)))
                        .show(ui, |ui| {
                            // Empresas relacionadas: clicables, abren a súa propia ficha.
                            if relacionadas.is_empty() {
                                ui.label(
                                    RichText::new("Sen outras empresas na UTE.")
                                        .small()
                                        .italics()
                                        .color(Color32::GRAY),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Empresas relacionadas")
                                        .small()
                                        .strong()
                                        .color(ute_color),
                                );
                                ui.style_mut().interaction.selectable_labels = false;
                                for m in &relacionadas {
                                    let resp = ui
                                        .horizontal_wrapped(|ui| {
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
                                        })
                                        .response
                                        .interact(egui::Sense::click());
                                    if resp.hovered() {
                                        resp.clone()
                                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    }
                                    if resp.clicked() {
                                        open_membro = Some((m.cif.clone(), m.nome.clone()));
                                    }
                                }
                            }

                            // Contratos adxudicados á UTE (táboa, igual cós directos).
                            if !u.contratos.is_empty() {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(format!(
                                        "Contratos adxudicados á UTE ({}) · {} en total",
                                        u.contratos.len(),
                                        format_importe(u.importe_total),
                                    ))
                                    .small()
                                    .strong(),
                                );
                                ui.add_space(2.0);
                                let refs: Vec<&EmpresaContrato> = u.contratos.iter().collect();
                                if let Some(id) =
                                    empresa_contratos_taboa(ui, &format!("ficha_ute_{idx}"), &refs)
                                {
                                    jump = Some(id);
                                }
                            }
                        });
                }
            }
        });

        // Premer unha empresa relacionada abre a súa ficha (queda na pestana
        // Empresas). Ten prioridade sobre o salto a contrato.
        if let Some((nif, nome)) = open_membro {
            self.open_empresa(&nif, &nome);
        } else if let Some(id) = jump
            && let Ok(Some(row)) = self.db.query_local_by_id(&id)
        {
            // Saltar á ficha do contrato premido: selecciónase e cámbiase á pestana
            // Contratos, que amosará a vista de detalle no seguinte fotograma.
            self.select_contract(row);
            self.tab = Tab::Contratos;
        }
    }

    /// Vista de relacións: grupos de razóns sociais que concorreron xuntas nunha
    /// mesma UTE adxudicataria (dato do propio contrato).
    fn relacions_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Relacións entre razóns sociais");
        ui.label(
            RichText::new(
                "Grupos de razóns sociais que concorreron xuntas nunha mesma Unión \
                 Temporal de Empresas (UTE) adxudicataria dos teus contratos. Cada grupo \
                 reúne todas as empresas que comparten UTE; se unha empresa participa en \
                 varias UTE, todas as súas socias caen na mesma trama.",
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
                    "Aínda non se atoparon relacións. As tramas xórdense das UTE \
                     adxudicatarias dos contratos importados.",
                )
                .italics()
                .color(Color32::GRAY),
            );
            return;
        }

        // Cor do vínculo por UTE.
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

                    // UTE que vinculan as razóns sociais do grupo.
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
                            ui.label(RichText::new(&e.empresa_nome).strong());
                            ui.label(
                                RichText::new(format!("· {} contratos", e.num_contratos))
                                    .small()
                                    .color(Color32::GRAY),
                            );
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
                        let id = egui::Id::new(("contratos_grupo", &g.empresas[0].cif));
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
            && let Ok(Some(row)) = self.db.query_local_by_id(&id)
        {
            self.select_contract(row);
            self.tab = Tab::Contratos;
        }
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

/// Renderiza, na ficha do contrato, a composición das UTE adxudicatarias: cada
/// empresa membro co seu CIF.
fn render_utes(ui: &mut egui::Ui, utes: &[UteDetalle]) {
    if utes.is_empty() {
        return;
    }
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);
    section_header(ui, "Composición da UTE");
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
                });
            }
        });
    }
}

/// Renderiza unha **táboa** de contratos da ficha de empresa (unha fila por
/// contrato; columnas ID/Data/Importe/Organismo/Obxecto, coa data amosando o
/// ano). Cada fila é clicable e salta á ficha do contrato; devolve o id premido,
/// se o hai. Sen scroll propio (`vscroll(false)`) para integrarse na ScrollArea
/// exterior da ficha; `id_salt` debe ser único entre as táboas da mesma vista.
fn empresa_contratos_taboa(
    ui: &mut egui::Ui,
    id_salt: &str,
    contratos: &[&EmpresaContrato],
) -> Option<String> {
    let mut jump: Option<String> = None;

    // Altura de cada fila axustada ao obxecto: como a columna Obxecto reparte
    // («wrap») o texto en varias liñas, predise canto ocupará. As dúas columnas
    // `remainder` (Organismo e Obxecto) reparten a partes iguais o espazo
    // sobrante; subestímase un chisco o ancho para que a altura estimada nunca
    // quede curta e recorte texto.
    let spacing_x = ui.spacing().item_spacing.x;
    let fixos = 64.0 + 96.0 + 120.0;
    let sobrante = (ui.available_width() - fixos - 4.0 * spacing_x).max(0.0);
    let obxecto_w = (sobrante / 2.0).max(180.0);
    let texto_w = (obxecto_w - 8.0).max(40.0); // restar o padding interior da cela
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let cor = ui.visuals().text_color();
    const FILA_MIN: f32 = 22.0;
    let alturas: Vec<f32> = contratos
        .iter()
        .map(|c| {
            let galley = ui
                .painter()
                .layout(c.asunto.clone(), font_id.clone(), cor, texto_w);
            (galley.size().y + 8.0).max(FILA_MIN)
        })
        .collect();

    // As celas non capturan o clic: así o cursor non entra en modo selección e o
    // clic chega á fila enteira (`sense(click)`).
    ui.style_mut().interaction.selectable_labels = false;
    TableBuilder::new(ui)
        .id_salt(id_salt)
        .striped(true)
        .vscroll(false)
        .sense(egui::Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(64.0).at_least(50.0))
        .column(Column::initial(96.0).at_least(80.0))
        .column(Column::initial(120.0).at_least(90.0))
        .column(
            Column::remainder()
                .at_least(140.0)
                .clip(true)
                .resizable(false),
        )
        .column(Column::remainder().at_least(180.0).resizable(false))
        .header(22.0, |mut h| {
            for (t, num) in [
                ("ID", false),
                ("Data", false),
                ("Importe", true),
                ("Organismo", false),
                ("Obxecto", false),
            ] {
                h.col(|ui| {
                    pad_cela(ui);
                    let txt = RichText::new(t).strong();
                    if num {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(txt));
                    } else {
                        ui.label(txt);
                    }
                });
            }
        })
        .body(|body| {
            body.heterogeneous_rows(alturas.into_iter(), |mut row| {
                let c = contratos[row.index()];
                row.col(|ui| {
                    pad_cela(ui);
                    ui.label(RichText::new(&c.contract_id).monospace());
                });
                row.col(|ui| {
                    pad_cela(ui);
                    ui.label(&c.publicacion);
                });
                row.col(|ui| {
                    pad_cela(ui);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(&c.importe_txt);
                    });
                });
                row.col(|ui| {
                    pad_cela(ui);
                    ui.label(&c.organismo);
                });
                row.col(|ui| {
                    pad_cela(ui);
                    // O obxecto reparte («wrap») en varias liñas en lugar de
                    // recortarse: a altura da fila xa se calculou para acollelo.
                    ui.add(egui::Label::new(&c.asunto).wrap());
                });
                let resp = row.response();
                resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    jump = Some(c.contract_id.clone());
                }
            });
        });
    jump
}

/// Campo de texto dunha liña con padding interior e ancho completo.
fn text_input(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .margin(egui::Margin::symmetric(8, 6))
            .desired_width(f32::INFINITY),
    )
}

/// Cabeceira dunha sección da vista de detalle. Semibold e un chisco maior ca o
/// corpo, para que se sitúe por riba dos valores na xerarquía visual.
fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .family(egui::FontFamily::Name("semibold".into()))
            .size(15.5),
    );
}

/// Fila «etiqueta + valor» para a vista de detalle. A etiqueta ocupa unha
/// columna fixa á esquerda e o valor axústase a varias liñas (wrap) para que o
/// texto longo non se saia do marco da vista. Omítese se o valor está baleiro.
fn kv(ui: &mut egui::Ui, label: &str, value: &str) {
    if value.trim().is_empty() || value == "_" {
        return;
    }
    // A etiqueta é unha simple guía: gris atenuada e pequena. O valor conserva
    // a cor de texto plena e peso medio, de xeito que destaca sobre a etiqueta.
    let muted = crate::theme::label_muted(ui.visuals().dark_mode);
    ui.horizontal_top(|ui| {
        // Etiqueta aliñada á dereita contra unha canle central: así os valores
        // arrincan todos na mesma columna e fórmase unha liña vertical limpa.
        ui.allocate_ui_with_layout(
            egui::vec2(170.0, 0.0),
            Layout::right_to_left(Align::TOP),
            |ui| {
                ui.add(egui::Label::new(RichText::new(label).color(muted).size(12.5)).wrap());
            },
        );
        ui.add_space(12.0);
        // `wrap()` fai que o valor se reparta en varias liñas dentro do ancho
        // restante en lugar de desbordar a vista.
        ui.add(
            egui::Label::new(RichText::new(value).family(egui::FontFamily::Name("medium".into())))
                .wrap(),
        );
    });
    ui.add_space(7.0);
}

/// Mostra unha etiqueta + valor se o valor non está baleiro (variante en liña,
/// para campos dentro dunha tarxeta). Mesma xerarquía visual ca `kv`: etiqueta
/// gris atenuada, valor en cor plena.
fn field(ui: &mut egui::Ui, label: &str, value: &str) {
    if value.trim().is_empty() || value == "_" {
        return;
    }
    let muted = crate::theme::label_muted(ui.visuals().dark_mode);
    ui.horizontal_top(|ui| {
        // Mesma canle ca `kv`, máis estreita por estar dentro dunha tarxeta.
        ui.allocate_ui_with_layout(
            egui::vec2(110.0, 0.0),
            Layout::right_to_left(Align::TOP),
            |ui| {
                ui.add(egui::Label::new(RichText::new(label).color(muted).small()).wrap());
            },
        );
        ui.add_space(10.0);
        ui.add(
            egui::Label::new(
                RichText::new(value)
                    .family(egui::FontFamily::Name("medium".into()))
                    .small(),
            )
            .wrap(),
        );
    });
    ui.add_space(3.0);
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
