use async_trait::async_trait;
use serde::Serialize;

use crate::auth::Actor;
use crate::database::{Model, ModelQuery, ProjectionQuery};
use crate::foundation::{AppContext, Result};

use super::column::DatatableColumn;
use super::context::DatatableContext;
use super::filter_meta::DatatableFilterRow;
use super::mapping::DatatableMapping;
use super::request::DatatableRequest;
use super::response::DatatableExportAccepted;
use super::response::DatatableJsonResponse;
use super::sort::DatatableSort;

// ---------------------------------------------------------------------------
// ModelDatatable — primary path
// ---------------------------------------------------------------------------

#[async_trait]
pub trait ModelDatatable: Send + Sync + 'static {
    type Model: Model + Serialize;

    const ID: &'static str;

    /// Base scoped query. Receives context so the implementor can scope
    /// by actor, tenant, or any other contextual constraint.
    fn query(ctx: &DatatableContext) -> ModelQuery<Self::Model>;

    /// Declared columns that participate in rendering, filtering, sorting, export.
    fn columns() -> Vec<DatatableColumn<Self::Model>>;

    /// Output-only computed fields. Mappings override columns with the same name.
    fn mappings() -> Vec<DatatableMapping<Self::Model>> {
        Vec::new()
    }

    /// Custom filter hook. Receives the query after auto-filters are applied
    /// so the implementor can add further refinements.
    async fn filters(
        _ctx: &DatatableContext,
        query: ModelQuery<Self::Model>,
    ) -> Result<ModelQuery<Self::Model>> {
        Ok(query)
    }

    /// Frontend filter metadata (controls, labels, options).
    async fn available_filters(
        _ctx: &DatatableContext,
    ) -> Result<Vec<DatatableFilterRow>> {
        Ok(Vec::new())
    }

    /// Default sort when no sort is specified in the request.
    fn default_sort() -> Vec<DatatableSort<Self::Model>> {
        Vec::new()
    }

    // -- provided output methods --------------------------------------------

    async fn json(
        app: &AppContext,
        actor: Option<&Actor>,
        request: DatatableRequest,
    ) -> Result<DatatableJsonResponse> {
        super::json::build_json_response::<Self::Model, Self>(app, actor, request).await
    }

    async fn download(
        app: &AppContext,
        actor: Option<&Actor>,
        request: DatatableRequest,
    ) -> Result<axum::response::Response> {
        super::download::build_download_response::<Self::Model, Self>(app, actor, request).await
    }

    async fn queue_email(
        app: &AppContext,
        actor: Option<&Actor>,
        request: DatatableRequest,
        recipient: &str,
    ) -> Result<DatatableExportAccepted> {
        super::export_job::dispatch_export::<Self>(app, actor, request, recipient).await
    }
}

// ---------------------------------------------------------------------------
// ProjectionDatatable — escape hatch for grouped/aggregate reports
// ---------------------------------------------------------------------------

#[async_trait]
pub trait ProjectionDatatable: Send + Sync + 'static {
    type Row: Clone + Send + Sync + Serialize + 'static;

    const ID: &'static str;

    fn query(ctx: &DatatableContext) -> ProjectionQuery<Self::Row>;

    fn columns() -> Vec<DatatableColumn<Self::Row>>;

    fn mappings() -> Vec<DatatableMapping<Self::Row>> {
        Vec::new()
    }
}
