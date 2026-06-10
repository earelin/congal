//! Fío traballador en segundo plano e canle de eventos cara á interface.

use crate::db::Db;
use crate::enrich::{self, EnrichMode, EnrichResult};
use crate::model::{FilterOptions, ImportParams, LocalFilters, Suggestion};
use crate::scraper::{self, Client};
use crate::sync::{self, SyncResult};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// Ordes que a interface envía ao fío traballador.
pub enum Command {
    LoadOptions,
    /// Importa un organismo: licitacións (con detalle) e contratos menores do ano.
    Import(ImportParams),
    /// Vincula os adxudicatarios con datoscif e descarga os cargos das empresas.
    /// O modo decide se só se procesan os novos ou se se reimportan todas.
    Enrich(EnrichMode),
    /// Resolve a man un caso de revisión: vincula o adxudicatario á entidade
    /// escollida (`Some`) ou descártao (`None`).
    ResolveMatch {
        adx_nome: String,
        escolla: Option<Suggestion>,
    },
    /// Busca en vivo en datoscif (proceso asistido de revisión). `adx_nome`
    /// identifica o caso para asociar os resultados.
    SearchDatoscif {
        adx_nome: String,
        termo: String,
    },
    Export {
        path: PathBuf,
        filters: LocalFilters,
    },
    /// Remata o bucle do fío para pechar limpamente a conexión SQLite.
    Shutdown,
}

/// Eventos que o fío traballador devolve á interface.
pub enum Event {
    Options(FilterOptions),
    SyncProgress {
        done: usize,
        total: usize,
        msg: String,
    },
    SyncDone(SyncResult),
    EnrichDone(EnrichResult),
    /// Un caso de revisión resolveuse (vinculado a man ou descartado).
    RevisionResolved {
        adx_nome: String,
        vinculado: bool,
    },
    /// Resultados dunha busca asistida en datoscif para un caso de revisión.
    DatoscifResults {
        adx_nome: String,
        suggestions: Vec<Suggestion>,
    },
    Exported(PathBuf, usize),
    Error(String),
    Log(String),
}

pub struct Worker {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
    /// Manexador do fío para poder esperar polo seu remate ao pechar.
    handle: Option<thread::JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(ctx: egui::Context, db_path: PathBuf) -> Worker {
        let (tx_cmd, rx_cmd) = mpsc::channel::<Command>();
        let (tx_evt, rx_evt) = mpsc::channel::<Event>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_thread = cancel.clone();

        let handle = thread::spawn(move || {
            let client = match Client::new() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx_evt.send(Event::Error(format!("Erro creando cliente HTTP: {e}")));
                    return;
                }
            };
            let mut db = match Db::open(&db_path) {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx_evt.send(Event::Error(format!("Erro abrindo a base de datos: {e}")));
                    return;
                }
            };

            while let Ok(cmd) = rx_cmd.recv() {
                cancel_thread.store(false, Ordering::Relaxed);
                match cmd {
                    Command::Shutdown => break,
                    Command::LoadOptions => match scraper::load_filter_options(&client) {
                        Ok(o) => {
                            let _ = tx_evt.send(Event::Options(o));
                        }
                        Err(e) => {
                            let _ =
                                tx_evt.send(Event::Error(format!("Erro cargando filtros: {e}")));
                        }
                    },
                    Command::Import(params) => {
                        match sync::run_import(&client, &mut db, &params, &tx_evt, &cancel_thread) {
                            Ok(r) => {
                                let _ = tx_evt.send(Event::SyncDone(r));
                            }
                            Err(e) => {
                                let _ =
                                    tx_evt.send(Event::Error(format!("Erro na importación: {e}")));
                            }
                        }
                    }
                    Command::Enrich(mode) => {
                        match enrich::run_enrich(&client, &mut db, &tx_evt, &cancel_thread, mode) {
                            Ok(r) => {
                                let _ = tx_evt.send(Event::EnrichDone(r));
                            }
                            Err(e) => {
                                let _ = tx_evt
                                    .send(Event::Error(format!("Erro no enriquecemento: {e}")));
                            }
                        }
                    }
                    Command::ResolveMatch { adx_nome, escolla } => {
                        let vinculado = escolla.is_some();
                        match enrich::vincular_manual(&client, &mut db, &tx_evt, &adx_nome, escolla)
                        {
                            Ok(()) => {
                                let _ = tx_evt.send(Event::RevisionResolved {
                                    adx_nome,
                                    vinculado,
                                });
                            }
                            Err(e) => {
                                let _ = tx_evt.send(Event::Error(format!(
                                    "Erro resolvendo «{adx_nome}»: {e}"
                                )));
                            }
                        }
                    }
                    Command::SearchDatoscif { adx_nome, termo } => {
                        match scraper::search_entities(&client, &termo) {
                            Ok(suggestions) => {
                                let _ = tx_evt.send(Event::DatoscifResults {
                                    adx_nome,
                                    suggestions,
                                });
                            }
                            Err(e) => {
                                let _ = tx_evt
                                    .send(Event::Error(format!("Erro buscando «{termo}»: {e}")));
                            }
                        }
                    }
                    Command::Export { path, filters } => {
                        let res = db.query_local(&filters).and_then(|rows| {
                            crate::export::export_ods(&path, &rows).map(|_| rows.len())
                        });
                        match res {
                            Ok(n) => {
                                let _ = tx_evt.send(Event::Exported(path, n));
                            }
                            Err(e) => {
                                let _ = tx_evt.send(Event::Error(format!("Erro exportando: {e}")));
                            }
                        }
                    }
                }
                ctx.request_repaint();
            }
            // Ao saír do bucle (Shutdown ou canle pechada) cae `db`, o que pecha
            // a conexión SQLite limpamente e fai o checkpoint do WAL.
        });

        Worker {
            tx: tx_cmd,
            rx: rx_evt,
            cancel,
            handle: Some(handle),
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Detén calquera operación en curso e espera a que o fío remate, garantindo
    /// que a súa conexión SQLite se pecha antes de saír da aplicación.
    pub fn shutdown(&mut self) {
        // Aborta unha sincronización longa que estea en curso.
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Command::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}
