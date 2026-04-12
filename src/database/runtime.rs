use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{
    DateTime as ChronoDateTime, NaiveDate as ChronoDate, NaiveDateTime as ChronoNaiveDateTime,
    NaiveTime as ChronoTime, Utc as ChronoUtc,
};
use futures_util::stream::{self, BoxStream};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnection, PgPoolOptions, PgRow};
use sqlx::{Column as _, PgPool, Postgres, Row, Transaction, TypeInfo as _};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use uuid::Uuid;

use crate::config::DatabaseConfig;
use crate::foundation::{Error, Result};
use crate::support::{Date, DateTime, LocalDateTime, Time};

use super::ast::{DbType, DbValue, Numeric};
use super::compiler::CompiledSql;

// ---------------------------------------------------------------------------
// SQL query logging
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) struct SqlLogConfig {
    pub log_queries: bool,
    pub slow_threshold: Option<Duration>,
}

impl SqlLogConfig {
    pub fn from_database_config(config: &DatabaseConfig) -> Self {
        Self {
            log_queries: config.log_queries,
            slow_threshold: if config.slow_query_threshold_ms > 0 {
                Some(Duration::from_millis(config.slow_query_threshold_ms))
            } else {
                None
            },
        }
    }

    #[allow(dead_code)]
    pub fn disabled() -> Self {
        Self {
            log_queries: false,
            slow_threshold: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SlowQueryEntry {
    pub sql: String,
    pub duration_ms: u64,
    pub label: Option<String>,
    pub recorded_at: String,
}

static SLOW_QUERY_LOG: OnceLock<std::sync::Mutex<VecDeque<SlowQueryEntry>>> = OnceLock::new();

fn slow_query_log() -> &'static std::sync::Mutex<VecDeque<SlowQueryEntry>> {
    SLOW_QUERY_LOG.get_or_init(|| std::sync::Mutex::new(VecDeque::with_capacity(100)))
}

fn record_slow_query(sql: &str, duration_ms: u64, label: Option<&str>) {
    if let Ok(mut log) = slow_query_log().lock() {
        if log.len() >= 100 {
            log.pop_front();
        }
        log.push_back(SlowQueryEntry {
            sql: sql.to_string(),
            duration_ms,
            label: label.map(|s| s.to_string()),
            recorded_at: ChronoUtc::now().to_rfc3339(),
        });
    }
}

pub fn recent_slow_queries() -> Vec<SlowQueryEntry> {
    slow_query_log()
        .lock()
        .map(|log| log.iter().cloned().collect())
        .unwrap_or_default()
}

pub type DbRecordStream<'a> = BoxStream<'a, Result<DbRecord>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryExecutionOptions {
    pub timeout: Option<Duration>,
    pub label: Option<String>,
    /// When true, forces this query to use the write (primary) pool instead
    /// of the read replica. Useful for reads that must see the most recent writes.
    pub use_write_pool: bool,
}

impl QueryExecutionOptions {
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_write_pool(mut self) -> Self {
        self.use_write_pool = true;
        self
    }
}

#[derive(Clone)]
pub struct DatabaseManager {
    state: Arc<DatabaseState>,
}

enum DatabaseState {
    Disabled,
    Ready(DatabaseRuntime),
}

struct DatabaseRuntime {
    pool: PgPool,
    read_pool: Option<PgPool>,
    adapters: Arc<RwLock<BTreeMap<String, DbType>>>,
    sql_log: SqlLogConfig,
}

impl DatabaseRuntime {
    /// Returns the pool to use for read operations. Falls back to the write
    /// pool when no read replica is configured or when `force_write` is true.
    fn pool_for_reads(&self, force_write: bool) -> &PgPool {
        if force_write {
            &self.pool
        } else {
            self.read_pool.as_ref().unwrap_or(&self.pool)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DbRecord {
    values: BTreeMap<String, DbValue>,
}

impl DbRecord {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: DbValue) {
        self.values.insert(key.into(), value);
    }

    pub fn get(&self, key: &str) -> Option<&DbValue> {
        self.values.get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &DbValue)> {
        self.values.iter()
    }

    /// Extract a text value, returning empty string if missing or wrong type.
    pub fn text(&self, field: &str) -> String {
        match self.get(field) {
            Some(DbValue::Text(s)) => s.clone(),
            _ => String::new(),
        }
    }

    /// Extract a text or UUID value as string.
    pub fn text_or_uuid(&self, field: &str) -> String {
        match self.get(field) {
            Some(DbValue::Text(s)) => s.clone(),
            Some(DbValue::Uuid(u)) => u.to_string(),
            _ => String::new(),
        }
    }

    /// Extract an optional text value.
    pub fn optional_text(&self, field: &str) -> Option<String> {
        match self.get(field) {
            Some(DbValue::Text(s)) => Some(s.clone()),
            _ => None,
        }
    }
}

pub struct DatabaseTransaction {
    inner: Mutex<Option<Transaction<'static, Postgres>>>,
    adapters: Arc<RwLock<BTreeMap<String, DbType>>>,
    sql_log: SqlLogConfig,
}

#[derive(Clone)]
pub(crate) struct DatabaseSession {
    pool: PgPool,
    inner: Arc<Mutex<Option<PoolConnection<Postgres>>>>,
    adapters: Arc<RwLock<BTreeMap<String, DbType>>>,
    sql_log: SqlLogConfig,
}

#[async_trait]
pub trait QueryExecutor: Send + Sync {
    async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>>;

    async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64>;

    fn stream_records<'a>(
        &'a self,
        compiled: CompiledSql,
        options: QueryExecutionOptions,
    ) -> DbRecordStream<'a>
    where
        Self: Sized,
    {
        fallback_stream(self, compiled, options)
    }

    async fn raw_query(&self, sql: &str, bindings: &[DbValue]) -> Result<Vec<DbRecord>> {
        self.raw_query_with(sql, bindings, QueryExecutionOptions::default())
            .await
    }

    async fn raw_execute(&self, sql: &str, bindings: &[DbValue]) -> Result<u64> {
        self.raw_execute_with(sql, bindings, QueryExecutionOptions::default())
            .await
    }

    async fn query_records_with(
        &self,
        compiled: &CompiledSql,
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>> {
        self.raw_query_with(&compiled.sql, &compiled.bindings, options)
            .await
    }

    async fn query_records(&self, compiled: &CompiledSql) -> Result<Vec<DbRecord>> {
        self.query_records_with(compiled, QueryExecutionOptions::default())
            .await
    }

    async fn execute_compiled_with(
        &self,
        compiled: &CompiledSql,
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        self.raw_execute_with(&compiled.sql, &compiled.bindings, options)
            .await
    }

    async fn execute_compiled(&self, compiled: &CompiledSql) -> Result<u64> {
        self.execute_compiled_with(compiled, QueryExecutionOptions::default())
            .await
    }
}

impl DatabaseManager {
    pub fn disabled() -> Self {
        Self {
            state: Arc::new(DatabaseState::Disabled),
        }
    }

    pub async fn from_config(config: &DatabaseConfig) -> Result<Self> {
        if config.url.trim().is_empty() {
            return Ok(Self::disabled());
        }

        if !config.url.starts_with("postgres://") && !config.url.starts_with("postgresql://") {
            return Err(Error::message(
                "Forge database runtime is Postgres-only and requires a postgres:// URL",
            ));
        }

        let pool = PgPoolOptions::new()
            .min_connections(config.min_connections)
            .max_connections(config.max_connections)
            .acquire_timeout(Duration::from_millis(config.acquire_timeout_ms))
            .idle_timeout(Duration::from_secs(config.idle_timeout_seconds))
            .max_lifetime(Duration::from_secs(config.max_lifetime_seconds))
            .connect(&config.url)
            .await
            .map_err(Error::other)?;

        let read_pool = if let Some(ref read_url) = config.read_url {
            if !read_url.trim().is_empty() {
                let rp = PgPoolOptions::new()
                    .min_connections(config.min_connections)
                    .max_connections(config.max_connections)
                    .acquire_timeout(Duration::from_millis(config.acquire_timeout_ms))
                    .idle_timeout(Duration::from_secs(config.idle_timeout_seconds))
                    .max_lifetime(Duration::from_secs(config.max_lifetime_seconds))
                    .connect(read_url)
                    .await
                    .map_err(Error::other)?;
                Some(rp)
            } else {
                None
            }
        } else {
            None
        };

        let sql_log = SqlLogConfig::from_database_config(config);

        Ok(Self {
            state: Arc::new(DatabaseState::Ready(DatabaseRuntime {
                pool,
                read_pool,
                adapters: Arc::new(RwLock::new(BTreeMap::new())),
                sql_log,
            })),
        })
    }

    pub fn is_configured(&self) -> bool {
        matches!(self.state.as_ref(), DatabaseState::Ready(_))
    }

    pub fn pool(&self) -> Result<&PgPool> {
        Ok(&self.runtime()?.pool)
    }

    pub fn register_type_adapter(
        &self,
        postgres_type_name: impl Into<String>,
        db_type: DbType,
    ) -> Result<()> {
        let mut adapters = self
            .runtime()?
            .adapters
            .write()
            .map_err(|_| Error::message("database type adapter registry lock poisoned"))?;
        adapters.insert(normalize_type_name(&postgres_type_name.into()), db_type);
        Ok(())
    }

    pub fn registered_type_adapter(&self, postgres_type_name: &str) -> Result<Option<DbType>> {
        let adapters = self
            .runtime()?
            .adapters
            .read()
            .map_err(|_| Error::message("database type adapter registry lock poisoned"))?;
        Ok(adapters
            .get(&normalize_type_name(postgres_type_name))
            .copied())
    }

    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(self.pool()?)
            .await
            .map_err(Error::other)?;
        Ok(())
    }

    pub async fn begin(&self) -> Result<DatabaseTransaction> {
        let runtime = self.runtime()?;
        let transaction = runtime.pool.begin().await.map_err(Error::other)?;
        Ok(DatabaseTransaction {
            inner: Mutex::new(Some(transaction)),
            adapters: runtime.adapters.clone(),
            sql_log: runtime.sql_log.clone(),
        })
    }

    pub(crate) async fn acquire_session(&self) -> Result<DatabaseSession> {
        let runtime = self.runtime()?;
        let connection = runtime.pool.acquire().await.map_err(Error::other)?;
        Ok(DatabaseSession {
            pool: runtime.pool.clone(),
            inner: Arc::new(Mutex::new(Some(connection))),
            adapters: runtime.adapters.clone(),
            sql_log: runtime.sql_log.clone(),
        })
    }

    pub async fn raw_query(&self, sql: &str, bindings: &[DbValue]) -> Result<Vec<DbRecord>> {
        <Self as QueryExecutor>::raw_query(self, sql, bindings).await
    }

    pub async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>> {
        <Self as QueryExecutor>::raw_query_with(self, sql, bindings, options).await
    }

    pub async fn raw_execute(&self, sql: &str, bindings: &[DbValue]) -> Result<u64> {
        <Self as QueryExecutor>::raw_execute(self, sql, bindings).await
    }

    pub async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        <Self as QueryExecutor>::raw_execute_with(self, sql, bindings, options).await
    }

    pub fn raw_stream<'a>(
        &'a self,
        sql: &'a str,
        bindings: &'a [DbValue],
        options: QueryExecutionOptions,
    ) -> DbRecordStream<'a> {
        let compiled = CompiledSql {
            sql: sql.to_string(),
            bindings: bindings.to_vec(),
        };
        <Self as QueryExecutor>::stream_records(self, compiled, options)
    }

    fn runtime(&self) -> Result<&DatabaseRuntime> {
        match self.state.as_ref() {
            DatabaseState::Disabled => Err(Error::message("database is not configured")),
            DatabaseState::Ready(runtime) => Ok(runtime),
        }
    }
}

#[async_trait]
impl QueryExecutor for DatabaseManager {
    async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>> {
        let runtime = self.runtime()?;
        let pool = runtime.pool_for_reads(options.use_write_pool);
        let mut connection = pool.acquire().await.map_err(Error::other)?;
        query_records_on_connection(
            connection.as_mut(),
            &runtime.adapters,
            sql,
            bindings,
            &options,
            TimeoutMode::Session,
            &runtime.sql_log,
        )
        .await
    }

    async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        let runtime = self.runtime()?;
        let mut connection = runtime.pool.acquire().await.map_err(Error::other)?;
        execute_on_connection(
            connection.as_mut(),
            sql,
            bindings,
            &options,
            TimeoutMode::Session,
            &runtime.sql_log,
        )
        .await
    }

    fn stream_records<'a>(
        &'a self,
        compiled: CompiledSql,
        options: QueryExecutionOptions,
    ) -> DbRecordStream<'a> {
        let runtime = match self.runtime() {
            Ok(runtime) => runtime,
            Err(error) => return single_error_stream(error),
        };

        let pool = runtime.pool_for_reads(options.use_write_pool);
        spawn_native_stream(
            pool.clone(),
            runtime.adapters.clone(),
            compiled,
            options,
            runtime.sql_log.clone(),
        )
    }
}

#[async_trait]
impl QueryExecutor for DatabaseTransaction {
    async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>> {
        let mut guard = self.inner.lock().await;
        let transaction = guard
            .as_mut()
            .ok_or_else(|| Error::message("database transaction has already been completed"))?;
        query_records_on_connection(
            transaction.as_mut(),
            &self.adapters,
            sql,
            bindings,
            &options,
            TimeoutMode::Local,
            &self.sql_log,
        )
        .await
    }

    async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        let mut guard = self.inner.lock().await;
        let transaction = guard
            .as_mut()
            .ok_or_else(|| Error::message("database transaction has already been completed"))?;
        execute_on_connection(
            transaction.as_mut(),
            sql,
            bindings,
            &options,
            TimeoutMode::Local,
            &self.sql_log,
        )
        .await
    }
}

impl DatabaseTransaction {
    pub async fn raw_query(&self, sql: &str, bindings: &[DbValue]) -> Result<Vec<DbRecord>> {
        <Self as QueryExecutor>::raw_query(self, sql, bindings).await
    }

    pub async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>> {
        <Self as QueryExecutor>::raw_query_with(self, sql, bindings, options).await
    }

    pub async fn raw_execute(&self, sql: &str, bindings: &[DbValue]) -> Result<u64> {
        <Self as QueryExecutor>::raw_execute(self, sql, bindings).await
    }

    pub async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        <Self as QueryExecutor>::raw_execute_with(self, sql, bindings, options).await
    }

    pub fn raw_stream<'a>(
        &'a self,
        sql: &'a str,
        bindings: &'a [DbValue],
        options: QueryExecutionOptions,
    ) -> DbRecordStream<'a> {
        let compiled = CompiledSql {
            sql: sql.to_string(),
            bindings: bindings.to_vec(),
        };
        <Self as QueryExecutor>::stream_records(self, compiled, options)
    }

    pub async fn commit(self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let transaction = guard
            .take()
            .ok_or_else(|| Error::message("database transaction has already been completed"))?;
        transaction.commit().await.map_err(Error::other)
    }

    pub async fn rollback(self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let transaction = guard
            .take()
            .ok_or_else(|| Error::message("database transaction has already been completed"))?;
        transaction.rollback().await.map_err(Error::other)
    }
}

#[async_trait]
impl QueryExecutor for DatabaseSession {
    async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>> {
        let mut connection = self.take_connection().await?;
        let result = query_records_on_connection(
            connection.as_mut(),
            &self.adapters,
            sql,
            bindings,
            &options,
            TimeoutMode::Session,
            &self.sql_log,
        )
        .await;
        self.return_connection(connection).await;
        result
    }

    async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        let mut connection = self.take_connection().await?;
        let result = execute_on_connection(
            connection.as_mut(),
            sql,
            bindings,
            &options,
            TimeoutMode::Session,
            &self.sql_log,
        )
        .await;
        self.return_connection(connection).await;
        result
    }

    fn stream_records<'a>(
        &'a self,
        compiled: CompiledSql,
        options: QueryExecutionOptions,
    ) -> DbRecordStream<'a> {
        spawn_session_stream(
            self.pool.clone(),
            self.inner.clone(),
            self.adapters.clone(),
            compiled,
            options,
            self.sql_log.clone(),
        )
    }
}

impl DatabaseSession {
    async fn take_connection(&self) -> Result<PoolConnection<Postgres>> {
        let mut guard = self.inner.lock().await;
        if let Some(connection) = guard.take() {
            return Ok(connection);
        }
        drop(guard);
        self.pool.acquire().await.map_err(Error::other)
    }

    async fn return_connection(&self, connection: PoolConnection<Postgres>) {
        let mut guard = self.inner.lock().await;
        if guard.is_none() {
            *guard = Some(connection);
        }
    }

    pub(crate) async fn begin_transaction(&self) -> Result<()> {
        self.raw_execute("BEGIN", &[]).await?;
        Ok(())
    }

    pub(crate) async fn commit_transaction(&self) -> Result<()> {
        self.raw_execute("COMMIT", &[]).await?;
        Ok(())
    }

    pub(crate) async fn rollback_transaction(&self) -> Result<()> {
        self.raw_execute("ROLLBACK", &[]).await?;
        Ok(())
    }

    pub(crate) async fn acquire_advisory_lock(&self, key: i64) -> Result<()> {
        self.raw_query("SELECT pg_advisory_lock($1)", &[DbValue::Int64(key)])
            .await?;
        Ok(())
    }

    pub(crate) async fn release_advisory_lock(&self, key: i64) -> Result<()> {
        self.raw_query("SELECT pg_advisory_unlock($1)", &[DbValue::Int64(key)])
            .await?;
        Ok(())
    }
}

fn fallback_stream<'a>(
    executor: &'a dyn QueryExecutor,
    compiled: CompiledSql,
    options: QueryExecutionOptions,
) -> DbRecordStream<'a> {
    enum State<'a> {
        Init {
            executor: &'a dyn QueryExecutor,
            compiled: CompiledSql,
            options: QueryExecutionOptions,
        },
        Ready(VecDeque<DbRecord>),
        Done,
    }

    Box::pin(stream::unfold(
        State::Init {
            executor,
            compiled,
            options,
        },
        |state| async move {
            match state {
                State::Init {
                    executor,
                    compiled,
                    options,
                } => match executor.query_records_with(&compiled, options).await {
                    Ok(records) => {
                        let mut queue: VecDeque<_> = records.into();
                        queue
                            .pop_front()
                            .map(|record| (Ok(record), State::Ready(queue)))
                    }
                    Err(error) => Some((Err(error), State::Done)),
                },
                State::Ready(mut queue) => queue
                    .pop_front()
                    .map(|record| (Ok(record), State::Ready(queue))),
                State::Done => None,
            }
        },
    ))
}

fn spawn_native_stream(
    pool: PgPool,
    adapters: Arc<RwLock<BTreeMap<String, DbType>>>,
    compiled: CompiledSql,
    options: QueryExecutionOptions,
    sql_log: SqlLogConfig,
) -> DbRecordStream<'static> {
    let (sender, receiver) = mpsc::channel(16);

    tokio::spawn(async move {
        if sql_log.log_queries {
            tracing::debug!(
                target: "forge.sql",
                sql = %compiled.sql,
                label = ?options.label,
                "stream started"
            );
        }

        let result = async {
            let mut connection = pool.acquire().await.map_err(Error::other)?;
            let adapter_snapshot = snapshot_adapters(&adapters)?;
            configure_statement_timeout(connection.as_mut(), &options, TimeoutMode::Session)
                .await?;

            let stream_result = stream_rows_from_connection(
                connection.as_mut(),
                &adapter_snapshot,
                &compiled,
                &options,
                &sender,
            )
            .await;

            let reset_result =
                reset_statement_timeout(connection.as_mut(), TimeoutMode::Session).await;
            stream_result?;
            reset_result
        }
        .await;

        if let Err(error) = result {
            let _ = sender.send(Err(error)).await;
        }
    });

    receiver_stream(receiver)
}

fn spawn_session_stream(
    pool: PgPool,
    holder: Arc<Mutex<Option<PoolConnection<Postgres>>>>,
    adapters: Arc<RwLock<BTreeMap<String, DbType>>>,
    compiled: CompiledSql,
    options: QueryExecutionOptions,
    sql_log: SqlLogConfig,
) -> DbRecordStream<'static> {
    let (sender, receiver) = mpsc::channel(16);

    tokio::spawn(async move {
        if sql_log.log_queries {
            tracing::debug!(
                target: "forge.sql",
                sql = %compiled.sql,
                label = ?options.label,
                "stream started"
            );
        }

        let result = async {
            let mut connection = {
                let mut guard = holder.lock().await;
                guard.take()
            };
            let mut connection = match connection.take() {
                Some(connection) => connection,
                None => pool.acquire().await.map_err(Error::other)?,
            };

            let adapter_snapshot = snapshot_adapters(&adapters)?;
            configure_statement_timeout(connection.as_mut(), &options, TimeoutMode::Session)
                .await?;

            let stream_result = stream_rows_from_connection(
                connection.as_mut(),
                &adapter_snapshot,
                &compiled,
                &options,
                &sender,
            )
            .await;

            let reset_result =
                reset_statement_timeout(connection.as_mut(), TimeoutMode::Session).await;
            {
                let mut guard = holder.lock().await;
                if guard.is_none() {
                    *guard = Some(connection);
                }
            }

            stream_result?;
            reset_result
        }
        .await;

        if let Err(error) = result {
            let _ = sender.send(Err(error)).await;
        }
    });

    receiver_stream(receiver)
}

async fn query_records_on_connection(
    connection: &mut PgConnection,
    adapters: &Arc<RwLock<BTreeMap<String, DbType>>>,
    sql: &str,
    bindings: &[DbValue],
    options: &QueryExecutionOptions,
    timeout_mode: TimeoutMode,
    sql_log: &SqlLogConfig,
) -> Result<Vec<DbRecord>> {
    log_sql_start(sql_log, sql, bindings, &options.label, "query");
    let start = Instant::now();

    let adapter_snapshot = snapshot_adapters(adapters)?;
    configure_statement_timeout(connection, options, timeout_mode).await?;
    let query = bind_query(sql, bindings)?;
    let rows =
        apply_outer_timeout(query.fetch_all(&mut *connection), options, "query", sql).await?;
    reset_statement_timeout(connection, timeout_mode).await?;
    let result: Result<Vec<DbRecord>> = rows
        .iter()
        .map(|row| decode_row(row, sql, options.label.as_deref(), &adapter_snapshot))
        .collect();

    log_sql_complete(sql_log, sql, start.elapsed(), &options.label, rows.len() as u64);
    result
}

async fn execute_on_connection(
    connection: &mut PgConnection,
    sql: &str,
    bindings: &[DbValue],
    options: &QueryExecutionOptions,
    timeout_mode: TimeoutMode,
    sql_log: &SqlLogConfig,
) -> Result<u64> {
    log_sql_start(sql_log, sql, bindings, &options.label, "execute");
    let start = Instant::now();

    configure_statement_timeout(connection, options, timeout_mode).await?;
    let query = bind_query(sql, bindings)?;
    let result =
        apply_outer_timeout(query.execute(&mut *connection), options, "execution", sql).await?;
    reset_statement_timeout(connection, timeout_mode).await?;
    let rows_affected = result.rows_affected();

    log_sql_complete(sql_log, sql, start.elapsed(), &options.label, rows_affected);
    Ok(rows_affected)
}

fn log_sql_start(
    sql_log: &SqlLogConfig,
    sql: &str,
    bindings: &[DbValue],
    label: &Option<String>,
    kind: &str,
) {
    if sql_log.log_queries {
        tracing::debug!(
            target: "forge.sql",
            sql = %sql,
            bindings = ?bindings,
            label = ?label,
            kind,
        );
    }
}

fn log_sql_complete(
    sql_log: &SqlLogConfig,
    sql: &str,
    elapsed: Duration,
    label: &Option<String>,
    rows: u64,
) {
    let elapsed_ms = elapsed.as_millis() as u64;

    if sql_log.log_queries {
        tracing::debug!(
            target: "forge.sql",
            duration_ms = elapsed_ms,
            rows,
            label = ?label,
            "completed"
        );
    }

    if let Some(threshold) = sql_log.slow_threshold {
        if elapsed > threshold {
            tracing::warn!(
                target: "forge.sql",
                sql = %sql,
                duration_ms = elapsed_ms,
                label = ?label,
                "slow query detected"
            );
            record_slow_query(sql, elapsed_ms, label.as_deref());
        }
    }
}

async fn stream_rows_from_connection(
    connection: &mut sqlx::postgres::PgConnection,
    adapters: &BTreeMap<String, DbType>,
    compiled: &CompiledSql,
    options: &QueryExecutionOptions,
    sender: &mpsc::Sender<Result<DbRecord>>,
) -> Result<()> {
    let query = bind_query(&compiled.sql, &compiled.bindings)?;
    let mut rows = query.fetch(connection);

    loop {
        let next_row = if let Some(timeout_duration) = options.timeout {
            timeout(safety_timeout(timeout_duration), rows.next())
                .await
                .map_err(|_| outer_timeout_error(options, "stream", &compiled.sql))?
        } else {
            rows.next().await
        };

        match next_row {
            Some(Ok(row)) => {
                let record = decode_row(&row, &compiled.sql, options.label.as_deref(), adapters)?;
                if sender.send(Ok(record)).await.is_err() {
                    break;
                }
            }
            Some(Err(error)) => {
                let mapped = map_sqlx_operation_error(error, options, "stream", &compiled.sql);
                let _ = sender.send(Err(mapped)).await;
                break;
            }
            None => break,
        }
    }

    Ok(())
}

async fn configure_statement_timeout(
    connection: &mut PgConnection,
    options: &QueryExecutionOptions,
    mode: TimeoutMode,
) -> Result<()> {
    let timeout_value = options
        .timeout
        .map(timeout_millis)
        .unwrap_or_else(|| "0".to_string());
    let local = matches!(mode, TimeoutMode::Local);

    sqlx::query("SELECT set_config('statement_timeout', $1, $2)")
        .bind(timeout_value)
        .bind(local)
        .execute(connection)
        .await
        .map_err(Error::other)?;
    Ok(())
}

async fn reset_statement_timeout(connection: &mut PgConnection, mode: TimeoutMode) -> Result<()> {
    sqlx::query("SELECT set_config('statement_timeout', '0', $1)")
        .bind(matches!(mode, TimeoutMode::Local))
        .execute(connection)
        .await
        .map_err(Error::other)?;
    Ok(())
}

fn snapshot_adapters(
    adapters: &Arc<RwLock<BTreeMap<String, DbType>>>,
) -> Result<BTreeMap<String, DbType>> {
    adapters
        .read()
        .map(|snapshot| snapshot.clone())
        .map_err(|_| Error::message("database type adapter registry lock poisoned"))
}

async fn apply_outer_timeout<F, T>(
    future: F,
    options: &QueryExecutionOptions,
    action: &str,
    sql: &str,
) -> Result<T>
where
    F: std::future::Future<Output = std::result::Result<T, sqlx::Error>>,
{
    if let Some(timeout_duration) = options.timeout {
        timeout(safety_timeout(timeout_duration), future)
            .await
            .map_err(|_| outer_timeout_error(options, action, sql))?
            .map_err(|error| map_sqlx_operation_error(error, options, action, sql))
    } else {
        future
            .await
            .map_err(|error| map_sqlx_operation_error(error, options, action, sql))
    }
}

fn receiver_stream(receiver: mpsc::Receiver<Result<DbRecord>>) -> DbRecordStream<'static> {
    Box::pin(stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    }))
}

fn single_error_stream<'a>(error: Error) -> DbRecordStream<'a> {
    Box::pin(stream::once(async move { Err(error) }))
}

fn outer_timeout_error(options: &QueryExecutionOptions, action: &str, sql: &str) -> Error {
    let duration = options
        .timeout
        .map(|timeout| timeout.as_millis().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Error::message(format!(
        "database {action} timed out after {duration}ms{} while running `{sql}`",
        label_suffix(options)
    ))
}

fn map_sqlx_operation_error(
    error: sqlx::Error,
    options: &QueryExecutionOptions,
    action: &str,
    sql: &str,
) -> Error {
    if is_statement_timeout(&error) {
        return Error::message(format!(
            "database {action} timed out after {}ms{} while running `{sql}`: {}",
            options
                .timeout
                .map(|timeout| timeout.as_millis())
                .unwrap_or_default(),
            label_suffix(options),
            error
        ));
    }

    Error::message(format!(
        "database {action} failed{} while running `{sql}`: {error}",
        label_suffix(options)
    ))
}

fn is_statement_timeout(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(database_error) => {
            database_error.code().as_deref() == Some("57014")
                && database_error
                    .message()
                    .to_ascii_lowercase()
                    .contains("statement timeout")
        }
        _ => false,
    }
}

fn label_suffix(options: &QueryExecutionOptions) -> String {
    options
        .label
        .as_ref()
        .map(|label| format!(" for `{label}`"))
        .unwrap_or_default()
}

fn safety_timeout(timeout_duration: Duration) -> Duration {
    timeout_duration.saturating_add(Duration::from_millis(50))
}

fn timeout_millis(timeout_duration: Duration) -> String {
    timeout_duration.as_millis().to_string()
}

#[derive(Clone, Copy)]
enum TimeoutMode {
    Session,
    Local,
}

fn normalize_type_name(type_name: &str) -> String {
    type_name.trim_matches('"').to_ascii_lowercase()
}

fn bind_query<'q>(
    sql: &'q str,
    bindings: &'q [DbValue],
) -> Result<sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments>> {
    let mut query = sqlx::query(sql);
    for binding in bindings {
        query = match binding {
            DbValue::Null(db_type) => bind_null(query, *db_type),
            DbValue::Int16(value) => query.bind(*value),
            DbValue::Int32(value) => query.bind(*value),
            DbValue::Int64(value) => query.bind(*value),
            DbValue::Bool(value) => query.bind(*value),
            DbValue::Float32(value) => query.bind(*value),
            DbValue::Float64(value) => query.bind(*value),
            DbValue::Numeric(value) => query.bind(value.to_string()),
            DbValue::Text(value) => query.bind(value.clone()),
            DbValue::Json(value) => query.bind(sqlx::types::Json(value.clone())),
            DbValue::Uuid(value) => query.bind(*value),
            DbValue::TimestampTz(value) => query.bind(value.as_chrono()),
            DbValue::Timestamp(value) => query.bind(value.as_chrono()),
            DbValue::Date(value) => query.bind(value.as_chrono()),
            DbValue::Time(value) => query.bind(value.as_chrono()),
            DbValue::Bytea(value) => query.bind(value.clone()),
            DbValue::Int16Array(value) => query.bind(value.clone()),
            DbValue::Int32Array(value) => query.bind(value.clone()),
            DbValue::Int64Array(value) => query.bind(value.clone()),
            DbValue::BoolArray(value) => query.bind(value.clone()),
            DbValue::Float32Array(value) => query.bind(value.clone()),
            DbValue::Float64Array(value) => query.bind(value.clone()),
            DbValue::NumericArray(value) => {
                query.bind(value.iter().map(ToString::to_string).collect::<Vec<_>>())
            }
            DbValue::TextArray(value) => query.bind(value.clone()),
            DbValue::JsonArray(value) => query.bind(
                value
                    .iter()
                    .cloned()
                    .map(sqlx::types::Json)
                    .collect::<Vec<_>>(),
            ),
            DbValue::UuidArray(value) => query.bind(value.clone()),
            DbValue::TimestampTzArray(value) => {
                query.bind(value.iter().map(DateTime::as_chrono).collect::<Vec<_>>())
            }
            DbValue::TimestampArray(value) => query.bind(
                value
                    .iter()
                    .map(LocalDateTime::as_chrono)
                    .collect::<Vec<_>>(),
            ),
            DbValue::DateArray(value) => {
                query.bind(value.iter().map(Date::as_chrono).collect::<Vec<_>>())
            }
            DbValue::TimeArray(value) => {
                query.bind(value.iter().map(Time::as_chrono).collect::<Vec<_>>())
            }
            DbValue::ByteaArray(value) => query.bind(value.clone()),
        };
    }
    Ok(query)
}

fn bind_null<'q>(
    query: sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments>,
    db_type: DbType,
) -> sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments> {
    match db_type {
        DbType::Int16 => query.bind(Option::<i16>::None),
        DbType::Int32 => query.bind(Option::<i32>::None),
        DbType::Int64 => query.bind(Option::<i64>::None),
        DbType::Bool => query.bind(Option::<bool>::None),
        DbType::Float32 => query.bind(Option::<f32>::None),
        DbType::Float64 => query.bind(Option::<f64>::None),
        DbType::Numeric => query.bind(Option::<String>::None),
        DbType::Text => query.bind(Option::<String>::None),
        DbType::Json => query.bind(Option::<sqlx::types::Json<serde_json::Value>>::None),
        DbType::Uuid => query.bind(Option::<Uuid>::None),
        DbType::TimestampTz => query.bind(Option::<ChronoDateTime<ChronoUtc>>::None),
        DbType::Timestamp => query.bind(Option::<ChronoNaiveDateTime>::None),
        DbType::Date => query.bind(Option::<ChronoDate>::None),
        DbType::Time => query.bind(Option::<ChronoTime>::None),
        DbType::Bytea => query.bind(Option::<Vec<u8>>::None),
        DbType::Int16Array => query.bind(Option::<Vec<i16>>::None),
        DbType::Int32Array => query.bind(Option::<Vec<i32>>::None),
        DbType::Int64Array => query.bind(Option::<Vec<i64>>::None),
        DbType::BoolArray => query.bind(Option::<Vec<bool>>::None),
        DbType::Float32Array => query.bind(Option::<Vec<f32>>::None),
        DbType::Float64Array => query.bind(Option::<Vec<f64>>::None),
        DbType::NumericArray => query.bind(Option::<Vec<String>>::None),
        DbType::TextArray => query.bind(Option::<Vec<String>>::None),
        DbType::JsonArray => query.bind(Option::<Vec<sqlx::types::Json<serde_json::Value>>>::None),
        DbType::UuidArray => query.bind(Option::<Vec<Uuid>>::None),
        DbType::TimestampTzArray => query.bind(Option::<Vec<ChronoDateTime<ChronoUtc>>>::None),
        DbType::TimestampArray => query.bind(Option::<Vec<ChronoNaiveDateTime>>::None),
        DbType::DateArray => query.bind(Option::<Vec<ChronoDate>>::None),
        DbType::TimeArray => query.bind(Option::<Vec<ChronoTime>>::None),
        DbType::ByteaArray => query.bind(Option::<Vec<Vec<u8>>>::None),
    }
}

fn decode_row(
    row: &PgRow,
    sql: &str,
    label: Option<&str>,
    adapters: &BTreeMap<String, DbType>,
) -> Result<DbRecord> {
    let mut record = DbRecord::new();

    for column in row.columns() {
        let name = column.name();
        let value = decode_column(row, name, column.type_info().name(), sql, label, adapters)?;
        record.insert(name.to_string(), value);
    }

    Ok(record)
}

fn decode_column(
    row: &PgRow,
    name: &str,
    type_name: &str,
    sql: &str,
    label: Option<&str>,
    adapters: &BTreeMap<String, DbType>,
) -> Result<DbValue> {
    let normalized = normalize_type_name(type_name);
    let mapped = match normalized.as_str() {
        "int2" => Some(DbType::Int16),
        "int4" => Some(DbType::Int32),
        "int8" => Some(DbType::Int64),
        "bool" => Some(DbType::Bool),
        "float4" => Some(DbType::Float32),
        "float8" => Some(DbType::Float64),
        "numeric" => Some(DbType::Numeric),
        "text" | "varchar" | "bpchar" | "name" => Some(DbType::Text),
        "json" | "jsonb" => Some(DbType::Json),
        "uuid" => Some(DbType::Uuid),
        "timestamptz" => Some(DbType::TimestampTz),
        "timestamp" => Some(DbType::Timestamp),
        "date" => Some(DbType::Date),
        "time" | "timetz" => Some(DbType::Time),
        "bytea" => Some(DbType::Bytea),
        "_int2" => Some(DbType::Int16Array),
        "_int4" => Some(DbType::Int32Array),
        "_int8" => Some(DbType::Int64Array),
        "_bool" => Some(DbType::BoolArray),
        "_float4" => Some(DbType::Float32Array),
        "_float8" => Some(DbType::Float64Array),
        "_numeric" => Some(DbType::NumericArray),
        "_text" | "_varchar" | "_bpchar" | "_name" => Some(DbType::TextArray),
        "_json" | "_jsonb" => Some(DbType::JsonArray),
        "_uuid" => Some(DbType::UuidArray),
        "_timestamptz" => Some(DbType::TimestampTzArray),
        "_timestamp" => Some(DbType::TimestampArray),
        "_date" => Some(DbType::DateArray),
        "_time" | "_timetz" => Some(DbType::TimeArray),
        "_bytea" => Some(DbType::ByteaArray),
        _ => adapters.get(&normalized).copied(),
    }
    .ok_or_else(|| unsupported_type_error(name, type_name, sql, label))?;

    decode_column_as(row, name, mapped).map_err(|error| {
        Error::message(format!(
            "failed to decode column `{name}` with postgres type `{type_name}`{}: {error}",
            format_query_context(sql, label)
        ))
    })
}

fn decode_column_as(row: &PgRow, name: &str, db_type: DbType) -> Result<DbValue> {
    match db_type {
        DbType::Int16 => row
            .try_get::<Option<i16>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Int16)
                    .unwrap_or(DbValue::Null(DbType::Int16))
            })
            .map_err(Error::other),
        DbType::Int32 => row
            .try_get::<Option<i32>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Int32)
                    .unwrap_or(DbValue::Null(DbType::Int32))
            })
            .map_err(Error::other),
        DbType::Int64 => row
            .try_get::<Option<i64>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Int64)
                    .unwrap_or(DbValue::Null(DbType::Int64))
            })
            .map_err(Error::other),
        DbType::Bool => row
            .try_get::<Option<bool>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Bool)
                    .unwrap_or(DbValue::Null(DbType::Bool))
            })
            .map_err(Error::other),
        DbType::Float32 => row
            .try_get::<Option<f32>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Float32)
                    .unwrap_or(DbValue::Null(DbType::Float32))
            })
            .map_err(Error::other),
        DbType::Float64 => row
            .try_get::<Option<f64>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Float64)
                    .unwrap_or(DbValue::Null(DbType::Float64))
            })
            .map_err(Error::other),
        DbType::Numeric => row
            .try_get::<Option<String>, _>(name)
            .map(|value| match value {
                Some(value) => Numeric::new(value).map(DbValue::Numeric),
                None => Ok(DbValue::Null(DbType::Numeric)),
            })
            .map_err(Error::other)?
            .map_err(Error::other),
        DbType::Text => row
            .try_get::<Option<String>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Text)
                    .unwrap_or(DbValue::Null(DbType::Text))
            })
            .map_err(Error::other),
        DbType::Json => row
            .try_get::<Option<sqlx::types::Json<serde_json::Value>>, _>(name)
            .map(|value| {
                value
                    .map(|value| DbValue::Json(value.0))
                    .unwrap_or(DbValue::Null(DbType::Json))
            })
            .map_err(Error::other),
        DbType::Uuid => row
            .try_get::<Option<Uuid>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Uuid)
                    .unwrap_or(DbValue::Null(DbType::Uuid))
            })
            .map_err(Error::other),
        DbType::TimestampTz => row
            .try_get::<Option<ChronoDateTime<ChronoUtc>>, _>(name)
            .map(|value| {
                value
                    .map(DateTime::from_chrono)
                    .map(DbValue::TimestampTz)
                    .unwrap_or(DbValue::Null(DbType::TimestampTz))
            })
            .map_err(Error::other),
        DbType::Timestamp => row
            .try_get::<Option<ChronoNaiveDateTime>, _>(name)
            .map(|value| {
                value
                    .map(LocalDateTime::from_chrono)
                    .map(DbValue::Timestamp)
                    .unwrap_or(DbValue::Null(DbType::Timestamp))
            })
            .map_err(Error::other),
        DbType::Date => row
            .try_get::<Option<ChronoDate>, _>(name)
            .map(|value| {
                value
                    .map(Date::from_chrono)
                    .map(DbValue::Date)
                    .unwrap_or(DbValue::Null(DbType::Date))
            })
            .map_err(Error::other),
        DbType::Time => row
            .try_get::<Option<ChronoTime>, _>(name)
            .map(|value| {
                value
                    .map(Time::from_chrono)
                    .map(DbValue::Time)
                    .unwrap_or(DbValue::Null(DbType::Time))
            })
            .map_err(Error::other),
        DbType::Bytea => row
            .try_get::<Option<Vec<u8>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Bytea)
                    .unwrap_or(DbValue::Null(DbType::Bytea))
            })
            .map_err(Error::other),
        DbType::Int16Array => row
            .try_get::<Option<Vec<i16>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Int16Array)
                    .unwrap_or(DbValue::Null(DbType::Int16Array))
            })
            .map_err(Error::other),
        DbType::Int32Array => row
            .try_get::<Option<Vec<i32>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Int32Array)
                    .unwrap_or(DbValue::Null(DbType::Int32Array))
            })
            .map_err(Error::other),
        DbType::Int64Array => row
            .try_get::<Option<Vec<i64>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Int64Array)
                    .unwrap_or(DbValue::Null(DbType::Int64Array))
            })
            .map_err(Error::other),
        DbType::BoolArray => row
            .try_get::<Option<Vec<bool>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::BoolArray)
                    .unwrap_or(DbValue::Null(DbType::BoolArray))
            })
            .map_err(Error::other),
        DbType::Float32Array => row
            .try_get::<Option<Vec<f32>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Float32Array)
                    .unwrap_or(DbValue::Null(DbType::Float32Array))
            })
            .map_err(Error::other),
        DbType::Float64Array => row
            .try_get::<Option<Vec<f64>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::Float64Array)
                    .unwrap_or(DbValue::Null(DbType::Float64Array))
            })
            .map_err(Error::other),
        DbType::NumericArray => row
            .try_get::<Option<Vec<String>>, _>(name)
            .map(|value| match value {
                Some(values) => values
                    .into_iter()
                    .map(Numeric::new)
                    .collect::<Result<Vec<_>>>()
                    .map(DbValue::NumericArray),
                None => Ok(DbValue::Null(DbType::NumericArray)),
            })
            .map_err(Error::other)?
            .map_err(Error::other),
        DbType::TextArray => row
            .try_get::<Option<Vec<String>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::TextArray)
                    .unwrap_or(DbValue::Null(DbType::TextArray))
            })
            .map_err(Error::other),
        DbType::JsonArray => row
            .try_get::<Option<Vec<sqlx::types::Json<serde_json::Value>>>, _>(name)
            .map(|value| match value {
                Some(values) => {
                    DbValue::JsonArray(values.into_iter().map(|value| value.0).collect())
                }
                None => DbValue::Null(DbType::JsonArray),
            })
            .map_err(Error::other),
        DbType::UuidArray => row
            .try_get::<Option<Vec<Uuid>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::UuidArray)
                    .unwrap_or(DbValue::Null(DbType::UuidArray))
            })
            .map_err(Error::other),
        DbType::TimestampTzArray => row
            .try_get::<Option<Vec<ChronoDateTime<ChronoUtc>>>, _>(name)
            .map(|value| {
                value
                    .map(|values| {
                        DbValue::TimestampTzArray(
                            values.into_iter().map(DateTime::from_chrono).collect(),
                        )
                    })
                    .unwrap_or(DbValue::Null(DbType::TimestampTzArray))
            })
            .map_err(Error::other),
        DbType::TimestampArray => row
            .try_get::<Option<Vec<ChronoNaiveDateTime>>, _>(name)
            .map(|value| {
                value
                    .map(|values| {
                        DbValue::TimestampArray(
                            values.into_iter().map(LocalDateTime::from_chrono).collect(),
                        )
                    })
                    .unwrap_or(DbValue::Null(DbType::TimestampArray))
            })
            .map_err(Error::other),
        DbType::DateArray => row
            .try_get::<Option<Vec<ChronoDate>>, _>(name)
            .map(|value| {
                value
                    .map(|values| {
                        DbValue::DateArray(values.into_iter().map(Date::from_chrono).collect())
                    })
                    .unwrap_or(DbValue::Null(DbType::DateArray))
            })
            .map_err(Error::other),
        DbType::TimeArray => row
            .try_get::<Option<Vec<ChronoTime>>, _>(name)
            .map(|value| {
                value
                    .map(|values| {
                        DbValue::TimeArray(values.into_iter().map(Time::from_chrono).collect())
                    })
                    .unwrap_or(DbValue::Null(DbType::TimeArray))
            })
            .map_err(Error::other),
        DbType::ByteaArray => row
            .try_get::<Option<Vec<Vec<u8>>>, _>(name)
            .map(|value| {
                value
                    .map(DbValue::ByteaArray)
                    .unwrap_or(DbValue::Null(DbType::ByteaArray))
            })
            .map_err(Error::other),
    }
}

fn unsupported_type_error(name: &str, type_name: &str, sql: &str, label: Option<&str>) -> Error {
    Error::message(format!(
        "unsupported postgres type `{type_name}` for column `{name}`{}; register a database type adapter or add first-class support",
        format_query_context(sql, label)
    ))
}

fn format_query_context(sql: &str, label: Option<&str>) -> String {
    let mut suffix = String::new();
    if let Some(label) = label {
        suffix.push_str(&format!(" in `{label}`"));
    }
    suffix.push_str(&format!(" while running `{sql}`"));
    suffix
}
