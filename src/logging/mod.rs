use std::collections::HashSet;
use std::fmt;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::HeaderName;
use axum::http::{request::Parts, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use tracing_subscriber::EnvFilter;

use crate::auth::AccessScope;
use crate::config::{ConfigRepository, ObservabilityConfig};
use crate::foundation::{AppContext, Error, Result};
use crate::http::{HttpRegistrar, HttpRouteOptions};
use crate::support::{GuardId, PermissionId, ProbeId};

pub const REQUEST_ID_HEADER: &str = "x-request-id";

pub const FRAMEWORK_BOOTSTRAP_PROBE: ProbeId = ProbeId::new("forge.bootstrap");
pub const RUNTIME_BACKEND_PROBE: ProbeId = ProbeId::new("forge.runtime_backend");
pub const REDIS_PING_PROBE: ProbeId = ProbeId::new("forge.redis_ping");

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_filter_directive(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

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
        HttpRouteOptions {
            access: self.access.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HttpOutcomeClass {
    Informational,
    Success,
    Redirection,
    ClientError,
    ServerError,
}

impl HttpOutcomeClass {
    pub fn from_status(status: StatusCode) -> Self {
        match status.as_u16() / 100 {
            1 => Self::Informational,
            2 => Self::Success,
            3 => Self::Redirection,
            4 => Self::ClientError,
            _ => Self::ServerError,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthOutcome {
    Success,
    Unauthorized,
    Forbidden,
    Error,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    Enqueued,
    Leased,
    Started,
    Succeeded,
    Retried,
    ExpiredLeaseRequeued,
    DeadLettered,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSocketConnectionState {
    Opened,
    Closed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackendKind {
    Redis,
    Memory,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerLeadershipState {
    Acquired,
    Lost,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Healthy,
    Unhealthy,
}

impl ProbeState {
    pub fn is_healthy(self) -> bool {
        matches!(self, Self::Healthy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(String);

impl RequestId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for RequestId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for RequestId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<S> FromRequestParts<S> for RequestId
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts.extensions.get::<RequestId>().cloned().ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "message": "request id missing from request context",
                })),
            )
                .into_response()
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeResult {
    pub id: ProbeId,
    pub state: ProbeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ProbeResult {
    pub fn healthy<I>(id: I) -> Self
    where
        I: Into<ProbeId>,
    {
        Self {
            id: id.into(),
            state: ProbeState::Healthy,
            message: None,
        }
    }

    pub fn unhealthy<I>(id: I, message: impl Into<String>) -> Self
    where
        I: Into<ProbeId>,
    {
        Self {
            id: id.into(),
            state: ProbeState::Unhealthy,
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LivenessReport {
    pub state: ProbeState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessReport {
    pub state: ProbeState,
    pub probes: Vec<ProbeResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub backend: RuntimeBackendKind,
    pub bootstrap_complete: bool,
    pub http: HttpRuntimeSnapshot,
    pub auth: AuthRuntimeSnapshot,
    pub websocket: WebSocketRuntimeSnapshot,
    pub scheduler: SchedulerRuntimeSnapshot,
    pub jobs: JobRuntimeSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpRuntimeSnapshot {
    pub requests_total: u64,
    pub informational_total: u64,
    pub success_total: u64,
    pub redirection_total: u64,
    pub client_error_total: u64,
    pub server_error_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthRuntimeSnapshot {
    pub success_total: u64,
    pub unauthorized_total: u64,
    pub forbidden_total: u64,
    pub error_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSocketRuntimeSnapshot {
    pub opened_total: u64,
    pub closed_total: u64,
    pub active_connections: u64,
    pub subscriptions_total: u64,
    pub unsubscribes_total: u64,
    pub active_subscriptions: u64,
    pub inbound_messages_total: u64,
    pub outbound_messages_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerRuntimeSnapshot {
    pub ticks_total: u64,
    pub executed_schedules_total: u64,
    pub leadership_acquired_total: u64,
    pub leadership_lost_total: u64,
    pub leader_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobRuntimeSnapshot {
    pub enqueued_total: u64,
    pub leased_total: u64,
    pub started_total: u64,
    pub succeeded_total: u64,
    pub retried_total: u64,
    pub expired_requeues_total: u64,
    pub dead_lettered_total: u64,
}

#[async_trait]
pub trait ReadinessCheck: Send + Sync + 'static {
    async fn run(&self, app: &AppContext) -> Result<ProbeResult>;
}

#[async_trait]
impl<F, Fut> ReadinessCheck for F
where
    F: Fn(&AppContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<ProbeResult>> + Send,
{
    async fn run(&self, app: &AppContext) -> Result<ProbeResult> {
        (self)(app).await
    }
}

pub(crate) type ReadinessRegistryHandle = Arc<Mutex<ReadinessRegistryBuilder>>;

#[derive(Default)]
pub(crate) struct ReadinessRegistryBuilder {
    checks: Vec<RegisteredReadinessCheck>,
    ids: HashSet<ProbeId>,
}

impl ReadinessRegistryBuilder {
    pub(crate) fn shared() -> ReadinessRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn register_arc<I>(&mut self, id: I, check: Arc<dyn ReadinessCheck>) -> Result<()>
    where
        I: Into<ProbeId>,
    {
        let id = id.into();
        if !self.ids.insert(id.clone()) {
            return Err(Error::message(format!(
                "readiness check `{id}` already registered"
            )));
        }

        self.checks.push(RegisteredReadinessCheck { id, check });
        Ok(())
    }

    pub(crate) fn freeze_shared(handle: ReadinessRegistryHandle) -> ReadinessRegistry {
        let mut builder = handle.lock().expect("readiness registry lock poisoned");
        ReadinessRegistry {
            checks: std::mem::take(&mut builder.checks),
        }
    }
}

pub(crate) struct ReadinessRegistry {
    checks: Vec<RegisteredReadinessCheck>,
}

struct RegisteredReadinessCheck {
    id: ProbeId,
    check: Arc<dyn ReadinessCheck>,
}

pub struct RuntimeDiagnostics {
    backend: RuntimeBackendKind,
    bootstrap_complete: AtomicBool,
    readiness: ReadinessRegistry,
    http: HttpCounters,
    auth: AuthCounters,
    websocket: WebSocketCounters,
    scheduler: SchedulerCounters,
    jobs: JobCounters,
}

impl RuntimeDiagnostics {
    pub(crate) fn new(backend: RuntimeBackendKind, readiness: ReadinessRegistry) -> Self {
        Self {
            backend,
            bootstrap_complete: AtomicBool::new(false),
            readiness,
            http: HttpCounters::default(),
            auth: AuthCounters::default(),
            websocket: WebSocketCounters::default(),
            scheduler: SchedulerCounters::default(),
            jobs: JobCounters::default(),
        }
    }

    pub fn backend_kind(&self) -> RuntimeBackendKind {
        self.backend
    }

    pub fn mark_bootstrap_complete(&self) {
        self.bootstrap_complete.store(true, Ordering::Relaxed);
    }

    pub fn bootstrap_complete(&self) -> bool {
        self.bootstrap_complete.load(Ordering::Relaxed)
    }

    pub fn liveness(&self) -> LivenessReport {
        LivenessReport {
            state: ProbeState::Healthy,
        }
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            backend: self.backend,
            bootstrap_complete: self.bootstrap_complete(),
            http: self.http.snapshot(),
            auth: self.auth.snapshot(),
            websocket: self.websocket.snapshot(),
            scheduler: self.scheduler.snapshot(),
            jobs: self.jobs.snapshot(),
        }
    }

    pub async fn run_readiness_checks(&self, app: &AppContext) -> Result<ReadinessReport> {
        let mut probes = Vec::with_capacity(self.readiness.checks.len());
        let mut state = ProbeState::Healthy;

        for registered in &self.readiness.checks {
            let probe = match registered.check.run(app).await {
                Ok(mut probe) => {
                    probe.id = registered.id.clone();
                    probe
                }
                Err(error) => ProbeResult::unhealthy(registered.id.clone(), error.to_string()),
            };

            if !probe.state.is_healthy() {
                state = ProbeState::Unhealthy;
            }
            probes.push(probe);
        }

        Ok(ReadinessReport { state, probes })
    }

    pub fn record_http_response(&self, status: StatusCode) {
        self.http.requests_total.fetch_add(1, Ordering::Relaxed);
        match HttpOutcomeClass::from_status(status) {
            HttpOutcomeClass::Informational => {
                self.http
                    .informational_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            HttpOutcomeClass::Success => {
                self.http.success_total.fetch_add(1, Ordering::Relaxed);
            }
            HttpOutcomeClass::Redirection => {
                self.http.redirection_total.fetch_add(1, Ordering::Relaxed);
            }
            HttpOutcomeClass::ClientError => {
                self.http.client_error_total.fetch_add(1, Ordering::Relaxed);
            }
            HttpOutcomeClass::ServerError => {
                self.http.server_error_total.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn record_auth_outcome(&self, outcome: AuthOutcome) {
        match outcome {
            AuthOutcome::Success => {
                self.auth.success_total.fetch_add(1, Ordering::Relaxed);
            }
            AuthOutcome::Unauthorized => {
                self.auth.unauthorized_total.fetch_add(1, Ordering::Relaxed);
            }
            AuthOutcome::Forbidden => {
                self.auth.forbidden_total.fetch_add(1, Ordering::Relaxed);
            }
            AuthOutcome::Error => {
                self.auth.error_total.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn record_websocket_connection(&self, state: WebSocketConnectionState) {
        match state {
            WebSocketConnectionState::Opened => {
                self.websocket.opened_total.fetch_add(1, Ordering::Relaxed);
                self.websocket
                    .active_connections
                    .fetch_add(1, Ordering::Relaxed);
            }
            WebSocketConnectionState::Closed => {
                self.websocket.closed_total.fetch_add(1, Ordering::Relaxed);
                decrement_saturating(&self.websocket.active_connections);
            }
        }
    }

    pub fn record_websocket_subscription_opened(&self) {
        self.websocket
            .subscriptions_total
            .fetch_add(1, Ordering::Relaxed);
        self.websocket
            .active_subscriptions
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_websocket_subscription_closed(&self) {
        self.websocket
            .unsubscribes_total
            .fetch_add(1, Ordering::Relaxed);
        decrement_saturating(&self.websocket.active_subscriptions);
    }

    pub fn record_websocket_inbound_message(&self) {
        self.websocket
            .inbound_messages_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_websocket_outbound_message(&self) {
        self.websocket
            .outbound_messages_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_scheduler_tick(&self) {
        self.scheduler.ticks_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_schedule_executed(&self) {
        self.scheduler
            .executed_schedules_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_scheduler_leadership(&self, state: SchedulerLeadershipState) {
        match state {
            SchedulerLeadershipState::Acquired => {
                self.scheduler
                    .leadership_acquired_total
                    .fetch_add(1, Ordering::Relaxed);
                self.scheduler.leader_active.store(true, Ordering::Relaxed);
            }
            SchedulerLeadershipState::Lost => {
                self.scheduler
                    .leadership_lost_total
                    .fetch_add(1, Ordering::Relaxed);
                self.scheduler.leader_active.store(false, Ordering::Relaxed);
            }
        }
    }

    pub fn set_scheduler_leader_active(&self, active: bool) {
        self.scheduler
            .leader_active
            .store(active, Ordering::Relaxed);
    }

    pub fn record_job_outcome(&self, outcome: JobOutcome) {
        match outcome {
            JobOutcome::Enqueued => {
                self.jobs.enqueued_total.fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::Leased => {
                self.jobs.leased_total.fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::Started => {
                self.jobs.started_total.fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::Succeeded => {
                self.jobs.succeeded_total.fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::Retried => {
                self.jobs.retried_total.fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::ExpiredLeaseRequeued => {
                self.jobs
                    .expired_requeues_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            JobOutcome::DeadLettered => {
                self.jobs
                    .dead_lettered_total
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[derive(Default)]
struct HttpCounters {
    requests_total: AtomicU64,
    informational_total: AtomicU64,
    success_total: AtomicU64,
    redirection_total: AtomicU64,
    client_error_total: AtomicU64,
    server_error_total: AtomicU64,
}

impl HttpCounters {
    fn snapshot(&self) -> HttpRuntimeSnapshot {
        HttpRuntimeSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            informational_total: self.informational_total.load(Ordering::Relaxed),
            success_total: self.success_total.load(Ordering::Relaxed),
            redirection_total: self.redirection_total.load(Ordering::Relaxed),
            client_error_total: self.client_error_total.load(Ordering::Relaxed),
            server_error_total: self.server_error_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct AuthCounters {
    success_total: AtomicU64,
    unauthorized_total: AtomicU64,
    forbidden_total: AtomicU64,
    error_total: AtomicU64,
}

impl AuthCounters {
    fn snapshot(&self) -> AuthRuntimeSnapshot {
        AuthRuntimeSnapshot {
            success_total: self.success_total.load(Ordering::Relaxed),
            unauthorized_total: self.unauthorized_total.load(Ordering::Relaxed),
            forbidden_total: self.forbidden_total.load(Ordering::Relaxed),
            error_total: self.error_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct WebSocketCounters {
    opened_total: AtomicU64,
    closed_total: AtomicU64,
    active_connections: AtomicU64,
    subscriptions_total: AtomicU64,
    unsubscribes_total: AtomicU64,
    active_subscriptions: AtomicU64,
    inbound_messages_total: AtomicU64,
    outbound_messages_total: AtomicU64,
}

impl WebSocketCounters {
    fn snapshot(&self) -> WebSocketRuntimeSnapshot {
        WebSocketRuntimeSnapshot {
            opened_total: self.opened_total.load(Ordering::Relaxed),
            closed_total: self.closed_total.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            subscriptions_total: self.subscriptions_total.load(Ordering::Relaxed),
            unsubscribes_total: self.unsubscribes_total.load(Ordering::Relaxed),
            active_subscriptions: self.active_subscriptions.load(Ordering::Relaxed),
            inbound_messages_total: self.inbound_messages_total.load(Ordering::Relaxed),
            outbound_messages_total: self.outbound_messages_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct SchedulerCounters {
    ticks_total: AtomicU64,
    executed_schedules_total: AtomicU64,
    leadership_acquired_total: AtomicU64,
    leadership_lost_total: AtomicU64,
    leader_active: AtomicBool,
}

impl SchedulerCounters {
    fn snapshot(&self) -> SchedulerRuntimeSnapshot {
        SchedulerRuntimeSnapshot {
            ticks_total: self.ticks_total.load(Ordering::Relaxed),
            executed_schedules_total: self.executed_schedules_total.load(Ordering::Relaxed),
            leadership_acquired_total: self.leadership_acquired_total.load(Ordering::Relaxed),
            leadership_lost_total: self.leadership_lost_total.load(Ordering::Relaxed),
            leader_active: self.leader_active.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct JobCounters {
    enqueued_total: AtomicU64,
    leased_total: AtomicU64,
    started_total: AtomicU64,
    succeeded_total: AtomicU64,
    retried_total: AtomicU64,
    expired_requeues_total: AtomicU64,
    dead_lettered_total: AtomicU64,
}

impl JobCounters {
    fn snapshot(&self) -> JobRuntimeSnapshot {
        JobRuntimeSnapshot {
            enqueued_total: self.enqueued_total.load(Ordering::Relaxed),
            leased_total: self.leased_total.load(Ordering::Relaxed),
            started_total: self.started_total.load(Ordering::Relaxed),
            succeeded_total: self.succeeded_total.load(Ordering::Relaxed),
            retried_total: self.retried_total.load(Ordering::Relaxed),
            expired_requeues_total: self.expired_requeues_total.load(Ordering::Relaxed),
            dead_lettered_total: self.dead_lettered_total.load(Ordering::Relaxed),
        }
    }
}

pub fn init(config: &ConfigRepository) -> Result<()> {
    static LOGGING: OnceLock<()> = OnceLock::new();

    if LOGGING.get().is_some() {
        return Ok(());
    }

    let level = config.logging()?.level.as_filter_directive();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();

    let _ = LOGGING.set(());
    Ok(())
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
        route_options,
    );
    Ok(())
}

pub(crate) async fn request_context_middleware(
    State(app): State<AppContext>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(generate_request_id);

    request
        .extensions_mut()
        .insert(RequestId::new(request_id.clone()));

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let span = tracing::info_span!(
        "forge.http.request",
        method = %method,
        path = %path,
        request_id = %request_id
    );

    let mut response = next.run(request).instrument(span).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    if let Ok(diagnostics) = app.diagnostics() {
        diagnostics.record_http_response(response.status());
    }
    response
}

fn join_route(base_path: &str, suffix: &str) -> String {
    let trimmed = base_path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        format!("/{suffix}")
    } else {
        format!("{trimmed}/{suffix}")
    }
}

fn generate_request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!("forge-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn decrement_saturating(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(1))
    });
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

fn internal_error_response(error: Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "message": error.to_string(),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{
        ProbeResult, ProbeState, ReadinessCheck, ReadinessRegistryBuilder, RuntimeBackendKind,
        RuntimeDiagnostics,
    };
    use crate::config::ConfigRepository;
    use crate::foundation::{AppContext, Container, Error};
    use crate::support::ProbeId;
    use crate::validation::RuleRegistry;

    struct PassingProbe;

    #[async_trait]
    impl ReadinessCheck for PassingProbe {
        async fn run(&self, _app: &AppContext) -> crate::Result<ProbeResult> {
            Ok(ProbeResult::healthy(ProbeId::new("provider.pass")))
        }
    }

    struct FailingProbe;

    #[async_trait]
    impl ReadinessCheck for FailingProbe {
        async fn run(&self, _app: &AppContext) -> crate::Result<ProbeResult> {
            Err(Error::message("not ready"))
        }
    }

    #[test]
    fn rejects_duplicate_probe_registration() {
        let mut builder = ReadinessRegistryBuilder::default();
        builder
            .register_arc(ProbeId::new("database"), Arc::new(PassingProbe))
            .unwrap();
        let error = builder
            .register_arc(ProbeId::new("database"), Arc::new(PassingProbe))
            .unwrap_err();

        assert!(error.to_string().contains("already registered"));
    }

    #[tokio::test]
    async fn readiness_aggregation_reports_failures() {
        let mut builder = ReadinessRegistryBuilder::default();
        builder
            .register_arc(ProbeId::new("provider.pass"), Arc::new(PassingProbe))
            .unwrap();
        builder
            .register_arc(ProbeId::new("provider.fail"), Arc::new(FailingProbe))
            .unwrap();

        let diagnostics = RuntimeDiagnostics::new(
            RuntimeBackendKind::Memory,
            ReadinessRegistryBuilder::freeze_shared(Arc::new(Mutex::new(builder))),
        );
        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        );
        let report = diagnostics.run_readiness_checks(&app).await.unwrap();

        assert_eq!(report.state, ProbeState::Unhealthy);
        assert_eq!(report.probes.len(), 2);
        assert_eq!(report.probes[0].state, ProbeState::Healthy);
        assert_eq!(report.probes[1].state, ProbeState::Unhealthy);
        assert_eq!(report.probes[1].id, ProbeId::new("provider.fail"));
    }
}
