# config

TOML-based configuration (ConfigRepository, AppConfig, etc.)

[Back to index](../index.md)

## forge::config

```rust
enum CacheDriver { Redis, Memory }
enum CacheErrorMode { Strict, FailOpen }
enum Environment { Development, Production, Staging, Testing, Custom }
  fn from_label(label: impl Into<String>) -> Self
  fn as_str(&self) -> &str
  fn is_production(&self) -> bool
  fn is_production_like(&self) -> bool
  fn is_development(&self) -> bool
  fn is_staging(&self) -> bool
  fn is_testing(&self) -> bool
enum GuardDriver { Token, Session, Custom }
enum HttpRateLimitByConfig { Ip, Actor, ActorOrIp }
struct AppConfig
  fn signing_key_bytes(&self) -> Result<Vec<u8>>
struct AuthConfig
struct CacheConfig
struct ConfigRepository
  fn empty() -> Self
  fn from_dir(path: impl AsRef<Path>) -> Result<Self>
  fn with_env_overlay_only() -> Result<Self>
  fn root(&self) -> Arc<Value>
  fn value(&self, path: &str) -> Option<Value>
  fn string(&self, path: &str) -> Option<String>
  fn section<T>(&self, section: &str) -> Result<T>
  fn server(&self) -> Result<ServerConfig>
  fn http(&self) -> Result<HttpConfig>
  fn app(&self) -> Result<AppConfig>
  fn redis(&self) -> Result<RedisConfig>
  fn database(&self) -> Result<DatabaseConfig>
  fn websocket(&self) -> Result<WebSocketConfig>
  fn jobs(&self) -> Result<JobsConfig>
  fn auth(&self) -> Result<AuthConfig>
  fn scheduler(&self) -> Result<SchedulerConfig>
  fn logging(&self) -> Result<LoggingConfig>
  fn i18n(&self) -> Result<I18nConfig>
  fn typescript(&self) -> Result<TypeScriptConfig>
  fn observability(&self) -> Result<ObservabilityConfig>
  fn storage(&self) -> Result<StorageConfig>
  fn email(&self) -> Result<EmailConfig>
  fn hashing(&self) -> Result<HashingConfig>
  fn cache(&self) -> Result<CacheConfig>
  fn crypt(&self) -> Result<CryptConfig>
struct CryptConfig
struct DatabaseConfig
struct DatabaseModelConfig
struct EmailVerificationConfig
struct GuardDriverConfig
struct HashingConfig
struct HttpConfig
struct HttpCorsConfig
struct HttpCsrfConfig
struct HttpRateLimitConfig
struct HttpSecurityHeadersConfig
struct HttpTrustedProxyConfig
struct I18nConfig
struct JobsConfig
struct LockoutConfig
struct LoggingConfig
struct MfaConfig
struct ObservabilityConfig
struct PasswordResetConfig
struct RedisConfig
struct SchedulerConfig
struct ServerConfig
struct SessionConfig
struct TokenConfig
  fn access_token_ttl_minutes_for_guard(&self, guard: &GuardId) -> u64
  fn refresh_token_ttl_days_for_guard(&self, guard: &GuardId) -> u64
struct TokenGuardConfig
struct TypeScriptConfig
struct WebSocketConfig
struct WebSocketObservabilityConfig
```

## Notes

- `AppConfig` fields: `name`, `environment`, `timezone`, `signing_key`, `background_shutdown_timeout_ms`.
- `HttpConfig` is optional and additive: global body cap, request timeout, CORS, CSRF, trusted proxy, and rate limiting are opt-in; security headers are enabled by default with HSTS off.
- `CacheConfig.error_mode` defaults to `strict`; `remember_singleflight` is enabled by default and distributed remember locks are opt-in.
- `DatabaseConfig.migration_lock_timeout_ms` defaults to `0`; `db:migrate` and `db:rollback` wait forever for the migration advisory lock unless overridden.
- `DatabaseConfig.redact_sql_literals` is enabled by default so SQL logs and `/_forge/sql` retain query shape without common literal values.
- `JobsConfig` includes `shutdown_timeout_ms` for active worker job draining; `0` aborts active jobs immediately.
- `JobsConfig.history_retention_days` defaults to `30`; `0` keeps `job_history` forever.
- `ObservabilityConfig.enabled` gates `/_forge/*` route registration; `capture_enabled` gates passive runtime capture.
- `SchedulerConfig` includes `shutdown_timeout_ms` for active schedule task draining; `0` aborts active schedules immediately.
- `WebSocketConfig` bounds inbound message/frame sizes and client-supplied channel, room, event, ack, and subscription cardinality.

