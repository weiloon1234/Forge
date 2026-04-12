use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::foundation::{AppContext, Result};
use crate::logging::SchedulerLeadershipState;
use crate::scheduler::{cron_due, ScheduleKind, ScheduleRegistry, ScheduledTask};
use crate::support::runtime::RuntimeBackend;
use crate::support::{DateTime, ScheduleId};

pub struct SchedulerKernel {
    app: AppContext,
    tasks: Vec<ScheduledTask>,
    backend: RuntimeBackend,
    tick_interval: Duration,
    leader_lease_ttl: Duration,
    owner_id: String,
    leader_active: AtomicBool,
    last_tick: Mutex<Option<DateTime>>,
    last_interval_run: Mutex<HashMap<ScheduleId, DateTime>>,
}

impl SchedulerKernel {
    pub fn new(app: AppContext, registry: ScheduleRegistry) -> Result<Self> {
        let backend = app.resolve::<RuntimeBackend>()?.as_ref().clone();
        let config = app.config().scheduler()?;
        Ok(Self {
            app,
            tasks: registry.tasks(),
            backend,
            tick_interval: Duration::from_millis(config.tick_interval_ms.max(1)),
            leader_lease_ttl: Duration::from_millis(config.leader_lease_ttl_ms.max(1)),
            owner_id: next_owner_id(),
            leader_active: AtomicBool::new(false),
            last_tick: Mutex::new(None),
            last_interval_run: Mutex::new(HashMap::new()),
        })
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub async fn tick(&self) -> Result<Vec<ScheduleId>> {
        self.tick_at(self.app.clock().now()).await
    }

    pub async fn run_once(&self) -> Result<Vec<ScheduleId>> {
        self.run_once_at(self.app.clock().now()).await
    }

    pub async fn run_once_at(&self, now: DateTime) -> Result<Vec<ScheduleId>> {
        if self.ensure_leadership().await? {
            return self.tick_at(now).await;
        }

        Ok(Vec::new())
    }

    pub async fn tick_at(&self, now: DateTime) -> Result<Vec<ScheduleId>> {
        if let Ok(diagnostics) = self.app.diagnostics() {
            diagnostics.record_scheduler_tick();
        }
        let previous = {
            let mut last_tick = self
                .last_tick
                .lock()
                .expect("scheduler tick mutex poisoned");
            let previous = last_tick.unwrap_or_else(|| now.sub_seconds(1));
            *last_tick = Some(now);
            previous
        };

        let mut executed = Vec::new();
        for task in &self.tasks {
            match &task.kind {
                ScheduleKind::Cron { expression } => {
                    if cron_due(expression, previous, now) {
                        (task.handler)(self.app.clone()).await?;
                        tracing::info!(
                            target: "forge.scheduler",
                            schedule = %task.id,
                            kind = "cron",
                            "Schedule executed"
                        );
                        if let Ok(diagnostics) = self.app.diagnostics() {
                            diagnostics.record_schedule_executed();
                        }
                        executed.push(task.id.clone());
                    }
                }
                ScheduleKind::Interval { every } => {
                    if interval_due(&self.last_interval_run, &task.id, *every, now) {
                        (task.handler)(self.app.clone()).await?;
                        tracing::info!(
                            target: "forge.scheduler",
                            schedule = %task.id,
                            kind = "interval",
                            interval_ms = every.as_millis() as u64,
                            "Schedule executed"
                        );
                        if let Ok(diagnostics) = self.app.diagnostics() {
                            diagnostics.record_schedule_executed();
                        }
                        executed.push(task.id.clone());
                    }
                }
            }
        }

        Ok(executed)
    }

    pub async fn run(self) -> Result<()> {
        let mut interval = tokio::time::interval(self.tick_interval);
        loop {
            interval.tick().await;
            let _ = self.run_once().await?;
        }
    }

    async fn ensure_leadership(&self) -> Result<bool> {
        let leader_active = self.leader_active.load(Ordering::Relaxed);
        if leader_active {
            if self
                .backend
                .renew_scheduler_leadership(&self.owner_id, self.leader_lease_ttl)
                .await?
            {
                self.leader_active.store(true, Ordering::Relaxed);
                if let Ok(diagnostics) = self.app.diagnostics() {
                    diagnostics.set_scheduler_leader_active(true);
                }
                return Ok(true);
            }

            self.leader_active.store(false, Ordering::Relaxed);
            tracing::warn!(
                target: "forge.scheduler",
                state = "lost",
                owner = %self.owner_id,
                "Scheduler leadership lost"
            );
            if let Ok(diagnostics) = self.app.diagnostics() {
                diagnostics.record_scheduler_leadership(SchedulerLeadershipState::Lost);
            }
            return Ok(false);
        }

        if self
            .backend
            .try_acquire_scheduler_leadership(&self.owner_id, self.leader_lease_ttl)
            .await?
        {
            self.leader_active.store(true, Ordering::Relaxed);
            tracing::info!(
                target: "forge.scheduler",
                state = "acquired",
                owner = %self.owner_id,
                "Scheduler leadership acquired"
            );
            if let Ok(diagnostics) = self.app.diagnostics() {
                diagnostics.record_scheduler_leadership(SchedulerLeadershipState::Acquired);
            }
            return Ok(true);
        }

        self.leader_active.store(false, Ordering::Relaxed);
        if let Ok(diagnostics) = self.app.diagnostics() {
            diagnostics.set_scheduler_leader_active(false);
        }
        Ok(false)
    }
}

impl Drop for SchedulerKernel {
    fn drop(&mut self) {
        let backend = self.backend.clone();
        let owner_id = self.owner_id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = backend.release_scheduler_leadership(&owner_id).await;
            });
        }
    }
}

fn interval_due(
    state: &Mutex<HashMap<ScheduleId, DateTime>>,
    id: &ScheduleId,
    every: Duration,
    now: DateTime,
) -> bool {
    let mut state = state.lock().expect("scheduler interval mutex poisoned");
    match state.get(id).cloned() {
        Some(last_run) => {
            if (now.as_chrono() - last_run.as_chrono())
                .to_std()
                .map(|elapsed| elapsed >= every)
                .unwrap_or(false)
            {
                state.insert(id.clone(), now);
                true
            } else {
                false
            }
        }
        None => {
            state.insert(id.clone(), now);
            false
        }
    }
}

fn next_owner_id() -> String {
    static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
    format!(
        "scheduler-{:x}-{:x}",
        DateTime::now().timestamp_micros(),
        NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
    )
}
