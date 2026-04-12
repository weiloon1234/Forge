use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use toml::Value;

use crate::foundation::{Error, Result};
use crate::logging::{LogFormat, LogLevel};
use crate::support::{GuardId, QueueId, Timezone};

#[derive(Clone, Debug)]
pub struct ConfigRepository {
    root: Arc<Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub timezone: Timezone,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            timezone: Timezone::utc(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct RedisConfig {
    pub url: String,
    pub namespace: String,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            namespace: "forge".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DatabaseModelConfig {
    pub timestamps_default: bool,
    pub soft_deletes_default: bool,
}

impl Default for DatabaseModelConfig {
    fn default() -> Self {
        Self {
            timestamps_default: true,
            soft_deletes_default: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub url: String,
    pub schema: String,
    pub migration_table: String,
    pub migrations_path: String,
    pub seeders_path: String,
    pub min_connections: u32,
    pub max_connections: u32,
    pub acquire_timeout_ms: u64,
    pub models: DatabaseModelConfig,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            schema: "public".to_string(),
            migration_table: "forge_migrations".to_string(),
            migrations_path: "database/migrations".to_string(),
            seeders_path: "database/seeders".to_string(),
            min_connections: 1,
            max_connections: 10,
            acquire_timeout_ms: 5_000,
            models: DatabaseModelConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct WebSocketConfig {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub heartbeat_interval_seconds: u64,
    pub heartbeat_timeout_seconds: u64,
    pub max_messages_per_second: u32,
    pub max_connections_per_user: u32,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3010,
            path: "/ws".to_string(),
            heartbeat_interval_seconds: 30,
            heartbeat_timeout_seconds: 10,
            max_messages_per_second: 50,
            max_connections_per_user: 5,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct JobsConfig {
    pub queue: QueueId,
    pub max_retries: u32,
    pub poll_interval_ms: u64,
    pub lease_ttl_ms: u64,
    pub requeue_batch_size: usize,
    pub workers: usize,
    pub timeout_seconds: u64,
    pub track_history: bool,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self {
            queue: QueueId::new("default"),
            max_retries: 5,
            poll_interval_ms: 100,
            lease_ttl_ms: 30_000,
            requeue_batch_size: 64,
            workers: 4,
            timeout_seconds: 300,
            track_history: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    pub tick_interval_ms: u64,
    pub leader_lease_ttl_ms: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            tick_interval_ms: 1_000,
            leader_lease_ttl_ms: 5_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub default_guard: GuardId,
    pub bearer_prefix: String,
    pub tokens: TokenConfig,
    pub sessions: SessionConfig,
    #[serde(default)]
    pub guards: std::collections::HashMap<String, GuardDriverConfig>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            default_guard: GuardId::new("api"),
            bearer_prefix: "Bearer".to_string(),
            tokens: TokenConfig::default(),
            sessions: SessionConfig::default(),
            guards: std::collections::HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct TokenConfig {
    pub access_token_ttl_minutes: u64,
    pub refresh_token_ttl_days: u64,
    pub token_length: usize,
    pub rotate_refresh_tokens: bool,
}

impl Default for TokenConfig {
    fn default() -> Self {
        Self {
            access_token_ttl_minutes: 15,
            refresh_token_ttl_days: 30,
            token_length: 32,
            rotate_refresh_tokens: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub ttl_minutes: u64,
    pub cookie_name: String,
    pub cookie_secure: bool,
    pub cookie_path: String,
    pub sliding_expiry: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            ttl_minutes: 120,
            cookie_name: "forge_session".to_string(),
            cookie_secure: true,
            cookie_path: "/".to_string(),
            sliding_expiry: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct GuardDriverConfig {
    pub driver: GuardDriver,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuardDriver {
    Token,
    Session,
    Custom,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: LogLevel,
    pub format: LogFormat,
    pub log_dir: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            format: LogFormat::default(),
            log_dir: "logs".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct I18nConfig {
    pub default_locale: String,
    pub fallback_locale: String,
    pub resource_path: String,
}

impl Default for I18nConfig {
    fn default() -> Self {
        Self {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            resource_path: "locales".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ObservabilityConfig {
    pub base_path: String,
    pub tracing_enabled: bool,
    pub otlp_endpoint: String,
    pub service_name: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            base_path: "/_forge".to_string(),
            tracing_enabled: false,
            otlp_endpoint: "http://localhost:4317".to_string(),
            service_name: "forge".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct HashingConfig {
    pub driver: String,
    #[serde(default = "default_memory_cost")]
    pub memory_cost: u32,
    #[serde(default = "default_time_cost")]
    pub time_cost: u32,
    #[serde(default = "default_parallelism")]
    pub parallelism: u32,
}

fn default_memory_cost() -> u32 {
    19456
}
fn default_time_cost() -> u32 {
    2
}
fn default_parallelism() -> u32 {
    1
}

impl Default for HashingConfig {
    fn default() -> Self {
        Self {
            driver: "argon2".to_string(),
            memory_cost: default_memory_cost(),
            time_cost: default_time_cost(),
            parallelism: default_parallelism(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(default)]
pub struct CryptConfig {
    pub key: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    pub driver: CacheDriver,
    pub prefix: String,
    pub ttl_seconds: u64,
    pub max_entries: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            driver: CacheDriver::Redis,
            prefix: "cache:".to_string(),
            ttl_seconds: 3600,
            max_entries: 10000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheDriver {
    Redis,
    Memory,
}

impl Default for ConfigRepository {
    fn default() -> Self {
        Self::empty()
    }
}

impl ConfigRepository {
    pub fn empty() -> Self {
        Self {
            root: Arc::new(Value::Table(Default::default())),
        }
    }

    pub fn from_dir(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_dir_with_defaults(path, std::iter::empty())
    }

    pub(crate) fn from_dir_with_defaults<I>(path: impl AsRef<Path>, defaults: I) -> Result<Self>
    where
        I: IntoIterator<Item = Value>,
    {
        let path = path.as_ref();
        let mut root = root_with_defaults(defaults);

        let mut entries = fs::read_dir(path)
            .map_err(Error::other)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("toml"))
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let file = entry.path();
            let content = fs::read_to_string(&file).map_err(Error::other)?;
            let value: Value = if content.trim().is_empty() {
                Value::Table(Default::default())
            } else {
                toml::from_str(&content).map_err(Error::other)?
            };
            merge_value(&mut root, value);
        }

        overlay_env_vars(&mut root)?;

        Ok(Self {
            root: Arc::new(root),
        })
    }

    pub fn with_env_overlay_only() -> Result<Self> {
        Self::with_env_overlay_and_defaults(std::iter::empty())
    }

    pub(crate) fn with_env_overlay_and_defaults<I>(defaults: I) -> Result<Self>
    where
        I: IntoIterator<Item = Value>,
    {
        let mut root = root_with_defaults(defaults);
        overlay_env_vars(&mut root)?;
        Ok(Self {
            root: Arc::new(root),
        })
    }

    pub fn root(&self) -> Arc<Value> {
        self.root.clone()
    }

    pub fn value(&self, path: &str) -> Option<Value> {
        let mut current = &*self.root;
        for segment in path.split('.') {
            current = current.get(segment)?;
        }
        Some(current.clone())
    }

    pub fn string(&self, path: &str) -> Option<String> {
        self.value(path)?.as_str().map(ToOwned::to_owned)
    }

    pub fn section<T>(&self, section: &str) -> Result<T>
    where
        T: DeserializeOwned + Default,
    {
        match self.value(section) {
            Some(value) => value.try_into().map_err(Error::other),
            None => Ok(T::default()),
        }
    }

    pub fn server(&self) -> Result<ServerConfig> {
        self.section("server")
    }

    pub fn app(&self) -> Result<AppConfig> {
        self.section("app")
    }

    pub fn redis(&self) -> Result<RedisConfig> {
        self.section("redis")
    }

    pub fn database(&self) -> Result<DatabaseConfig> {
        self.section("database")
    }

    pub fn websocket(&self) -> Result<WebSocketConfig> {
        self.section("websocket")
    }

    pub fn jobs(&self) -> Result<JobsConfig> {
        self.section("jobs")
    }

    pub fn auth(&self) -> Result<AuthConfig> {
        self.section("auth")
    }

    pub fn scheduler(&self) -> Result<SchedulerConfig> {
        self.section("scheduler")
    }

    pub fn logging(&self) -> Result<LoggingConfig> {
        self.section("logging")
    }

    pub fn i18n(&self) -> Result<I18nConfig> {
        self.section("i18n")
    }

    pub fn observability(&self) -> Result<ObservabilityConfig> {
        self.section("observability")
    }

    pub fn storage(&self) -> Result<crate::storage::StorageConfig> {
        self.section("storage")
    }

    pub fn email(&self) -> Result<crate::email::config::EmailConfig> {
        self.section("email")
    }

    pub fn hashing(&self) -> Result<HashingConfig> {
        self.section("hashing")
    }

    pub fn cache(&self) -> Result<CacheConfig> {
        self.section("cache")
    }

    pub fn crypt(&self) -> Result<CryptConfig> {
        self.section("crypt")
    }
}

fn root_with_defaults<I>(defaults: I) -> Value
where
    I: IntoIterator<Item = Value>,
{
    let mut root = Value::Table(Default::default());
    for defaults in defaults {
        merge_value(&mut root, defaults);
    }
    root
}

fn merge_value(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Table(target_table), Value::Table(source_table)) => {
            for (key, value) in source_table {
                match target_table.get_mut(&key) {
                    Some(existing) => merge_value(existing, value),
                    None => {
                        target_table.insert(key, value);
                    }
                }
            }
        }
        (target, source) => {
            *target = source;
        }
    }
}

fn overlay_env_vars(root: &mut Value) -> Result<()> {
    for (key, raw_value) in std::env::vars() {
        if !key.contains("__") {
            continue;
        }

        let segments = key
            .split("__")
            .filter(|segment| !segment.is_empty())
            .map(|segment| segment.to_ascii_lowercase())
            .collect::<Vec<_>>();

        if segments.is_empty() {
            continue;
        }

        let value = parse_env_value(&raw_value)?;
        set_value(root, &segments, value);
    }

    Ok(())
}

fn parse_env_value(raw: &str) -> Result<Value> {
    if let Ok(boolean) = raw.parse::<bool>() {
        return Ok(Value::Boolean(boolean));
    }
    if let Ok(integer) = raw.parse::<i64>() {
        return Ok(Value::Integer(integer));
    }
    if let Ok(float) = raw.parse::<f64>() {
        return Ok(Value::Float(float));
    }
    if raw.starts_with('[') || raw.starts_with('{') {
        let wrapped = format!("value = {raw}");
        let parsed: BTreeMap<String, Value> = toml::from_str(&wrapped).map_err(Error::other)?;
        if let Some(value) = parsed.get("value") {
            return Ok(value.clone());
        }
    }

    Ok(Value::String(raw.to_string()))
}

fn set_value(root: &mut Value, path: &[String], value: Value) {
    let mut current = root;
    for segment in &path[..path.len() - 1] {
        match current {
            Value::Table(table) => {
                current = table
                    .entry(segment.clone())
                    .or_insert_with(|| Value::Table(Default::default()));
            }
            _ => return,
        }
    }

    if let Value::Table(table) = current {
        table.insert(path[path.len() - 1].clone(), value);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    use tempfile::tempdir;

    use super::{
        AppConfig, AuthConfig, ConfigRepository, DatabaseConfig, JobsConfig, LoggingConfig,
        ObservabilityConfig, RedisConfig, SchedulerConfig, WebSocketConfig,
    };
    use crate::logging::{LogFormat, LogLevel};
    use crate::support::{GuardId, QueueId};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn merges_config_files_in_lexical_order() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("SERVER__PORT");
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-base.toml"),
            r#"
                [server]
                host = "127.0.0.1"
                port = 3000
            "#,
        )
        .unwrap();
        fs::write(
            directory.path().join("10-override.toml"),
            r#"
                [server]
                port = 4001
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let server = config.server().unwrap();

        assert_eq!(server.host, "127.0.0.1");
        assert_eq!(server.port, 4001);
    }

    #[test]
    fn parses_app_timezone_config_section() {
        let _guard = env_lock().lock().unwrap();
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-app.toml"),
            r#"
                [app]
                timezone = "Asia/Kuala_Lumpur"
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let app: AppConfig = config.app().unwrap();

        assert_eq!(app.timezone.to_string(), "Asia/Kuala_Lumpur");
    }

    #[test]
    fn overlays_app_timezone_from_env() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("APP__TIMEZONE", "Asia/Tokyo");

        let config = ConfigRepository::with_env_overlay_only().unwrap();
        let app = config.app().unwrap();

        std::env::remove_var("APP__TIMEZONE");

        assert_eq!(app.timezone.to_string(), "Asia/Tokyo");
    }

    #[test]
    fn rejects_invalid_app_timezone() {
        let _guard = env_lock().lock().unwrap();
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-app.toml"),
            r#"
                [app]
                timezone = "Mars/Olympus"
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let error = config.app().unwrap_err();

        assert!(error.to_string().contains("invalid timezone"));
    }

    #[test]
    fn overlays_env_vars_using_double_underscore_paths() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("SERVER__PORT");
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-base.toml"),
            r#"
                [server]
                port = 3000
            "#,
        )
        .unwrap();
        std::env::set_var("SERVER__PORT", "4123");

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let server = config.server().unwrap();

        std::env::remove_var("SERVER__PORT");
        assert_eq!(server.port, 4123);
    }

    #[test]
    fn parses_phase_two_config_sections() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("REDIS__URL");
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-runtime.toml"),
            r#"
                [database]
                url = "postgres://forge:secret@127.0.0.1:5432/forge"
                schema = "forge_test"
                migration_table = "schema_migrations"
                migrations_path = "database/migrations"
                seeders_path = "database/seeders"
                max_connections = 2

                [redis]
                url = "redis://127.0.0.1/"
                namespace = "forge-tests"

                [websocket]
                port = 4100
                path = "/realtime"

                [jobs]
                queue = "critical"
                max_retries = 9
                lease_ttl_ms = 45000
                requeue_batch_size = 12

                [scheduler]
                tick_interval_ms = 250
                leader_lease_ttl_ms = 7000
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let database: DatabaseConfig = config.database().unwrap();
        let redis: RedisConfig = config.redis().unwrap();
        let websocket: WebSocketConfig = config.websocket().unwrap();
        let jobs: JobsConfig = config.jobs().unwrap();
        let scheduler: SchedulerConfig = config.scheduler().unwrap();

        assert_eq!(database.url, "postgres://forge:secret@127.0.0.1:5432/forge");
        assert_eq!(database.schema, "forge_test");
        assert_eq!(database.migration_table, "schema_migrations");
        assert_eq!(database.migrations_path, "database/migrations");
        assert_eq!(database.seeders_path, "database/seeders");
        assert_eq!(database.max_connections, 2);
        assert!(database.models.timestamps_default);
        assert!(!database.models.soft_deletes_default);
        assert_eq!(redis.url, "redis://127.0.0.1/");
        assert_eq!(redis.namespace, "forge-tests");
        assert_eq!(websocket.path, "/realtime");
        assert_eq!(websocket.port, 4100);
        assert_eq!(jobs.queue, QueueId::new("critical"));
        assert_eq!(jobs.max_retries, 9);
        assert_eq!(jobs.lease_ttl_ms, 45_000);
        assert_eq!(jobs.requeue_batch_size, 12);
        assert_eq!(scheduler.tick_interval_ms, 250);
        assert_eq!(scheduler.leader_lease_ttl_ms, 7_000);
    }

    #[test]
    fn parses_auth_config_section() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("AUTH__DEFAULT_GUARD");
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-auth.toml"),
            r#"
                [auth]
                default_guard = "admin"
                bearer_prefix = "Token"
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let auth: AuthConfig = config.auth().unwrap();

        assert_eq!(auth.default_guard, GuardId::new("admin"));
        assert_eq!(auth.bearer_prefix, "Token");
    }

    #[test]
    fn merges_defaults_before_app_config_and_env_overlay() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("SERVER__PORT");
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-app.toml"),
            r#"
                [server]
                host = "0.0.0.0"
            "#,
        )
        .unwrap();
        std::env::set_var("SERVER__PORT", "4555");

        let config = ConfigRepository::from_dir_with_defaults(
            directory.path(),
            vec![toml::from_str(
                r#"
                    [server]
                    host = "127.0.0.1"
                    port = 3000
                "#,
            )
            .unwrap()],
        )
        .unwrap();
        let server = config.server().unwrap();

        std::env::remove_var("SERVER__PORT");
        assert_eq!(server.host, "0.0.0.0");
        assert_eq!(server.port, 4555);
    }

    #[test]
    fn parses_database_model_defaults() {
        let _guard = env_lock().lock().unwrap();
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-database.toml"),
            r#"
                [database.models]
                timestamps_default = false
                soft_deletes_default = true
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let database: DatabaseConfig = config.database().unwrap();

        assert!(!database.models.timestamps_default);
        assert!(database.models.soft_deletes_default);
    }

    #[test]
    fn parses_logging_config_section() {
        let _guard = env_lock().lock().unwrap();
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-logging.toml"),
            r#"
                [logging]
                level = "debug"
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let logging: LoggingConfig = config.logging().unwrap();

        assert_eq!(logging.level, LogLevel::Debug);
    }

    #[test]
    fn parses_observability_config_section() {
        let _guard = env_lock().lock().unwrap();
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-observability.toml"),
            r#"
                [observability]
                base_path = "/_ops"
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let observability: ObservabilityConfig = config.observability().unwrap();

        assert_eq!(observability.base_path, "/_ops");
    }

    #[test]
    fn parses_logging_config_with_format_and_log_dir() {
        let _guard = env_lock().lock().unwrap();
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("00-logging.toml"),
            r#"
                [logging]
                level = "debug"
                format = "json"
                log_dir = "var/log"
            "#,
        )
        .unwrap();

        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let logging: LoggingConfig = config.logging().unwrap();

        assert_eq!(logging.level, LogLevel::Debug);
        assert_eq!(logging.format, LogFormat::Json);
        assert_eq!(logging.log_dir, "var/log");
    }

    #[test]
    fn logging_config_defaults_to_json_with_logs_dir() {
        let _guard = env_lock().lock().unwrap();
        let directory = tempdir().unwrap();
        let config = ConfigRepository::from_dir(directory.path()).unwrap();
        let logging: LoggingConfig = config.logging().unwrap();

        assert_eq!(logging.level, LogLevel::Info);
        assert_eq!(logging.format, LogFormat::Json);
        assert_eq!(logging.log_dir, "logs");
    }
}
