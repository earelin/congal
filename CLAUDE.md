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

./script/ci.sh                 # corre en local as mesmas comprobacións que CI (fmt, clippy, build, test)
```

**Antes de facer `git push`, executa sempre `./script/ci.sh`** e asegúrate de que pasa.
Reproduce exactamente o pipeline de GitHub Actions (`.github/workflows/ci.yml`):
`cargo fmt --check`, `cargo clippy` con `-D warnings`, build en release e tests. Se algunha
comprobación falla, arránxaa antes de empuxar para non deixar o CI en vermello.

The `live_end_to_end` test (in `src/sync.rs`) is `#[ignore]` by default because it requires
network access and exercises the full cycle against the real server: licitacións API → detail →
contratos menores (date-window iteration) → DB → local query.

## Architecture

The data flow is: **GUI → Worker thread → scraper/sync/db → events back to GUI**.

- **`main.rs`** — entry point, eframe window setup, and two key helpers: `db_path()`
  (resolves the per-user data dir via `directories::ProjectDirs("gal","congal","congal")`,
  e.g. `~/.local/share/congal/contratos.sqlite`) and `system_dark()`.

- **`app.rs`** — all egui UI, single `App` struct holding the full UI state. Import runs from a
  modal dialog (pick **one organismo** + a **year**). The main view has three tabs: `Contratos`
  (local listing + contract detail) split into **two sub-tabs** — `Licitacións` and `Contratos
  menores` (`sub_tab: TipoContrato`, drives `LocalFilters.tipo`; the menores table swaps the
  Estado/Imp.resolución columns for NIF/Duración) — plus `Empresas` (one row per awardee:
  name, NIF, nº of contracts and total awarded, via `db::empresas_con_contratos`) and
  `Relacions` (tramas **derived only from shared UTE membership**). The Empresas and Relacións
  tabs honour the side-panel filters but, unlike Contratos, ignore the licitación/menor sub-tab
  (they consider both contract types). The UI **never blocks on I/O**: it sends `Command`s to the
  worker and drains `Event`s each frame.

- **`worker.rs`** — the concurrency boundary. `Worker::spawn` starts one background thread
  owning the HTTP `Client` and the `Db`. Communication is two `mpsc` channels (`Command`
  in, `Event` out) plus an `Arc<AtomicBool>` cancel flag. Long operations report progress
  via `Event::SyncProgress` and call `ctx.request_repaint()` to wake the UI.

- **`sync.rs`** — per-organismo import (`Command::Import(ImportParams{org_id,org_nome,ano})` →
  `run_import`). Fetches the organismo's **licitacións of the chosen year** via
  `scraper::search_licitaciones` (resultadoIndex, server-side `OR`+`YEAR` filter — the perfil API
  has NO year filter), then for the **new or still-in-progress** ones (stored state not
  `is_estado_terminal`, e.g. formalizado/deserto/anulado) re-downloads the detail
  (`scraper::fetch_detail`). Then fetches the **contratos menores** of the same year (perfil API).
  A `THROTTLE` (350 ms) between detail requests; honours the cancel flag. Imports accumulate (you
  add organismos over time); the listing scopes to one organismo via the existing organismo
  filter (empty = all). Both sources store `cod_organismo` = the **organoL id** (`org_id`).

  > **Nota:** este prototipo NON enriquece datos desde fontes externas (datoscif.es).
  > Toda a información provén de contratosdegalicia.gal. As únicas «relacións» son as
  > tramas derivadas da composición das UTE adxudicatarias (ver `db::relacions_compartidas`).

- **`scraper/`** — HTTP client and parsing, one concern per file:
  - `mod.rs` — `Client` (cookie store + session priming on the portal), and the shared
    `decode_bytes` (the site is ISO-8859-1 / Windows-1252) and `clean_text` helpers.
  - `search.rs` — `search_licitaciones`: `POST resultadoIndex.jsp` (HTML, ISO-8859-1) filtered by
    `OR` (organismo) + `YEAR`, parses the hidden `#resSearch` JSON into `ContractSummary`
    (tipo=licitacion), overriding `cod_organismo`/`organismo` with the import's `org_id`/`org_nome`
    so both contract types share the same organismo id.
  - `organismo.rs` — the **JSON API** of an organismo's "perfil do contratante" (UTF-8, parsed
    directly with `serde_json` — NOT `decode_bytes`): `fetch_contratos_menores` (GET
    `api/v1/organismos/{id}/contratosmenores/table`, `length`≤100) returns `MenorRow` — that
    endpoint only accepts short **date windows** (≤~3 months, longer → HTTP 500), so it iterates
    ≤80-day windows over the year, dedup by id. The `{id}` is the `organoL` dropdown value,
    already parsed by `options.rs`. Menores carry their adxudicatario (nome + NIF) in the listing,
    so they need **no detail page**. (`parse_ano` validates the year.)
  - `detail.rs` — `GET licitacion?OP=50&N=<id>`; parses the detail page into
    `ContractDetail` + per-lot `Resolucion` rows. The adxudicatario's **NIF/CIF** is not in the
    main resolution table; it lives in the hidden licitadores/formalización tables (still present
    in the HTML), from which `extract_nif_map` builds a name→NIF map keyed by `company_key`.
    `parse_utes` reads the same licitadores popup for **UTEs** (a `<li>` with a nested `<ul>` of
    `CIF - NAME` members) and returns the **awarded** ones (UTE name matching an adxudicatario) —
    works the same with a single participant. The contract detail view (`render_utes`) reflects
    the UTE composition (members + their CIF, all from the contract page itself).
    When the resolution shows the placeholder **«Múltiples adxudicatarios do procedemento»** (one
    contract awarded to several companies, NOT a UTE), `parse_multiples_adx` reads the hidden
    `ADX_NOM_*` (holds the CIF) / `ADX_CIF_*` (holds the name) inputs and **expands** it into one
    `Resolucion` per real adxudicatario (importe left blank — the portal gives no per-company
    breakdown). They are normal
    co-adxudicatarios: reflected everywhere, but the relations graph never links two adxudicatarios
    just for sharing a contract — only shared UTE membership relates them.
  - `options.rs` — parses the filter dropdowns from `portada.jsp` into `FilterOptions`.

- **`db.rs`** — SQLite schema and queries (`Db`). Tables: `contracts` (listing, normalised:
  a `tipo` column `'licitacion'`|`'menor'`, only `importe_num`, dates as ISO 8601 text,
  `cod_organismo` + `cod_estado` FKs, plus `duracion` used only by menores; `cod_organismo` is
  the **API/organoL id**, NOT the old listado `codOrganismo`. Menores store their adxudicatario as
  one `contract_resolucion` row via `upsert_menores`, so the aggregation and relations work the
  same for both types),
  `organismos` (`cod_organismo` → `nome`, the organism name lives here, loaded via JOIN),
  `estados` (`cod_estado` → `nome`; the server only sends the estado text, so `cod_estado`
  is a surrogate key auto-assigned on first insert of each name; loaded via JOIN),
  `contract_detail` (1:1), `contract_resolucion` (1:N per lot, includes the adxudicatario `nif`),
  plus a `meta` key/value
  table (e.g. `ultima_sync`), and `ute_membro` (the composition of each awarded UTE per contract —
  `ute_key = company_key(ute_nome)`, members as `membro_cif`/`membro_nome` — all parsed from the
  contract detail page, no external source). A SQL function `cokey(x)` (sibling of `nrm`) joins the
  UTE name to its resolution (`cokey(contract_resolucion.adxudicatario) = ute_membro.ute_key`) to
  attribute the UTE's importe. `relacions_compartidas` builds the **tramas purely from shared UTE
  membership**: a member-company node is keyed by its CIF (or `company_key(nome)` when no CIF);
  companies in the same awarded UTE (≥2 members) are linked, union-find groups them across
  contracts, and each awarded-UTE contract is attributed once to the group (members don't
  double-count the importe). Only groups with ≥2 companies surface (`GrupoRelacion.empresas`/
  `.utes`/`.contratos`).
  WAL mode, foreign keys on. The project is a **prototype**: no schema versioning/migration —
  delete the local `.sqlite` to apply schema changes. `upsert_*` are the write paths;
  `query_local` powers the offline Local tab (one row per contract, lotes aggregated;
  importe/date/organismo are derived for display), including search by adxudicatario. The
  listing loads **progressively**: the UI calls `count_local` for the total and
  `query_local_page(offset,limit)` to fetch 8000-row chunks on demand as the virtual table
  scrolls (cached per chunk in `App.row_cache`); `query_local_by_id` fetches a single row for
  jump-to-contract. `query_local` (unbounded) still backs the ODS export.
  `empresas_con_contratos` powers the Empresas tab: it aggregates `contract_resolucion` over the
  filtered contracts (same `local_where` CTE as the relations view), grouping awardees by NIF
  (or `cokey(adxudicatario)` when no NIF) into `EmpresaContratos` (name, NIF, distinct contract
  count, summed importe), ordered by total awarded descending. It also
  flags `LocalRow.participante_unico` (`MAX(participacion) == 1`) so the listing marks single-bidder
  contracts with a ⚠ icon (a possible irregularity signal). The listing is sortable by clicking a
  column header: `LocalFilters.sort_col`/`sort_asc` drive a dynamic `ORDER BY`
  (`SortColumn::order_sql`, fixed expressions; numbers/dates sort by their stored numeric/ISO value).

- **`model.rs`** — all domain types and pure helpers. Notable: `TipoContrato` (`Licitacion`/
  `Menor`); `ContractSummary` (contract-write DTO, also produced by `search.rs`); `MenorRow`
  (serde row from the menores JSON API); `ImportParams` (organismo + year); `parse_importe` / `format_importe` convert between Galician-formatted
  amounts (`1.000.000,00 €`) and `f64`; `parse_data` / `format_data_gl` convert between portal
  dates, ISO 8601, and `DD/MM/YYYY` display; `is_estado_terminal` drives the incremental-import
  skip decision. Name/NIF helpers: `company_key` (normalised comparison key — feeds `upsert_utes`
  and the `cokey` SQL function for UTE importe attribution) and `normalize_nif` (uppercase
  alphanumerics, used by `detail.rs` to compare CIFs). The UTE/relations types live here too:
  `Ute`/`UteMembro` (contract data), and `EmpresaNodo`/`UteRelacion`/`ContratoAdxudicado`/
  `GrupoRelacion` (the UTE-only trama returned by `db::relacions_compartidas`).
  `EmpresaContratos` (name, NIF, contract count, total awarded) backs the Empresas tab via
  `db::empresas_con_contratos`.

- **`export.rs`** — writes the filtered local rows to an OpenDocument Spreadsheet (`.ods`)
  via `spreadsheet-ods`.

- **`theme.rs`** — Apple-HIG-inspired styling; bundles the Inter font with `include_bytes!`.

## Domain notes

- The whole **contratosdegalicia.gal** site is **ISO-8859-1**; always decode response bytes
  through `scraper::decode_bytes` rather than treating them as UTF-8. The organismo perfil JSON
  API (`organismo.rs`) is the exception: it's UTF-8, parsed directly with `serde_json`.
- This prototype does **not** enrich data from external sources (e.g. datoscif.es); all data
  comes from contratosdegalicia.gal, and the only relations are the UTE-membership tramas.
- The resolution PDF is reCAPTCHA-protected and is **not** downloaded; only the public
  resolution URL (`enlace_resolucion`) is stored.
- Parsing tests load real captured pages from `tests/fixtures/*.html` via `include_str!` /
  `include_bytes!` from inside the `scraper` modules — update fixtures if the site's markup changes.

## License note

The Inter font (`assets/fonts/`) is under the SIL Open Font License
(`assets/fonts/Inter-OFL.txt`).
