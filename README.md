# Congal

Aplicación de escritorio (Windows / macOS / Linux) escrita en **Rust** para extraer,
almacenar, analizar e exportar a información de contratos públicos da Xunta de Galicia desde
[contratosdegalicia.gal](https://www.contratosdegalicia.gal).

Ademais de descargar e gardar os contratos, Congal revela **tramas**: razóns sociais
conectadas por formar parte dunha mesma UTE (Unión Temporal de Empresas) adxudicataria. Toda a
información provén de contratosdegalicia.gal.

## Características

### Interface
- **Interface gráfica** (egui) con estilo inspirado nas *Apple Human Interface Guidelines*
  (fonte Inter, iconas Lucide, modo claro/escuro automático, acento azul sistema).
- A interface **nunca se bloquea**: as operacións de rede e de disco execútanse nun fío de
  traballo en segundo plano e a vista actualízase cos eventos de progreso.
- Dúas pestanas: **Contratos** e **Relacións**.

### Contratos
- **Listado local** dos contratos xa importados, con busca sen rede (texto,
  **adxudicatario**, organismo, estado, ano) e panel de detalle por contrato. As buscas son
  insensibles a maiúsculas e a acentos.
- O listado é **ordenable** premendo na cabeceira de calquera columna, e marca cun ⚠ os
  contratos cun **único licitador** (posible indicio de irregularidade).
- De cada contrato gárdanse os datos do listado, o detalle completo e os **datos da
  resolución** (adxudicatario, importe, estado por lote e enlace á resolución).
- No caso das **UTE**, o detalle amosa a súa composición (empresas membro co seu CIF).

### Importación de datos
- Diálogo modal cos mesmos filtros ca a web (estado, ano, órgano de contratación, busca
  textual, tipo de contrato / procedemento / tramitación, sistema, materia CPV); o ano
  preséntase nun despregable co ano actual preseleccionado.
- **Importación incremental**: os contratos xa resoltos (estado terminal) non se volven
  descargar; só se actualizan os que seguían en proceso e os novos, mantendo o histórico.
- Barra de progreso nun segundo modal, **cancelable** en calquera momento.

### Relacións
- A pestana **Relacións** amosa as **tramas**: grupos de razóns sociais que concorreron xuntas
  nunha mesma UTE adxudicataria. Se unha empresa participa en varias UTE, todas as súas socias
  caen na mesma trama.
- Cada grupo amosa as empresas membro, as UTE que as vinculan e un despregable cos contratos
  adxudicados (e o importe total). As relacións derívanse unicamente dos datos de
  contratosdegalicia.gal (a composición das UTE), sen recorrer a fontes externas.

### Exportación
- **Exportación a OpenDocument Spreadsheet (.ods)** da selección filtrada.

### Almacenamento
- Todo se persiste localmente en **SQLite** (`rusqlite`, incluído de serie).

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
cargo test                       # probas unitarias (parseo sobre páxinas reais, sen rede)
cargo test importe_galego        # unha única proba polo seu nome
cargo test --release -- --ignored --nocapture live_end_to_end   # proba en vivo (require rede)
```

A proba `live_end_to_end` ignórase por defecto: require acceso a rede e exercita un ciclo
completo busca → detalle → BD → exportación ODS contra un contrato coñecido.

## Integración continua

```bash
./script/ci.sh                   # corre en local as mesmas comprobacións que CI
```

Reproduce o pipeline de GitHub Actions (`.github/workflows/ci.yml`): `cargo fmt --check`,
`cargo clippy` con `-D warnings`, build en release e probas.

## Onde se gardan os datos

A base de datos `contratos.sqlite` créase no cartafol de datos do usuario
(`directories::ProjectDirs("gal","congal","congal")`), p.ex.
`~/.local/share/congal/contratos.sqlite` en Linux.

> **Nota:** o proxecto é un prototipo e non ten versionado nin migración de esquema. Se cambia o
> esquema, elimina o `.sqlite` local para aplicar os cambios.

## Notas técnicas

- O listado obtense por `POST resultadoIndex.jsp`, que devolve todos os resultados nun array
  JSON oculto (`#resSearch`); a páxina web pagina no cliente.
- O detalle obtense por `GET licitacion?OP=50&N=<id>`. O NIF/CIF do adxudicatario non está na
  táboa principal da resolución: lese das táboas ocultas de licitadores/formalización, das que
  tamén se extraen as UTE.
- O documento PDF da resolución está protexido por reCAPTCHA e **non** se descarga; gárdase só a
  URL pública da resolución.
- Todo o sitio **contratosdegalicia.gal** está en **ISO-8859-1**; o contido decodifícase
  explicitamente. A excepción é a API JSON do perfil do contratante (contratos menores), que vai
  en UTF-8.

## Licenzas de terceiros

A fonte **Inter** distribúese baixo a SIL Open Font License (ver `assets/fonts/Inter-OFL.txt`).

As iconas **Lucide** (`assets/fonts/lucide.ttf`) distribúense baixo a licenza ISC
(<https://lucide.dev/license>).
