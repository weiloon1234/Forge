mod backend;

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::config::JobsConfig;
use crate::database::{DbType, DbValue};
use crate::foundation::{AppContext, Error, Result};
use crate::logging::{JobOutcome as RecordedJobOutcome, RuntimeDiagnostics};
use crate::support::runtime::RuntimeBackend;
use crate::support::{JobId, QueueId};

use self::backend::ClaimedJobLease;

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
            if let Err(error) = mw.before(job_id, context).await {
                tracing::warn!(
                    target: "forge.worker",
                    job = %job_id,
                    error = %error,
                    "job middleware before hook failed"
                );
            }
        }
    }

    async fn run_after(&self, job_id: &JobId, context: &JobContext) {
        for mw in &self.middlewares {
            if let Err(error) = mw.after(job_id, context).await {
                tracing::warn!(
                    target: "forge.worker",
                    job = %job_id,
                    error = %error,
                    "job middleware after hook failed"
                );
            }
        }
    }

    async fn run_failed(&self, job_id: &JobId, context: &JobContext, err: &str) {
        for mw in &self.middlewares {
            if let Err(error) = mw.failed(job_id, context, err).await {
                tracing::warn!(
                    target: "forge.worker",
                    job = %job_id,
                    error = %error,
                    "job middleware failed hook failed"
                );
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
        // Unique job check: skip dispatch if a duplicate exists within the window
        if let Some(unique_duration) = job.unique_for() {
            let unique_suffix = job.unique_key().unwrap_or_else(|| J::ID.to_string());
            let unique_redis_key = format!("jobs:unique:{}:{}", J::ID, unique_suffix);
            let ttl_secs = unique_duration.as_secs().max(1);
            let is_new = self
                .runtime
                .backend
                .set_if_absent(&unique_redis_key, ttl_secs)
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
        }

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
        let total = self.jobs.len() as u64;

        // Build on_complete envelope string for storage
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

        // Store batch metadata
        self.dispatcher
            .runtime
            .backend
            .create_batch(
                &batch_id,
                total,
                on_complete_payload.as_deref(),
                on_complete_queue.as_deref(),
            )
            .await?;

        // Dispatch each job with the batch_id embedded
        let now = Utc::now().timestamp_millis();
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
            self.dispatcher
                .runtime
                .backend
                .enqueue_job(&queue, &token, &serialized)
                .await?;
            self.dispatcher
                .diagnostics
                .record_job_outcome(RecordedJobOutcome::Enqueued);
        }

        tracing::info!(
            target: "forge.worker",
            batch_id = %batch_id,
            total = total,
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
        // 0 = unlimited (use a large semaphore that never blocks in practice)
        let max_concurrent = if self.runtime.config.max_concurrent_jobs == 0 {
            u32::MAX >> 1 // ~1 billion — effectively unlimited
        } else {
            self.runtime.config.max_concurrent_jobs as u32
        };
        let worker = Arc::new(self);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent as usize));

        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        {
            let tx = shutdown_tx.clone();
            tokio::spawn(async move {
                crate::kernel::shutdown::shutdown_signal().await;
                let _ = tx.send(true);
            });
        }
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
            if *shutdown_rx.borrow() {
                tracing::info!(target: "forge.worker", "shutting down, waiting for in-flight jobs");
                let _ = semaphore.acquire_many(max_concurrent).await;
                maintenance_handle.abort();
                tracing::info!(target: "forge.worker", "all jobs drained, worker stopped");
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

            match worker.runtime.claim_job().await {
                Ok(Some(lease)) => {
                    worker
                        .diagnostics
                        .record_job_outcome(RecordedJobOutcome::Leased);
                    let w = worker.clone();
                    tokio::spawn(async move {
                        if let Err(error) = w.process_claimed_job(lease).await {
                            tracing::error!(target: "forge.worker", error = %error, "job processing failed");
                        }
                        drop(permit);
                    });
                }
                Ok(None) => {
                    drop(permit);
                    tokio::time::sleep(worker.runtime.poll_interval()).await;
                }
                Err(error) => {
                    drop(permit);
                    tracing::error!(target: "forge.worker", error = %error, "claim failed");
                    tokio::time::sleep(worker.runtime.poll_interval()).await;
                }
            }
        }

        Ok(())
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

        let envelope: JobEnvelope = serde_json::from_str(&lease.payload).map_err(Error::other)?;
        let registration = self
            .runtime
            .registry
            .jobs
            .get(&envelope.job)
            .ok_or_else(|| Error::message(format!("job `{}` is not registered", envelope.job)))?;

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
                self.runtime
                    .retry_job(
                        &lease.queue,
                        &lease.token,
                        &requeue_token,
                        &payload,
                        requeue_at,
                    )
                    .await?;
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

        let middleware = self.app.resolve::<JobMiddlewareRegistry>().ok();
        let job_context = JobContext::new(
            self.app.clone(),
            envelope.queue.clone(),
            envelope.attempts + 1,
        );

        // Before hooks
        if let Some(ref mw) = middleware {
            mw.run_before(&envelope.job, &job_context).await;
        }

        let started_at = Utc::now().timestamp_millis();

        let (heartbeat, shutdown_heartbeat) =
            self.spawn_lease_heartbeat(lease.queue.clone(), lease.token.clone());
        let default_timeout = Duration::from_secs(self.runtime.config.timeout_seconds.max(1));
        let execution = registration
            .handler
            .execute(
                &self.app,
                &envelope,
                self.runtime.config.max_retries,
                default_timeout,
            )
            .await?;
        let _ = shutdown_heartbeat.send(());
        heartbeat.abort();
        let _ = heartbeat.await;

        match execution {
            JobExecutionOutcome::Success => {
                if let Some(ref mw) = middleware {
                    mw.run_after(&envelope.job, &job_context).await;
                }
                if !self.runtime.ack_job(&lease.queue, &lease.token).await? {
                    tracing::warn!(
                        target: "forge.worker",
                        queue = %lease.queue,
                        token = %lease.token,
                        "Lost job lease before ack"
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

                // --- Batch completion check ---
                if let Some(ref batch_id) = envelope.batch_id {
                    if let Err(error) = self.handle_batch_completion(batch_id).await {
                        tracing::error!(
                            target: "forge.worker",
                            batch_id = %batch_id,
                            error = %error,
                            "Failed to handle batch completion"
                        );
                    }
                }

                // --- Chain continuation ---
                if let Some(remaining) = envelope.chain_remaining {
                    if let Err(error) = self.handle_chain_continuation(remaining).await {
                        tracing::error!(
                            target: "forge.worker",
                            error = %error,
                            "Failed to dispatch next job in chain"
                        );
                    }
                }

                Ok(())
            }
            JobExecutionOutcome::Retry {
                run_at_millis,
                attempts,
            } => {
                if let Some(ref mw) = middleware {
                    mw.run_failed(&envelope.job, &job_context, "job failed, scheduling retry")
                        .await;
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
                    error: Some("job failed, scheduling retry"),
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

                Ok(())
            }
        }
    }

    fn spawn_lease_heartbeat(
        &self,
        queue: QueueId,
        token: String,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let runtime = self.runtime.clone();
        let heartbeat_every = runtime.lease_heartbeat_interval();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let heartbeat = tokio::spawn(async move {
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
        (heartbeat, shutdown_tx)
    }

    /// After a batched job succeeds, increment the completion counter and
    /// dispatch the on_complete callback when all jobs are done.
    async fn handle_batch_completion(&self, batch_id: &str) -> Result<()> {
        let (completed, total, on_complete_payload, on_complete_queue) = self
            .runtime
            .backend
            .increment_batch_completed(batch_id)
            .await?;

        tracing::debug!(
            target: "forge.worker",
            batch_id = %batch_id,
            completed = completed,
            total = total,
            "Batch progress"
        );

        if completed >= total {
            if let Some(payload) = on_complete_payload {
                let queue = on_complete_queue
                    .map(QueueId::owned)
                    .unwrap_or_else(|| self.runtime.config.queue.clone());
                let token = next_delivery_token();
                self.runtime
                    .backend
                    .enqueue_job(&queue, &token, &payload)
                    .await?;
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
                    "Batch completed (no on_complete callback)"
                );
            }
        }

        Ok(())
    }

    /// After a chained job succeeds, dispatch the next job in the chain,
    /// carrying forward the remaining chain entries.
    async fn handle_chain_continuation(&self, mut remaining: Vec<ChainedJob>) -> Result<()> {
        if remaining.is_empty() {
            return Ok(());
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
        self.runtime
            .backend
            .enqueue_job(&next.queue, &token, &serialized)
            .await?;
        self.diagnostics
            .record_job_outcome(RecordedJobOutcome::Enqueued);

        tracing::info!(
            target: "forge.worker",
            job = %next.job,
            "Chain continuation dispatched"
        );

        Ok(())
    }
}

/// Terminal status for a job recorded in the `job_history` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, forge_macros::AppEnum, ts_rs::TS, forge_macros::TS)]
#[ts(export)]
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

    async fn ack_job(&self, queue: &QueueId, token: &str) -> Result<bool> {
        self.backend.ack_job(queue, token).await
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

enum JobExecutionOutcome {
    Success,
    Retry { run_at_millis: i64, attempts: u32 },
    DeadLetter { error: String, attempts: u32 },
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
        let result = tokio::time::timeout(timeout_duration, job.handle(context)).await;

        let error_msg = match result {
            Ok(Ok(())) => return Ok(JobExecutionOutcome::Success),
            Ok(Err(error)) => error.to_string(),
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
            });
        }
    }

    fn check_rate_limit(&self, envelope: &JobEnvelope) -> Option<(u32, Duration)> {
        let job: J = serde_json::from_value(envelope.payload.clone()).ok()?;
        job.rate_limit()
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
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use serde::{Deserialize, Serialize};

    use super::{Job, JobContext, JobDispatcher, JobRegistryBuilder, JobRuntime, Worker};
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
