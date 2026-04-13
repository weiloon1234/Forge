# logging

Structured logging, observability, health probes, diagnostics

[Back to index](../index.md)

## forge::logging

```rust
pub const FRAMEWORK_BOOTSTRAP_PROBE: ProbeId;
pub const REDIS_PING_PROBE: ProbeId;
pub const REQUEST_ID_HEADER: &str;
pub const RUNTIME_BACKEND_PROBE: ProbeId;
enum AuthOutcome { Success, Unauthorized, Forbidden, Error }
enum HttpOutcomeClass { Informational, Success, Redirection, ClientError, ServerError }
  fn from_status(status: StatusCode) -> Self
enum JobOutcome { Enqueued, Leased, Started, Succeeded, Retried, ExpiredLeaseRequeued, DeadLettered }
enum LogFormat { Json, Text }
enum LogLevel { Trace, Debug, Info, Warn, Error }
  fn as_filter_directive(self) -> &'static str
enum ProbeState { Healthy, Unhealthy }
  fn is_healthy(self) -> bool
enum RuntimeBackendKind { Redis, Memory }
enum SchedulerLeadershipState { Acquired, Lost }
enum WebSocketConnectionState { Opened, Closed }
struct LivenessReport
struct ObservabilityOptions
  fn new() -> Self
  fn guard<I>(self, guard: I) -> Self
  fn permission<I>(self, permission: I) -> Self
  fn permissions<I, P>(self, permissions: I) -> Self
  fn access(&self) -> &AccessScope
struct ProbeResult
  fn healthy<I>(id: I) -> Self
  fn unhealthy<I>(id: I, message: impl Into<String>) -> Self
struct ReadinessReport
struct RequestId
  fn new(value: impl Into<String>) -> Self
  fn as_str(&self) -> &str
struct RuntimeDiagnostics
  fn backend_kind(&self) -> RuntimeBackendKind
  fn mark_bootstrap_complete(&self)
  fn bootstrap_complete(&self) -> bool
  fn liveness(&self) -> LivenessReport
  fn snapshot(&self) -> RuntimeSnapshot
  async fn run_readiness_checks( &self, app: &AppContext, ) -> Result<ReadinessReport>
  fn record_http_response(&self, status: StatusCode)
  fn record_auth_outcome(&self, outcome: AuthOutcome)
  fn record_websocket_connection(&self, state: WebSocketConnectionState)
  fn record_websocket_subscription_opened(&self)
  fn record_websocket_subscription_closed(&self)
  fn record_websocket_inbound_message(&self)
  fn record_websocket_outbound_message(&self)
  fn record_scheduler_tick(&self)
  fn record_schedule_executed(&self)
  fn record_scheduler_leadership(&self, state: SchedulerLeadershipState)
  fn set_scheduler_leader_active(&self, active: bool)
  fn record_job_outcome(&self, outcome: JobOutcome)
struct RuntimeSnapshot
trait ReadinessCheck
  fn run<'life0, 'life1, 'async_trait>(
fn init(config: &ConfigRepository) -> Result<()>
```

