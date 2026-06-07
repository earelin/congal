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

- **`app.rs`** — all egui UI, single `App` struct holding the full UI state. Import runs from a
  modal dialog; the main view has three tabs: `Contratos` (local listing + contract detail),
  `Relacions` (tramas) and `Revision` (manual review queue: candidate-based cases plus a
  collapsible "sen correspondencia" section, each with an **assisted live datoscif search** —
  `Command::SearchDatoscif` → `Event::DatoscifResults` — to find and link a match by hand). The
  UI **never blocks on I/O**: it sends `Command`s to the worker and drains `Event`s each frame.

- **`worker.rs`** — the concurrency boundary. `Worker::spawn` starts one background thread
  owning the HTTP `Client` and the `Db`. Communication is two `mpsc` channels (`Command`
  in, `Event` out) plus an `Arc<AtomicBool>` cancel flag. Long operations report progress
  via `Event::SyncProgress` and call `ctx.request_repaint()` to wake the UI.

- **`sync.rs`** — incremental sync logic. After a search, contracts whose stored state is
  **terminal** (`is_estado_terminal`, e.g. formalizado/deserto/anulado) are skipped; only
  new or still-in-progress contracts get their detail re-downloaded. A `THROTTLE`
  (350 ms) is applied between detail requests; honours the cancel flag.

- **`enrich.rs`** — `Command::Enrich(EnrichMode)` flow (manual "Importar relacións" button).
  Two modes: `Novos` (process each adxudicatario not yet in `adxudicatario_match`) and
  `Reimportar` (re-download the ficha + cargos of every already-linked empresa to refresh data,
  offered as a button in the result dialog). The matching core is `decide_match`: it searches
  datoscif by name variants (`model::match_suggestions`); if there's no single clear winner and
  the resolution gave us the adxudicatario's **NIF/CIF** (`db::nif_de_adxudicatario`), it retries
  with a reduced search term (`fallback_search_term`) and **validates by CIF** — fetching each
  candidate empresa's ficha and matching `taxID` against the NIF (`Confianza::Cif`, the most
  reliable signal). For matched **empresas** it downloads their cargos. `decide_match` returns a
  `Decision`: `Auto` (link, high-confidence/CIF-validated), `Revisar` (no reliable link but
  **plausible candidates** exist → saved with `estado='revisar'` plus the candidates, for the
  manual review queue) or `SenMatch` (nothing → `pendente`). `Command::ResolveMatch` /
  `enrich::vincular_manual` resolves a queued case: links the chosen entity (downloading its
  ficha + cargos like an auto-link) or discards it, then clears the case's candidates. After the
  adxudicatarios, it also matches the **members of awarded UTEs** (`db::ute_membros_pendentes`):
  `match_member` searches by the (truncated) member name and **requires a CIF match**, then stores
  the member's ficha + cargos so it joins the relations graph. The **UTE name itself is never
  searched** in datoscif (it isn't there) — `adxudicatarios_pendentes` and the review queues skip
  UTEs (`cokey` in `ute_membro`, or a UTE NIF `U…`), and `upsert_utes` clears any UTE row a prior
  enrich may have left in `adxudicatario_match`. Same `THROTTLE`/cancel pattern as
  `sync.rs`. (Note: datoscif has **no CIF search** — `sugerencias.ajax` is name-prefix only; the
  CIF only serves to validate name candidates.)

- **`scraper/`** — HTTP client and parsing, one concern per file:
  - `mod.rs` — `Client` (cookie store + session priming on the portal), and the shared
    `decode_bytes` (the site is ISO-8859-1 / Windows-1252) and `clean_text` helpers.
  - `search.rs` — `POST resultadoIndex.jsp`; the server returns ALL results in a hidden
    JSON array (`#resSearch`), the web paginates client-side. Returns `ContractSummary`.
  - `detail.rs` — `GET licitacion?OP=50&N=<id>`; parses the detail page into
    `ContractDetail` + per-lot `Resolucion` rows. The adxudicatario's **NIF/CIF** is not in the
    main resolution table; it lives in the hidden licitadores/formalización tables (still present
    in the HTML), from which `extract_nif_map` builds a name→NIF map keyed by `company_key`.
    `parse_utes` reads the same licitadores popup for **UTEs** (a `<li>` with a nested `<ul>` of
    `CIF - NAME` members) and returns the **awarded** ones (UTE name matching an adxudicatario) —
    works the same with a single participant. The contract detail view (`render_utes`) reflects
    the UTE composition (members + their datoscif ficha/cargos, resolved by CIF via
    `db::entidade_por_cif`).
    When the resolution shows the placeholder **«Múltiples adxudicatarios do procedemento»** (one
    contract awarded to several companies, NOT a UTE), `parse_multiples_adx` reads the hidden
    `ADX_NOM_*` (holds the CIF) / `ADX_CIF_*` (holds the name) inputs and **expands** it into one
    `Resolucion` per real adxudicatario (importe left blank — the portal gives no per-company
    breakdown). They are normal
    co-adxudicatarios: reflected everywhere and matched individually, but the relations graph never
    links two adxudicatarios just for sharing a contract — only a shared admin or a UTE relates them.
    `search_entities` follows datoscif's `url_new`/`nombre_new` (renamed entities) to the current
    slug, so the ficha CIF/cargos are found even after a rename.
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
  `contract_detail` (1:1), `contract_resolucion` (1:N per lot, includes the adxudicatario `nif`),
  plus a `meta` key/value
  table (e.g. `ultima_sync`). The datoscif enrichment adds `datoscif_entidade`
  (empresa/persoa by canonical slug, plus the company ficha: cif/domicilio/cod_postal/
  municipio/provincia), `datoscif_cargo` (persona→empresa role + validity),
  `adxudicatario_match` (keyed by `company_key` of the adxudicatario → datoscif entity,
  with `confianza`/`estado` ∈ auto/pendente/revisar/manual/descartado), and
  `adxudicatario_candidato` (candidates stored for `revisar` cases; `casos_para_revisar` reads
  the queue, `casos_sen_match` the `pendente` ones for the assisted search), and `ute_membro`
  (members of awarded UTEs per contract; members resolve to datoscif by CIF =
  `datoscif_entidade.cif`). A SQL function `cokey(x)` (sibling of `nrm`) joins
  `contract_resolucion.adxudicatario` to `adxudicatario_match`. `relacions_compartidas`
  finds people holding a cargo in ≥2 contracted razóns sociais; each person↔empresa edge keeps
  an `activo` flag (`MAX(cargo.activo)`) so the UI can flag **historical** links — a company tied
  to a trama only by past/cesados cargos (`EmpresaNodo.activa = false`), and per person the
  active vs past company counts. A past tie is still shown (it's an indicator) but marked as not
  current. Besides shared-admin edges, **UTE membership** adds empresa↔empresa edges: the members
  of an awarded UTE form a trama on their own (`GrupoRelacion.utes`; `EmpresaNodo.con_cargos`
  distinguishes a UTE-only link from a historical-cargo one). Awarded-UTE contracts are attributed
  once to the group (members don't double-count the importe).
  WAL mode, foreign keys on. The project is a **prototype**: no schema versioning/migration —
  delete the local `.sqlite` to apply schema changes. `upsert_*` are the write paths;
  `query_local` powers the offline Local tab (one row per contract, lotes aggregated;
  importe/date/organismo are derived for display), including search by adxudicatario. It also
  flags `LocalRow.participante_unico` (`MAX(participacion) == 1`) so the listing marks single-bidder
  contracts with a ⚠ icon (a possible irregularity signal). The listing is sortable by clicking a
  column header: `LocalFilters.sort_col`/`sort_asc` drive a dynamic `ORDER BY`
  (`SortColumn::order_sql`, fixed expressions; numbers/dates sort by their stored numeric/ISO value).

- **`model.rs`** — all domain types and pure helpers. Notable: `EstadoGroup` maps the four
  UI status checkboxes to the numeric `ESTADO` codes the server expects; `parse_importe` /
  `format_importe` convert between Galician-formatted amounts (`1.000.000,00 €`) and `f64`;
  `parse_data` / `format_data_gl` convert between portal dates, ISO 8601, and `DD/MM/YYYY`
  display; `is_estado_terminal` drives the incremental-sync skip decision. The datoscif
  matching helpers live here too: `company_key`/`company_core`/`strip_legal_suffix`
  (normalise names, drop SL/SA… suffixes), `search_variants` (reorders person names
  «nome apelido1 apelido2» → «apelido1 apelido2 nome» for datoscif's substring search),
  `match_suggestions` (Exacta > Núcleo > Tokens; returns `Unico`/`Ambiguo(candidatos)`/`Ningun`),
  `fallback_search_term` (núcleo without legal suffix nor 1-2-letter words, for retry), and the
  NIF helpers `normalize_nif`/`nif_kind` (CIF→empresa, NIF/NIE→persoa). `Confianza` includes
  `Cif` (highest) for CIF-validated matches.

- **`export.rs`** — writes the filtered local rows to an OpenDocument Spreadsheet (`.ods`)
  via `spreadsheet-ods`.

- **`theme.rs`** — Apple-HIG-inspired styling; bundles the Inter font with `include_bytes!`.

## Domain notes

- The whole **contratosdegalicia.gal** site is **ISO-8859-1**; always decode response bytes
  through `scraper::decode_bytes` rather than treating them as UTF-8. By contrast,
  **datoscif.es** returns **JSON in UTF-8** — parse it directly with `serde_json`. **But** the
  `sugerencias.ajax` **request** (`nombre`) must be sent **ISO-8859-1**-encoded (ñ = `%F1`, not
  UTF-8 `%C3%91`), or accented/ñ names return `[]`; `search_entities` builds the body manually
  with `form_encode_latin1`.
- datoscif search is **prefix-anchored** (matches from the start of the stored name, not an
  arbitrary substring) and **folds acute accents (á→a) but keeps the ñ**. So search terms use
  `normalize_busca_datoscif` (keeps ñ, unlike `company_key`/`normalize_search` which fold ñ→n for
  *matching*). Person names there are stored "apelido1 apelido2 nome" while contracts use "nome
  apelido1 apelido2" — hence `search_variants` reorders.
- The resolution PDF is reCAPTCHA-protected and is **not** downloaded; only the public
  resolution URL (`enlace_resolucion`) is stored.
- Parsing tests load real captured pages from `tests/fixtures/*.html` (and datoscif
  `tests/fixtures/datoscif_*.json`) via `include_str!` / `include_bytes!` from inside the
  `scraper` modules — update fixtures if the sites' markup/JSON changes.

## License note

The Inter font (`assets/fonts/`) is under the SIL Open Font License
(`assets/fonts/Inter-OFL.txt`).
