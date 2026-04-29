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
use crate::http::{HttpAuthorizeContext, HttpRegistrar, HttpRouteOptions};
use crate::openapi::spec::{generate_openapi_spec, DocumentedRoute};
use crate::support::{GuardId, PermissionId};

#[derive(Default)]
pub struct ObservabilityOptions {
    access: AccessScope,
    authorize: Option<crate::http::HttpAuthorizeCallback>,
}

impl Clone for ObservabilityOptions {
    fn clone(&self) -> Self {
        Self {
            access: self.access.clone(),
            authorize: self.authorize.clone(),
        }
    }
}

impl std::fmt::Debug for ObservabilityOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObservabilityOptions")
            .field("access", &self.access)
            .field("has_authorize", &self.authorize.is_some())
            .finish()
    }
}

impl PartialEq for ObservabilityOptions {
    fn eq(&self, other: &Self) -> bool {
        self.access == other.access
            && match (&self.authorize, &other.authorize) {
                (None, None) => true,
                (Some(left), Some(right)) => std::sync::Arc::ptr_eq(left, right),
                _ => false,
            }
    }
}

impl Eq for ObservabilityOptions {}

impl ObservabilityOptions {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObservabilityOptions {
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

    /// Add a dynamic authorization callback for all observability routes.
    ///
    /// Called after guard and permission checks succeed. Return `Ok(())` to
    /// allow access or `Err(...)` to reject with a project-defined response.
    pub fn authorize<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(HttpAuthorizeContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.authorize = Some(std::sync::Arc::new(move |ctx| Box::pin(f(ctx))));
        self
    }

    pub fn access(&self) -> &AccessScope {
        &self.access
    }

    pub(crate) fn http_route_options(&self) -> HttpRouteOptions {
        let mut opts = HttpRouteOptions::new();
        opts.access = self.access.clone();
        opts.authorize = self.authorize.clone();
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
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "ws/presence/{channel}"),
        get(ws_presence),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "ws/channels"),
        get(ws_channels),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "ws/history/{channel}"),
        get(ws_history),
        route_options.clone(),
    );
    registrar.route_with_options(
        &join_route(&config.base_path, "ws/stats"),
        get(ws_stats),
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
            "SELECT job_id, queue, status, attempt, error, started_at, completed_at, duration_ms, created_at FROM job_history WHERE status IN ('dead_lettered', 'retried') ORDER BY created_at DESC LIMIT 50",
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

async fn ws_channels(State(app): State<AppContext>) -> Response {
    let registry = match app.websocket_channels() {
        Ok(registry) => registry,
        Err(error) => return internal_error_response(error),
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "channels": registry.descriptors() })),
    )
        .into_response()
}

async fn ws_presence(
    State(app): State<AppContext>,
    axum::extract::Path(channel): axum::extract::Path<crate::support::ChannelId>,
) -> Response {
    let registry = match app.websocket_channels() {
        Ok(registry) => registry,
        Err(error) => return internal_error_response(error),
    };
    let descriptor = match registry.find(&channel) {
        Some(d) => d,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "channel not registered" })),
            )
                .into_response();
        }
    };
    if !descriptor.presence {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "presence not enabled for channel" })),
        )
            .into_response();
    }

    let backend = match crate::support::runtime::RuntimeBackend::from_config(app.config()) {
        Ok(b) => b,
        Err(error) => return internal_error_response(error),
    };
    let raw = match backend
        .smembers(&crate::websocket::presence_key(&channel))
        .await
    {
        Ok(members) => members,
        Err(error) => return internal_error_response(error),
    };

    let members: Vec<serde_json::Value> = raw
        .iter()
        .filter_map(|s| serde_json::from_str::<crate::websocket::PresenceInfo>(s).ok())
        .map(|info| {
            serde_json::json!({
                "actor_id": info.actor_id,
                "joined_at": info.joined_at,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "channel": channel.as_str(),
            "count": members.len(),
            "members": members,
        })),
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

#[derive(serde::Deserialize)]
struct WsHistoryQuery {
    limit: Option<i64>,
}

async fn ws_history(
    State(app): State<AppContext>,
    axum::extract::Path(channel): axum::extract::Path<crate::support::ChannelId>,
    axum::extract::Query(params): axum::extract::Query<WsHistoryQuery>,
) -> Response {
    const HISTORY_BUFFER_MAX: i64 = 50;

    let registry = match app.websocket_channels() {
        Ok(registry) => registry,
        Err(error) => return internal_error_response(error),
    };
    if registry.find(&channel).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "channel not registered" })),
        )
            .into_response();
    }

    let limit = params
        .limit
        .unwrap_or(HISTORY_BUFFER_MAX)
        .clamp(1, HISTORY_BUFFER_MAX);

    let backend = match crate::support::runtime::RuntimeBackend::from_config(app.config()) {
        Ok(backend) => backend,
        Err(error) => return internal_error_response(error),
    };

    let history_key = format!("ws:history:{}", channel.as_str());
    let entries = match backend.lrange(&history_key, 0, limit - 1).await {
        Ok(e) => e,
        Err(error) => return internal_error_response(error),
    };

    let include_payloads = match app.config().observability() {
        Ok(cfg) => cfg.websocket.include_payloads,
        Err(error) => return internal_error_response(error),
    };

    let messages: Vec<serde_json::Value> = entries
        .iter()
        .filter_map(|raw| {
            let message = serde_json::from_str::<crate::websocket::ServerMessage>(raw).ok()?;
            let mut obj = serde_json::Map::new();
            obj.insert(
                "channel".to_string(),
                serde_json::Value::String(message.channel.as_str().to_string()),
            );
            obj.insert(
                "event".to_string(),
                serde_json::Value::String(message.event.as_str().to_string()),
            );
            obj.insert(
                "room".to_string(),
                match message.room {
                    Some(r) => serde_json::Value::String(r),
                    None => serde_json::Value::Null,
                },
            );
            if include_payloads {
                obj.insert("payload".to_string(), message.payload);
            } else {
                let size = serde_json::to_vec(&message.payload)
                    .map(|v| v.len() as u64)
                    .unwrap_or(0);
                obj.insert("payload_size_bytes".to_string(), size.into());
            }
            Some(serde_json::Value::Object(obj))
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "channel": channel.as_str(),
            "messages": messages,
        })),
    )
        .into_response()
}

async fn ws_stats(State(app): State<AppContext>) -> Response {
    let diagnostics = match app.diagnostics() {
        Ok(d) => d,
        Err(error) => return internal_error_response(error),
    };
    let ws = diagnostics.snapshot().websocket;

    let channels: Vec<serde_json::Value> = ws
        .channels
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id.as_str(),
                "subscriptions_total": c.subscriptions_total,
                "unsubscribes_total": c.unsubscribes_total,
                "active_subscriptions": c.active_subscriptions,
                "inbound_messages_total": c.inbound_messages_total,
                "outbound_messages_total": c.outbound_messages_total,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "global": {
                "active_connections": ws.active_connections,
                "active_subscriptions": ws.active_subscriptions,
                "subscriptions_total": ws.subscriptions_total,
                "unsubscribes_total": ws.unsubscribes_total,
                "inbound_messages_total": ws.inbound_messages_total,
                "outbound_messages_total": ws.outbound_messages_total,
                "opened_total": ws.opened_total,
                "closed_total": ws.closed_total,
            },
            "channels": channels,
        })),
    )
        .into_response()
}

fn join_route(base_path: &str, suffix: &str) -> String {
    let trimmed = base_path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        format!("/{suffix}")
    } else {
        format!("{trimmed}/{suffix}")
    }
}
