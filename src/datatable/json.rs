use std::collections::HashMap;

use serde::Serialize;

use crate::database::{Model, Pagination};
use crate::foundation::{AppContext, Error, Result};

use super::column::DatatableColumn;
use super::context::DatatableContext;
use super::datatable_trait::ModelDatatable;
use super::mapping::DatatableMapping;
use super::response::{DatatableColumnMeta, DatatableJsonResponse, DatatablePaginationMeta};

/// Build a paginated JSON response for a model-backed datatable.
pub async fn build_json_response<M, D>(
    app: &AppContext,
    actor: Option<&crate::auth::Actor>,
    request: super::request::DatatableRequest,
) -> Result<DatatableJsonResponse>
where
    M: Model + Serialize,
    D: ModelDatatable<Model = M> + ?Sized,
{
    let ctx = DatatableContext::new(app, actor, &request);

    // 1-4. Build scoped + filtered + sorted query
    let columns = D::columns();
    let query = super::query_pipeline::prepare_query::<M, D>(&ctx, &columns).await?;

    // 5. Paginate
    let pagination = Pagination::new(request.page, request.per_page);
    let db = app.database()?;
    let paginated = query.paginate(db.as_ref(), pagination).await?;

    // 6. Build rows
    let mappings = D::mappings();
    let rows = build_rows(&paginated.data, &columns, &mappings, &ctx)?;

    // 7. Build column metadata
    let column_meta: Vec<DatatableColumnMeta> = columns
        .iter()
        .map(|c| DatatableColumnMeta {
            name: c.name.clone(),
            label: c.label.clone(),
            sortable: c.sortable,
            filterable: c.filterable,
        })
        .collect();

    // 8. Get available filters
    let filters = D::available_filters(&ctx).await?;

    // 9. Build pagination meta
    let pagination_meta = DatatablePaginationMeta::new(
        paginated.pagination.page,
        paginated.pagination.per_page,
        paginated.total,
    );

    Ok(DatatableJsonResponse {
        rows,
        columns: column_meta,
        filters,
        pagination: pagination_meta,
        applied_filters: request.filters,
        sorts: request.sort,
    })
}

fn build_rows<M>(
    data: &crate::support::Collection<M>,
    columns: &[DatatableColumn<M>],
    mappings: &[DatatableMapping<M>],
    ctx: &DatatableContext,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>>
where
    M: Model + Serialize,
{
    let mapping_index: HashMap<&str, &DatatableMapping<M>> =
        mappings.iter().map(|m| (m.name.as_str(), m)).collect();

    let mut rows = Vec::with_capacity(data.len());

    for model in data.iter() {
        let mut map = serde_json::Map::new();

        let model_value = serde_json::to_value(model)
            .map_err(|e| Error::message(format!("failed to serialize model: {e}")))?;

        if let serde_json::Value::Object(obj) = model_value {
            for col in columns {
                // Skip columns overridden by a mapping
                if let Some(mapping) = mapping_index.get(col.name.as_str()) {
                    let value: serde_json::Value = mapping.compute(model, ctx).into();
                    map.insert(col.name.clone(), value);
                } else if let Some(val) = obj.get(&col.name) {
                    map.insert(col.name.clone(), val.clone());
                }
            }
        }

        // Add mapping-only fields (not in columns)
        for mapping in mappings {
            if !map.contains_key(&mapping.name) {
                let value: serde_json::Value = mapping.compute(model, ctx).into();
                map.insert(mapping.name.clone(), value);
            }
        }

        rows.push(map);
    }

    Ok(rows)
}
