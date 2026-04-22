use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use super::probes::{LivenessReport, ProbeResult, ReadinessRegistry, ReadinessReport};
use super::types::{
    AuthOutcome, HttpOutcomeClass, JobOutcome, ProbeState, RuntimeBackendKind,
    SchedulerLeadershipState, WebSocketConnectionState,
};
use crate::foundation::{AppContext, Result};
use crate::support::ChannelId;

const HTTP_REQUEST_DURATION_BUCKETS_MS: [u64; 12] = [
    5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 30_000,
];

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
    pub duration_ms: HttpDurationHistogramSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpDurationHistogramSnapshot {
    pub count: u64,
    pub sum_ms: u64,
    pub buckets: Vec<HttpDurationBucketSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpDurationBucketSnapshot {
    pub le_ms: u64,
    pub cumulative_count: u64,
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
    pub channels: Vec<WebSocketChannelSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSocketChannelSnapshot {
    pub id: ChannelId,
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

struct HttpDurationHistogram {
    count: AtomicU64,
    sum_ms: AtomicU64,
    buckets: [AtomicU64; HTTP_REQUEST_DURATION_BUCKETS_MS.len()],
}

impl Default for HttpDurationHistogram {
    fn default() -> Self {
        Self {
            count: AtomicU64::new(0),
            sum_ms: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl HttpDurationHistogram {
    fn record(&self, duration_ms: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_ms.fetch_add(duration_ms, Ordering::Relaxed);

        if let Some(index) = HTTP_REQUEST_DURATION_BUCKETS_MS
            .iter()
            .position(|upper_bound_ms| duration_ms <= *upper_bound_ms)
        {
            self.buckets[index].fetch_add(1, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> HttpDurationHistogramSnapshot {
        let mut cumulative_count = 0;
        let buckets = HTTP_REQUEST_DURATION_BUCKETS_MS
            .iter()
            .enumerate()
            .map(|(index, le_ms)| {
                cumulative_count += self.buckets[index].load(Ordering::Relaxed);
                HttpDurationBucketSnapshot {
                    le_ms: *le_ms,
                    cumulative_count,
                }
            })
            .collect();

        HttpDurationHistogramSnapshot {
            count: self.count.load(Ordering::Relaxed),
            sum_ms: self.sum_ms.load(Ordering::Relaxed),
            buckets,
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
    duration_ms: HttpDurationHistogram,
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
            duration_ms: self.duration_ms.snapshot(),
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
struct PerChannelWebSocketCounters {
    subscriptions_total: AtomicU64,
    unsubscribes_total: AtomicU64,
    active_subscriptions: AtomicU64,
    inbound_messages_total: AtomicU64,
    outbound_messages_total: AtomicU64,
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
    per_channel: RwLock<HashMap<ChannelId, Arc<PerChannelWebSocketCounters>>>,
}

impl WebSocketCounters {
    fn snapshot(&self) -> WebSocketRuntimeSnapshot {
        let map = self.per_channel.read().expect("per_channel lock poisoned");
        let mut channels: Vec<WebSocketChannelSnapshot> = map
            .iter()
            .map(|(id, counters)| WebSocketChannelSnapshot {
                id: id.clone(),
                subscriptions_total: counters.subscriptions_total.load(Ordering::Relaxed),
                unsubscribes_total: counters.unsubscribes_total.load(Ordering::Relaxed),
                active_subscriptions: counters.active_subscriptions.load(Ordering::Relaxed),
                inbound_messages_total: counters.inbound_messages_total.load(Ordering::Relaxed),
                outbound_messages_total: counters.outbound_messages_total.load(Ordering::Relaxed),
            })
            .collect();
        drop(map);
        channels.sort_unstable_by(|a, b| a.id.cmp(&b.id));

        WebSocketRuntimeSnapshot {
            opened_total: self.opened_total.load(Ordering::Relaxed),
            closed_total: self.closed_total.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            subscriptions_total: self.subscriptions_total.load(Ordering::Relaxed),
            unsubscribes_total: self.unsubscribes_total.load(Ordering::Relaxed),
            active_subscriptions: self.active_subscriptions.load(Ordering::Relaxed),
            inbound_messages_total: self.inbound_messages_total.load(Ordering::Relaxed),
            outbound_messages_total: self.outbound_messages_total.load(Ordering::Relaxed),
            channels,
        }
    }

    fn entry(&self, channel: &ChannelId) -> Arc<PerChannelWebSocketCounters> {
        // Fast path: read lock and return if present.
        {
            let map = self.per_channel.read().expect("per_channel lock poisoned");
            if let Some(existing) = map.get(channel) {
                return existing.clone();
            }
        }
        // Slow path: upgrade to write lock and insert.
        let mut map = self.per_channel.write().expect("per_channel lock poisoned");
        map.entry(channel.clone())
            .or_insert_with(|| Arc::new(PerChannelWebSocketCounters::default()))
            .clone()
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

    pub fn record_http_response(&self, status: axum::http::StatusCode) {
        self.record_http_response_inner(status, None);
    }

    pub fn record_http_response_with_duration(
        &self,
        status: axum::http::StatusCode,
        duration_ms: u64,
    ) {
        self.record_http_response_inner(status, Some(duration_ms));
    }

    fn record_http_response_inner(&self, status: axum::http::StatusCode, duration_ms: Option<u64>) {
        self.http.requests_total.fetch_add(1, Ordering::Relaxed);
        if let Some(duration_ms) = duration_ms {
            self.http.duration_ms.record(duration_ms);
        }
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

    #[deprecated(
        since = "0.2.0",
        note = "use `record_websocket_subscription_opened_on(&channel)` — the global-only variant bypasses per-channel tracking"
    )]
    pub fn record_websocket_subscription_opened(&self) {
        self.websocket
            .subscriptions_total
            .fetch_add(1, Ordering::Relaxed);
        self.websocket
            .active_subscriptions
            .fetch_add(1, Ordering::Relaxed);
    }

    #[deprecated(
        since = "0.2.0",
        note = "use `record_websocket_subscription_closed_on(&channel)` — the global-only variant bypasses per-channel tracking"
    )]
    pub fn record_websocket_subscription_closed(&self) {
        self.websocket
            .unsubscribes_total
            .fetch_add(1, Ordering::Relaxed);
        decrement_saturating(&self.websocket.active_subscriptions);
    }

    #[deprecated(
        since = "0.2.0",
        note = "use `record_websocket_inbound_message_on(&channel)` — the global-only variant bypasses per-channel tracking"
    )]
    pub fn record_websocket_inbound_message(&self) {
        self.websocket
            .inbound_messages_total
            .fetch_add(1, Ordering::Relaxed);
    }

    #[deprecated(
        since = "0.2.0",
        note = "use `record_websocket_outbound_message_on(&channel)` — the global-only variant bypasses per-channel tracking"
    )]
    pub fn record_websocket_outbound_message(&self) {
        self.websocket
            .outbound_messages_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_websocket_subscription_opened_on(&self, channel: &ChannelId) {
        self.websocket
            .subscriptions_total
            .fetch_add(1, Ordering::Relaxed);
        self.websocket
            .active_subscriptions
            .fetch_add(1, Ordering::Relaxed);
        let entry = self.websocket.entry(channel);
        entry.subscriptions_total.fetch_add(1, Ordering::Relaxed);
        entry.active_subscriptions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_websocket_subscription_closed_on(&self, channel: &ChannelId) {
        self.websocket
            .unsubscribes_total
            .fetch_add(1, Ordering::Relaxed);
        decrement_saturating(&self.websocket.active_subscriptions);
        let entry = self.websocket.entry(channel);
        entry.unsubscribes_total.fetch_add(1, Ordering::Relaxed);
        decrement_saturating(&entry.active_subscriptions);
    }

    pub fn record_websocket_inbound_message_on(&self, channel: &ChannelId) {
        self.websocket
            .inbound_messages_total
            .fetch_add(1, Ordering::Relaxed);
        self.websocket
            .entry(channel)
            .inbound_messages_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_websocket_outbound_message_on(&self, channel: &ChannelId) {
        self.websocket
            .outbound_messages_total
            .fetch_add(1, Ordering::Relaxed);
        self.websocket
            .entry(channel)
            .outbound_messages_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn register_websocket_channel(&self, channel: &ChannelId) {
        let _ = self.websocket.entry(channel);
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

fn decrement_saturating(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(1))
    });
}

#[cfg(test)]
impl Default for RuntimeDiagnostics {
    fn default() -> Self {
        Self::new(
            crate::logging::types::RuntimeBackendKind::Memory,
            super::probes::ReadinessRegistry { checks: Vec::new() },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_channel_counters_start_at_zero_and_increment() {
        use crate::support::ChannelId;

        let diagnostics = RuntimeDiagnostics::default();
        let chat = ChannelId::new("chat");

        diagnostics.record_websocket_subscription_opened_on(&chat);
        diagnostics.record_websocket_inbound_message_on(&chat);
        diagnostics.record_websocket_outbound_message_on(&chat);
        diagnostics.record_websocket_outbound_message_on(&chat);

        let snapshot = diagnostics.snapshot().websocket;
        let channel = snapshot
            .channels
            .iter()
            .find(|c| c.id == chat)
            .expect("channel snapshot missing");
        assert_eq!(channel.subscriptions_total, 1);
        assert_eq!(channel.active_subscriptions, 1);
        assert_eq!(channel.inbound_messages_total, 1);
        assert_eq!(channel.outbound_messages_total, 2);

        assert_eq!(snapshot.subscriptions_total, 1);
        assert_eq!(snapshot.inbound_messages_total, 1);
        assert_eq!(snapshot.outbound_messages_total, 2);
    }

    #[test]
    fn http_duration_histogram_tracks_cumulative_buckets() {
        use axum::http::StatusCode;

        let diagnostics = RuntimeDiagnostics::default();

        diagnostics.record_http_response_with_duration(StatusCode::OK, 12);
        diagnostics.record_http_response_with_duration(StatusCode::OK, 600);
        diagnostics.record_http_response_with_duration(StatusCode::OK, 35_000);

        let histogram = diagnostics.snapshot().http.duration_ms;
        assert_eq!(histogram.count, 3);
        assert_eq!(histogram.sum_ms, 35_612);

        let le_25 = histogram
            .buckets
            .iter()
            .find(|bucket| bucket.le_ms == 25)
            .expect("25ms bucket missing");
        assert_eq!(le_25.cumulative_count, 1);

        let le_1_000 = histogram
            .buckets
            .iter()
            .find(|bucket| bucket.le_ms == 1_000)
            .expect("1000ms bucket missing");
        assert_eq!(le_1_000.cumulative_count, 2);

        let le_30_000 = histogram
            .buckets
            .iter()
            .find(|bucket| bucket.le_ms == 30_000)
            .expect("30000ms bucket missing");
        assert_eq!(le_30_000.cumulative_count, 2);
    }
}
