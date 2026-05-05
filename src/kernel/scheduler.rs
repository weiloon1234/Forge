use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::foundation::shutdown_drain::{
    drain_tasks, ShutdownDrainMessages, ShutdownDrainTarget, ShutdownDrainTask,
};
use crate::foundation::{AppContext, Result};
use crate::logging::SchedulerLeadershipState;
use crate::scheduler::{cron_due, ScheduleKind, ScheduleRegistry, ScheduledTask};
use crate::support::runtime::RuntimeBackend;
use crate::support::{DateTime, ScheduleId};
use tokio::task::JoinHandle;

pub struct SchedulerKernel {
    app: AppContext,
    tasks: Vec<ScheduledTask>,
    backend: RuntimeBackend,
    tick_interval: Duration,
    leader_lease_ttl: Duration,
    shutdown_timeout: Duration,
    owner_id: String,
    leader_active: AtomicBool,
    last_tick: Mutex<Option<DateTime>>,
    last_interval_run: Mutex<HashMap<ScheduleId, DateTime>>,
    active_tasks: Mutex<Vec<ScheduleTaskHandle>>,
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
            shutdown_timeout: Duration::from_millis(config.shutdown_timeout_ms),
            owner_id: next_owner_id(),
            leader_active: AtomicBool::new(false),
            last_tick: Mutex::new(None),
            last_interval_run: Mutex::new(HashMap::new()),
            active_tasks: Mutex::new(Vec::new()),
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
        self.prune_finished_tasks().await;

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

        // Check current environment for per-task environment filtering
        let current_env = self
            .app
            .config()
            .app()
            .map(|c| c.environment.to_string())
            .unwrap_or_else(|_| "development".to_string());

        let mut executed = Vec::new();
        for task in &self.tasks {
            let is_due = match &task.kind {
                ScheduleKind::Cron { expression } => cron_due(expression, previous, now),
                ScheduleKind::Interval { every } => {
                    interval_due(&self.last_interval_run, &task.id, *every, now)
                }
            };

            if !is_due {
                continue;
            }

            // Environment filter
            if !task.options.environments.is_empty()
                && !task.options.environments.iter().any(|e| e == &current_env)
            {
                continue;
            }

            let task_id = task.id.clone();
            let app = self.app.clone();
            let handler = task.handler.clone();
            let options = task.options.clone();
            let backend = self.backend.clone();
            let kind_label = match &task.kind {
                ScheduleKind::Cron { .. } => "cron",
                ScheduleKind::Interval { .. } => "interval",
            };

            // Spawn each task independently — no blocking the tick loop
            let diagnostics = self.app.diagnostics().ok();
            let spawned_id = task_id.clone();
            let handle = tokio::spawn(async move {
                let task_id = spawned_id;
                // Overlap prevention via distributed lock (Drop guard releases on panic too)
                let _lock_guard = if options.without_overlapping {
                    let lock_key = format!("schedule:{task_id}");
                    match backend.set_nx_value(&lock_key, "1", 3600).await {
                        Ok(true) => Some(ScheduleLockGuard {
                            backend: backend.clone(),
                            key: lock_key,
                        }),
                        Ok(false) => {
                            tracing::debug!(
                                target: "forge.scheduler",
                                schedule = %task_id,
                                "Skipped (previous invocation still running)"
                            );
                            return;
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "forge.scheduler",
                                schedule = %task_id,
                                error = %e,
                                "Failed to acquire overlap lock, running anyway"
                            );
                            None
                        }
                    }
                } else {
                    None
                };

                // Before hook
                if let Some(ref before) = options.before_hook {
                    if let Err(e) = before(app.clone()).await {
                        tracing::warn!(
                            target: "forge.scheduler",
                            schedule = %task_id,
                            error = %e,
                            "Before hook failed"
                        );
                    }
                }

                // Execute the task with error isolation
                let result = crate::logging::scope_current_execution(
                    crate::logging::ExecutionContext::Scheduler {
                        id: task_id.to_string(),
                    },
                    handler(app.clone()),
                )
                .await;

                match &result {
                    Ok(()) => {
                        tracing::info!(
                            target: "forge.scheduler",
                            schedule = %task_id,
                            kind = kind_label,
                            "Schedule executed"
                        );
                        if let Some(ref diagnostics) = diagnostics {
                            diagnostics.record_schedule_executed();
                        }

                        // After hook (success)
                        if let Some(ref after) = options.after_hook {
                            if let Err(e) = after(app.clone()).await {
                                tracing::warn!(
                                    target: "forge.scheduler",
                                    schedule = %task_id,
                                    error = %e,
                                    "After hook failed"
                                );
                            }
                        }
                    }
                    Err(error) => {
                        tracing::error!(
                            target: "forge.scheduler",
                            schedule = %task_id,
                            kind = kind_label,
                            error = %error,
                            "Schedule failed"
                        );

                        // On failure hook
                        if let Some(ref on_failure) = options.on_failure {
                            if let Err(e) = on_failure(app.clone()).await {
                                tracing::warn!(
                                    target: "forge.scheduler",
                                    schedule = %task_id,
                                    error = %e,
                                    "On-failure hook failed"
                                );
                            }
                        }
                    }
                }

                // _lock_guard releases via Drop (safe on panic too)
                drop(_lock_guard);
            });
            self.track_active_task(handle);

            executed.push(task_id);
        }

        Ok(executed)
    }

    pub async fn run(self) -> Result<()> {
        self.run_until(super::shutdown::shutdown_signal()).await
    }

    async fn run_until<S>(self, shutdown: S) -> Result<()>
    where
        S: Future<Output = ()>,
    {
        let mut interval = tokio::time::interval(self.tick_interval);
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    tracing::info!(target: "forge.scheduler", "scheduler shutdown requested");
                    break;
                }
                _ = interval.tick() => {
                    // Error from run_once is only from leadership — not from tasks
                    // (tasks are spawned and isolated). Leadership errors are recoverable.
                    if let Err(e) = self.run_once().await {
                        tracing::warn!(
                            target: "forge.scheduler",
                            error = %e,
                            "Scheduler tick error (leadership), will retry"
                        );
                    }
                }
            }
        }

        self.drain_active_tasks().await;
        Ok(())
    }

    fn track_active_task(&self, handle: JoinHandle<()>) {
        self.active_tasks
            .lock()
            .expect("scheduler active task mutex poisoned")
            .push(ScheduleTaskHandle(handle));
    }

    async fn prune_finished_tasks(&self) {
        let mut finished = Vec::new();
        {
            let mut active_tasks = self
                .active_tasks
                .lock()
                .expect("scheduler active task mutex poisoned");
            let mut index = 0;
            while index < active_tasks.len() {
                if active_tasks[index].is_finished() {
                    finished.push(active_tasks.swap_remove(index));
                } else {
                    index += 1;
                }
            }
        }

        for handle in finished {
            handle.wait_finished().await;
        }
    }

    async fn drain_active_tasks(&self) {
        let active_tasks = {
            let mut active_tasks = self
                .active_tasks
                .lock()
                .expect("scheduler active task mutex poisoned");
            std::mem::take(&mut *active_tasks)
        };

        drain_tasks(
            active_tasks,
            self.shutdown_timeout,
            ShutdownDrainMessages {
                target: ShutdownDrainTarget::Scheduler,
                timeout_disabled:
                    "scheduler shutdown timeout disabled; aborting active schedule tasks",
                waiting: "waiting for active schedule tasks during shutdown",
                drained: "active schedule tasks drained",
                timeout_elapsed:
                    "scheduler shutdown timeout elapsed; aborting active schedule tasks",
            },
        )
        .await;
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

struct ScheduleTaskHandle(JoinHandle<()>);

#[async_trait::async_trait]
impl ShutdownDrainTask for ScheduleTaskHandle {
    fn is_finished(&mut self) -> bool {
        self.0.is_finished()
    }

    async fn wait_finished(self) {
        if let Err(error) = self.0.await {
            tracing::warn!(
                target: "forge.scheduler",
                error = %error,
                "Schedule task finished with join error"
            );
        }
    }

    fn abort(&self) {
        self.0.abort();
    }

    async fn wait_after_abort(self) {
        let _ = self.0.await;
    }
}

/// Drop guard that releases a schedule overlap lock, even on panic.
struct ScheduleLockGuard {
    backend: RuntimeBackend,
    key: String,
}

impl Drop for ScheduleLockGuard {
    fn drop(&mut self) {
        let backend = self.backend.clone();
        let key = std::mem::take(&mut self.key);
        if !key.is_empty() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = backend.del_key(&key).await;
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tempfile::tempdir;

    #[tokio::test]
    async fn scheduler_run_exits_when_shutdown_future_completes() {
        let kernel = crate::App::builder()
            .build_scheduler_kernel()
            .await
            .unwrap();

        kernel.run_until(async {}).await.unwrap();
    }

    #[tokio::test]
    async fn scheduler_shutdown_waits_for_active_schedule_tasks() {
        let completed = Arc::new(AtomicBool::new(false));
        let kernel = crate::App::builder()
            .build_scheduler_kernel()
            .await
            .unwrap();

        let task_completed = completed.clone();
        kernel.track_active_task(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            task_completed.store(true, Ordering::SeqCst);
        }));

        kernel.drain_active_tasks().await;

        assert!(completed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn scheduler_shutdown_aborts_active_schedule_tasks_after_timeout() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("scheduler.toml"),
            r#"
            [scheduler]
            shutdown_timeout_ms = 1
            "#,
        )
        .unwrap();

        let completed = Arc::new(AtomicBool::new(false));
        let kernel = crate::App::builder()
            .load_config_dir(dir.path())
            .build_scheduler_kernel()
            .await
            .unwrap();

        let task_completed = completed.clone();
        kernel.track_active_task(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            task_completed.store(true, Ordering::SeqCst);
        }));

        tokio::time::timeout(Duration::from_millis(100), kernel.drain_active_tasks())
            .await
            .unwrap();

        assert!(!completed.load(Ordering::SeqCst));
    }
}
