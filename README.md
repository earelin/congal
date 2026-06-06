# congal

Aplicación de escritorio (Windows / macOS / Linux) escrita en **Rust** para extraer,
almacenar e exportar a información de contratos públicos da Xunta de Galicia desde
[contratosdegalicia.gal](https://www.contratosdegalicia.gal).

## Características

- **Interface gráfica** (egui) con estilo inspirado nas *Apple Human Interface Guidelines*
  (fonte Inter, modo claro/escuro automático, acento azul sistema).
- Dúas áreas de traballo:
  - **Scraper / Sincronización**: busca cos mesmos filtros ca a web (estado, ano, órgano de
    contratación, busca textual, tipo de contrato / procedemento / tramitación, sistema,
    materia CPV) e descarga o detalle de cada contrato.
  - **Traballo local**: busca sobre a base de datos sen rede, incluída a **busca por
    adxudicatario**, e exportación a folla de cálculo.
- **Sincronización incremental**: os contratos xa resoltos non se volven descargar; só se
  actualizan os que seguían en proceso e os novos (mantense o histórico).
- Almacenamento local en **SQLite**.
- De cada contrato gárdanse os datos do listado, o detalle completo e os **datos da
  resolución** (adxudicatario, importe da resolución, estado por lote e enlace á resolución).
- **Exportación a OpenDocument Spreadsheet (.ods)** da selección filtrada.

## Compilación

Requírese Rust estable (edición 2024).

```bash
cargo build --release
./target/release/congal
```

Non hai dependencias de sistema: SQLite vai incluído (`rusqlite/bundled`), o TLS é `rustls`
e a interface non precisa *webview*.

## Probas

```bash
cargo test                       # probas unitarias (parseo sobre páxinas reais)
cargo test --release -- --ignored --nocapture live_end_to_end   # proba en vivo (require rede)
```

## Onde se gardan os datos

A base de datos `contratos.sqlite` créase no cartafol de datos do usuario
(`directories::ProjectDirs`), p.ex. `~/.local/share/congal/` en Linux.

## Notas técnicas

- O listado obtense por `POST resultadoIndex.jsp`, que devolve todos os resultados nun array
  JSON oculto (`#resSearch`); a páxina web pagina no cliente.
- O detalle obtense por `GET licitacion?OP=50&N=<id>`. O documento PDF da resolución está
  protexido por reCAPTCHA e non se descarga automaticamente; no seu lugar gárdase a URL
  pública da resolución.
- Todo o sitio está en ISO-8859-1; o contido decodifícase explicitamente.

## Licenzas de terceiros

A fonte **Inter** distribúese baixo a SIL Open Font License (ver `assets/fonts/Inter-OFL.txt`).
