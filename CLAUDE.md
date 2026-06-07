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

- **`enrich.rs`** — `Command::Enrich` flow (manual "Vincular con datoscif" button). For each
  distinct adxudicatario not yet in `adxudicatario_match`, searches datoscif, picks a
  high-confidence entity (`model::best_match`), stores the match, and for matched **empresas**
  downloads their cargos. Auto-links only high-confidence/unambiguous matches; the rest are
  saved as `pendente` (no link). Same `THROTTLE`/cancel/`SyncProgress` pattern as `sync.rs`.

- **`scraper/`** — HTTP client and parsing, one concern per file:
  - `mod.rs` — `Client` (cookie store + session priming on the portal), and the shared
    `decode_bytes` (the site is ISO-8859-1 / Windows-1252) and `clean_text` helpers.
  - `search.rs` — `POST resultadoIndex.jsp`; the server returns ALL results in a hidden
    JSON array (`#resSearch`), the web paginates client-side. Returns `ContractSummary`.
  - `detail.rs` — `GET licitacion?OP=50&N=<id>`; parses the detail page into
    `ContractDetail` + per-lot `Resolucion` rows.
  - `options.rs` — parses the filter dropdowns from `portada.jsp` into `FilterOptions`.
  - `datoscif.rs` — datoscif.es APIs: `search_entities` (`POST /sugerencias.ajax`, substring
    search) and `fetch_cargos` (`POST /filtros.ajax`, paginated) return **JSON in UTF-8** (no
    `decode_bytes`). `fetch_empresa_info` GETs the `/empresa/<slug>` **HTML** page and reads
    the company ficha (CIF, domicilio, CP, municipio, provincia) from schema.org **microdata**
    `itemprop` spans (`taxID`/`streetAddress`/`postalCode`/`addressLocality`/`addressRegion`).
    A person's `persona_url` slug is the stable canonical key linking the same individual
    across companies; ownership shows up as a cargo (`Socio Unico`/`Socio`).

- **`db.rs`** — SQLite schema and queries (`Db`). Tables: `contracts` (listing, normalised:
  only `importe_num`, dates stored as ISO 8601 text, `cod_organismo` + `cod_estado` FKs),
  `organismos` (`cod_organismo` → `nome`, the organism name lives here, loaded via JOIN),
  `estados` (`cod_estado` → `nome`; the server only sends the estado text, so `cod_estado`
  is a surrogate key auto-assigned on first insert of each name; loaded via JOIN),
  `contract_detail` (1:1), `contract_resolucion` (1:N per lot), plus a `meta` key/value
  table (e.g. `ultima_sync`). The datoscif enrichment adds `datoscif_entidade`
  (empresa/persoa by canonical slug, plus the company ficha: cif/domicilio/cod_postal/
  municipio/provincia), `datoscif_cargo` (persona→empresa role + validity),
  and `adxudicatario_match` (keyed by `company_key` of the adxudicatario → datoscif entity,
  with `confianza`/`estado`). A SQL function `cokey(x)` (sibling of `nrm`) joins
  `contract_resolucion.adxudicatario` to `adxudicatario_match`. `relacions_compartidas`
  finds people holding a cargo in ≥2 contracted razóns sociais.
  WAL mode, foreign keys on. The project is a **prototype**: no schema versioning/migration —
  delete the local `.sqlite` to apply schema changes. `upsert_*` are the write paths;
  `query_local` powers the offline Local tab (one row per contract, lotes aggregated;
  importe/date/organismo are derived for display), including search by adxudicatario.

- **`model.rs`** — all domain types and pure helpers. Notable: `EstadoGroup` maps the four
  UI status checkboxes to the numeric `ESTADO` codes the server expects; `parse_importe` /
  `format_importe` convert between Galician-formatted amounts (`1.000.000,00 €`) and `f64`;
  `parse_data` / `format_data_gl` convert between portal dates, ISO 8601, and `DD/MM/YYYY`
  display; `is_estado_terminal` drives the incremental-sync skip decision. The datoscif
  matching helpers live here too: `company_key`/`company_core`/`strip_legal_suffix`
  (normalise names, drop SL/SA… suffixes), `search_variants` (reorders person names
  «nome apelido1 apelido2» → «apelido1 apelido2 nome» for datoscif's substring search), and
  `best_match` (Exacta > Núcleo > Tokens, auto-links only an unambiguous winner).

- **`export.rs`** — writes the filtered local rows to an OpenDocument Spreadsheet (`.ods`)
  via `spreadsheet-ods`.

- **`theme.rs`** — Apple-HIG-inspired styling; bundles the Inter font with `include_bytes!`.

## Domain notes

- The whole **contratosdegalicia.gal** site is **ISO-8859-1**; always decode response bytes
  through `scraper::decode_bytes` rather than treating them as UTF-8. By contrast,
  **datoscif.es** returns **JSON in UTF-8** — parse it directly with `serde_json`.
- datoscif search is **substring-based**; person names there are stored "apelido1 apelido2
  nome" while contracts use "nome apelido1 apelido2" — hence `search_variants` reorders.
- The resolution PDF is reCAPTCHA-protected and is **not** downloaded; only the public
  resolution URL (`enlace_resolucion`) is stored.
- Parsing tests load real captured pages from `tests/fixtures/*.html` (and datoscif
  `tests/fixtures/datoscif_*.json`) via `include_str!` / `include_bytes!` from inside the
  `scraper` modules — update fixtures if the sites' markup/JSON changes.

## License note

The Inter font (`assets/fonts/`) is under the SIL Open Font License
(`assets/fonts/Inter-OFL.txt`).
