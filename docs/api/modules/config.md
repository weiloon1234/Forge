# config

TOML-based configuration (ConfigRepository, AppConfig, etc.)

[Back to index](../index.md)

## forge::config

```rust
enum CacheDriver { Redis, Memory }
enum Environment { Development, Production, Testing }
  fn is_production(self) -> bool
  fn is_development(self) -> bool
  fn is_testing(self) -> bool
enum GuardDriver { Token, Session, Custom }
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
struct GuardDriverConfig
struct HashingConfig
struct I18nConfig
struct JobsConfig
struct LockoutConfig
struct LoggingConfig
struct MfaConfig
struct ObservabilityConfig
struct RedisConfig
struct SchedulerConfig
struct ServerConfig
struct SessionConfig
struct TokenConfig
struct TypeScriptConfig
struct WebSocketConfig
struct WebSocketObservabilityConfig
```

