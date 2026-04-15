# jobs

Background job queue with leased at-least-once delivery

[Back to index](../index.md)

## forge::jobs

```rust
enum JobHistoryStatus { Succeeded, Retried, DeadLettered }
  fn as_str(&self) -> &'static str
struct JobBatchBuilder
  fn add<J: Job>(self, job: J) -> Result<Self>
  fn on_complete<J: Job>(self, job: J) -> Result<Self>
  async fn dispatch(self) -> Result<String>
struct JobChainBuilder
  fn add<J: Job>(self, job: J) -> Result<Self>
  async fn dispatch(self) -> Result<()>
struct JobContext
  fn app(&self) -> &AppContext
  fn queue(&self) -> &QueueId
  fn attempt(&self) -> u32
struct JobDispatcher
  async fn dispatch<J>(&self, job: J) -> Result<()>
  async fn dispatch_later<J>(&self, job: J, run_at_millis: i64) -> Result<()>
  fn batch(&self, name: &str) -> JobBatchBuilder
  fn chain(&self) -> JobChainBuilder
struct JobMiddlewareRegistry
struct Worker
  fn from_app(app: AppContext) -> Result<Self>
  fn app(&self) -> &AppContext
  async fn run(self) -> Result<()>
  async fn run_once(&self) -> Result<bool>
trait Job: DeserializeOwned + Debug
  fn handle<'life0, 'async_trait>(
  fn max_retries(&self) -> Option<u32>
  fn backoff(&self, attempt: u32) -> Duration
  fn timeout(&self) -> Option<Duration>
  fn rate_limit(&self) -> Option<(u32, Duration)>
  fn unique_for(&self) -> Option<Duration>
  fn unique_key(&self) -> Option<String>
trait JobMiddleware
  fn before<'life0, 'life1, 'life2, 'async_trait>(
  fn after<'life0, 'life1, 'life2, 'async_trait>(
  fn failed<'life0, 'life1, 'life2, 'life3, 'async_trait>(
fn spawn_worker(app: AppContext) -> Result<JoinHandle<()>>
```

