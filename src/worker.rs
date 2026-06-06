//! Fío traballador en segundo plano e canle de eventos cara á interface.

use crate::db::Db;
use crate::model::{FilterOptions, Filters, LocalFilters};
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
    Sync(Filters),
    Export {
        path: PathBuf,
        filters: LocalFilters,
    },
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
    Exported(PathBuf, usize),
    Error(String),
    Log(String),
}

pub struct Worker {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
}

impl Worker {
    pub fn spawn(ctx: egui::Context, db_path: PathBuf) -> Worker {
        let (tx_cmd, rx_cmd) = mpsc::channel::<Command>();
        let (tx_evt, rx_evt) = mpsc::channel::<Event>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_thread = cancel.clone();

        thread::spawn(move || {
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
                    Command::LoadOptions => match scraper::load_filter_options(&client) {
                        Ok(o) => {
                            let _ = tx_evt.send(Event::Options(o));
                        }
                        Err(e) => {
                            let _ = tx_evt.send(Event::Error(format!("Erro cargando filtros: {e}")));
                        }
                    },
                    Command::Sync(filters) => {
                        match sync::run_sync(&client, &mut db, &filters, &tx_evt, &cancel_thread) {
                            Ok(r) => {
                                let _ = tx_evt.send(Event::SyncDone(r));
                            }
                            Err(e) => {
                                let _ = tx_evt
                                    .send(Event::Error(format!("Erro na sincronización: {e}")));
                            }
                        }
                    }
                    Command::Export { path, filters } => {
                        let res = db
                            .query_local(&filters)
                            .and_then(|rows| crate::export::export_ods(&path, &rows).map(|_| rows.len()));
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
        });

        Worker {
            tx: tx_cmd,
            rx: rx_evt,
            cancel,
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
