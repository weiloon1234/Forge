mod backend;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::FutureExt;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::config::JobsConfig;
use crate::database::{DbType, DbValue};
use crate::foundation::shutdown_drain::{
    drain_tasks, ShutdownDrainMessages, ShutdownDrainTarget, ShutdownDrainTask,
};
use crate::foundation::{AppContext, Error, Result};
use crate::logging::{JobOutcome as RecordedJobOutcome, RuntimeDiagnostics};
use crate::support::runtime::RuntimeBackend;
use crate::support::{JobId, QueueId};

use self::backend::{ClaimedJobLease, JobToEnqueue, SuccessfulJobEffects};

const INVALID_JOB_ENVELOPE_ID: JobId = JobId::new("forge.invalid_job_envelope");

// ---------------------------------------------------------------------------
// Job middleware
// ---------------------------------------------------------------------------

#[async_trait]
pub trait JobMiddleware: Send + Sync + 'static {
    async fn before(&self, _job_id: &JobId, _context: &JobContext) -> Result<()> {
        Ok(())
    }
    async fn after(&self, _job_id: &JobId, _context: &JobContext) -> Result<()> {
        Ok(())
    }
    async fn failed(&self, _job_id: &JobId, _context: &JobContext, _error: &str) -> Result<()> {
        Ok(())
    }

    async fn on_dead_lettered(&self, _context: &JobDeadLetterContext) -> Result<()> {
        Ok(())
    }
}

pub(crate) type JobMiddlewareRegistryHandle = Arc<Mutex<JobMiddlewareRegistryBuilder>>;

#[derive(Default)]
pub(crate) struct JobMiddlewareRegistryBuilder {
    middlewares: Vec<Arc<dyn JobMiddleware>>,
}

impl JobMiddlewareRegistryBuilder {
    pub(crate) fn shared() -> JobMiddlewareRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn register(&mut self, middleware: Arc<dyn JobMiddleware>) {
        self.middlewares.push(middleware);
    }

    pub(crate) fn freeze_shared(handle: JobMiddlewareRegistryHandle) -> JobMiddlewareRegistry {
        let mut builder = handle
            .lock()
            .expect("job middleware registry lock poisoned");
        JobMiddlewareRegistry {
            middlewares: std::mem::take(&mut builder.middlewares),
        }
    }
}

pub struct JobMiddlewareRegistry {
    middlewares: Vec<Arc<dyn JobMiddleware>>,
}

impl JobMiddlewareRegistry {
    async fn run_before(&self, job_id: &JobId, context: &JobContext) {
        for mw in &self.middlewares {
            match AssertUnwindSafe(mw.before(job_id, context))
                .catch_unwind()
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(
                        target: "forge.worker",
                        job = %job_id,
                        error = %error,
                        "job middleware before hook failed"
                    );
                }
                Err(panic) => {
                    tracing::warn!(
                        target: "forge.worker",
                        job = %job_id,
                        panic = %panic_payload_message(panic),
                        "job middleware before hook panicked"
                    );
                }
            }
        }
    }

    async fn run_after(&self, job_id: &JobId, context: &JobContext) {
        for mw in &self.middlewares {
            match AssertUnwindSafe(mw.after(job_id, context))
                .catch_unwind()
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(
                        target: "forge.worker",
                        job = %job_id,
                        error = %error,
                        "job middleware after hook failed"
                    );
                }
                Err(panic) => {
                    tracing::warn!(
                        target: "forge.worker",
                        job = %job_id,
                        panic = %panic_payload_message(panic),
                        "job middleware after hook panicked"
                    );
                }
            }
        }
    }

    async fn run_failed(&self, job_id: &JobId, context: &JobContext, err: &str) {
        for mw in &self.middlewares {
            match AssertUnwindSafe(mw.failed(job_id, context, err))
                .catch_unwind()
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(
                        target: "forge.worker",
                        job = %job_id,
                        error = %error,
                        "job middleware failed hook failed"
                    );
                }
                Err(panic) => {
                    tracing::warn!(
                        target: "forge.worker",
                        job = %job_id,
                        panic = %panic_payload_message(panic),
                        "job middleware failed hook panicked"
                    );
                }
            }
        }
    }

    async fn run_dead_lettered(&self, context: &JobDeadLetterContext) {
        for mw in &self.middlewares {
            match AssertUnwindSafe(mw.on_dead_lettered(context))
                .catch_unwind()
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(
                        target: "forge.worker",
                        job = %context.class,
                        job_id = %context.id,
                        error = %error,
                        "job middleware dead-letter hook failed"
                    );
                }
                Err(panic) => {
                    tracing::warn!(
                        target: "forge.worker",
                        job = %context.class,
                        job_id = %context.id,
                        panic = %panic_payload_message(panic),
                        "job middleware dead-letter hook panicked"
                    );
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct JobContext {
    app: AppContext,
    queue: QueueId,
    attempt: u32,
}

impl JobContext {
    fn new(app: AppContext, queue: QueueId, attempt: u32) -> Self {
        Self {
            app,
            queue,
            attempt,
        }
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub fn queue(&self) -> &QueueId {
        &self.queue
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }
}

#[derive(Clone)]
pub struct JobDeadLetterContext {
    pub class: String,
    pub id: String,
    pub attempts: u32,
    pub last_error: String,
    pub payload: serde_json::Value,
    pub app: AppContext,
}

#[async_trait]
pub trait Job: Serialize + DeserializeOwned + Send + Sync + Debug + 'static {
    const ID: JobId;
    const QUEUE: Option<QueueId> = None;

    async fn handle(&self, context: JobContext) -> Result<()>;

    fn max_retries(&self) -> Option<u32> {
        None
    }

    fn backoff(&self, attempt: u32) -> Duration {
        match attempt {
            1 => Duration::from_secs(5),
            2 => Duration::from_secs(30),
            3 => Duration::from_secs(60),
            4 => Duration::from_secs(300),
            _ => Duration::from_secs(600),
        }
    }

    /// Maximum execution time for this job. Override for long-running jobs.
    /// Default uses the global `timeout_seconds` config (300s / 5 minutes).
    fn timeout(&self) -> Option<Duration> {
        None // None = use global config default
    }

    /// Optional rate limit for this job type.
    /// Returns `(max_per_window, window_duration)`. When the limit is
    /// exceeded the job is requeued with a short delay instead of being
    /// counted as a retry attempt.
    fn rate_limit(&self) -> Option<(u32, Duration)> {
        None
    }

    /// If set, prevents duplicate dispatch of this job type within the
    /// returned duration. A second dispatch with the same unique key
    /// inside the window is silently dropped.
    fn unique_for(&self) -> Option<Duration> {
        None
    }

    /// Key used for the uniqueness check. Defaults to the job ID when
    /// `None` is returned. Override to include payload-specific fields
    /// (e.g. a user ID) so that *different* payloads are treated as
    /// distinct jobs.
    fn unique_key(&self) -> Option<String> {
        None
    }
}

#[derive(Clone)]
pub struct JobDispatcher {
    runtime: Arc<JobRuntime>,
    diagnostics: Arc<RuntimeDiagnostics>,
}

struct UniqueJobReservation {
    key: String,
    owner: String,
    job_id: JobId,
    unique_key: String,
}

impl UniqueJobReservation {
    async fn rollback(&self, backend: &RuntimeBackend) {
        match backend.del_if_value(&self.key, &self.owner).await {
            Ok(true) => {
                tracing::debug!(
                    target: "forge.worker",
                    job = %self.job_id,
                    unique_key = %self.unique_key,
                    "Rolled back unique job reservation after dispatch failure"
                );
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(
                    target: "forge.worker",
                    job = %self.job_id,
                    unique_key = %self.unique_key,
                    error = %error,
                    "Failed to roll back unique job reservation after dispatch failure"
                );
            }
        }
    }
}

impl JobDispatcher {
    pub(crate) fn new(runtime: Arc<JobRuntime>, diagnostics: Arc<RuntimeDiagnostics>) -> Self {
        Self {
            runtime,
            diagnostics,
        }
    }

    pub async fn dispatch<J>(&self, job: J) -> Result<()>
    where
        J: Job,
    {
        self.dispatch_at(job, Utc::now().timestamp_millis()).await
    }

    pub async fn dispatch_later<J>(&self, job: J, run_at_millis: i64) -> Result<()>
    where
        J: Job,
    {
        self.dispatch_at(job, run_at_millis).await
    }

    async fn dispatch_at<J>(&self, job: J, run_at_millis: i64) -> Result<()>
    where
        J: Job,
    {
        let mut unique_reservation = None;

        // Unique job check: skip dispatch if a duplicate exists within the window
        if let Some(unique_duration) = job.unique_for() {
            let unique_suffix = job.unique_key().unwrap_or_else(|| J::ID.to_string());
            let unique_redis_key = format!("jobs:unique:{}:{}", J::ID, unique_suffix);
            let unique_owner = next_delivery_token();
            let ttl_secs = unique_duration.as_secs().max(1);
            let is_new = self
                .runtime
                .backend
                .set_nx_value(&unique_redis_key, &unique_owner, ttl_secs)
                .await?;
            if !is_new {
                tracing::debug!(
                    target: "forge.worker",
                    job = %J::ID,
                    unique_key = %unique_suffix,
                    "Skipping duplicate job dispatch (unique constraint)"
                );
                return Ok(());
            }

            unique_reservation = Some(UniqueJobReservation {
                key: unique_redis_key,
                owner: unique_owner,
                job_id: J::ID,
                unique_key: unique_suffix,
            });
        }

        let dispatch_result = async {
            let queue = J::QUEUE
                .clone()
                .unwrap_or_else(|| self.runtime.config.queue.clone());
            let envelope = JobEnvelope {
                job: J::ID,
                queue: queue.clone(),
                attempts: 0,
                scheduled_at: run_at_millis,
                payload: serde_json::to_value(job).map_err(Error::other)?,
                batch_id: None,
                chain_remaining: None,
            };
            let payload = serde_json::to_string(&envelope).map_err(Error::other)?;
            let token = next_delivery_token();

            if run_at_millis > Utc::now().timestamp_millis() {
                self.runtime
                    .backend
                    .schedule_job(&queue, &token, &payload, run_at_millis)
                    .await?;
            } else {
                self.runtime
                    .backend
                    .enqueue_job(&queue, &token, &payload)
                    .await?;
            }

            self.diagnostics
                .record_job_outcome(RecordedJobOutcome::Enqueued);

            Ok(())
        }
        .await;

        if let Err(error) = dispatch_result {
            if let Some(reservation) = &unique_reservation {
                reservation.rollback(&self.runtime.backend).await;
            }
            return Err(error);
        }

        Ok(())
    }

    /// Start building a batch of jobs that execute concurrently with an
    /// optional completion callback.
    pub fn batch(&self, name: &str) -> JobBatchBuilder {
        JobBatchBuilder {
            dispatcher: self.clone(),
            name: name.to_string(),
            jobs: Vec::new(),
            on_complete: None,
        }
    }

    /// Start building a chain of jobs that execute sequentially.
    pub fn chain(&self) -> JobChainBuilder {
        JobChainBuilder {
            dispatcher: self.clone(),
            jobs: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Job batching
// ---------------------------------------------------------------------------

/// Builder for dispatching a group of jobs with an optional completion callback.
pub struct JobBatchBuilder {
    dispatcher: JobDispatcher,
    name: String,
    jobs: Vec<(JobId, QueueId, serde_json::Value)>,
    on_complete: Option<(JobId, QueueId, serde_json::Value)>,
}

impl JobBatchBuilder {
    /// Add a job to the batch.
    #[allow(clippy::should_implement_trait)]
    pub fn add<J: Job>(mut self, job: J) -> Result<Self> {
        let queue = J::QUEUE
            .clone()
            .unwrap_or_else(|| self.dispatcher.runtime.config.queue.clone());
        let payload = serde_json::to_value(&job).map_err(Error::other)?;
        self.jobs.push((J::ID, queue, payload));
        Ok(self)
    }

    /// Set a callback job that fires when all batch jobs complete successfully.
    pub fn on_complete<J: Job>(mut self, job: J) -> Result<Self> {
        let queue = J::QUEUE
            .clone()
            .unwrap_or_else(|| self.dispatcher.runtime.config.queue.clone());
        let payload = serde_json::to_value(&job).map_err(Error::other)?;
        self.on_complete = Some((J::ID, queue, payload));
        Ok(self)
    }

    /// Dispatch all batch jobs. Returns the batch ID.
    pub async fn dispatch(self) -> Result<String> {
        if self.jobs.is_empty() {
            return Err(Error::message("cannot dispatch an empty batch"));
        }

        let batch_id = format!("batch-{}-{}", self.name, next_delivery_token());
        let on_complete_payload = match &self.on_complete {
            Some((job_id, queue, payload)) => {
                let envelope = JobEnvelope {
                    job: job_id.clone(),
                    queue: queue.clone(),
                    attempts: 0,
                    scheduled_at: 0,
                    payload: payload.clone(),
                    batch_id: None,
                    chain_remaining: None,
                };
                Some(serde_json::to_string(&envelope).map_err(Error::other)?)
            }
            None => None,
        };
        let on_complete_queue = self.on_complete.as_ref().map(|(_, q, _)| q.to_string());

        let now = Utc::now().timestamp_millis();
        let mut jobs = Vec::with_capacity(self.jobs.len());
        for (job_id, queue, payload) in self.jobs {
            let envelope = JobEnvelope {
                job: job_id,
                queue: queue.clone(),
                attempts: 0,
                scheduled_at: now,
                payload,
                batch_id: Some(batch_id.clone()),
                chain_remaining: None,
            };
            let serialized = serde_json::to_string(&envelope).map_err(Error::other)?;
            let token = next_delivery_token();
            jobs.push(JobToEnqueue {
                queue,
                token,
                payload: serialized,
            });
        }

        let enqueued = self
            .dispatcher
            .runtime
            .backend
            .dispatch_batch(
                &batch_id,
                on_complete_payload.as_deref(),
                on_complete_queue.as_deref(),
                jobs,
            )
            .await?;

        for _ in 0..enqueued {
            self.dispatcher
                .diagnostics
                .record_job_outcome(RecordedJobOutcome::Enqueued);
        }

        tracing::info!(
            target: "forge.worker",
            batch_id = %batch_id,
            total = enqueued,
            "Batch dispatched"
        );

        Ok(batch_id)
    }
}

// ---------------------------------------------------------------------------
// Job chaining
// ---------------------------------------------------------------------------

/// Builder for dispatching a sequence of jobs that run one after another.
pub struct JobChainBuilder {
    dispatcher: JobDispatcher,
    jobs: Vec<ChainedJob>,
}

impl JobChainBuilder {
    /// Add a job to the end of the chain.
    #[allow(clippy::should_implement_trait)]
    pub fn add<J: Job>(mut self, job: J) -> Result<Self> {
        let queue = J::QUEUE
            .clone()
            .unwrap_or_else(|| self.dispatcher.runtime.config.queue.clone());
        let payload = serde_json::to_value(&job).map_err(Error::other)?;
        self.jobs.push(ChainedJob {
            job: J::ID,
            queue,
            payload,
        });
        Ok(self)
    }

    /// Dispatch the chain. Only the first job is enqueued immediately;
    /// subsequent jobs are stored in the envelope and dispatched on success.
    pub async fn dispatch(mut self) -> Result<()> {
        if self.jobs.is_empty() {
            return Err(Error::message("cannot dispatch an empty chain"));
        }

        let first = self.jobs.remove(0);
        let remaining = if self.jobs.is_empty() {
            None
        } else {
            Some(self.jobs)
        };

        let now = Utc::now().timestamp_millis();
        let envelope = JobEnvelope {
            job: first.job,
            queue: first.queue.clone(),
            attempts: 0,
            scheduled_at: now,
            payload: first.payload,
            batch_id: None,
            chain_remaining: remaining,
        };
        let serialized = serde_json::to_string(&envelope).map_err(Error::other)?;
        let token = next_delivery_token();
        self.dispatcher
            .runtime
            .backend
            .enqueue_job(&first.queue, &token, &serialized)
            .await?;
        self.dispatcher
            .diagnostics
            .record_job_outcome(RecordedJobOutcome::Enqueued);

        Ok(())
    }
}

pub struct Worker {
    app: AppContext,
    runtime: Arc<JobRuntime>,
    diagnostics: Arc<RuntimeDiagnostics>,
}

impl Worker {
    pub fn from_app(app: AppContext) -> Result<Self> {
        let runtime = app.job_runtime()?;
        let diagnostics = app.diagnostics()?;
        Ok(Self {
            app,
            runtime,
            diagnostics,
        })
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    /// Run the worker. Spawns a tokio task per claimed job (goroutine-style).
    /// When `max_concurrent_jobs` is set (> 0), a semaphore bounds concurrency.
    /// When 0 (default), jobs spawn without limit — true goroutine behavior.
    pub async fn run(self) -> Result<()> {
        self.run_until(crate::kernel::shutdown::shutdown_signal())
            .await
    }

    pub(crate) async fn run_until<S>(self, shutdown: S) -> Result<()>
    where
        S: Future<Output = ()> + Send + 'static,
    {
        // 0 = unlimited (use a large semaphore that never blocks in practice)
        let max_concurrent = if self.runtime.config.max_concurrent_jobs == 0 {
            u32::MAX >> 1 // ~1 billion — effectively unlimited
        } else {
            self.runtime.config.max_concurrent_jobs as u32
        };
        let shutdown_timeout = self.runtime.shutdown_timeout();
        let worker = Arc::new(self);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent as usize));
        let active_jobs = Arc::new(ActiveWorkerJobs::new(shutdown_timeout));

        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        let shutdown_handle = {
            let tx = shutdown_tx.clone();
            tokio::spawn(async move {
                shutdown.await;
                let _ = tx.send(true);
            })
        };
        let mut shutdown_rx = shutdown_tx.subscribe();

        tracing::info!(
            target: "forge.worker",
            max_concurrent = max_concurrent,
            "worker started"
        );

        // Separate maintenance task — runs on its own timer, not on every claim
        let maintenance_worker = worker.clone();
        let mut maintenance_shutdown = shutdown_tx.subscribe();
        let maintenance_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(maintenance_worker.runtime.poll_interval());
            loop {
                tokio::select! {
                    biased;
                    _ = maintenance_shutdown.changed() => break,
                    _ = interval.tick() => {
                        let now_millis = Utc::now().timestamp_millis();
                        let _ = maintenance_worker.runtime.promote_due_jobs(now_millis).await;
                        let requeued = maintenance_worker.runtime.requeue_expired_jobs(now_millis).await.unwrap_or(0);
                        for _ in 0..requeued {
                            maintenance_worker.diagnostics.record_job_outcome(RecordedJobOutcome::ExpiredLeaseRequeued);
                        }
                    }
                }
            }
        });

        loop {
            active_jobs.prune_finished().await;

            if *shutdown_rx.borrow() {
                maintenance_handle.abort();
                let _ = maintenance_handle.await;
                active_jobs.drain().await;
                tracing::info!(target: "forge.worker", "worker stopped");
                break;
            }

            // Acquire permit before claiming — bounds concurrency
            let permit = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => continue,
                permit = semaphore.clone().acquire_owned() => match permit {
                    Ok(p) => p,
                    Err(_) => break,
                }
            };

            let claim = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    drop(permit);
                    continue;
                }
                claim = worker.runtime.claim_job() => claim,
            };

            match claim {
                Ok(Some(lease)) => {
                    worker
                        .diagnostics
                        .record_job_outcome(RecordedJobOutcome::Leased);
                    let w = worker.clone();
                    let handle = tokio::spawn(async move {
                        if let Err(error) = w.process_claimed_job(lease).await {
                            tracing::error!(target: "forge.worker", error = %error, "job processing failed");
                        }
                        drop(permit);
                    });
                    active_jobs.track(handle);
                }
                Ok(None) => {
                    drop(permit);
                    Self::sleep_or_shutdown(&mut shutdown_rx, worker.runtime.poll_interval()).await;
                }
                Err(error) => {
                    drop(permit);
                    tracing::error!(target: "forge.worker", error = %error, "claim failed");
                    Self::sleep_or_shutdown(&mut shutdown_rx, worker.runtime.poll_interval()).await;
                }
            }
        }

        shutdown_handle.abort();
        let _ = shutdown_handle.await;

        Ok(())
    }

    async fn sleep_or_shutdown(
        shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
        duration: Duration,
    ) {
        if *shutdown_rx.borrow() {
            return;
        }

        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {}
            _ = tokio::time::sleep(duration) => {}
        }
    }

    pub async fn run_once(&self) -> Result<bool> {
        let now_millis = Utc::now().timestamp_millis();
        let promoted = self.runtime.promote_due_jobs(now_millis).await?;
        let requeued = self.runtime.requeue_expired_jobs(now_millis).await?;
        for _ in 0..requeued {
            self.diagnostics
                .record_job_outcome(RecordedJobOutcome::ExpiredLeaseRequeued);
        }

        if let Some(lease) = self.runtime.claim_job().await? {
            self.diagnostics
                .record_job_outcome(RecordedJobOutcome::Leased);
            self.process_claimed_job(lease).await?;
            return Ok(true);
        }

        Ok(promoted > 0 || requeued > 0)
    }

    async fn process_claimed_job(&self, lease: ClaimedJobLease) -> Result<()> {
        self.diagnostics
            .record_job_outcome(RecordedJobOutcome::Started);

        let started_at = Utc::now().timestamp_millis();
        let middleware = self.app.resolve::<JobMiddlewareRegistry>().ok();
        let envelope: JobEnvelope = match serde_json::from_str(&lease.payload) {
            Ok(envelope) => envelope,
            Err(error) => {
                let poison_envelope = JobEnvelope {
                    job: INVALID_JOB_ENVELOPE_ID,
                    queue: lease.queue.clone(),
                    attempts: 0,
                    scheduled_at: started_at,
                    payload: serde_json::Value::String(lease.payload.clone()),
                    batch_id: None,
                    chain_remaining: None,
                };
                let job_context = JobContext::new(self.app.clone(), lease.queue.clone(), 1);
                self.dead_letter_claimed_job(DeadLetterClaimedJob {
                    lease: &lease,
                    envelope: poison_envelope,
                    error: format!("job envelope could not be deserialized: {error}"),
                    attempts: 1,
                    started_at,
                    middleware: middleware.as_deref(),
                    job_context: Some(&job_context),
                })
                .await?;
                return Ok(());
            }
        };
        let Some(registration) = self.runtime.registry.jobs.get(&envelope.job) else {
            let attempts = envelope.attempts + 1;
            let job_context = JobContext::new(self.app.clone(), envelope.queue.clone(), attempts);
            let error = format!("job `{}` is not registered", envelope.job);
            self.dead_letter_claimed_job(DeadLetterClaimedJob {
                lease: &lease,
                envelope,
                error,
                attempts,
                started_at,
                middleware: middleware.as_deref(),
                job_context: Some(&job_context),
            })
            .await?;
            return Ok(());
        };

        // Rate limit check: requeue without incrementing attempts if over limit
        if let Some((max_per_window, window)) = registration.handler.check_rate_limit(&envelope) {
            let window_secs = window.as_secs().max(1);
            let window_bucket = Utc::now().timestamp() / window_secs as i64;
            let rate_key = format!("jobs:rate:{}:{}", envelope.job, window_bucket);
            let current_count = self
                .runtime
                .backend
                .incr_with_ttl(&rate_key, window_secs)
                .await?;
            if current_count > max_per_window as u64 {
                // Over the rate limit — requeue with the same attempt count
                // and a short delay so it retries soon without counting as a failure.
                let delay_ms = 1000; // 1 second delay before retry
                let requeue_at = Utc::now().timestamp_millis() + delay_ms;
                let requeue_envelope = JobEnvelope {
                    scheduled_at: requeue_at,
                    ..envelope
                };
                let payload = serde_json::to_string(&requeue_envelope).map_err(Error::other)?;
                let requeue_token = next_delivery_token();
                if !self
                    .runtime
                    .retry_job(
                        &lease.queue,
                        &lease.token,
                        &requeue_token,
                        &payload,
                        requeue_at,
                    )
                    .await?
                {
                    tracing::warn!(
                        target: "forge.worker",
                        queue = %lease.queue,
                        token = %lease.token,
                        "Lost job lease before rate-limit requeue"
                    );
                    return Ok(());
                }
                tracing::debug!(
                    target: "forge.worker",
                    job = %requeue_envelope.job,
                    count = current_count,
                    limit = max_per_window,
                    "Job rate-limited, requeued with delay"
                );
                return Ok(());
            }
        }

        let job_context = JobContext::new(
            self.app.clone(),
            envelope.queue.clone(),
            envelope.attempts + 1,
        );

        // Before hooks
        if let Some(ref mw) = middleware {
            mw.run_before(&envelope.job, &job_context).await;
        }

        let heartbeat = self.spawn_lease_heartbeat(lease.queue.clone(), lease.token.clone());
        let default_timeout = Duration::from_secs(self.runtime.config.timeout_seconds.max(1));
        let execution = crate::logging::scope_current_execution(
            crate::logging::ExecutionContext::Job {
                class: envelope.job.to_string(),
                id: lease.token.clone(),
            },
            registration.handler.execute(
                &self.app,
                &envelope,
                self.runtime.config.max_retries,
                default_timeout,
            ),
        )
        .await;
        heartbeat.shutdown().await;
        let execution = execution?;

        match execution {
            JobExecutionOutcome::Success => {
                if let Some(ref mw) = middleware {
                    mw.run_after(&envelope.job, &job_context).await;
                }
                let chain_effect =
                    Self::build_chain_continuation(envelope.chain_remaining.clone())?;
                let success = self
                    .runtime
                    .complete_successful_job(
                        &lease.queue,
                        &lease.token,
                        SuccessfulJobEffects {
                            chain: chain_effect,
                            batch_id: envelope.batch_id.clone(),
                            batch_callback_token: envelope
                                .batch_id
                                .as_ref()
                                .map(|_| next_delivery_token()),
                        },
                    )
                    .await?;
                if !success.lease_released {
                    tracing::warn!(
                        target: "forge.worker",
                        queue = %lease.queue,
                        token = %lease.token,
                        "Lost job lease before success finalization"
                    );
                    return Ok(());
                }
                tracing::info!(
                    target: "forge.worker",
                    job = %envelope.job,
                    queue = %envelope.queue,
                    attempt = envelope.attempts + 1,
                    "Job succeeded"
                );
                self.diagnostics
                    .record_job_outcome(RecordedJobOutcome::Succeeded);

                let duration_ms = Utc::now().timestamp_millis() - started_at;
                self.record_job_history(JobHistoryEntry {
                    job_id: &envelope.job,
                    queue: &envelope.queue,
                    status: JobHistoryStatus::Succeeded,
                    attempt: envelope.attempts + 1,
                    error: None,
                    started_at,
                    duration_ms,
                })
                .await;

                if let Some(ref batch_id) = envelope.batch_id {
                    if let Some(batch) = success.batch {
                        tracing::debug!(
                            target: "forge.worker",
                            batch_id = %batch_id,
                            completed = batch.completed,
                            total = batch.total,
                            "Batch progress"
                        );
                        if batch.completed >= batch.total {
                            if batch.on_complete_enqueued {
                                self.diagnostics
                                    .record_job_outcome(RecordedJobOutcome::Enqueued);
                                tracing::info!(
                                    target: "forge.worker",
                                    batch_id = %batch_id,
                                    "Batch completed, on_complete job dispatched"
                                );
                            } else {
                                tracing::info!(
                                    target: "forge.worker",
                                    batch_id = %batch_id,
                                    "Batch completed"
                                );
                            }
                        }
                    } else {
                        tracing::warn!(
                            target: "forge.worker",
                            batch_id = %batch_id,
                            "Batch metadata missing during success finalization"
                        );
                    }
                }

                if success.chain_enqueued {
                    self.diagnostics
                        .record_job_outcome(RecordedJobOutcome::Enqueued);
                    tracing::info!(
                        target: "forge.worker",
                        job = %envelope.job,
                        "Chain continuation dispatched"
                    );
                }

                Ok(())
            }
            JobExecutionOutcome::Retry {
                run_at_millis,
                attempts,
                error,
            } => {
                if let Some(ref mw) = middleware {
                    mw.run_failed(&envelope.job, &job_context, &error).await;
                }
                let retry_job_id = envelope.job.clone();
                let retry_queue = envelope.queue.clone();
                let retry_envelope = JobEnvelope {
                    attempts,
                    scheduled_at: run_at_millis,
                    ..envelope
                };
                let payload = serde_json::to_string(&retry_envelope).map_err(Error::other)?;
                let retry_token = next_delivery_token();
                if !self
                    .runtime
                    .retry_job(
                        &lease.queue,
                        &lease.token,
                        &retry_token,
                        &payload,
                        run_at_millis,
                    )
                    .await?
                {
                    tracing::warn!(
                        target: "forge.worker",
                        queue = %lease.queue,
                        token = %lease.token,
                        "Lost job lease before retry scheduling"
                    );
                    return Ok(());
                }
                self.diagnostics
                    .record_job_outcome(RecordedJobOutcome::Retried);

                let duration_ms = Utc::now().timestamp_millis() - started_at;
                self.record_job_history(JobHistoryEntry {
                    job_id: &retry_job_id,
                    queue: &retry_queue,
                    status: JobHistoryStatus::Retried,
                    attempt: attempts,
                    error: Some(&error),
                    started_at,
                    duration_ms,
                })
                .await;

                Ok(())
            }
            JobExecutionOutcome::DeadLetter { error, attempts } => {
                if let Some(ref mw) = middleware {
                    mw.run_failed(&envelope.job, &job_context, &error).await;
                }
                let job_name = envelope.job.clone();
                let queue_name = envelope.queue.clone();
                let payload_json = envelope.payload.clone();
                let dead_letter = FailedJobEnvelope {
                    failed_at: Utc::now().timestamp_millis(),
                    error: error.clone(),
                    envelope: JobEnvelope {
                        attempts,
                        ..envelope
                    },
                };
                let payload = serde_json::to_string(&dead_letter).map_err(Error::other)?;
                if !self
                    .runtime
                    .dead_letter_job(&lease.queue, &lease.token, &payload)
                    .await?
                {
                    tracing::warn!(
                        target: "forge.worker",
                        queue = %lease.queue,
                        token = %lease.token,
                        "Lost job lease before dead-letter transition"
                    );
                    return Ok(());
                }
                tracing::error!(
                    target: "forge.worker",
                    job = %job_name,
                    queue = %queue_name,
                    attempts = attempts,
                    error = %error,
                    "Job dead-lettered"
                );
                self.diagnostics
                    .record_job_outcome(RecordedJobOutcome::DeadLettered);

                let duration_ms = Utc::now().timestamp_millis() - started_at;
                self.record_job_history(JobHistoryEntry {
                    job_id: &job_name,
                    queue: &queue_name,
                    status: JobHistoryStatus::DeadLettered,
                    attempt: attempts,
                    error: Some(&error),
                    started_at,
                    duration_ms,
                })
                .await;

                if let Some(ref mw) = middleware {
                    mw.run_dead_lettered(&JobDeadLetterContext {
                        class: job_name.to_string(),
                        id: lease.token.clone(),
                        attempts,
                        last_error: error.clone(),
                        payload: payload_json,
                        app: self.app.clone(),
                    })
                    .await;
                }

                Ok(())
            }
        }
    }

    async fn dead_letter_claimed_job(&self, job: DeadLetterClaimedJob<'_>) -> Result<()> {
        let DeadLetterClaimedJob {
            lease,
            envelope,
            error,
            attempts,
            started_at,
            middleware,
            job_context,
        } = job;

        if let (Some(middleware), Some(job_context)) = (middleware, job_context) {
            middleware
                .run_failed(&envelope.job, job_context, &error)
                .await;
        }

        let job_name = envelope.job.clone();
        let queue_name = envelope.queue.clone();
        let payload_json = envelope.payload.clone();
        let dead_letter = FailedJobEnvelope {
            failed_at: Utc::now().timestamp_millis(),
            error: error.clone(),
            envelope: JobEnvelope {
                attempts,
                ..envelope
            },
        };
        let payload = serde_json::to_string(&dead_letter).map_err(Error::other)?;
        if !self
            .runtime
            .dead_letter_job(&lease.queue, &lease.token, &payload)
            .await?
        {
            tracing::warn!(
                target: "forge.worker",
                queue = %lease.queue,
                token = %lease.token,
                "Lost job lease before poison dead-letter transition"
            );
            return Ok(());
        }

        tracing::error!(
            target: "forge.worker",
            job = %job_name,
            queue = %queue_name,
            attempts = attempts,
            error = %error,
            "Job dead-lettered"
        );
        self.diagnostics
            .record_job_outcome(RecordedJobOutcome::DeadLettered);

        let duration_ms = Utc::now().timestamp_millis() - started_at;
        self.record_job_history(JobHistoryEntry {
            job_id: &job_name,
            queue: &queue_name,
            status: JobHistoryStatus::DeadLettered,
            attempt: attempts,
            error: Some(&error),
            started_at,
            duration_ms,
        })
        .await;

        if let Some(middleware) = middleware {
            middleware
                .run_dead_lettered(&JobDeadLetterContext {
                    class: job_name.to_string(),
                    id: lease.token.clone(),
                    attempts,
                    last_error: error,
                    payload: payload_json,
                    app: self.app.clone(),
                })
                .await;
        }

        Ok(())
    }

    fn spawn_lease_heartbeat(&self, queue: QueueId, token: String) -> LeaseHeartbeat {
        LeaseHeartbeat::spawn(self.runtime.clone(), queue, token)
    }

    fn build_chain_continuation(
        remaining: Option<Vec<ChainedJob>>,
    ) -> Result<Option<JobToEnqueue>> {
        let Some(mut remaining) = remaining else {
            return Ok(None);
        };
        if remaining.is_empty() {
            return Ok(None);
        }

        let next = remaining.remove(0);
        let chain_remaining = if remaining.is_empty() {
            None
        } else {
            Some(remaining)
        };

        let now = Utc::now().timestamp_millis();
        let envelope = JobEnvelope {
            job: next.job.clone(),
            queue: next.queue.clone(),
            attempts: 0,
            scheduled_at: now,
            payload: next.payload,
            batch_id: None,
            chain_remaining,
        };
        let serialized = serde_json::to_string(&envelope).map_err(Error::other)?;
        let token = next_delivery_token();
        Ok(Some(JobToEnqueue {
            queue: next.queue,
            token,
            payload: serialized,
        }))
    }
}

struct ActiveWorkerJobs {
    tasks: Mutex<Vec<WorkerJobTask>>,
    shutdown_timeout: Duration,
}

impl ActiveWorkerJobs {
    fn new(shutdown_timeout: Duration) -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
            shutdown_timeout,
        }
    }

    fn track(&self, handle: JoinHandle<()>) {
        self.tasks
            .lock()
            .expect("worker active job mutex poisoned")
            .push(WorkerJobTask::new(handle));
    }

    async fn prune_finished(&self) {
        let mut finished = Vec::new();
        {
            let mut tasks = self.tasks.lock().expect("worker active job mutex poisoned");
            let mut index = 0;
            while index < tasks.len() {
                if tasks[index].is_finished() {
                    finished.push(tasks.swap_remove(index));
                } else {
                    index += 1;
                }
            }
        }

        for task in finished {
            task.wait_finished().await;
        }
    }

    async fn drain(&self) {
        let tasks = {
            let mut tasks = self.tasks.lock().expect("worker active job mutex poisoned");
            std::mem::take(&mut *tasks)
        };

        drain_tasks(
            tasks,
            self.shutdown_timeout,
            ShutdownDrainMessages {
                target: ShutdownDrainTarget::Worker,
                timeout_disabled: "worker shutdown timeout disabled; aborting active jobs",
                waiting: "waiting for active jobs during worker shutdown",
                drained: "active worker jobs drained",
                timeout_elapsed: "worker shutdown timeout elapsed; aborting active jobs",
            },
        )
        .await;
    }
}

struct WorkerJobTask {
    handle: Option<JoinHandle<()>>,
}

impl WorkerJobTask {
    fn new(handle: JoinHandle<()>) -> Self {
        Self {
            handle: Some(handle),
        }
    }
}

#[async_trait]
impl ShutdownDrainTask for WorkerJobTask {
    fn is_finished(&mut self) -> bool {
        self.handle
            .as_ref()
            .map(|handle| handle.is_finished())
            .unwrap_or(true)
    }

    async fn wait_finished(mut self) {
        if let Some(handle) = self.handle.take() {
            if let Err(error) = handle.await {
                tracing::warn!(
                    target: "forge.worker",
                    error = %error,
                    "Worker job task finished with join error"
                );
            }
        }
    }

    fn abort(&self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }

    async fn wait_after_abort(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for WorkerJobTask {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            if !handle.is_finished() {
                handle.abort();
            }
        }
    }
}

struct LeaseHeartbeat {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl LeaseHeartbeat {
    fn spawn(runtime: Arc<JobRuntime>, queue: QueueId, token: String) -> Self {
        let heartbeat_every = runtime.lease_heartbeat_interval();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_every);
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = interval.tick() => {
                        match runtime.renew_job_lease(&queue, &token).await {
                            Ok(true) => {}
                            Ok(false) => break,
                            Err(error) => {
                                tracing::warn!(
                                    target: "forge.worker",
                                    queue = %queue,
                                    token = %token,
                                    error = %error,
                                    "Failed to renew lease"
                                );
                                break;
                            }
                        }
                    }
                }
            }
        });

        Self {
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        }
    }

    async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for LeaseHeartbeat {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

/// Terminal status for a job recorded in the `job_history` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, forge_macros::AppEnum)]
pub enum JobHistoryStatus {
    Succeeded,
    Retried,
    DeadLettered,
}

impl JobHistoryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Retried => "retried",
            Self::DeadLettered => "dead_lettered",
        }
    }
}

impl std::fmt::Display for JobHistoryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

struct JobHistoryEntry<'a> {
    job_id: &'a JobId,
    queue: &'a QueueId,
    status: JobHistoryStatus,
    attempt: u32,
    error: Option<&'a str>,
    started_at: i64,
    duration_ms: i64,
}

impl Worker {
    async fn record_job_history(&self, entry: JobHistoryEntry<'_>) {
        let JobHistoryEntry {
            job_id,
            queue,
            status,
            attempt,
            error,
            started_at,
            duration_ms,
        } = entry;
        if !self.runtime.config.track_history {
            return;
        }
        let Ok(db) = self.app.database() else {
            return;
        };
        if !db.is_configured() {
            return;
        }

        if let Err(error) = db
            .raw_execute(
                "INSERT INTO job_history (job_id, queue, status, attempt, error, started_at, completed_at, duration_ms) VALUES ($1, $2, $3, $4, $5, to_timestamp($6::double precision / 1000), NOW(), $7)",
                &[
                    DbValue::Text(job_id.to_string()),
                    DbValue::Text(queue.to_string()),
                    DbValue::Text(status.to_string()),
                    DbValue::Int32(attempt as i32),
                    if let Some(e) = error {
                        DbValue::Text(e.to_string())
                    } else {
                        DbValue::Null(DbType::Text)
                    },
                    DbValue::Int64(started_at),
                    DbValue::Int64(duration_ms),
                ],
            )
            .await
        {
            tracing::warn!(
                target: "forge.worker",
                job = %job_id,
                error = %error,
                "failed to record job history"
            );
        }
    }
}

pub fn spawn_worker(app: AppContext) -> Result<tokio::task::JoinHandle<()>> {
    let worker_app = app.clone();
    if let Some(handle) = app.spawn_managed_background_task("forge.worker", move |shutdown_rx| {
        let worker = Worker::from_app(worker_app)?;
        Ok(async move {
            let result = worker
                .run_until(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
            if let Err(error) = result {
                tracing::error!("forge worker exited with error: {error}");
            }
        })
    })? {
        return Ok(handle);
    }

    let kernel = crate::kernel::worker::WorkerKernel::new(app)?;
    Ok(tokio::spawn(async move {
        if let Err(error) = kernel.run().await {
            tracing::error!("forge worker exited with error: {error}");
        }
    }))
}

pub(crate) type JobRegistryHandle = Arc<Mutex<JobRegistryBuilder>>;

#[derive(Default)]
pub(crate) struct JobRegistryBuilder {
    jobs: HashMap<JobId, JobRegistrationBuilder>,
}

impl JobRegistryBuilder {
    pub(crate) fn shared() -> JobRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn register<J>(&mut self) -> Result<()>
    where
        J: Job,
    {
        if self.jobs.contains_key(&J::ID) {
            return Err(Error::message(format!(
                "job `{}` already registered",
                J::ID
            )));
        }

        self.jobs.insert(
            J::ID,
            JobRegistrationBuilder {
                queue: J::QUEUE.clone(),
                handler: Arc::new(JobHandlerAdapter::<J> {
                    marker: PhantomData,
                }),
            },
        );
        Ok(())
    }

    pub(crate) fn freeze_shared(
        handle: JobRegistryHandle,
        config: &JobsConfig,
    ) -> JobRegistrySnapshot {
        let mut builder = handle.lock().expect("job registry lock poisoned");
        let jobs = std::mem::take(&mut builder.jobs)
            .into_iter()
            .map(|(name, registration)| {
                let queue = registration.queue.unwrap_or_else(|| config.queue.clone());
                (
                    name,
                    JobRegistration {
                        queue,
                        handler: registration.handler,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let mut queues = HashSet::new();
        queues.insert(config.queue.clone());
        for registration in jobs.values() {
            queues.insert(registration.queue.clone());
        }

        let mut queues: Vec<QueueId> = queues.into_iter().collect();
        // Sort by configured priority (lower = higher priority, default = 5)
        queues.sort_by_key(|q| {
            config
                .queue_priorities
                .get(q.as_ref())
                .copied()
                .unwrap_or(5)
        });

        JobRegistrySnapshot { jobs, queues }
    }
}

pub(crate) struct JobRuntime {
    backend: RuntimeBackend,
    config: JobsConfig,
    registry: JobRegistrySnapshot,
}

impl JobRuntime {
    pub(crate) fn new(
        backend: RuntimeBackend,
        config: JobsConfig,
        registry: JobRegistrySnapshot,
    ) -> Self {
        Self {
            backend,
            config,
            registry,
        }
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.config.poll_interval_ms.max(1))
    }

    fn lease_ttl(&self) -> Duration {
        Duration::from_millis(self.config.lease_ttl_ms.max(1))
    }

    fn lease_heartbeat_interval(&self) -> Duration {
        let millis = (self.config.lease_ttl_ms / 3).max(1);
        Duration::from_millis(millis)
    }

    fn shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.config.shutdown_timeout_ms)
    }

    async fn promote_due_jobs(&self, now_millis: i64) -> Result<usize> {
        self.backend
            .promote_due_jobs(
                &self.registry.queues,
                now_millis,
                self.config.requeue_batch_size,
            )
            .await
    }

    async fn requeue_expired_jobs(&self, now_millis: i64) -> Result<usize> {
        self.backend
            .requeue_expired_jobs(
                &self.registry.queues,
                now_millis,
                self.config.requeue_batch_size,
            )
            .await
    }

    async fn claim_job(&self) -> Result<Option<ClaimedJobLease>> {
        self.backend
            .claim_job(&self.registry.queues, self.lease_ttl())
            .await
    }

    async fn renew_job_lease(&self, queue: &QueueId, token: &str) -> Result<bool> {
        self.backend
            .renew_job_lease(queue, token, self.lease_ttl())
            .await
    }

    async fn retry_job(
        &self,
        queue: &QueueId,
        token: &str,
        new_token: &str,
        payload: &str,
        run_at_millis: i64,
    ) -> Result<bool> {
        self.backend
            .retry_job(queue, token, new_token, payload, run_at_millis)
            .await
    }

    async fn dead_letter_job(&self, queue: &QueueId, token: &str, payload: &str) -> Result<bool> {
        self.backend.dead_letter_job(queue, token, payload).await
    }

    async fn complete_successful_job(
        &self,
        queue: &QueueId,
        token: &str,
        effects: SuccessfulJobEffects,
    ) -> Result<backend::SuccessfulJobCompletion> {
        self.backend
            .complete_successful_job(queue, token, &self.config.queue, effects)
            .await
    }
}

pub(crate) struct JobRegistrySnapshot {
    jobs: HashMap<JobId, JobRegistration>,
    queues: Vec<QueueId>,
}

struct JobRegistrationBuilder {
    queue: Option<QueueId>,
    handler: Arc<dyn DynJobHandler>,
}

struct JobRegistration {
    queue: QueueId,
    handler: Arc<dyn DynJobHandler>,
}

#[derive(Clone, Serialize, Deserialize)]
struct JobEnvelope {
    job: JobId,
    queue: QueueId,
    attempts: u32,
    scheduled_at: i64,
    payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    batch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chain_remaining: Option<Vec<ChainedJob>>,
}

/// A serialized job entry used in chain sequences.
#[derive(Clone, Serialize, Deserialize)]
struct ChainedJob {
    job: JobId,
    queue: QueueId,
    payload: serde_json::Value,
}

#[derive(Clone, Serialize, Deserialize)]
struct FailedJobEnvelope {
    failed_at: i64,
    error: String,
    envelope: JobEnvelope,
}

struct DeadLetterClaimedJob<'a> {
    lease: &'a ClaimedJobLease,
    envelope: JobEnvelope,
    error: String,
    attempts: u32,
    started_at: i64,
    middleware: Option<&'a JobMiddlewareRegistry>,
    job_context: Option<&'a JobContext>,
}

enum JobExecutionOutcome {
    Success,
    Retry {
        run_at_millis: i64,
        attempts: u32,
        error: String,
    },
    DeadLetter {
        error: String,
        attempts: u32,
    },
}

#[async_trait]
trait DynJobHandler: Send + Sync {
    async fn execute(
        &self,
        app: &AppContext,
        envelope: &JobEnvelope,
        default_max_retries: u32,
        default_timeout: Duration,
    ) -> Result<JobExecutionOutcome>;

    /// Check whether the job type has a rate limit, and if so, return it.
    /// Deserializes the payload to read the concrete job's `rate_limit()`.
    fn check_rate_limit(&self, envelope: &JobEnvelope) -> Option<(u32, Duration)>;
}

struct JobHandlerAdapter<J> {
    marker: PhantomData<J>,
}

#[async_trait]
impl<J> DynJobHandler for JobHandlerAdapter<J>
where
    J: Job,
{
    async fn execute(
        &self,
        app: &AppContext,
        envelope: &JobEnvelope,
        default_max_retries: u32,
        default_timeout: Duration,
    ) -> Result<JobExecutionOutcome> {
        let job: J = match serde_json::from_value(envelope.payload.clone()) {
            Ok(job) => job,
            Err(error) => {
                return Ok(JobExecutionOutcome::DeadLetter {
                    error: error.to_string(),
                    attempts: envelope.attempts + 1,
                });
            }
        };

        let timeout_duration = job.timeout().unwrap_or(default_timeout);
        let context = JobContext::new(app.clone(), envelope.queue.clone(), envelope.attempts + 1);
        let result = tokio::time::timeout(
            timeout_duration,
            AssertUnwindSafe(job.handle(context)).catch_unwind(),
        )
        .await;

        let error_msg = match result {
            Ok(Ok(Ok(()))) => return Ok(JobExecutionOutcome::Success),
            Ok(Ok(Err(error))) => error.to_string(),
            Ok(Err(panic)) => format!("job panicked: {}", panic_payload_message(panic)),
            Err(_elapsed) => format!("job timed out after {}s", timeout_duration.as_secs()),
        };

        // Failure — decide retry vs dead-letter
        let attempts = envelope.attempts + 1;
        let max_retries = job.max_retries().unwrap_or(default_max_retries);
        if attempts >= max_retries {
            return Ok(JobExecutionOutcome::DeadLetter {
                error: error_msg,
                attempts,
            });
        } else {
            let run_at_millis =
                Utc::now().timestamp_millis() + job.backoff(attempts).as_millis() as i64;
            return Ok(JobExecutionOutcome::Retry {
                run_at_millis,
                attempts,
                error: error_msg,
            });
        }
    }

    fn check_rate_limit(&self, envelope: &JobEnvelope) -> Option<(u32, Duration)> {
        let job: J = serde_json::from_value(envelope.payload.clone()).ok()?;
        job.rate_limit()
    }
}

fn panic_payload_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn next_delivery_token() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    format!(
        "{:x}-{:x}",
        Utc::now().timestamp_micros(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use serde::{Deserialize, Serialize};

    use super::{
        ChainedJob, Job, JobContext, JobDeadLetterContext, JobDispatcher, JobEnvelope,
        JobMiddleware, JobMiddlewareRegistryBuilder, JobRegistryBuilder, JobRuntime,
        SuccessfulJobEffects, Worker,
    };
    use crate::config::JobsConfig;
    use crate::foundation::{AppContext, Container, Error};
    use crate::logging::{ReadinessRegistryBuilder, RuntimeBackendKind, RuntimeDiagnostics};
    use crate::support::runtime::RuntimeBackend;
    use crate::support::{JobId, QueueId};
    use crate::validation::RuleRegistry;

    #[derive(Debug, Serialize, Deserialize)]
    struct FailingJob;

    #[async_trait]
    impl Job for FailingJob {
        const ID: JobId = JobId::new("failing.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            Err(Error::message("boom"))
        }

        fn max_retries(&self) -> Option<u32> {
            Some(1)
        }

        fn backoff(&self, _attempt: u32) -> Duration {
            Duration::from_millis(0)
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct PanickingJob;

    #[async_trait]
    impl Job for PanickingJob {
        const ID: JobId = JobId::new("panicking.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            panic!("job explode")
        }

        fn max_retries(&self) -> Option<u32> {
            Some(1)
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct PanicThenSucceedJob;

    #[async_trait]
    impl Job for PanicThenSucceedJob {
        const ID: JobId = JobId::new("panic.then.succeed.job");

        async fn handle(&self, context: JobContext) -> crate::Result<()> {
            if context.attempt() == 1 {
                panic!("flaky panic")
            }
            Ok(())
        }

        fn max_retries(&self) -> Option<u32> {
            Some(2)
        }

        fn backoff(&self, _attempt: u32) -> Duration {
            Duration::from_millis(0)
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct UniqueOkJob {
        key: String,
    }

    #[async_trait]
    impl Job for UniqueOkJob {
        const ID: JobId = JobId::new("unique.ok.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            Ok(())
        }

        fn unique_for(&self) -> Option<Duration> {
            Some(Duration::from_secs(60))
        }

        fn unique_key(&self) -> Option<String> {
            Some(self.key.clone())
        }
    }

    #[derive(Debug, Deserialize)]
    struct UniqueSerializationFailJob;

    impl serde::Serialize for UniqueSerializationFailJob {
        fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("unique job serialization failed"))
        }
    }

    #[async_trait]
    impl Job for UniqueSerializationFailJob {
        const ID: JobId = JobId::new("unique.serialization.fail.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            Ok(())
        }

        fn unique_for(&self) -> Option<Duration> {
            Some(Duration::from_secs(60))
        }

        fn unique_key(&self) -> Option<String> {
            Some("fixed".to_string())
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct BlockingJob {
        tag: String,
    }

    #[async_trait]
    impl Job for BlockingJob {
        const ID: JobId = JobId::new("blocking.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            let mut state = take_worker_lifecycle_state(&self.tag);
            if let Some(started) = state.started.take() {
                let _ = started.send(());
            }

            let _guard = WorkerLifecycleGuard {
                completed: state.completed_flag.clone(),
                aborted: state.aborted_flag.clone(),
            };

            if let Some(release) = state.release.take() {
                let _ = release.await;
            } else {
                std::future::pending::<()>().await;
            }

            state.completed_flag.store(true, Ordering::SeqCst);
            if let Some(completed) = state.completed.take() {
                let _ = completed.send(());
            }
            Ok(())
        }
    }

    struct WorkerLifecycleState {
        started: Option<tokio::sync::oneshot::Sender<()>>,
        release: Option<tokio::sync::oneshot::Receiver<()>>,
        completed: Option<tokio::sync::oneshot::Sender<()>>,
        completed_flag: Arc<AtomicBool>,
        aborted_flag: Arc<AtomicBool>,
    }

    struct WorkerLifecycleProbe {
        started: tokio::sync::oneshot::Receiver<()>,
        release: Option<tokio::sync::oneshot::Sender<()>>,
        completed: tokio::sync::oneshot::Receiver<()>,
        completed_flag: Arc<AtomicBool>,
        aborted_flag: Arc<AtomicBool>,
    }

    struct WorkerLifecycleGuard {
        completed: Arc<AtomicBool>,
        aborted: Arc<AtomicBool>,
    }

    impl Drop for WorkerLifecycleGuard {
        fn drop(&mut self) {
            if !self.completed.load(Ordering::SeqCst) {
                self.aborted.store(true, Ordering::SeqCst);
            }
        }
    }

    static WORKER_LIFECYCLE_STATES: std::sync::LazyLock<
        Mutex<std::collections::HashMap<String, WorkerLifecycleState>>,
    > = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

    fn worker_lifecycle_probe(tag: &str, releasable: bool) -> WorkerLifecycleProbe {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let completed_flag = Arc::new(AtomicBool::new(false));
        let aborted_flag = Arc::new(AtomicBool::new(false));

        WORKER_LIFECYCLE_STATES.lock().unwrap().insert(
            tag.to_string(),
            WorkerLifecycleState {
                started: Some(started_tx),
                release: releasable.then_some(release_rx),
                completed: Some(completed_tx),
                completed_flag: completed_flag.clone(),
                aborted_flag: aborted_flag.clone(),
            },
        );

        WorkerLifecycleProbe {
            started: started_rx,
            release: releasable.then_some(release_tx),
            completed: completed_rx,
            completed_flag,
            aborted_flag,
        }
    }

    fn take_worker_lifecycle_state(tag: &str) -> WorkerLifecycleState {
        WORKER_LIFECYCLE_STATES
            .lock()
            .unwrap()
            .remove(tag)
            .unwrap_or_else(|| panic!("missing lifecycle state for `{tag}`"))
    }

    async fn wait_for_flag(flag: &AtomicBool) {
        for _ in 0..50 {
            if flag.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("flag was not set");
    }

    fn build_app(runtime: Arc<JobRuntime>, diagnostics: Arc<RuntimeDiagnostics>) -> AppContext {
        let container = Container::new();
        let app = AppContext::new(
            container,
            crate::config::ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        app.container().singleton_arc(runtime).unwrap();
        app.container().singleton_arc(diagnostics).unwrap();
        app
    }

    fn build_blocking_runtime(
        namespace: &str,
        jobs_config: JobsConfig,
    ) -> (
        RuntimeBackend,
        Arc<JobRuntime>,
        Arc<RuntimeDiagnostics>,
        JobDispatcher,
    ) {
        let backend = RuntimeBackend::memory(namespace);
        let mut registry = JobRegistryBuilder::default();
        registry.register::<BlockingJob>().unwrap();

        let runtime = Arc::new(JobRuntime::new(
            backend.clone(),
            jobs_config.clone(),
            JobRegistryBuilder::freeze_shared(Arc::new(Mutex::new(registry)), &jobs_config),
        ));
        let diagnostics = Arc::new(RuntimeDiagnostics::new(
            RuntimeBackendKind::Memory,
            ReadinessRegistryBuilder::freeze_shared(ReadinessRegistryBuilder::shared()),
        ));
        let dispatcher = JobDispatcher::new(runtime.clone(), diagnostics.clone());
        (backend, runtime, diagnostics, dispatcher)
    }

    fn build_panic_runtime(
        namespace: &str,
        jobs_config: JobsConfig,
    ) -> (
        RuntimeBackend,
        Arc<JobRuntime>,
        Arc<RuntimeDiagnostics>,
        JobDispatcher,
    ) {
        let backend = RuntimeBackend::memory(namespace);
        let mut registry = JobRegistryBuilder::default();
        registry.register::<PanickingJob>().unwrap();
        registry.register::<PanicThenSucceedJob>().unwrap();

        let runtime = Arc::new(JobRuntime::new(
            backend.clone(),
            jobs_config.clone(),
            JobRegistryBuilder::freeze_shared(Arc::new(Mutex::new(registry)), &jobs_config),
        ));
        let diagnostics = Arc::new(RuntimeDiagnostics::new(
            RuntimeBackendKind::Memory,
            ReadinessRegistryBuilder::freeze_shared(ReadinessRegistryBuilder::shared()),
        ));
        let dispatcher = JobDispatcher::new(runtime.clone(), diagnostics.clone());
        (backend, runtime, diagnostics, dispatcher)
    }

    struct PanicRecordingMiddleware {
        failed: Arc<Mutex<Vec<String>>>,
        dead_lettered: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl JobMiddleware for PanicRecordingMiddleware {
        async fn failed(
            &self,
            job_id: &JobId,
            _context: &JobContext,
            error: &str,
        ) -> crate::Result<()> {
            self.failed
                .lock()
                .unwrap()
                .push(format!("{job_id}:{error}"));
            Ok(())
        }

        async fn on_dead_lettered(&self, context: &JobDeadLetterContext) -> crate::Result<()> {
            self.dead_lettered
                .lock()
                .unwrap()
                .push(context.class.clone());
            Ok(())
        }
    }

    fn register_panic_middleware(
        app: &AppContext,
        failed: Arc<Mutex<Vec<String>>>,
        dead_lettered: Arc<Mutex<Vec<String>>>,
    ) {
        let mut middleware_builder = JobMiddlewareRegistryBuilder::default();
        middleware_builder.register(Arc::new(PanicRecordingMiddleware {
            failed,
            dead_lettered,
        }));
        app.container()
            .singleton_arc(Arc::new(JobMiddlewareRegistryBuilder::freeze_shared(
                Arc::new(Mutex::new(middleware_builder)),
            )))
            .unwrap();
    }

    #[tokio::test]
    async fn panicking_job_run_once_dead_letters_without_panicking() {
        let backend_namespace = "job-panic-dead-letter";
        let (backend, runtime, diagnostics, dispatcher) =
            build_panic_runtime(backend_namespace, JobsConfig::default());
        let app = build_app(runtime, diagnostics.clone());
        let failed = Arc::new(Mutex::new(Vec::new()));
        let dead_lettered = Arc::new(Mutex::new(Vec::new()));
        register_panic_middleware(&app, failed.clone(), dead_lettered.clone());

        dispatcher.dispatch(PanickingJob).await.unwrap();
        let worker = Worker::from_app(app).unwrap();

        assert!(worker.run_once().await.unwrap());

        let dead_letters = backend
            .dead_letters(&QueueId::new("default"))
            .await
            .unwrap();
        assert_eq!(dead_letters.len(), 1);
        let payload: serde_json::Value = serde_json::from_str(&dead_letters[0]).unwrap();
        assert_eq!(payload["error"], "job panicked: job explode");

        assert_eq!(
            failed.lock().unwrap().as_slice(),
            &["panicking.job:job panicked: job explode"]
        );
        assert_eq!(dead_lettered.lock().unwrap().as_slice(), &["panicking.job"]);

        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.jobs.dead_lettered_total, 1);
        assert_eq!(snapshot.jobs.retried_total, 0);
        assert_eq!(snapshot.jobs.succeeded_total, 0);
    }

    #[tokio::test]
    async fn panicking_job_retries_then_succeeds() {
        let (_backend, runtime, diagnostics, dispatcher) = build_panic_runtime(
            "job-panic-retry",
            JobsConfig {
                poll_interval_ms: 1,
                lease_ttl_ms: 50,
                ..JobsConfig::default()
            },
        );
        let app = build_app(runtime, diagnostics.clone());
        let failed = Arc::new(Mutex::new(Vec::new()));
        let dead_lettered = Arc::new(Mutex::new(Vec::new()));
        register_panic_middleware(&app, failed.clone(), dead_lettered.clone());

        dispatcher.dispatch(PanicThenSucceedJob).await.unwrap();
        let worker = Worker::from_app(app).unwrap();

        assert!(worker.run_once().await.unwrap());
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.jobs.retried_total, 1);
        assert_eq!(snapshot.jobs.dead_lettered_total, 0);
        assert_eq!(snapshot.jobs.succeeded_total, 0);
        assert_eq!(
            failed.lock().unwrap().as_slice(),
            &["panic.then.succeed.job:job panicked: flaky panic"]
        );

        assert!(worker.run_once().await.unwrap());
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.jobs.retried_total, 1);
        assert_eq!(snapshot.jobs.dead_lettered_total, 0);
        assert_eq!(snapshot.jobs.succeeded_total, 1);
        assert!(dead_lettered.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn worker_shutdown_waits_for_active_job_completion() {
        let jobs_config = JobsConfig {
            poll_interval_ms: 1,
            lease_ttl_ms: 50,
            shutdown_timeout_ms: 500,
            ..JobsConfig::default()
        };
        let (_backend, runtime, diagnostics, dispatcher) =
            build_blocking_runtime("worker-shutdown-drain", jobs_config);
        let app = build_app(runtime, diagnostics);
        let mut probe = worker_lifecycle_probe("drain", true);

        dispatcher
            .dispatch(BlockingJob {
                tag: "drain".to_string(),
            })
            .await
            .unwrap();

        let worker = Worker::from_app(app).unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let worker_task = tokio::spawn(async move {
            worker
                .run_until(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        probe.started.await.unwrap();
        shutdown_tx.send(()).unwrap();
        probe.release.take().unwrap().send(()).unwrap();
        probe.completed.await.unwrap();

        tokio::time::timeout(Duration::from_millis(500), worker_task)
            .await
            .unwrap()
            .unwrap();
        assert!(probe.completed_flag.load(Ordering::SeqCst));
        assert!(!probe.aborted_flag.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn worker_shutdown_aborts_active_job_after_timeout_and_requeues_after_lease_expiry() {
        let jobs_config = JobsConfig {
            poll_interval_ms: 1,
            lease_ttl_ms: 30,
            shutdown_timeout_ms: 1,
            max_concurrent_jobs: 1,
            ..JobsConfig::default()
        };
        let (_backend, runtime, diagnostics, dispatcher) =
            build_blocking_runtime("worker-shutdown-abort", jobs_config);
        let app = build_app(runtime.clone(), diagnostics);
        let probe = worker_lifecycle_probe("abort", false);

        dispatcher
            .dispatch(BlockingJob {
                tag: "abort".to_string(),
            })
            .await
            .unwrap();

        let worker = Worker::from_app(app).unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let worker_task = tokio::spawn(async move {
            worker
                .run_until(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        probe.started.await.unwrap();
        shutdown_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_millis(500), worker_task)
            .await
            .unwrap()
            .unwrap();
        wait_for_flag(&probe.aborted_flag).await;
        assert!(!probe.completed_flag.load(Ordering::SeqCst));

        tokio::time::sleep(Duration::from_millis(80)).await;
        let requeued = runtime
            .requeue_expired_jobs(chrono::Utc::now().timestamp_millis())
            .await
            .unwrap();
        assert_eq!(requeued, 1);
        assert!(runtime.claim_job().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn aborting_worker_coordinator_aborts_active_jobs() {
        let jobs_config = JobsConfig {
            poll_interval_ms: 1,
            lease_ttl_ms: 50,
            shutdown_timeout_ms: 500,
            max_concurrent_jobs: 1,
            ..JobsConfig::default()
        };
        let (_backend, runtime, diagnostics, dispatcher) =
            build_blocking_runtime("worker-coordinator-abort", jobs_config);
        let app = build_app(runtime, diagnostics);
        let probe = worker_lifecycle_probe("coordinator-abort", false);

        dispatcher
            .dispatch(BlockingJob {
                tag: "coordinator-abort".to_string(),
            })
            .await
            .unwrap();

        let worker = Worker::from_app(app).unwrap();
        let worker_task = tokio::spawn(async move {
            worker
                .run_until(std::future::pending::<()>())
                .await
                .unwrap();
        });

        probe.started.await.unwrap();
        worker_task.abort();
        let _ = worker_task.await;
        wait_for_flag(&probe.aborted_flag).await;
        assert!(!probe.completed_flag.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn moves_failed_jobs_to_dead_letter() {
        let _guard = tracing::subscriber::set_default(tracing::subscriber::NoSubscriber::default());
        let backend = RuntimeBackend::memory("jobs-unit-tests");
        let mut registry = JobRegistryBuilder::default();
        registry.register::<FailingJob>().unwrap();

        let jobs_config = JobsConfig {
            max_retries: 1,
            poll_interval_ms: 1,
            lease_ttl_ms: 50,
            requeue_batch_size: 8,
            ..JobsConfig::default()
        };
        let runtime = Arc::new(JobRuntime::new(
            backend.clone(),
            jobs_config.clone(),
            JobRegistryBuilder::freeze_shared(Arc::new(Mutex::new(registry)), &jobs_config),
        ));
        let diagnostics = Arc::new(RuntimeDiagnostics::new(
            RuntimeBackendKind::Memory,
            ReadinessRegistryBuilder::freeze_shared(ReadinessRegistryBuilder::shared()),
        ));
        let dispatcher = JobDispatcher::new(runtime.clone(), diagnostics.clone());
        let app = build_app(runtime.clone(), diagnostics);

        dispatcher.dispatch(FailingJob).await.unwrap();
        let worker = Worker::from_app(app).unwrap();
        assert!(worker.run_once().await.unwrap());

        let dead_letters = backend
            .dead_letters(&QueueId::new("default"))
            .await
            .unwrap();
        assert_eq!(dead_letters.len(), 1);
    }

    struct RecordingMiddleware {
        target: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl JobMiddleware for RecordingMiddleware {
        async fn on_dead_lettered(&self, context: &JobDeadLetterContext) -> crate::Result<()> {
            self.target
                .lock()
                .unwrap()
                .push(format!("{}:{}", context.class, context.id));
            Ok(())
        }
    }

    #[derive(Default)]
    struct PanickingMiddleware {
        before: bool,
        after: bool,
        failed: bool,
        dead_lettered: bool,
    }

    #[async_trait]
    impl JobMiddleware for PanickingMiddleware {
        async fn before(&self, _job_id: &JobId, _context: &JobContext) -> crate::Result<()> {
            if self.before {
                panic!("middleware before explode");
            }
            Ok(())
        }

        async fn after(&self, _job_id: &JobId, _context: &JobContext) -> crate::Result<()> {
            if self.after {
                panic!("middleware after explode");
            }
            Ok(())
        }

        async fn failed(
            &self,
            _job_id: &JobId,
            _context: &JobContext,
            _error: &str,
        ) -> crate::Result<()> {
            if self.failed {
                panic!("middleware failed explode");
            }
            Ok(())
        }

        async fn on_dead_lettered(&self, _context: &JobDeadLetterContext) -> crate::Result<()> {
            if self.dead_lettered {
                panic!("middleware dead-letter explode");
            }
            Ok(())
        }
    }

    fn register_job_middleware(app: &AppContext, middleware: Arc<dyn JobMiddleware>) {
        let mut middleware_builder = JobMiddlewareRegistryBuilder::default();
        middleware_builder.register(middleware);
        app.container()
            .singleton_arc(Arc::new(JobMiddlewareRegistryBuilder::freeze_shared(
                Arc::new(Mutex::new(middleware_builder)),
            )))
            .unwrap();
    }

    #[tokio::test]
    async fn dead_lettered_jobs_trigger_middleware_hook() {
        let _guard = tracing::subscriber::set_default(tracing::subscriber::NoSubscriber::default());
        let backend = RuntimeBackend::memory("jobs-dead-letter-hook");
        let mut registry = JobRegistryBuilder::default();
        registry.register::<FailingJob>().unwrap();

        let jobs_config = JobsConfig {
            max_retries: 1,
            poll_interval_ms: 1,
            lease_ttl_ms: 50,
            requeue_batch_size: 8,
            ..JobsConfig::default()
        };
        let runtime = Arc::new(JobRuntime::new(
            backend,
            jobs_config.clone(),
            JobRegistryBuilder::freeze_shared(Arc::new(Mutex::new(registry)), &jobs_config),
        ));
        let diagnostics = Arc::new(RuntimeDiagnostics::new(
            RuntimeBackendKind::Memory,
            ReadinessRegistryBuilder::freeze_shared(ReadinessRegistryBuilder::shared()),
        ));
        let dispatcher = JobDispatcher::new(runtime.clone(), diagnostics.clone());
        let app = build_app(runtime, diagnostics);
        let target = Arc::new(Mutex::new(Vec::new()));
        register_job_middleware(
            &app,
            Arc::new(RecordingMiddleware {
                target: target.clone(),
            }),
        );

        dispatcher.dispatch(FailingJob).await.unwrap();
        let worker = Worker::from_app(app).unwrap();
        assert!(worker.run_once().await.unwrap());

        let entries = target.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].starts_with("failing.job:"));
    }

    #[tokio::test]
    async fn middleware_before_after_panics_do_not_block_success_finalization() {
        let tag = "middleware-success-panic";
        let (_backend, runtime, diagnostics, dispatcher) =
            build_runtime_and_dispatcher("middleware-success-panic");
        let app = build_app(runtime, diagnostics.clone());
        register_job_middleware(
            &app,
            Arc::new(PanickingMiddleware {
                before: true,
                after: true,
                ..PanickingMiddleware::default()
            }),
        );

        dispatcher
            .dispatch(StepJob {
                tag: tag.into(),
                name: "ok".into(),
            })
            .await
            .unwrap();
        let worker = Worker::from_app(app).unwrap();

        assert!(worker.run_once().await.unwrap());
        assert!(!worker.run_once().await.unwrap());
        assert_eq!(read_log_filtered(&format!("{tag}:")), vec!["ok"]);
        assert_eq!(diagnostics.snapshot().jobs.succeeded_total, 1);
    }

    #[tokio::test]
    async fn middleware_failure_panics_do_not_block_dead_letter_transition() {
        let (backend, runtime, diagnostics, dispatcher) =
            build_runtime_and_dispatcher("middleware-failure-panic");
        let app = build_app(runtime, diagnostics.clone());
        register_job_middleware(
            &app,
            Arc::new(PanickingMiddleware {
                failed: true,
                dead_lettered: true,
                ..PanickingMiddleware::default()
            }),
        );

        dispatcher.dispatch(FailingJob).await.unwrap();
        let worker = Worker::from_app(app).unwrap();

        assert!(worker.run_once().await.unwrap());
        let dead_letters = backend
            .dead_letters(&QueueId::new("default"))
            .await
            .unwrap();
        assert_eq!(dead_letters.len(), 1);
        assert_eq!(diagnostics.snapshot().jobs.dead_lettered_total, 1);
    }

    // --- Batch & chain test helpers ---

    static EXECUTION_LOG: std::sync::LazyLock<std::sync::Mutex<Vec<String>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

    fn append_log(entry: String) {
        EXECUTION_LOG.lock().unwrap().push(entry);
    }

    fn read_log_filtered(prefix: &str) -> Vec<String> {
        EXECUTION_LOG
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.starts_with(prefix))
            .map(|e| e.strip_prefix(prefix).unwrap_or(e).to_string())
            .collect()
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct StepJob {
        tag: String,
        name: String,
    }

    #[async_trait]
    impl Job for StepJob {
        const ID: JobId = JobId::new("step.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            append_log(format!("{}:{}", self.tag, self.name));
            Ok(())
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct CompletionJob {
        tag: String,
        label: String,
    }

    #[async_trait]
    impl Job for CompletionJob {
        const ID: JobId = JobId::new("completion.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            append_log(format!("{}:complete:{}", self.tag, self.label));
            Ok(())
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct RateLimitedJob {
        tag: String,
    }

    #[async_trait]
    impl Job for RateLimitedJob {
        const ID: JobId = JobId::new("rate.limited.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            append_log(format!("{}:handled", self.tag));
            Ok(())
        }

        fn rate_limit(&self) -> Option<(u32, Duration)> {
            Some((0, Duration::from_secs(60)))
        }
    }

    fn build_runtime_and_dispatcher(
        namespace: &str,
    ) -> (
        RuntimeBackend,
        Arc<JobRuntime>,
        Arc<RuntimeDiagnostics>,
        JobDispatcher,
    ) {
        let backend = RuntimeBackend::memory(namespace);
        let mut registry = JobRegistryBuilder::default();
        registry.register::<FailingJob>().unwrap();
        registry.register::<StepJob>().unwrap();
        registry.register::<CompletionJob>().unwrap();
        registry.register::<RateLimitedJob>().unwrap();

        let jobs_config = JobsConfig {
            max_retries: 1,
            poll_interval_ms: 1,
            lease_ttl_ms: 50,
            requeue_batch_size: 8,
            ..JobsConfig::default()
        };
        let runtime = Arc::new(JobRuntime::new(
            backend.clone(),
            jobs_config.clone(),
            JobRegistryBuilder::freeze_shared(Arc::new(Mutex::new(registry)), &jobs_config),
        ));
        let diagnostics = Arc::new(RuntimeDiagnostics::new(
            RuntimeBackendKind::Memory,
            ReadinessRegistryBuilder::freeze_shared(ReadinessRegistryBuilder::shared()),
        ));
        let dispatcher = JobDispatcher::new(runtime.clone(), diagnostics.clone());
        (backend, runtime, diagnostics, dispatcher)
    }

    #[tokio::test]
    async fn malformed_job_envelope_is_dead_lettered_without_requeue_loop() {
        let (backend, runtime, diagnostics, _dispatcher) =
            build_runtime_and_dispatcher("poison-malformed-envelope");
        let queue = QueueId::new("default");
        backend
            .enqueue_job(&queue, "poison-token", "not-json")
            .await
            .unwrap();

        let app = build_app(runtime.clone(), diagnostics.clone());
        let worker = Worker::from_app(app).unwrap();
        assert!(worker.run_once().await.unwrap());

        let dead_letters = backend.dead_letters(&queue).await.unwrap();
        assert_eq!(dead_letters.len(), 1);
        let payload: serde_json::Value = serde_json::from_str(&dead_letters[0]).unwrap();
        assert!(payload["error"]
            .as_str()
            .unwrap()
            .starts_with("job envelope could not be deserialized:"));
        assert_eq!(payload["envelope"]["job"], "forge.invalid_job_envelope");
        assert_eq!(payload["envelope"]["payload"], "not-json");

        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.jobs.dead_lettered_total, 1);
        assert_eq!(
            runtime
                .requeue_expired_jobs(chrono::Utc::now().timestamp_millis())
                .await
                .unwrap(),
            0
        );
        assert!(runtime.claim_job().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn unregistered_job_envelope_is_dead_lettered_without_requeue_loop() {
        let (backend, runtime, diagnostics, _dispatcher) =
            build_runtime_and_dispatcher("poison-unregistered-envelope");
        let queue = QueueId::new("default");
        let envelope = JobEnvelope {
            job: JobId::new("missing.job"),
            queue: queue.clone(),
            attempts: 0,
            scheduled_at: chrono::Utc::now().timestamp_millis(),
            payload: serde_json::json!({ "id": 123 }),
            batch_id: None,
            chain_remaining: None,
        };
        let payload = serde_json::to_string(&envelope).unwrap();
        backend
            .enqueue_job(&queue, "missing-token", &payload)
            .await
            .unwrap();

        let app = build_app(runtime.clone(), diagnostics.clone());
        let worker = Worker::from_app(app).unwrap();
        assert!(worker.run_once().await.unwrap());

        let dead_letters = backend.dead_letters(&queue).await.unwrap();
        assert_eq!(dead_letters.len(), 1);
        let payload: serde_json::Value = serde_json::from_str(&dead_letters[0]).unwrap();
        assert_eq!(payload["error"], "job `missing.job` is not registered");
        assert_eq!(payload["envelope"]["job"], "missing.job");
        assert_eq!(payload["envelope"]["attempts"], 1);

        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.jobs.dead_lettered_total, 1);
        assert_eq!(
            runtime
                .requeue_expired_jobs(chrono::Utc::now().timestamp_millis())
                .await
                .unwrap(),
            0
        );
        assert!(runtime.claim_job().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn unique_dispatch_serialization_failure_rolls_back_reservation() {
        let (backend, _runtime, _diagnostics, dispatcher) =
            build_runtime_and_dispatcher("unique-serialization-rollback");
        let unique_key = format!(
            "jobs:unique:{}:{}",
            <UniqueSerializationFailJob as Job>::ID,
            "fixed"
        );

        let result = dispatcher.dispatch(UniqueSerializationFailJob).await;

        assert!(result.is_err());
        assert!(!backend.key_exists(&unique_key).await.unwrap());
        assert!(backend
            .set_nx_value(&unique_key, "after-rollback", 60)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn successful_unique_dispatch_keeps_reservation_and_skips_duplicates() {
        let (backend, _runtime, _diagnostics, dispatcher) =
            build_runtime_and_dispatcher("unique-success-keeps-reservation");
        let unique_key = format!("jobs:unique:{}:{}", <UniqueOkJob as Job>::ID, "same");
        let queue = QueueId::new("default");

        dispatcher
            .dispatch(UniqueOkJob {
                key: "same".to_string(),
            })
            .await
            .unwrap();
        dispatcher
            .dispatch(UniqueOkJob {
                key: "same".to_string(),
            })
            .await
            .unwrap();

        assert!(backend.key_exists(&unique_key).await.unwrap());
        assert!(backend
            .claim_job(std::slice::from_ref(&queue), Duration::from_millis(50))
            .await
            .unwrap()
            .is_some());
        assert!(backend
            .claim_job(std::slice::from_ref(&queue), Duration::from_millis(50))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn batch_dispatches_all_jobs_and_fires_on_complete() {
        let tag = "batch1";
        let (_backend, runtime, diagnostics, dispatcher) =
            build_runtime_and_dispatcher("batch-complete");
        let app = build_app(runtime, diagnostics);

        let batch_id = dispatcher
            .batch("test")
            .add(StepJob {
                tag: tag.into(),
                name: "a".into(),
            })
            .unwrap()
            .add(StepJob {
                tag: tag.into(),
                name: "b".into(),
            })
            .unwrap()
            .on_complete(CompletionJob {
                tag: tag.into(),
                label: "done".into(),
            })
            .unwrap()
            .dispatch()
            .await
            .unwrap();
        assert!(batch_id.starts_with("batch-test-"));

        let worker = Worker::from_app(app).unwrap();
        // Process both batch jobs
        assert!(worker.run_once().await.unwrap());
        assert!(worker.run_once().await.unwrap());
        // Process the on_complete callback
        assert!(worker.run_once().await.unwrap());

        let log = read_log_filtered(&format!("{tag}:"));
        // The two step jobs executed (order may vary), then the completion
        assert!(log.contains(&"a".to_string()));
        assert!(log.contains(&"b".to_string()));
        assert!(log.contains(&"complete:done".to_string()));
        // Completion is always last
        assert_eq!(log.last().unwrap(), "complete:done");
    }

    #[tokio::test]
    async fn batch_without_on_complete_works() {
        let tag = "batch2";
        let (_backend, runtime, diagnostics, dispatcher) =
            build_runtime_and_dispatcher("batch-no-callback");
        let app = build_app(runtime, diagnostics);

        dispatcher
            .batch("simple")
            .add(StepJob {
                tag: tag.into(),
                name: "x".into(),
            })
            .unwrap()
            .dispatch()
            .await
            .unwrap();

        let worker = Worker::from_app(app).unwrap();
        assert!(worker.run_once().await.unwrap());
        // No more work
        assert!(!worker.run_once().await.unwrap());

        let log = read_log_filtered(&format!("{tag}:"));
        assert_eq!(log, vec!["x"]);
    }

    #[tokio::test]
    async fn chain_executes_jobs_sequentially() {
        let tag = "chain1";
        let (_backend, runtime, diagnostics, dispatcher) =
            build_runtime_and_dispatcher("chain-sequential");
        let app = build_app(runtime, diagnostics);

        dispatcher
            .chain()
            .add(StepJob {
                tag: tag.into(),
                name: "first".into(),
            })
            .unwrap()
            .add(StepJob {
                tag: tag.into(),
                name: "second".into(),
            })
            .unwrap()
            .add(StepJob {
                tag: tag.into(),
                name: "third".into(),
            })
            .unwrap()
            .dispatch()
            .await
            .unwrap();

        let worker = Worker::from_app(app).unwrap();
        // Process chain — each run_once handles one job and enqueues the next
        for _ in 0..10 {
            let _ = worker.run_once().await;
        }

        let log = read_log_filtered(&format!("{tag}:"));
        assert_eq!(log, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn success_finalization_lost_lease_does_not_dispatch_chain_continuation() {
        let tag = "chain-lost-lease";
        let (backend, runtime, diagnostics, _dispatcher) =
            build_runtime_and_dispatcher("chain-lost-lease");
        let app = build_app(runtime.clone(), diagnostics.clone());
        let queue = QueueId::new("default");
        let first = StepJob {
            tag: tag.into(),
            name: "first".into(),
        };
        let second = StepJob {
            tag: tag.into(),
            name: "second".into(),
        };
        let envelope = JobEnvelope {
            job: StepJob::ID,
            queue: queue.clone(),
            attempts: 0,
            scheduled_at: chrono::Utc::now().timestamp_millis(),
            payload: serde_json::to_value(first).unwrap(),
            batch_id: None,
            chain_remaining: Some(vec![ChainedJob {
                job: StepJob::ID,
                queue: queue.clone(),
                payload: serde_json::to_value(second).unwrap(),
            }]),
        };
        let payload = serde_json::to_string(&envelope).unwrap();
        backend
            .enqueue_job(&queue, "lost-success-token", &payload)
            .await
            .unwrap();

        let lease = runtime.claim_job().await.unwrap().unwrap();
        let completed = backend
            .complete_successful_job(
                &queue,
                &lease.token,
                &queue,
                SuccessfulJobEffects::default(),
            )
            .await
            .unwrap();
        assert!(completed.lease_released);

        let worker = Worker::from_app(app).unwrap();
        worker.process_claimed_job(lease).await.unwrap();

        assert!(runtime.claim_job().await.unwrap().is_none());
        let log = read_log_filtered(&format!("{tag}:"));
        assert_eq!(log, vec!["first"]);
        assert_eq!(diagnostics.snapshot().jobs.succeeded_total, 0);
    }

    #[tokio::test]
    async fn rate_limit_requeue_lost_lease_does_not_schedule_duplicate_job() {
        let tag = "rate-limit-lost-lease";
        let (backend, runtime, diagnostics, _dispatcher) =
            build_runtime_and_dispatcher("rate-limit-lost-lease");
        let app = build_app(runtime.clone(), diagnostics.clone());
        let queue = QueueId::new("default");
        let envelope = JobEnvelope {
            job: RateLimitedJob::ID,
            queue: queue.clone(),
            attempts: 0,
            scheduled_at: chrono::Utc::now().timestamp_millis(),
            payload: serde_json::to_value(RateLimitedJob { tag: tag.into() }).unwrap(),
            batch_id: None,
            chain_remaining: None,
        };
        backend
            .enqueue_job(
                &queue,
                "rate-limit-lost-token",
                &serde_json::to_string(&envelope).unwrap(),
            )
            .await
            .unwrap();

        let lease = runtime.claim_job().await.unwrap().unwrap();
        let completed = backend
            .complete_successful_job(
                &queue,
                &lease.token,
                &queue,
                SuccessfulJobEffects::default(),
            )
            .await
            .unwrap();
        assert!(completed.lease_released);

        let worker = Worker::from_app(app).unwrap();
        worker.process_claimed_job(lease).await.unwrap();

        assert_eq!(
            runtime
                .promote_due_jobs(chrono::Utc::now().timestamp_millis() + 2_000)
                .await
                .unwrap(),
            0
        );
        assert!(runtime.claim_job().await.unwrap().is_none());
        assert!(read_log_filtered(&format!("{tag}:")).is_empty());

        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.jobs.retried_total, 0);
        assert_eq!(snapshot.jobs.succeeded_total, 0);
        assert_eq!(snapshot.jobs.dead_lettered_total, 0);
    }

    #[tokio::test]
    async fn batch_on_complete_is_enqueued_once_when_completion_count_exceeds_total() {
        let tag = "batch-callback-once";
        let (backend, runtime, diagnostics, dispatcher) =
            build_runtime_and_dispatcher("batch-callback-once");
        let app = build_app(runtime, diagnostics);
        let queue = QueueId::new("default");

        let batch_id = dispatcher
            .batch("callback-once")
            .add(StepJob {
                tag: tag.into(),
                name: "primary".into(),
            })
            .unwrap()
            .on_complete(CompletionJob {
                tag: tag.into(),
                label: "done".into(),
            })
            .unwrap()
            .dispatch()
            .await
            .unwrap();

        let duplicate = StepJob {
            tag: tag.into(),
            name: "duplicate".into(),
        };
        let duplicate_envelope = JobEnvelope {
            job: StepJob::ID,
            queue: queue.clone(),
            attempts: 0,
            scheduled_at: chrono::Utc::now().timestamp_millis(),
            payload: serde_json::to_value(duplicate).unwrap(),
            batch_id: Some(batch_id),
            chain_remaining: None,
        };
        backend
            .enqueue_job(
                &queue,
                "duplicate-batch-completion-token",
                &serde_json::to_string(&duplicate_envelope).unwrap(),
            )
            .await
            .unwrap();

        let worker = Worker::from_app(app).unwrap();
        assert!(worker.run_once().await.unwrap());
        assert!(worker.run_once().await.unwrap());
        assert!(worker.run_once().await.unwrap());
        assert!(!worker.run_once().await.unwrap());

        let log = read_log_filtered(&format!("{tag}:"));
        assert!(log.contains(&"primary".to_string()));
        assert!(log.contains(&"duplicate".to_string()));
        assert_eq!(
            log.iter().filter(|entry| *entry == "complete:done").count(),
            1
        );
    }

    #[tokio::test]
    async fn empty_batch_returns_error() {
        let (_backend, _runtime, _diagnostics, dispatcher) =
            build_runtime_and_dispatcher("batch-empty");
        let result = dispatcher.batch("empty").dispatch().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn empty_chain_returns_error() {
        let (_backend, _runtime, _diagnostics, dispatcher) =
            build_runtime_and_dispatcher("chain-empty");
        let result = dispatcher.chain().dispatch().await;
        assert!(result.is_err());
    }
}
