use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::probes::{LivenessReport, ProbeResult, ReadinessRegistry, ReadinessReport};
use super::types::{
    AuthOutcome, HttpOutcomeClass, JobOutcome, ProbeState, RuntimeBackendKind,
    SchedulerLeadershipState, WebSocketConnectionState,
};
use crate::foundation::{AppContext, Result};

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

fn decrement_saturating(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(1))
    });
}
