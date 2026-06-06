# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

`congal` is a cross-platform desktop app (Rust, edition 2024) that scrapes, stores and
exports public-contract data from the Galician government portal
[contratosdegalicia.gal](https://www.contratosdegalicia.gal). The GUI is built with
**egui/eframe**; data is persisted locally in **SQLite** (`rusqlite`, bundled). No system
dependencies and no webview — TLS is `rustls`, SQLite is statically linked.

The codebase, comments and UI strings are in **Galician**. Keep new comments and
user-facing strings in Galician to match.

## Commands

```bash
cargo build --release          # build (binary: ./target/release/congal)
cargo run                      # run the GUI in debug
cargo test                     # unit/parsing tests (offline, run against HTML fixtures)
cargo test importe_galego      # run a single test by name
cargo test --release -- --ignored --nocapture live_end_to_end   # live network test (hits the real server)
```

The `live_end_to_end` test (in `src/sync.rs`) is `#[ignore]` by default because it requires
network access and exercises a full search → detail → DB → ODS export cycle against a
known contract id.

## Architecture

The data flow is: **GUI → Worker thread → scraper/sync/db → events back to GUI**.

- **`main.rs`** — entry point, eframe window setup, and two key helpers: `db_path()`
  (resolves the per-user data dir via `directories::ProjectDirs("gal","congal","congal")`,
  e.g. `~/.local/share/congal/contratos.sqlite`) and `system_dark()`.

- **`app.rs`** — all egui UI, single `App` struct holding the full UI state. Two tabs:
  `Scraper` (sync with web filters) and `Local` (offline DB queries + ODS export). The UI
  **never blocks on I/O**: it sends `Command`s to the worker and drains `Event`s each frame.

- **`worker.rs`** — the concurrency boundary. `Worker::spawn` starts one background thread
  owning the HTTP `Client` and the `Db`. Communication is two `mpsc` channels (`Command`
  in, `Event` out) plus an `Arc<AtomicBool>` cancel flag. Long operations report progress
  via `Event::SyncProgress` and call `ctx.request_repaint()` to wake the UI.

- **`sync.rs`** — incremental sync logic. After a search, contracts whose stored state is
  **terminal** (`is_estado_terminal`, e.g. formalizado/deserto/anulado) are skipped; only
  new or still-in-progress contracts get their detail re-downloaded. A `THROTTLE`
  (350 ms) is applied between detail requests; honours the cancel flag.

- **`scraper/`** — HTTP client and parsing, one concern per file:
  - `mod.rs` — `Client` (cookie store + session priming on the portal), and the shared
    `decode_bytes` (the site is ISO-8859-1 / Windows-1252) and `clean_text` helpers.
  - `search.rs` — `POST resultadoIndex.jsp`; the server returns ALL results in a hidden
    JSON array (`#resSearch`), the web paginates client-side. Returns `ContractSummary`.
  - `detail.rs` — `GET licitacion?OP=50&N=<id>`; parses the detail page into
    `ContractDetail` + per-lot `Resolucion` rows.
  - `options.rs` — parses the filter dropdowns from `portada.jsp` into `FilterOptions`.

- **`db.rs`** — SQLite schema and queries (`Db`). Tables: `contracts` (listing, normalised:
  only `importe_num`, dates stored as ISO 8601 text, `cod_organismo` + `cod_estado` FKs),
  `organismos` (`cod_organismo` → `nome`, the organism name lives here, loaded via JOIN),
  `estados` (`cod_estado` → `nome`; the server only sends the estado text, so `cod_estado`
  is a surrogate key auto-assigned on first insert of each name; loaded via JOIN),
  `contract_detail` (1:1), `contract_resolucion` (1:N per lot), plus a `meta` key/value
  table (e.g. `ultima_sync`).
  WAL mode, foreign keys on. The project is a **prototype**: no schema versioning/migration —
  delete the local `.sqlite` to apply schema changes. `upsert_*` are the write paths;
  `query_local` powers the offline Local tab (one row per contract, lotes aggregated;
  importe/date/organismo are derived for display), including search by adxudicatario.

- **`model.rs`** — all domain types and pure helpers. Notable: `EstadoGroup` maps the four
  UI status checkboxes to the numeric `ESTADO` codes the server expects; `parse_importe` /
  `format_importe` convert between Galician-formatted amounts (`1.000.000,00 €`) and `f64`;
  `parse_data` / `format_data_gl` convert between portal dates, ISO 8601, and `DD/MM/YYYY`
  display; `is_estado_terminal` drives the incremental-sync skip decision.

- **`export.rs`** — writes the filtered local rows to an OpenDocument Spreadsheet (`.ods`)
  via `spreadsheet-ods`.

- **`theme.rs`** — Apple-HIG-inspired styling; bundles the Inter font with `include_bytes!`.

## Domain notes

- The whole target site is **ISO-8859-1**; always decode response bytes through
  `scraper::decode_bytes` rather than treating them as UTF-8.
- The resolution PDF is reCAPTCHA-protected and is **not** downloaded; only the public
  resolution URL (`enlace_resolucion`) is stored.
- Parsing tests load real captured pages from `tests/fixtures/*.html` via `include_str!` /
  `include_bytes!` from inside the `scraper` modules — update fixtures if the site's markup
  changes.

## License note

The Inter font (`assets/fonts/`) is under the SIL Open Font License
(`assets/fonts/Inter-OFL.txt`).
