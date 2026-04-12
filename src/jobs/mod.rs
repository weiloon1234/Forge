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
use crate::foundation::{AppContext, Error, Result};
use crate::logging::{JobOutcome as RecordedJobOutcome, RuntimeDiagnostics};
use crate::support::runtime::RuntimeBackend;
use crate::support::{JobId, QueueId};

use self::backend::ClaimedJobLease;

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
        let queue = J::QUEUE
            .clone()
            .unwrap_or_else(|| self.runtime.config.queue.clone());
        let envelope = JobEnvelope {
            job: J::ID,
            queue: queue.clone(),
            attempts: 0,
            scheduled_at: run_at_millis,
            payload: serde_json::to_value(job).map_err(Error::other)?,
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

    pub async fn run(self) -> Result<()> {
        loop {
            let did_work = self.run_once().await?;
            if !did_work {
                tokio::time::sleep(self.runtime.poll_interval()).await;
            }
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

        let envelope: JobEnvelope = serde_json::from_str(&lease.payload).map_err(Error::other)?;
        let registration = self
            .runtime
            .registry
            .jobs
            .get(&envelope.job)
            .ok_or_else(|| Error::message(format!("job `{}` is not registered", envelope.job)))?;

        let (heartbeat, shutdown_heartbeat) =
            self.spawn_lease_heartbeat(lease.queue.clone(), lease.token.clone());
        let execution = registration
            .handler
            .execute(&self.app, &envelope, self.runtime.config.max_retries)
            .await?;
        let _ = shutdown_heartbeat.send(());
        heartbeat.abort();
        let _ = heartbeat.await;

        match execution {
            JobExecutionOutcome::Success => {
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
                Ok(())
            }
            JobExecutionOutcome::Retry {
                run_at_millis,
                attempts,
            } => {
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
                Ok(())
            }
            JobExecutionOutcome::DeadLetter { error, attempts } => {
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

        JobRegistrySnapshot {
            jobs,
            queues: queues.into_iter().collect(),
        }
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
    ) -> Result<JobExecutionOutcome>;
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

        let context = JobContext::new(app.clone(), envelope.queue.clone(), envelope.attempts + 1);
        match job.handle(context).await {
            Ok(()) => Ok(JobExecutionOutcome::Success),
            Err(error) => {
                let attempts = envelope.attempts + 1;
                let max_retries = job.max_retries().unwrap_or(default_max_retries);
                if attempts >= max_retries {
                    Ok(JobExecutionOutcome::DeadLetter {
                        error: error.to_string(),
                        attempts,
                    })
                } else {
                    let run_at_millis =
                        Utc::now().timestamp_millis() + job.backoff(attempts).as_millis() as i64;
                    Ok(JobExecutionOutcome::Retry {
                        run_at_millis,
                        attempts,
                    })
                }
            }
        }
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
}
