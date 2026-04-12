use std::sync::OnceLock;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;

use super::metrics;
use crate::auth::AccessScope;
use crate::config::ObservabilityConfig;
use crate::database::DbValue;
use crate::foundation::{AppContext, Error, Result};
use crate::http::{HttpRegistrar, HttpRouteOptions};
use crate::openapi::spec::{generate_openapi_spec, DocumentedRoute};
use crate::support::{GuardId, PermissionId};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObservabilityOptions {
    access: AccessScope,
}

impl ObservabilityOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn guard<I>(mut self, guard: I) -> Self
    where
        I: Into<GuardId>,
    {
        self.access = self.access.with_guard(guard);
        self
    }

    pub fn permission<I>(mut self, permission: I) -> Self
    where
        I: Into<PermissionId>,
    {
        self.access = self.access.with_permission(permission);
        self
    }

    pub fn permissions<I, P>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PermissionId>,
    {
        self.access = self.access.with_permissions(permissions);
        self
    }

    pub fn access(&self) -> &AccessScope {
        &self.access
    }

    pub(crate) fn http_route_options(&self) -> HttpRouteOptions {
        let mut opts = HttpRouteOptions::new();
        opts.access = self.access.clone();
        opts
    }
}

pub(crate) fn register_observability_routes(
    registrar: &mut HttpRegistrar,
    config: &ObservabilityConfig,
    options: &ObservabilityOptions,
) -> Result<()> {
    let route_options = options.http_route_options();
    registrar.route_with_options(
        &join_route(&config.base_path, "health"),
        get(observability_liveness),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "ready"),
        get(observability_readiness),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "runtime"),
        get(observability_runtime),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "metrics"),
        get(observability_metrics),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "jobs/stats"),
        get(jobs_stats),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "jobs/failed"),
        get(jobs_failed),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "sql"),
        get(slow_queries),
        route_options,
    );
    Ok(())
}

async fn observability_liveness(State(app): State<AppContext>) -> Response {
    match app.diagnostics() {
        Ok(diagnostics) => (StatusCode::OK, Json(diagnostics.liveness())).into_response(),
        Err(error) => internal_error_response(error),
    }
}

async fn observability_readiness(State(app): State<AppContext>) -> Response {
    match app.diagnostics() {
        Ok(diagnostics) => match diagnostics.run_readiness_checks(&app).await {
            Ok(report) => {
                let status = if report.state.is_healthy() {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                (status, Json(report)).into_response()
            }
            Err(error) => internal_error_response(error),
        },
        Err(error) => internal_error_response(error),
    }
}

async fn observability_runtime(State(app): State<AppContext>) -> Response {
    match app.diagnostics() {
        Ok(diagnostics) => (StatusCode::OK, Json(diagnostics.snapshot())).into_response(),
        Err(error) => internal_error_response(error),
    }
}

async fn observability_metrics(State(app): State<AppContext>) -> Response {
    match app.diagnostics() {
        Ok(diagnostics) => {
            let body = metrics::format_prometheus(&diagnostics.snapshot());
            (
                StatusCode::OK,
                [(
                    header::CONTENT_TYPE,
                    "text/plain; version=0.0.4; charset=utf-8",
                )],
                body,
            )
                .into_response()
        }
        Err(error) => internal_error_response(error),
    }
}

async fn jobs_stats(State(app): State<AppContext>) -> Response {
    let db = match app.database() {
        Ok(db) => db,
        Err(error) => return internal_error_response(error),
    };

    match db
        .raw_query(
            "SELECT status, COUNT(*) as count FROM job_history GROUP BY status",
            &[],
        )
        .await
    {
        Ok(rows) => {
            let stats: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| {
                    let status = match row.get("status") {
                        Some(DbValue::Text(s)) => s.clone(),
                        _ => "unknown".to_string(),
                    };
                    let count = match row.get("count") {
                        Some(DbValue::Int64(n)) => *n,
                        _ => 0,
                    };
                    serde_json::json!({ "status": status, "count": count })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!({ "stats": stats }))).into_response()
        }
        Err(error) => internal_error_response(error),
    }
}

async fn jobs_failed(State(app): State<AppContext>) -> Response {
    let db = match app.database() {
        Ok(db) => db,
        Err(error) => return internal_error_response(error),
    };

    match db
        .raw_query(
            "SELECT job_id, queue, status, attempt, error, started_at, completed_at, duration_ms, created_at FROM job_history WHERE status IN ('failed', 'dead_lettered') ORDER BY created_at DESC LIMIT 50",
            &[],
        )
        .await
    {
        Ok(rows) => {
            let jobs: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| {
                    let mut entry = serde_json::Map::new();
                    for field in &[
                        "job_id", "queue", "status", "attempt", "error",
                        "started_at", "completed_at", "duration_ms", "created_at",
                    ] {
                        if let Some(value) = row.get(field) {
                            entry.insert(
                                field.to_string(),
                                db_value_to_json(value),
                            );
                        }
                    }
                    serde_json::Value::Object(entry)
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!({ "failed_jobs": jobs }))).into_response()
        }
        Err(error) => internal_error_response(error),
    }
}

async fn slow_queries(State(_app): State<AppContext>) -> Response {
    let queries = crate::database::recent_slow_queries();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "slow_queries": queries })),
    )
        .into_response()
}

fn db_value_to_json(value: &DbValue) -> serde_json::Value {
    match value {
        DbValue::Text(s) => serde_json::Value::String(s.clone()),
        DbValue::Int32(n) => serde_json::json!(n),
        DbValue::Int64(n) => serde_json::json!(n),
        DbValue::Bool(b) => serde_json::json!(b),
        DbValue::Float64(f) => serde_json::json!(f),
        DbValue::Json(v) => v.clone(),
        DbValue::Null(_) => serde_json::Value::Null,
        _ => serde_json::Value::String(format!("{value:?}")),
    }
}

fn internal_error_response(error: Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "message": error.to_string(),
        })),
    )
        .into_response()
}

/// Cached OpenAPI spec shared across requests.
static OPENAPI_SPEC: OnceLock<serde_json::Value> = OnceLock::new();

/// Store the OpenAPI spec for serving. Call this at bootstrap with
/// the collected documented routes.
pub(crate) fn set_openapi_spec(title: &str, version: &str, routes: &[DocumentedRoute]) {
    let spec = generate_openapi_spec(title, version, routes);
    let _ = OPENAPI_SPEC.set(spec);
}

pub(crate) fn register_openapi_route(
    registrar: &mut HttpRegistrar,
    config: &ObservabilityConfig,
    options: &ObservabilityOptions,
) -> Result<()> {
    registrar.route_with_options(
        &join_route(&config.base_path, "openapi.json"),
        get(openapi_spec_handler),
        options.http_route_options(),
    );
    Ok(())
}

async fn openapi_spec_handler() -> Response {
    match OPENAPI_SPEC.get() {
        Some(spec) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            Json(spec.clone()),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": "OpenAPI spec not available"})),
        )
            .into_response(),
    }
}

fn join_route(base_path: &str, suffix: &str) -> String {
    let trimmed = base_path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        format!("/{suffix}")
    } else {
        format!("{trimmed}/{suffix}")
    }
}
