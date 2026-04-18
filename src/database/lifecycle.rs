use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Local;
use clap::{Arg, ArgAction, Command};
use forge_build::{discover_migration_sources, discover_seeder_sources};

use crate::cli::{CommandInvocation, CommandRegistrar};
use crate::config::DatabaseConfig;
use crate::foundation::{AppContext, Error, Result};
use crate::support::{CommandId, MigrationId, SeederId};

use super::runtime::{DatabaseSession, QueryExecutionOptions, QueryExecutor};
use super::{DatabaseManager, DbRecord, DbValue};

const DB_MIGRATE_COMMAND: CommandId = CommandId::new("db:migrate");
const DB_MIGRATE_STATUS_COMMAND: CommandId = CommandId::new("db:migrate:status");
const DB_ROLLBACK_COMMAND: CommandId = CommandId::new("db:rollback");
const DB_SEED_COMMAND: CommandId = CommandId::new("db:seed");
const MAKE_MIGRATION_COMMAND: CommandId = CommandId::new("make:migration");
const MAKE_SEEDER_COMMAND: CommandId = CommandId::new("make:seeder");
const MAKE_MODEL_COMMAND: CommandId = CommandId::new("make:model");
const MAKE_JOB_COMMAND: CommandId = CommandId::new("make:job");
const MAKE_COMMAND_COMMAND: CommandId = CommandId::new("make:command");

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppliedMigration {
    pub id: MigrationId,
    pub batch: i64,
    pub applied_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MigrationStatus {
    pub id: MigrationId,
    pub applied: Option<AppliedMigration>,
}

#[derive(Clone, Debug)]
pub(crate) struct GeneratedDatabasePaths {
    migration_dirs: Vec<PathBuf>,
    seeder_dirs: Vec<PathBuf>,
}

impl GeneratedDatabasePaths {
    pub(crate) fn new(migration_dirs: Vec<PathBuf>, seeder_dirs: Vec<PathBuf>) -> Self {
        Self {
            migration_dirs,
            seeder_dirs,
        }
    }

    pub(crate) fn migration_dirs(&self) -> &[PathBuf] {
        &self.migration_dirs
    }

    pub(crate) fn seeder_dirs(&self) -> &[PathBuf] {
        &self.seeder_dirs
    }

    pub(crate) fn primary_migration_dir(&self) -> Option<&Path> {
        self.migration_dirs.first().map(PathBuf::as_path)
    }

    pub(crate) fn primary_seeder_dir(&self) -> Option<&Path> {
        self.seeder_dirs.first().map(PathBuf::as_path)
    }
}

pub struct MigrationContext<'a> {
    app: &'a AppContext,
    database: &'a DatabaseManager,
    executor: &'a dyn QueryExecutor,
}

impl<'a> MigrationContext<'a> {
    fn new(
        app: &'a AppContext,
        database: &'a DatabaseManager,
        executor: &'a dyn QueryExecutor,
    ) -> Self {
        Self {
            app,
            database,
            executor,
        }
    }

    pub fn app(&self) -> &AppContext {
        self.app
    }

    pub fn database(&self) -> &DatabaseManager {
        self.database
    }

    pub fn executor(&self) -> &dyn QueryExecutor {
        self.executor
    }

    pub async fn raw_query(&self, sql: &str, bindings: &[DbValue]) -> Result<Vec<DbRecord>> {
        self.executor.raw_query(sql, bindings).await
    }

    pub async fn raw_execute(&self, sql: &str, bindings: &[DbValue]) -> Result<u64> {
        self.executor.raw_execute(sql, bindings).await
    }
}

#[async_trait]
impl QueryExecutor for MigrationContext<'_> {
    async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>> {
        self.executor.raw_query_with(sql, bindings, options).await
    }

    async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        self.executor.raw_execute_with(sql, bindings, options).await
    }
}

pub struct SeederContext<'a> {
    app: &'a AppContext,
    database: &'a DatabaseManager,
    executor: &'a dyn QueryExecutor,
}

impl<'a> SeederContext<'a> {
    fn new(
        app: &'a AppContext,
        database: &'a DatabaseManager,
        executor: &'a dyn QueryExecutor,
    ) -> Self {
        Self {
            app,
            database,
            executor,
        }
    }

    pub fn app(&self) -> &AppContext {
        self.app
    }

    pub fn database(&self) -> &DatabaseManager {
        self.database
    }

    pub fn executor(&self) -> &dyn QueryExecutor {
        self.executor
    }

    pub async fn raw_query(&self, sql: &str, bindings: &[DbValue]) -> Result<Vec<DbRecord>> {
        self.executor.raw_query(sql, bindings).await
    }

    pub async fn raw_execute(&self, sql: &str, bindings: &[DbValue]) -> Result<u64> {
        self.executor.raw_execute(sql, bindings).await
    }
}

#[async_trait]
impl QueryExecutor for SeederContext<'_> {
    async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<DbRecord>> {
        self.executor.raw_query_with(sql, bindings, options).await
    }

    async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        self.executor.raw_execute_with(sql, bindings, options).await
    }
}

#[async_trait]
pub trait MigrationFile: Send + Sync + 'static {
    fn run_in_transaction() -> bool {
        true
    }

    async fn up(ctx: &MigrationContext<'_>) -> Result<()>;

    async fn down(ctx: &MigrationContext<'_>) -> Result<()>;
}

#[async_trait]
pub trait SeederFile: Send + Sync + 'static {
    fn run_in_transaction() -> bool {
        true
    }

    async fn run(ctx: &SeederContext<'_>) -> Result<()>;
}

#[async_trait]
trait DynMigration: Send + Sync {
    fn id(&self) -> MigrationId;

    fn run_in_transaction(&self) -> bool;

    async fn up(
        &self,
        app: &AppContext,
        database: &DatabaseManager,
        executor: &dyn QueryExecutor,
    ) -> Result<()>;

    async fn down(
        &self,
        app: &AppContext,
        database: &DatabaseManager,
        executor: &dyn QueryExecutor,
    ) -> Result<()>;
}

#[async_trait]
trait DynSeeder: Send + Sync {
    fn id(&self) -> SeederId;

    fn run_in_transaction(&self) -> bool;

    async fn run(
        &self,
        app: &AppContext,
        database: &DatabaseManager,
        executor: &dyn QueryExecutor,
    ) -> Result<()>;
}

struct MigrationFileAdapter<M> {
    id: MigrationId,
    marker: std::marker::PhantomData<M>,
}

#[async_trait]
impl<M> DynMigration for MigrationFileAdapter<M>
where
    M: MigrationFile,
{
    fn id(&self) -> MigrationId {
        self.id.clone()
    }

    fn run_in_transaction(&self) -> bool {
        M::run_in_transaction()
    }

    async fn up(
        &self,
        app: &AppContext,
        database: &DatabaseManager,
        executor: &dyn QueryExecutor,
    ) -> Result<()> {
        let context = MigrationContext::new(app, database, executor);
        M::up(&context).await
    }

    async fn down(
        &self,
        app: &AppContext,
        database: &DatabaseManager,
        executor: &dyn QueryExecutor,
    ) -> Result<()> {
        let context = MigrationContext::new(app, database, executor);
        M::down(&context).await
    }
}

struct SeederFileAdapter<S> {
    id: SeederId,
    marker: std::marker::PhantomData<S>,
}

#[async_trait]
impl<S> DynSeeder for SeederFileAdapter<S>
where
    S: SeederFile,
{
    fn id(&self) -> SeederId {
        self.id.clone()
    }

    fn run_in_transaction(&self) -> bool {
        S::run_in_transaction()
    }

    async fn run(
        &self,
        app: &AppContext,
        database: &DatabaseManager,
        executor: &dyn QueryExecutor,
    ) -> Result<()> {
        let context = SeederContext::new(app, database, executor);
        S::run(&context).await
    }
}

pub(crate) type MigrationRegistryHandle = Arc<Mutex<MigrationRegistryBuilder>>;
pub(crate) type SeederRegistryHandle = Arc<Mutex<SeederRegistryBuilder>>;

#[derive(Default)]
pub(crate) struct MigrationRegistryBuilder {
    migrations: BTreeMap<MigrationId, Arc<dyn DynMigration>>,
}

impl MigrationRegistryBuilder {
    pub(crate) fn shared() -> MigrationRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn register_file<M>(&mut self, id: MigrationId) -> Result<()>
    where
        M: MigrationFile,
    {
        if self.migrations.contains_key(&id) {
            return Err(Error::message(format!(
                "migration `{id}` already registered"
            )));
        }

        self.migrations.insert(
            id.clone(),
            Arc::new(MigrationFileAdapter::<M> {
                id,
                marker: std::marker::PhantomData,
            }),
        );
        Ok(())
    }

    pub(crate) fn freeze_shared(handle: MigrationRegistryHandle) -> Result<MigrationRegistry> {
        let builder = handle
            .lock()
            .map_err(|_| Error::message("migration registry lock poisoned"))?;
        Ok(MigrationRegistry {
            migrations: builder.migrations.values().cloned().collect(),
        })
    }
}

#[derive(Default)]
pub(crate) struct SeederRegistryBuilder {
    seeders: Vec<Arc<dyn DynSeeder>>,
    ids: HashSet<SeederId>,
}

impl SeederRegistryBuilder {
    pub(crate) fn shared() -> SeederRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn register_file<S>(&mut self, id: SeederId) -> Result<()>
    where
        S: SeederFile,
    {
        if !self.ids.insert(id.clone()) {
            return Err(Error::message(format!("seeder `{id}` already registered")));
        }

        self.seeders.push(Arc::new(SeederFileAdapter::<S> {
            id,
            marker: std::marker::PhantomData,
        }));
        Ok(())
    }

    pub(crate) fn freeze_shared(handle: SeederRegistryHandle) -> Result<SeederRegistry> {
        let mut builder = handle
            .lock()
            .map_err(|_| Error::message("seeder registry lock poisoned"))?;
        Ok(SeederRegistry {
            seeders: std::mem::take(&mut builder.seeders),
        })
    }
}

pub(crate) struct MigrationRegistry {
    migrations: Vec<Arc<dyn DynMigration>>,
}

impl MigrationRegistry {
    pub(crate) fn ids(&self) -> Vec<MigrationId> {
        self.migrations
            .iter()
            .map(|migration| migration.id())
            .collect()
    }

    pub(crate) fn contains(&self, id: &MigrationId) -> bool {
        self.migrations
            .iter()
            .any(|migration| migration.id() == *id)
    }

    fn migration(&self, id: &MigrationId) -> Option<&Arc<dyn DynMigration>> {
        self.migrations
            .iter()
            .find(|migration| migration.id() == *id)
    }

    fn entries(&self) -> &[Arc<dyn DynMigration>] {
        &self.migrations
    }
}

pub(crate) struct SeederRegistry {
    seeders: Vec<Arc<dyn DynSeeder>>,
}

impl SeederRegistry {
    pub(crate) fn ids(&self) -> Vec<SeederId> {
        self.seeders.iter().map(|seeder| seeder.id()).collect()
    }

    pub(crate) fn contains(&self, id: &SeederId) -> bool {
        self.seeders.iter().any(|seeder| seeder.id() == *id)
    }

    fn entries(&self) -> &[Arc<dyn DynSeeder>] {
        &self.seeders
    }
}

pub(crate) fn builtin_cli_registrar() -> CommandRegistrar {
    Arc::new(|registry| {
        registry.command(
            DB_MIGRATE_COMMAND,
            Command::new(DB_MIGRATE_COMMAND.as_str().to_string())
                .about("Apply pending Forge database migrations"),
            |invocation| async move { db_migrate_command(invocation).await },
        )?;
        registry.command(
            DB_MIGRATE_STATUS_COMMAND,
            Command::new(DB_MIGRATE_STATUS_COMMAND.as_str().to_string())
                .about("Show the current Forge database migration status"),
            |invocation| async move { db_migrate_status_command(invocation).await },
        )?;
        registry.command(
            DB_ROLLBACK_COMMAND,
            Command::new(DB_ROLLBACK_COMMAND.as_str().to_string())
                .about("Rollback the latest Forge migration batch"),
            |invocation| async move { db_rollback_command(invocation).await },
        )?;
        registry.command(
            DB_SEED_COMMAND,
            Command::new(DB_SEED_COMMAND.as_str().to_string())
                .about("Run registered Forge database seeders")
                .arg(
                    Arg::new("id")
                        .long("id")
                        .value_name("SEEDER_ID")
                        .action(ArgAction::Append)
                        .help("Run a specific seeder id; repeat to run more than one"),
                ),
            |invocation| async move { db_seed_command(invocation).await },
        )?;
        registry.command(
            MAKE_MIGRATION_COMMAND,
            Command::new(MAKE_MIGRATION_COMMAND.as_str().to_string())
                .about("Generate a Rust migration scaffold")
                .arg(
                    Arg::new("name")
                        .long("name")
                        .value_name("SLUG")
                        .required(true)
                        .help("Migration slug to include in the timestamped filename"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Overwrite an existing generated file"),
                ),
            |invocation| async move { make_migration_command(invocation).await },
        )?;
        registry.command(
            MAKE_SEEDER_COMMAND,
            Command::new(MAKE_SEEDER_COMMAND.as_str().to_string())
                .about("Generate a Rust seeder scaffold")
                .arg(
                    Arg::new("name")
                        .long("name")
                        .value_name("NAME")
                        .required(true)
                        .help("Seeder name to generate"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Overwrite an existing generated file"),
                ),
            |invocation| async move { make_seeder_command(invocation).await },
        )?;
        registry.command(
            MAKE_MODEL_COMMAND,
            Command::new(MAKE_MODEL_COMMAND.as_str().to_string())
                .about("Generate a Rust model scaffold")
                .arg(
                    Arg::new("name")
                        .long("name")
                        .value_name("NAME")
                        .required(true)
                        .help("Model name in PascalCase (e.g. User, SendWelcomeEmail)"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Overwrite an existing generated file"),
                ),
            |invocation| async move { make_model_command(invocation).await },
        )?;
        registry.command(
            MAKE_JOB_COMMAND,
            Command::new(MAKE_JOB_COMMAND.as_str().to_string())
                .about("Generate a Rust job scaffold")
                .arg(
                    Arg::new("name")
                        .long("name")
                        .value_name("NAME")
                        .required(true)
                        .help("Job name in PascalCase (e.g. SendWelcomeEmail)"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Overwrite an existing generated file"),
                ),
            |invocation| async move { make_job_command(invocation).await },
        )?;
        registry.command(
            MAKE_COMMAND_COMMAND,
            Command::new(MAKE_COMMAND_COMMAND.as_str().to_string())
                .about("Generate a Rust CLI command scaffold")
                .arg(
                    Arg::new("name")
                        .long("name")
                        .value_name("NAME")
                        .required(true)
                        .help("Command name in PascalCase (e.g. SyncInventory)"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Overwrite an existing generated file"),
                ),
            |invocation| async move { make_command_command(invocation).await },
        )?;
        Ok(())
    })
}

async fn db_migrate_command(invocation: CommandInvocation) -> Result<()> {
    let lifecycle = DatabaseLifecycle::from_app(invocation.app())?;
    let summary = lifecycle.migrate().await?;
    match summary.batch {
        Some(batch) => println!("applied {} migration(s) in batch {}", summary.count, batch),
        None => println!("applied 0 migration(s)"),
    }
    Ok(())
}

async fn db_migrate_status_command(invocation: CommandInvocation) -> Result<()> {
    let lifecycle = DatabaseLifecycle::from_app(invocation.app())?;
    for status in lifecycle.statuses().await? {
        match status.applied {
            Some(applied) => println!(
                "{} | Applied | batch {} | {}",
                status.id, applied.batch, applied.applied_at
            ),
            None => println!("{} | Pending", status.id),
        }
    }
    Ok(())
}

async fn db_rollback_command(invocation: CommandInvocation) -> Result<()> {
    let lifecycle = DatabaseLifecycle::from_app(invocation.app())?;
    let summary = lifecycle.rollback_latest_batch().await?;
    match summary.batch {
        Some(batch) => println!(
            "reverted {} migration(s) from batch {}",
            summary.count, batch
        ),
        None => println!("reverted 0 migration(s)"),
    }
    Ok(())
}

async fn db_seed_command(invocation: CommandInvocation) -> Result<()> {
    let lifecycle = DatabaseLifecycle::from_app(invocation.app())?;
    let selected_ids = invocation.matches().get_many::<String>("id").map(|values| {
        values
            .map(|value| SeederId::owned(value.to_string()))
            .collect::<BTreeSet<_>>()
    });
    let count = lifecycle.seed(selected_ids).await?;
    println!("ran {} seeder(s)", count);
    Ok(())
}

async fn make_migration_command(invocation: CommandInvocation) -> Result<()> {
    let config = invocation.app().config().database()?;
    let name = invocation
        .matches()
        .get_one::<String>("name")
        .ok_or_else(|| Error::message("missing required `--name` argument"))?;
    let migration_dir = preferred_migrations_path(invocation.app(), &config)?;
    let basename = format!(
        "{}_{}",
        Local::now().format("%Y%m%d%H%M"),
        normalize_slug(name)
    );
    let migration_path = migration_dir.join(format!("{basename}.rs"));

    fs::create_dir_all(&migration_dir).map_err(Error::other)?;
    ensure_writable(&migration_path, invocation.matches().get_flag("force"))?;
    fs::write(&migration_path, render_migration_template()).map_err(Error::other)?;

    println!("wrote {}", migration_path.display());
    println!("rebuild the app before running db:migrate so the new migration is discovered");
    Ok(())
}

async fn make_seeder_command(invocation: CommandInvocation) -> Result<()> {
    let config = invocation.app().config().database()?;
    let name = invocation
        .matches()
        .get_one::<String>("name")
        .ok_or_else(|| Error::message("missing required `--name` argument"))?;
    let seeder_dir = preferred_seeders_path(invocation.app(), &config)?;
    let basename = to_snake_case(name);
    let seeder_path = seeder_dir.join(format!("{basename}.rs"));

    fs::create_dir_all(&seeder_dir).map_err(Error::other)?;
    ensure_writable(&seeder_path, invocation.matches().get_flag("force"))?;
    fs::write(&seeder_path, render_seeder_template()).map_err(Error::other)?;

    println!("wrote {}", seeder_path.display());
    println!("rebuild the app before running db:seed so the new seeder is discovered");
    Ok(())
}

async fn make_model_command(invocation: CommandInvocation) -> Result<()> {
    let name = invocation
        .matches()
        .get_one::<String>("name")
        .ok_or_else(|| Error::message("missing required `--name` argument"))?;
    let pascal = to_pascal_case(name);
    let snake = to_snake_case(name);
    let model_dir = resolve_app_path("src/app/models")?;
    let model_path = model_dir.join(format!("{snake}.rs"));

    fs::create_dir_all(&model_dir).map_err(Error::other)?;
    ensure_writable(&model_path, invocation.matches().get_flag("force"))?;
    fs::write(&model_path, render_model_template(&pascal, &snake)).map_err(Error::other)?;

    println!("wrote {}", model_path.display());
    Ok(())
}

async fn make_job_command(invocation: CommandInvocation) -> Result<()> {
    let name = invocation
        .matches()
        .get_one::<String>("name")
        .ok_or_else(|| Error::message("missing required `--name` argument"))?;
    let pascal = to_pascal_case(name);
    let snake = to_snake_case(name);
    let screaming = to_screaming_snake_case(&snake);
    let job_dir = resolve_app_path("src/app/jobs")?;
    let job_path = job_dir.join(format!("{snake}.rs"));

    fs::create_dir_all(&job_dir).map_err(Error::other)?;
    ensure_writable(&job_path, invocation.matches().get_flag("force"))?;
    fs::write(&job_path, render_job_template(&pascal, &snake, &screaming)).map_err(Error::other)?;

    println!("wrote {}", job_path.display());
    Ok(())
}

async fn make_command_command(invocation: CommandInvocation) -> Result<()> {
    let name = invocation
        .matches()
        .get_one::<String>("name")
        .ok_or_else(|| Error::message("missing required `--name` argument"))?;
    let pascal = to_pascal_case(name);
    let snake = to_snake_case(name);
    let screaming = to_screaming_snake_case(&snake);
    let command_dir = resolve_app_path("src/app/commands")?;
    let command_path = command_dir.join(format!("{snake}.rs"));

    fs::create_dir_all(&command_dir).map_err(Error::other)?;
    ensure_writable(&command_path, invocation.matches().get_flag("force"))?;
    fs::write(
        &command_path,
        render_command_template(&pascal, &snake, &screaming),
    )
    .map_err(Error::other)?;

    println!("wrote {}", command_path.display());
    Ok(())
}

struct DatabaseLifecycle {
    app: AppContext,
    database: Arc<DatabaseManager>,
    config: DatabaseConfig,
    migrations: Arc<MigrationRegistry>,
    seeders: Arc<SeederRegistry>,
    generated_paths: Option<Arc<GeneratedDatabasePaths>>,
}

impl DatabaseLifecycle {
    fn from_app(app: &AppContext) -> Result<Self> {
        let database = app.database()?;
        if !database.is_configured() {
            return Err(Error::message("database is not configured"));
        }

        Ok(Self {
            app: app.clone(),
            config: app.config().database()?,
            database,
            migrations: app.resolve::<MigrationRegistry>()?,
            seeders: app.resolve::<SeederRegistry>()?,
            generated_paths: app.resolve::<GeneratedDatabasePaths>().ok(),
        })
    }

    async fn statuses(&self) -> Result<Vec<MigrationStatus>> {
        self.ensure_generated_database_is_registered()?;
        let session = self.database.acquire_session().await?;
        ensure_ledger_table(&self.config, &session).await?;
        let applied = applied_migrations(&self.config, &session).await?;
        ensure_applied_migrations_exist(&applied, &self.migrations)?;

        Ok(self
            .migrations
            .ids()
            .into_iter()
            .map(|id| MigrationStatus {
                applied: applied.get(&id).cloned(),
                id,
            })
            .collect())
    }

    async fn migrate(&self) -> Result<MigrationRunSummary> {
        self.ensure_generated_database_is_registered()?;
        let session = self.database.acquire_session().await?;
        let lock_key = advisory_lock_key(&self.config);
        session.acquire_advisory_lock(lock_key).await?;
        let result = migrate_locked(
            self.app.clone(),
            self.database.clone(),
            self.config.clone(),
            self.migrations.clone(),
            &session,
        )
        .await;
        finish_locked_operation(&session, lock_key, result).await
    }

    async fn rollback_latest_batch(&self) -> Result<MigrationRunSummary> {
        self.ensure_generated_database_is_registered()?;
        let session = self.database.acquire_session().await?;
        let lock_key = advisory_lock_key(&self.config);
        session.acquire_advisory_lock(lock_key).await?;
        let result = rollback_locked(
            self.app.clone(),
            self.database.clone(),
            self.config.clone(),
            self.migrations.clone(),
            &session,
        )
        .await;
        finish_locked_operation(&session, lock_key, result).await
    }

    async fn seed(&self, selected_ids: Option<BTreeSet<SeederId>>) -> Result<usize> {
        self.ensure_generated_database_is_registered()?;
        if let Some(selected_ids) = &selected_ids {
            for id in selected_ids {
                if !self.seeders.contains(id) {
                    return Err(Error::message(format!("seeder `{id}` is not registered")));
                }
            }
        }

        let session = self.database.acquire_session().await?;
        let mut ran = 0usize;
        for seeder in self.seeders.entries() {
            if selected_ids
                .as_ref()
                .is_some_and(|selected| !selected.contains(&seeder.id()))
            {
                continue;
            }

            run_seeder(
                self.app.clone(),
                self.database.clone(),
                &session,
                seeder.as_ref(),
            )
            .await?;
            ran += 1;
        }

        Ok(ran)
    }
    fn ensure_generated_database_is_registered(&self) -> Result<()> {
        let Some(paths) = &self.generated_paths else {
            return Ok(());
        };

        let migration_ids = self
            .migrations
            .ids()
            .into_iter()
            .map(|id| id.to_string())
            .collect::<HashSet<_>>();
        for source in discover_migration_sources(paths.migration_dirs()).map_err(Error::other)? {
            if !migration_ids.contains(&source.id) {
                return Err(Error::message(format!(
                    "migration file `{}` exists but is not registered in the current binary; rebuild the app",
                    source.path.display()
                )));
            }
        }

        let seeder_ids = self
            .seeders
            .ids()
            .into_iter()
            .map(|id| id.to_string())
            .collect::<HashSet<_>>();
        for source in discover_seeder_sources(paths.seeder_dirs()).map_err(Error::other)? {
            if !seeder_ids.contains(&source.id) {
                return Err(Error::message(format!(
                    "seeder file `{}` exists but is not registered in the current binary; rebuild the app",
                    source.path.display()
                )));
            }
        }

        Ok(())
    }
}

struct MigrationRunSummary {
    count: usize,
    batch: Option<i64>,
}

async fn finish_locked_operation(
    session: &DatabaseSession,
    lock_key: i64,
    result: Result<MigrationRunSummary>,
) -> Result<MigrationRunSummary> {
    match (result, session.release_advisory_lock(lock_key).await) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(unlock_error)) => Err(unlock_error),
        (Err(error), Err(unlock_error)) => Err(Error::message(format!(
            "{error}; advisory unlock failed: {unlock_error}"
        ))),
    }
}

async fn migrate_locked(
    app: AppContext,
    database: Arc<DatabaseManager>,
    config: DatabaseConfig,
    migrations: Arc<MigrationRegistry>,
    session: &DatabaseSession,
) -> Result<MigrationRunSummary> {
    ensure_ledger_table(&config, session).await?;
    let applied = applied_migrations(&config, session).await?;
    ensure_applied_migrations_exist(&applied, &migrations)?;

    let pending = migrations
        .entries()
        .iter()
        .filter(|migration| !applied.contains_key(&migration.id()))
        .cloned()
        .collect::<Vec<_>>();

    if pending.is_empty() {
        return Ok(MigrationRunSummary {
            count: 0,
            batch: None,
        });
    }

    let next_batch = applied
        .values()
        .map(|migration| migration.batch)
        .max()
        .unwrap_or(0)
        + 1;

    for migration in &pending {
        run_migration_up(app.clone(), database.clone(), session, migration.as_ref()).await?;
        record_applied_migration(session, &config, &migration.id(), next_batch).await?;
    }

    Ok(MigrationRunSummary {
        count: pending.len(),
        batch: Some(next_batch),
    })
}

async fn rollback_locked(
    app: AppContext,
    database: Arc<DatabaseManager>,
    config: DatabaseConfig,
    migrations: Arc<MigrationRegistry>,
    session: &DatabaseSession,
) -> Result<MigrationRunSummary> {
    ensure_ledger_table(&config, session).await?;
    let applied = applied_migrations(&config, session).await?;
    ensure_applied_migrations_exist(&applied, &migrations)?;

    let latest_batch = applied.values().map(|migration| migration.batch).max();
    let Some(latest_batch) = latest_batch else {
        return Ok(MigrationRunSummary {
            count: 0,
            batch: None,
        });
    };

    let rollback = migrations
        .ids()
        .into_iter()
        .rev()
        .filter_map(|id| {
            applied
                .get(&id)
                .filter(|migration| migration.batch == latest_batch)
                .and_then(|_| migrations.migration(&id).cloned())
        })
        .collect::<Vec<_>>();

    for migration in &rollback {
        run_migration_down(app.clone(), database.clone(), session, migration.as_ref()).await?;
        delete_applied_migration(session, &config, &migration.id()).await?;
    }

    Ok(MigrationRunSummary {
        count: rollback.len(),
        batch: Some(latest_batch),
    })
}

async fn run_migration_up(
    app: AppContext,
    database: Arc<DatabaseManager>,
    session: &DatabaseSession,
    migration: &dyn DynMigration,
) -> Result<()> {
    if !migration.run_in_transaction() {
        return migration.up(&app, &database, session).await;
    }

    session.begin_transaction().await?;
    let result = migration.up(&app, &database, session).await;
    match result {
        Ok(()) => session.commit_transaction().await,
        Err(error) => match session.rollback_transaction().await {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(Error::message(format!(
                "{error}; rollback failed: {rollback_error}"
            ))),
        },
    }
}

async fn run_migration_down(
    app: AppContext,
    database: Arc<DatabaseManager>,
    session: &DatabaseSession,
    migration: &dyn DynMigration,
) -> Result<()> {
    if !migration.run_in_transaction() {
        return migration.down(&app, &database, session).await;
    }

    session.begin_transaction().await?;
    let result = migration.down(&app, &database, session).await;
    match result {
        Ok(()) => session.commit_transaction().await,
        Err(error) => match session.rollback_transaction().await {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(Error::message(format!(
                "{error}; rollback failed: {rollback_error}"
            ))),
        },
    }
}

async fn run_seeder(
    app: AppContext,
    database: Arc<DatabaseManager>,
    session: &DatabaseSession,
    seeder: &dyn DynSeeder,
) -> Result<()> {
    if !seeder.run_in_transaction() {
        return seeder.run(&app, &database, session).await;
    }

    session.begin_transaction().await?;
    let result = seeder.run(&app, &database, session).await;
    match result {
        Ok(()) => session.commit_transaction().await,
        Err(error) => match session.rollback_transaction().await {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(Error::message(format!(
                "{error}; rollback failed: {rollback_error}"
            ))),
        },
    }
}

async fn ensure_ledger_table(config: &DatabaseConfig, executor: &dyn QueryExecutor) -> Result<()> {
    let schema = quote_identifier(&config.schema);
    executor
        .raw_execute(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"), &[])
        .await?;
    executor
        .raw_execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {} (id TEXT PRIMARY KEY, batch BIGINT NOT NULL, applied_at TIMESTAMPTZ NOT NULL)",
                qualified_migration_table(config)
            ),
            &[],
        )
        .await?;
    Ok(())
}

async fn applied_migrations(
    config: &DatabaseConfig,
    executor: &dyn QueryExecutor,
) -> Result<BTreeMap<MigrationId, AppliedMigration>> {
    let records = executor
        .raw_query(
            &format!(
                "SELECT id, batch, applied_at::TEXT AS applied_at FROM {} ORDER BY id",
                qualified_migration_table(config)
            ),
            &[],
        )
        .await?;

    let mut applied = BTreeMap::new();
    for record in records {
        let id = MigrationId::owned(record.decode::<String>("id")?);
        applied.insert(
            id.clone(),
            AppliedMigration {
                id,
                batch: record.decode("batch")?,
                applied_at: record.decode("applied_at")?,
            },
        );
    }
    Ok(applied)
}

fn ensure_applied_migrations_exist(
    applied: &BTreeMap<MigrationId, AppliedMigration>,
    migrations: &MigrationRegistry,
) -> Result<()> {
    for migration_id in applied.keys() {
        if !migrations.contains(migration_id) {
            return Err(Error::message(format!(
                "applied migration `{migration_id}` is missing from the registered migration set"
            )));
        }
    }
    Ok(())
}

async fn record_applied_migration(
    executor: &dyn QueryExecutor,
    config: &DatabaseConfig,
    migration_id: &MigrationId,
    batch: i64,
) -> Result<()> {
    executor
        .raw_execute(
            &format!(
                "INSERT INTO {} (id, batch, applied_at) VALUES ($1, $2, NOW())",
                qualified_migration_table(config)
            ),
            &[migration_id.as_str().into(), batch.into()],
        )
        .await?;
    Ok(())
}

async fn delete_applied_migration(
    executor: &dyn QueryExecutor,
    config: &DatabaseConfig,
    migration_id: &MigrationId,
) -> Result<()> {
    executor
        .raw_execute(
            &format!(
                "DELETE FROM {} WHERE id = $1",
                qualified_migration_table(config)
            ),
            &[migration_id.as_str().into()],
        )
        .await?;
    Ok(())
}

fn preferred_migrations_path(app: &AppContext, config: &DatabaseConfig) -> Result<PathBuf> {
    if let Ok(paths) = app.resolve::<GeneratedDatabasePaths>() {
        if let Some(path) = paths.primary_migration_dir() {
            return Ok(path.to_path_buf());
        }
    }

    resolve_configured_path(&config.migrations_path)
}

fn preferred_seeders_path(app: &AppContext, config: &DatabaseConfig) -> Result<PathBuf> {
    if let Ok(paths) = app.resolve::<GeneratedDatabasePaths>() {
        if let Some(path) = paths.primary_seeder_dir() {
            return Ok(path.to_path_buf());
        }
    }

    resolve_configured_path(&config.seeders_path)
}

fn resolve_configured_path(path: &str) -> Result<PathBuf> {
    let configured = PathBuf::from(path);
    if configured.is_absolute() {
        return Ok(configured);
    }

    let cwd = std::env::current_dir().map_err(Error::other)?;
    Ok(cwd.join(configured))
}

fn ensure_writable(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Err(Error::message(format!(
            "refusing to overwrite `{}` without `--force`",
            path.display()
        )));
    }
    Ok(())
}

fn render_migration_template() -> String {
    "use async_trait::async_trait;\nuse forge::prelude::*;\n\npub struct Entry;\n\n#[async_trait]\nimpl MigrationFile for Entry {\n    async fn up(ctx: &MigrationContext<'_>) -> Result<()> {\n        ctx.raw_execute(\n            r#\"CREATE TABLE your_table (id UUID PRIMARY KEY DEFAULT uuidv7());\"#,\n            &[],\n        )\n        .await?;\n        Ok(())\n    }\n\n    async fn down(ctx: &MigrationContext<'_>) -> Result<()> {\n        ctx.raw_execute(\n            r#\"DROP TABLE IF EXISTS your_table;\"#,\n            &[],\n        )\n        .await?;\n        Ok(())\n    }\n}\n"
        .to_string()
}

fn render_seeder_template() -> String {
    "use async_trait::async_trait;\nuse forge::prelude::*;\n\npub struct Entry;\n\n#[async_trait]\nimpl SeederFile for Entry {\n    async fn run(ctx: &SeederContext<'_>) -> Result<()> {\n        ctx.raw_execute(\n            r#\"INSERT INTO your_table (id) VALUES (uuidv7());\"#,\n            &[],\n        )\n        .await?;\n        Ok(())\n    }\n}\n"
        .to_string()
}

fn normalize_slug(input: &str) -> String {
    let slug = input
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();

    let slug = slug
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    if slug.is_empty() {
        "migration".to_string()
    } else {
        slug
    }
}

fn to_snake_case(value: &str) -> String {
    let mut output = String::new();
    let mut previous_was_separator = true;

    for character in value.chars() {
        if !character.is_ascii_alphanumeric() {
            if !output.ends_with('_') {
                output.push('_');
            }
            previous_was_separator = true;
            continue;
        }

        if character.is_ascii_uppercase() && !previous_was_separator && !output.ends_with('_') {
            output.push('_');
        }

        output.push(character.to_ascii_lowercase());
        previous_was_separator = false;
    }

    let collapsed = output
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    if collapsed.is_empty() {
        "generated_seeder".to_string()
    } else {
        collapsed
    }
}

fn qualified_migration_table(config: &DatabaseConfig) -> String {
    format!(
        "{}.{}",
        quote_identifier(&config.schema),
        quote_identifier(&config.migration_table)
    )
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn resolve_app_path(relative: &str) -> Result<PathBuf> {
    let cwd = std::env::current_dir().map_err(Error::other)?;
    Ok(cwd.join(relative))
}

fn to_pascal_case(value: &str) -> String {
    let snake = to_snake_case(value);
    snake
        .split('_')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = first.to_ascii_uppercase().to_string();
                    word.extend(chars);
                    word
                }
                None => String::new(),
            }
        })
        .collect()
}

fn to_screaming_snake_case(snake: &str) -> String {
    snake.to_ascii_uppercase()
}

fn render_model_template(pascal: &str, snake: &str) -> String {
    // Pluralize table name with simple 's' suffix
    let table_name = format!("{snake}s");
    format!(
        "use forge::prelude::*;\n\
         \n\
         #[derive(Clone, Debug, forge::Model)]\n\
         #[forge(model = \"{table_name}\")]\n\
         pub struct {pascal} {{\n\
         \x20   pub id: ModelId<{pascal}>,\n\
         \x20   pub created_at: DateTime,\n\
         \x20   pub updated_at: DateTime,\n\
         }}\n"
    )
}

fn render_job_template(pascal: &str, snake: &str, screaming: &str) -> String {
    format!(
        "use async_trait::async_trait;\n\
         use forge::prelude::*;\n\
         \n\
         pub const {screaming}_JOB: JobId = JobId::new(\"{snake}\");\n\
         \n\
         #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]\n\
         pub struct {pascal};\n\
         \n\
         #[async_trait]\n\
         impl Job for {pascal} {{\n\
         \x20   const ID: JobId = {screaming}_JOB;\n\
         \n\
         \x20   async fn handle(&self, _context: JobContext) -> Result<()> {{\n\
         \x20       // TODO: implement\n\
         \x20       Ok(())\n\
         \x20   }}\n\
         }}\n"
    )
}

fn render_command_template(pascal: &str, snake: &str, screaming: &str) -> String {
    format!(
        "use forge::prelude::*;\n\
         \n\
         pub const {screaming}_COMMAND: CommandId = CommandId::new(\"{snake}\");\n\
         \n\
         pub fn register(registry: &mut CommandRegistry) -> Result<()> {{\n\
         \x20   registry.command(\n\
         \x20       {screaming}_COMMAND,\n\
         \x20       clap::Command::new(\"{snake}\").about(\"{pascal} command\"),\n\
         \x20       |_invocation: CommandInvocation| async move {{\n\
         \x20           // TODO: implement\n\
         \x20           Ok(())\n\
         \x20       }},\n\
         \x20   )?;\n\
         \x20   Ok(())\n\
         }}\n"
    )
}

fn advisory_lock_key(config: &DatabaseConfig) -> i64 {
    let input = format!("forge:{}:{}", config.schema, config.migration_table);
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in input.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash & 0x7fff_ffff_ffff_ffff) as i64
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{
        advisory_lock_key, AppliedMigration, GeneratedDatabasePaths, MigrationContext,
        MigrationFile, MigrationId, MigrationRegistryBuilder, MigrationStatus, SeederContext,
        SeederFile, SeederId, SeederRegistryBuilder,
    };
    use crate::config::DatabaseConfig;
    use crate::foundation::Result;

    struct CreateUsers;

    #[async_trait]
    impl MigrationFile for CreateUsers {
        async fn up(_ctx: &MigrationContext<'_>) -> Result<()> {
            Ok(())
        }

        async fn down(_ctx: &MigrationContext<'_>) -> Result<()> {
            Ok(())
        }
    }

    struct SeedUsers;

    #[async_trait]
    impl SeederFile for SeedUsers {
        async fn run(_ctx: &SeederContext<'_>) -> Result<()> {
            Ok(())
        }
    }

    struct FileSeedUsers;

    #[async_trait]
    impl SeederFile for FileSeedUsers {
        async fn run(_ctx: &SeederContext<'_>) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn rejects_duplicate_migration_ids() {
        let mut builder = MigrationRegistryBuilder::default();
        builder
            .register_file::<CreateUsers>(MigrationId::new("202604091200_create_users"))
            .unwrap();

        let error = builder
            .register_file::<CreateUsers>(MigrationId::new("202604091200_create_users"))
            .err()
            .unwrap();
        assert!(error.to_string().contains("already registered"));
    }

    #[test]
    fn rejects_duplicate_seeder_ids() {
        let mut builder = SeederRegistryBuilder::default();
        builder
            .register_file::<SeedUsers>(SeederId::new("users.seed"))
            .unwrap();

        let error = builder
            .register_file::<SeedUsers>(SeederId::new("users.seed"))
            .err()
            .unwrap();
        assert!(error.to_string().contains("already registered"));
    }

    #[test]
    fn preserves_seeder_registration_order() {
        let mut builder = SeederRegistryBuilder::default();
        builder
            .register_file::<SeedUsers>(SeederId::new("users.seed"))
            .unwrap();
        builder
            .register_file::<FileSeedUsers>(SeederId::new("users.file"))
            .unwrap();

        let registry = SeederRegistryBuilder::freeze_shared(Arc::new(Mutex::new(builder))).unwrap();
        assert_eq!(
            registry.ids(),
            vec![SeederId::new("users.seed"), SeederId::new("users.file")]
        );
    }

    #[test]
    fn maps_applied_and_pending_statuses() {
        let applied = AppliedMigration {
            id: MigrationId::new("202604090900_init"),
            batch: 1,
            applied_at: "2026-04-09 09:00:00+00".to_string(),
        };
        let statuses = [
            MigrationStatus {
                id: applied.id.clone(),
                applied: Some(applied.clone()),
            },
            MigrationStatus {
                id: MigrationId::new("202604091000_users"),
                applied: None,
            },
        ];

        assert_eq!(statuses[0].applied.as_ref(), Some(&applied));
        assert!(statuses[1].applied.is_none());
    }

    #[test]
    fn generated_database_paths_expose_primary_dirs() {
        let paths = GeneratedDatabasePaths::new(
            vec!["/tmp/migrations".into()],
            vec!["/tmp/seeders".into()],
        );
        assert_eq!(
            paths.primary_migration_dir().unwrap(),
            std::path::Path::new("/tmp/migrations")
        );
        assert_eq!(
            paths.primary_seeder_dir().unwrap(),
            std::path::Path::new("/tmp/seeders")
        );
    }

    #[test]
    fn advisory_lock_key_depends_on_schema_and_table() {
        let public = DatabaseConfig::default();
        let custom = DatabaseConfig {
            schema: "forge".to_string(),
            migration_table: "custom_migrations".to_string(),
            ..DatabaseConfig::default()
        };

        assert_ne!(advisory_lock_key(&public), advisory_lock_key(&custom));
    }

    #[test]
    fn to_pascal_case_from_snake() {
        assert_eq!(
            super::to_pascal_case("send_welcome_email"),
            "SendWelcomeEmail"
        );
    }

    #[test]
    fn to_pascal_case_from_pascal() {
        assert_eq!(
            super::to_pascal_case("SendWelcomeEmail"),
            "SendWelcomeEmail"
        );
    }

    #[test]
    fn to_pascal_case_single_word() {
        assert_eq!(super::to_pascal_case("user"), "User");
        assert_eq!(super::to_pascal_case("User"), "User");
    }

    #[test]
    fn to_screaming_snake_case_converts() {
        assert_eq!(
            super::to_screaming_snake_case("send_welcome_email"),
            "SEND_WELCOME_EMAIL"
        );
    }

    #[test]
    fn render_model_template_contains_struct() {
        let output = super::render_model_template("User", "user");
        assert!(output.contains("pub struct User {"));
        assert!(output.contains("#[forge(model = \"users\")]"));
        assert!(output.contains("pub id: ModelId<User>"));
    }

    #[test]
    fn render_job_template_contains_const_and_impl() {
        let output = super::render_job_template(
            "SendWelcomeEmail",
            "send_welcome_email",
            "SEND_WELCOME_EMAIL",
        );
        assert!(output.contains("pub const SEND_WELCOME_EMAIL_JOB: JobId"));
        assert!(output.contains("pub struct SendWelcomeEmail;"));
        assert!(output.contains("const ID: JobId = SEND_WELCOME_EMAIL_JOB;"));
    }

    #[test]
    fn render_command_template_contains_register() {
        let output =
            super::render_command_template("SyncInventory", "sync_inventory", "SYNC_INVENTORY");
        assert!(output.contains("pub const SYNC_INVENTORY_COMMAND: CommandId"));
        assert!(output.contains("pub fn register("));
        assert!(output.contains("Command::new(\"sync_inventory\")"));
    }
}
