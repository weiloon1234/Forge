use crate::foundation::Result;

use super::column::DatatableColumn;
use super::context::DatatableContext;
use super::datatable_trait::Datatable;
use super::filter_engine::{
    apply_auto_filters_with_relation_filters, apply_default_sorts, apply_sorts,
};

/// Shared query-build pipeline used by both JSON and download modes.
///
/// Steps: scoped base query -> auto-filters -> custom filter hook -> sorting.
pub async fn prepare_query<D>(
    ctx: &DatatableContext<'_>,
    columns: &[DatatableColumn<D::Row>],
) -> Result<D::Query>
where
    D: Datatable + ?Sized,
{
    let query = D::query(ctx);
    let relation_filters = D::relation_filters();
    let query = apply_auto_filters_with_relation_filters(
        query,
        &ctx.request.filters,
        columns,
        &relation_filters,
    )?;
    let query = D::filters(ctx, query).await?;

    if ctx.request.sort.is_empty() {
        apply_default_sorts(query, &D::default_sort())
    } else {
        apply_sorts(query, &ctx.request.sort, columns)
    }
}
