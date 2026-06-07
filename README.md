# Congal

Aplicación de escritorio (Windows / macOS / Linux) escrita en **Rust** para extraer,
almacenar, analizar e exportar a información de contratos públicos da Xunta de Galicia desde
[contratosdegalicia.gal](https://www.contratosdegalicia.gal).

Ademais de descargar e gardar os contratos, Congal **enriquece** cada adxudicatario coa
información societaria de [datoscif.es](https://www.datoscif.es) (CIF, domicilio, cargos) e
revela **tramas**: razóns sociais conectadas por administradores compartidos ou por formar parte
da mesma UTE.

## Características

### Interface
- **Interface gráfica** (egui) con estilo inspirado nas *Apple Human Interface Guidelines*
  (fonte Inter, iconas Lucide, modo claro/escuro automático, acento azul sistema).
- A interface **nunca se bloquea**: as operacións de rede e de disco execútanse nun fío de
  traballo en segundo plano e a vista actualízase cos eventos de progreso.
- Tres pestanas: **Contratos**, **Relacións** e **Revisión**.

### Contratos
- **Listado local** dos contratos xa importados, con busca sen rede (texto,
  **adxudicatario**, organismo, estado, ano) e panel de detalle por contrato. As buscas son
  insensibles a maiúsculas e a acentos.
- O listado é **ordenable** premendo na cabeceira de calquera columna, e marca cun ⚠ os
  contratos cun **único licitador** (posible indicio de irregularidade).
- De cada contrato gárdanse os datos do listado, o detalle completo e os **datos da
  resolución** (adxudicatario, importe, estado por lote e enlace á resolución).
- O detalle amosa a información societaria do adxudicatario obtida de datoscif (ficha e cargos)
  e, no caso das **UTE**, a súa composición (membros + a ficha de cada un).

### Importación de datos
- Diálogo modal cos mesmos filtros ca a web (estado, ano, órgano de contratación, busca
  textual, tipo de contrato / procedemento / tramitación, sistema, materia CPV); o ano
  preséntase nun despregable co ano actual preseleccionado.
- **Importación incremental**: os contratos xa resoltos (estado terminal) non se volven
  descargar; só se actualizan os que seguían en proceso e os novos, mantendo o histórico.
- Barra de progreso nun segundo modal, **cancelable** en calquera momento.

### Enriquecemento e relacións (datoscif)
- O botón **«Importar relacións»** empareza cada adxudicatario coa súa entidade en datoscif.
  Cando hai un gañador claro (ou se valida o CIF) o vínculo créase automaticamente; cando hai
  candidatos plausibles pero ningún seguro, o caso vai á **cola de revisión manual**.
- A coincidencia valídase preferentemente polo **CIF** (o sinal máis fiable): aínda que
  datoscif só permite buscar por nome, o CIF do adxudicatario serve para confirmar o candidato
  correcto.
- Tamén se emparellan os **membros das UTE** adxudicatarias, que se incorporan ao grafo de
  relacións.
- O botón **«Reimportar datos das empresas»** volve descargar a ficha e os cargos das empresas
  xa vinculadas para actualizar a información.
- A pestana **Relacións** amosa as **tramas**: persoas que controlan varias razóns sociais
  contratadas (administradores compartidos) e empresas unidas por formar parte dunha mesma UTE.
  Os vínculos baseados só en cargos **pasados/cesados** resáltanse como indicio non actual.

### Revisión manual
- A pestana **Revisión** xestiona os casos que o emparellamento automático non puido resolver
  con seguridade: casos con candidatos e unha sección de «sen correspondencia».
- Cada caso inclúe unha **busca asistida en vivo en datoscif** para atopar e vincular a man a
  entidade correcta (ou descartar o caso).

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
  explicitamente. Por contra, as APIs de **datoscif.es** devolven **JSON en UTF-8** (pero a
  petición de busca debe enviarse codificada en ISO-8859-1).
- A busca en datoscif está anclada ao prefixo do nome e a información societaria das fichas lese
  da microdata schema.org (`taxID`, `streetAddress`, etc.).

## Licenzas de terceiros

A fonte **Inter** distribúese baixo a SIL Open Font License (ver `assets/fonts/Inter-OFL.txt`).

As iconas **Lucide** (`assets/fonts/lucide.ttf`) distribúense baixo a licenza ISC
(<https://lucide.dev/license>).
