//! Exportación a OpenDocument Spreadsheet (.ods).

use crate::model::{LocalRow, parse_importe};
use anyhow::{Context, Result};
use spreadsheet_ods::{Sheet, WorkBook, write_ods};
use std::path::Path;

const CABECEIRAS: [&str; 10] = [
    "ID",
    "Referencia",
    "Obxecto",
    "Importe",
    "Estado",
    "Data publicación",
    "Organismo",
    "Adxudicatario",
    "Importe resolución",
    "Enlace resolución",
];

/// Exporta as filas dadas a un ficheiro .ods (unha fila por contrato, cos
/// adxudicatarios agregados e o importe total adxudicado).
pub fn export_ods(path: &Path, rows: &[LocalRow]) -> Result<()> {
    let mut wb = WorkBook::new_empty();
    let mut sheet = Sheet::new("Contratos");

    for (c, h) in CABECEIRAS.iter().enumerate() {
        sheet.set_value(0, c as u32, *h);
    }

    for (i, r) in rows.iter().enumerate() {
        let row = (i + 1) as u32;
        sheet.set_value(row, 0, r.id.as_str());
        sheet.set_value(row, 1, r.referencia.as_str());
        sheet.set_value(row, 2, r.asunto.as_str());
        match parse_importe(&r.importe_txt) {
            Some(n) => sheet.set_value(row, 3, n),
            None => sheet.set_value(row, 3, r.importe_txt.as_str()),
        }
        sheet.set_value(row, 4, r.estado.as_str());
        sheet.set_value(row, 5, r.publicacion.as_str());
        sheet.set_value(row, 6, r.organismo.as_str());
        sheet.set_value(row, 7, r.adxudicatario.as_str());
        match parse_importe(&r.importe_resolucion_txt) {
            Some(n) => sheet.set_value(row, 8, n),
            None => sheet.set_value(row, 8, r.importe_resolucion_txt.as_str()),
        }
        sheet.set_value(row, 9, r.enlace_resolucion.as_str());
    }

    wb.push_sheet(sheet);
    write_ods(&mut wb, path).context("escribindo o ficheiro ODS")?;
    Ok(())
}
