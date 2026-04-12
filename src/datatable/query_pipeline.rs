use serde::Serialize;

use crate::database::{Model, ModelQuery};
use crate::foundation::Result;

use super::column::DatatableColumn;
use super::context::DatatableContext;
use super::datatable_trait::ModelDatatable;
use super::filter_engine::{apply_auto_filters, apply_default_sorts, apply_sorts};

/// Shared query-build pipeline used by both JSON and download modes.
///
/// Steps: scoped base query → auto-filters → custom filter hook → sorting.
pub async fn prepare_query<M, D>(
    ctx: &DatatableContext<'_>,
    columns: &[DatatableColumn<M>],
) -> Result<ModelQuery<M>>
where
    M: Model + Serialize,
    D: ModelDatatable<Model = M> + ?Sized,
{
    let table_name = M::table_meta().name();

    // 1. Scoped base query
    let query = D::query(ctx);

    // 2. Auto-filter
    let query = apply_auto_filters(query, &ctx.request.filters, columns, table_name)?;

    // 3. Custom filter hook
    let query = D::filters(ctx, query).await?;

    // 4. Sorting
    let query = if ctx.request.sort.is_empty() {
        apply_default_sorts(query, &D::default_sort(), table_name)?
    } else {
        apply_sorts(query, &ctx.request.sort, columns, table_name)?
    };

    Ok(query)
}
