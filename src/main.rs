#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod db;
mod export;
mod model;
mod scraper;
mod sync;
mod theme;
mod worker;

use chrono::Local;
use std::path::PathBuf;

/// Marca de tempo lexible para rexistros e metadatos.
pub fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Ruta da base de datos local (cartafol de datos do usuario).
pub fn db_path() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("gal", "congal", "congal") {
        let dir = dirs.data_dir().to_path_buf();
        let _ = std::fs::create_dir_all(&dir);
        return dir.join("contratos.sqlite");
    }
    PathBuf::from("contratos.sqlite")
}

/// Detecta se o sistema operativo está en modo escuro.
pub fn system_dark() -> bool {
    matches!(dark_light::detect(), Ok(dark_light::Mode::Dark))
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([920.0, 560.0])
            .with_title("Congal"),
        ..Default::default()
    };

    eframe::run_native(
        "Congal",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
