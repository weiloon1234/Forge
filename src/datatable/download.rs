use std::collections::HashMap;

use serde::Serialize;

use crate::foundation::{AppContext, Error, Result};

use super::column::DatatableColumn;
use super::context::DatatableContext;
use super::datatable_trait::{Datatable, DatatableQuery};
use super::mapping::DatatableMapping;

/// Build an XLSX download response for a datatable.
///
/// Executes the full scoped + filtered query (no pagination) and writes
/// results into an XLSX workbook via `rust_xlsxwriter`.
pub async fn build_download_response<D>(
    app: &AppContext,
    actor: Option<&crate::auth::Actor>,
    request: super::request::DatatableRequest,
) -> Result<axum::response::Response>
where
    D: Datatable + ?Sized,
    D::Row: Serialize,
{
    let bytes = build_xlsx_bytes::<D>(app, actor, request).await?;

    let filename = format!("{}.xlsx", D::ID);
    axum::response::Response::builder()
        .header(
            "Content-Type",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .header(
            "Content-Disposition",
            format!("attachment; filename=\"{filename}\""),
        )
        .body(axum::body::Body::from(bytes))
        .map_err(|e| Error::message(format!("failed to build download response: {e}")))
}

/// Generate XLSX bytes from a datatable query (no pagination).
///
/// Shared between the download endpoint and the export job.
pub(super) async fn build_xlsx_bytes<D>(
    app: &AppContext,
    actor: Option<&crate::auth::Actor>,
    request: super::request::DatatableRequest,
) -> Result<Vec<u8>>
where
    D: Datatable + ?Sized,
    D::Row: Serialize,
{
    let ctx = DatatableContext::new(app, actor, &request);

    let columns = D::columns();
    let query = super::query_pipeline::prepare_query::<D>(&ctx, &columns).await?;

    let db = app.database()?;
    let data = query.get(db.as_ref()).await?;

    let exportable_columns: Vec<&DatatableColumn<D::Row>> =
        columns.iter().filter(|c| c.exportable).collect();
    let mappings = D::mappings();

    build_xlsx(&data, &exportable_columns, &mappings, &ctx)
}

fn build_xlsx<Row>(
    data: &crate::support::Collection<Row>,
    columns: &[&DatatableColumn<Row>],
    mappings: &[DatatableMapping<Row>],
    ctx: &DatatableContext,
) -> Result<Vec<u8>>
where
    Row: Serialize,
{
    use rust_xlsxwriter::{Format, Workbook};

    let mapping_index: HashMap<&str, &DatatableMapping<Row>> =
        mappings.iter().map(|m| (m.name.as_str(), m)).collect();

    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();

    let header_format = Format::new().set_bold();

    for (col_idx, col) in columns.iter().enumerate() {
        worksheet
            .write_string_with_format(0, col_idx as u16, &col.label, &header_format)
            .map_err(|e| Error::message(format!("xlsx write error: {e}")))?;
    }

    for (row_idx, row) in data.iter().enumerate() {
        let row_index = (row_idx + 1) as u32;

        let row_value = serde_json::to_value(row)
            .map_err(|e| Error::message(format!("failed to serialize row: {e}")))?;
        let obj = match &row_value {
            serde_json::Value::Object(obj) => obj,
            _ => continue,
        };

        for (col_idx, col) in columns.iter().enumerate() {
            let col_pos = col_idx as u16;

            let value = if let Some(mapping) = mapping_index.get(col.name.as_str()) {
                mapping.compute(row, ctx).into()
            } else {
                obj.get(&col.name)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            };

            write_cell(worksheet, row_index, col_pos, &value)
                .map_err(|e| Error::message(format!("xlsx write error: {e}")))?;
        }
    }

    for col_idx in 0..columns.len() {
        worksheet
            .set_column_width(col_idx as u16, 15)
            .map_err(|e| Error::message(format!("xlsx format error: {e}")))?;
    }

    let buf = workbook
        .save_to_buffer()
        .map_err(|e| Error::message(format!("xlsx save error: {e}")))?;

    Ok(buf)
}

fn write_cell(
    worksheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    value: &serde_json::Value,
) -> std::result::Result<(), rust_xlsxwriter::XlsxError> {
    match value {
        serde_json::Value::Null => worksheet.write_string(row, col, ""),
        serde_json::Value::Bool(b) => worksheet.write_boolean(row, col, *b),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                worksheet.write_number(row, col, f)
            } else {
                worksheet.write_string(row, col, n.to_string())
            }
        }
        serde_json::Value::String(s) => worksheet.write_string(row, col, s),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            worksheet.write_string(row, col, value.to_string())
        }
    }?;
    Ok(())
}
