use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use crate::auth::{
    Actor, AuthManager, AuthenticatableRegistry, AuthenticatableRegistryBuilder, Authorizer,
    GuardRegistryBuilder, PolicyRegistryBuilder,
};
use crate::cli::CommandRegistrar;
use crate::config::ConfigRepository;
use crate::database::{
    set_runtime_model_defaults, DatabaseManager, DatabaseTransaction, MigrationRegistryBuilder,
    ModelWriteExecutor, QueryExecutionOptions, QueryExecutor, SeederRegistryBuilder,
};
use crate::email::{job::SendQueuedEmailJob, EmailDriverRegistryBuilder, EmailManager};
use crate::events::{EventBus, EventRegistryBuilder};
use crate::foundation::{Container, Error, Result, ServiceProvider, ServiceRegistrar};
use crate::http::middleware::MiddlewareConfig;
use crate::http::RouteRegistrar;
use crate::jobs::{JobDispatcher, JobMiddlewareRegistryBuilder, JobRegistryBuilder, JobRuntime};
use crate::kernel::{
    cli::CliKernel, http::HttpKernel, scheduler::SchedulerKernel, websocket::WebSocketKernel,
    worker::WorkerKernel,
};
use crate::logging::{
    ObservabilityOptions, ProbeResult, ReadinessRegistryBuilder, ReadinessRegistryHandle,
    RuntimeBackendKind, RuntimeDiagnostics, FRAMEWORK_BOOTSTRAP_PROBE, REDIS_PING_PROBE,
    RUNTIME_BACKEND_PROBE,
};
use crate::plugin::{Plugin, PluginRegistry};
use crate::redis::RedisManager;
use crate::scheduler::ScheduleRegistrar;
use crate::storage::{StorageDriverRegistryBuilder, StorageManager};
use crate::support::runtime::RuntimeBackend;
use crate::support::{Clock, CryptManager, GuardId, HashManager, Timezone, ValidationRuleId};
use crate::validation::{RuleRegistry, ValidationRule};
use crate::websocket::{WebSocketPublisher, WebSocketRouteRegistrar};

#[derive(Clone)]
pub struct AppContext {
    container: Container,
    config: ConfigRepository,
    timezone: Timezone,
    rules: RuleRegistry,
}

type AfterCommitFn =
    Box<dyn FnOnce(AppContext) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

pub struct AppTransaction {
    app: AppContext,
    transaction: DatabaseTransaction,
    after_commit: Vec<AfterCommitFn>,
    actor: Option<Actor>,
}

impl AppContext {
    pub fn new(
        container: Container,
        config: ConfigRepository,
        rules: RuleRegistry,
    ) -> Result<Self> {
        let timezone = config.app()?.timezone;
        Ok(Self {
            container,
            config,
            timezone,
            rules,
        })
    }

    pub fn container(&self) -> &Container {
        &self.container
    }

    pub fn config(&self) -> &ConfigRepository {
        &self.config
    }

    pub fn timezone(&self) -> Result<Timezone> {
        Ok(self.timezone.clone())
    }

    pub fn clock(&self) -> Clock {
        Clock::new(self.timezone.clone())
    }

    pub fn rules(&self) -> &RuleRegistry {
        &self.rules
    }

    pub fn resolve<T>(&self) -> Result<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.container.resolve::<T>()
    }

    pub fn events(&self) -> Result<Arc<EventBus>> {
        self.resolve::<EventBus>()
    }

    pub fn auth(&self) -> Result<Arc<AuthManager>> {
        self.resolve::<AuthManager>()
    }

    pub fn authorizer(&self) -> Result<Arc<Authorizer>> {
        self.resolve::<Authorizer>()
    }

    pub fn jobs(&self) -> Result<Arc<JobDispatcher>> {
        self.resolve::<JobDispatcher>()
    }

    pub fn websocket(&self) -> Result<Arc<WebSocketPublisher>> {
        self.resolve::<WebSocketPublisher>()
    }

    pub fn database(&self) -> Result<Arc<DatabaseManager>> {
        self.resolve::<DatabaseManager>()
    }

    pub fn redis(&self) -> Result<Arc<RedisManager>> {
        self.resolve::<RedisManager>()
    }

    pub fn storage(&self) -> Result<Arc<StorageManager>> {
        self.resolve::<StorageManager>()
    }

    pub fn email(&self) -> Result<Arc<EmailManager>> {
        self.resolve::<EmailManager>()
    }

    pub fn hash(&self) -> Result<Arc<HashManager>> {
        self.resolve::<HashManager>()
    }

    pub fn crypt(&self) -> Result<Arc<CryptManager>> {
        self.resolve::<CryptManager>()
    }

    pub async fn begin_transaction(&self) -> Result<AppTransaction> {
        let database = self.database()?;
        let transaction = database.begin().await?;
        Ok(AppTransaction {
            app: self.clone(),
            transaction,
            after_commit: Vec::new(),
            actor: None,
        })
    }

    pub fn diagnostics(&self) -> Result<Arc<RuntimeDiagnostics>> {
        self.resolve::<RuntimeDiagnostics>()
    }

    pub fn i18n(&self) -> Result<Arc<crate::i18n::I18nManager>> {
        self.resolve::<crate::i18n::I18nManager>()
    }

    pub fn plugins(&self) -> Result<Arc<PluginRegistry>> {
        self.resolve::<PluginRegistry>()
    }

    pub fn datatables(&self) -> Result<Arc<crate::datatable::DatatableRegistry>> {
        self.resolve::<crate::datatable::DatatableRegistry>()
    }

    pub fn authenticatables(&self) -> Result<Arc<AuthenticatableRegistry>> {
        self.resolve::<AuthenticatableRegistry>()
    }

    pub fn tokens(&self) -> Result<Arc<crate::auth::token::TokenManager>> {
        self.resolve::<crate::auth::token::TokenManager>()
    }

    pub fn sessions(&self) -> Result<Arc<crate::auth::session::SessionManager>> {
        self.resolve::<crate::auth::session::SessionManager>()
    }

    pub fn password_resets(&self) -> Result<Arc<crate::auth::password_reset::PasswordResetManager>> {
        self.resolve::<crate::auth::password_reset::PasswordResetManager>()
    }

    pub fn email_verification(&self) -> Result<Arc<crate::auth::email_verification::EmailVerificationManager>> {
        self.resolve::<crate::auth::email_verification::EmailVerificationManager>()
    }

    pub fn cache(&self) -> Result<Arc<crate::cache::CacheManager>> {
        self.resolve::<crate::cache::CacheManager>()
    }

    pub fn lock(&self) -> Result<Arc<crate::support::lock::DistributedLock>> {
        self.resolve::<crate::support::lock::DistributedLock>()
    }

    pub async fn notify(
        &self,
        notifiable: &dyn crate::notifications::Notifiable,
        notification: &dyn crate::notifications::Notification,
    ) -> Result<()> {
        crate::notifications::notify(self, notifiable, notification).await
    }

    /// Dispatch a notification asynchronously via the job queue.
    pub async fn notify_queued(
        &self,
        notifiable: &dyn crate::notifications::Notifiable,
        notification: &dyn crate::notifications::Notification,
    ) -> Result<()> {
        crate::notifications::notify_queued(self, notifiable, notification).await
    }

    /// Generate a URL from a named route.
    ///
    /// ```ignore
    /// let url = app.route_url("users.show", &[("id", "123")])?;
    /// ```
    pub fn route_url(&self, name: &str, params: &[(&str, &str)]) -> Result<String> {
        let registry = self.resolve::<crate::http::routes::RouteRegistry>()?;
        registry.url(name, params)
    }

    /// Generate a signed URL from a named route.
    pub fn signed_route_url(
        &self,
        name: &str,
        params: &[(&str, &str)],
        expires_at: crate::support::DateTime,
    ) -> Result<String> {
        let registry = self.resolve::<crate::http::routes::RouteRegistry>()?;
        let signing_key = self.config().app()?.signing_key_bytes()?;
        registry.signed_url(name, params, &signing_key, expires_at)
    }

    /// Verify a signed URL.
    pub fn verify_signed_url(&self, url: &str) -> Result<()> {
        let signing_key = self.config().app()?.signing_key_bytes()?;
        crate::http::routes::RouteRegistry::verify_signature(url, &signing_key)
    }

    /// Shut down all registered plugins in reverse dependency order.
    /// Called automatically during graceful shutdown.
    pub async fn shutdown_plugins(&self) -> Result<()> {
        let list = match self.resolve::<PluginShutdownList>() {
            Ok(list) => list,
            Err(_) => return Ok(()), // no plugins registered
        };
        for plugin in &list.0 {
            if let Err(e) = plugin.shutdown(self).await {
                tracing::warn!(
                    plugin = %plugin.manifest().id(),
                    error = %e,
                    "plugin shutdown failed"
                );
            }
        }
        Ok(())
    }

    pub(crate) fn job_runtime(&self) -> Result<Arc<JobRuntime>> {
        self.resolve::<JobRuntime>()
    }
}

impl AppTransaction {
    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub fn transaction(&self) -> &DatabaseTransaction {
        &self.transaction
    }

    /// Set the actor for audit trail support in lifecycle hooks.
    ///
    /// When an actor is set, it will be available via `ModelHookContext::actor()`
    /// in all lifecycle hooks (creating, created, updating, updated, deleting, deleted)
    /// triggered through this transaction.
    pub fn set_actor(&mut self, actor: Actor) {
        self.actor = Some(actor);
    }

    pub fn actor(&self) -> Option<&Actor> {
        self.actor.as_ref()
    }

    /// Buffer a job dispatch that will only execute after a successful `commit()`.
    ///
    /// If the transaction is rolled back (or dropped), the job is never dispatched.
    pub fn dispatch_after_commit<J: crate::jobs::Job>(&mut self, job: J) {
        self.after_commit.push(Box::new(move |app| {
            Box::pin(async move { app.jobs()?.dispatch(job).await })
        }));
    }

    /// Buffer a queued notification that will only be dispatched after a successful `commit()`.
    ///
    /// Channel payloads are pre-rendered immediately (at call time) so
    /// the notification/notifiable do not need to outlive the transaction.
    pub fn notify_after_commit(
        &mut self,
        notifiable: &dyn crate::notifications::Notifiable,
        notification: &dyn crate::notifications::Notification,
    ) {
        let job = crate::notifications::build_notification_job(notifiable, notification);
        self.dispatch_after_commit(job);
    }

    /// Register an arbitrary async callback to run after a successful `commit()`.
    pub fn after_commit<F, Fut>(&mut self, callback: F)
    where
        F: FnOnce(AppContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.after_commit
            .push(Box::new(move |app| Box::pin(callback(app))));
    }

    /// Commit the database transaction, then flush all pending after-commit callbacks.
    ///
    /// If the commit itself fails, no callbacks are executed.
    /// If an after-commit callback fails, the error is logged but remaining callbacks
    /// continue to execute (the database commit is not rolled back).
    pub async fn commit(self) -> Result<()> {
        self.transaction.commit().await?;

        for callback in self.after_commit {
            if let Err(error) = callback(self.app.clone()).await {
                tracing::error!(error = %error, "after-commit dispatch failed");
            }
        }

        Ok(())
    }

    /// Roll back the database transaction. All pending after-commit callbacks are dropped.
    pub async fn rollback(self) -> Result<()> {
        // `self.after_commit` is dropped — callbacks never execute.
        self.transaction.rollback().await
    }
}

#[async_trait]
impl QueryExecutor for AppContext {
    async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[crate::database::DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<crate::database::DbRecord>> {
        self.database()?
            .raw_query_with(sql, bindings, options)
            .await
    }

    async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[crate::database::DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        self.database()?
            .raw_execute_with(sql, bindings, options)
            .await
    }
}

#[async_trait]
impl QueryExecutor for AppTransaction {
    async fn raw_query_with(
        &self,
        sql: &str,
        bindings: &[crate::database::DbValue],
        options: QueryExecutionOptions,
    ) -> Result<Vec<crate::database::DbRecord>> {
        self.transaction
            .raw_query_with(sql, bindings, options)
            .await
    }

    async fn raw_execute_with(
        &self,
        sql: &str,
        bindings: &[crate::database::DbValue],
        options: QueryExecutionOptions,
    ) -> Result<u64> {
        self.transaction
            .raw_execute_with(sql, bindings, options)
            .await
    }
}

impl ModelWriteExecutor for AppContext {
    fn app_context(&self) -> &AppContext {
        self
    }
}

impl ModelWriteExecutor for AppTransaction {
    fn app_context(&self) -> &AppContext {
        &self.app
    }

    fn active_transaction(&self) -> Option<&DatabaseTransaction> {
        Some(&self.transaction)
    }

    fn actor(&self) -> Option<&Actor> {
        self.actor.as_ref()
    }
}

/// Plugin instances stored in reverse dependency order for graceful shutdown.
struct PluginShutdownList(Vec<Arc<dyn Plugin>>);

pub struct App;

impl App {
    pub fn builder() -> AppBuilder {
        AppBuilder::new()
    }
}

pub struct AppBuilder {
    load_env: bool,
    config_dir: Option<PathBuf>,
    plugins: Vec<Arc<dyn Plugin>>,
    providers: Vec<Arc<dyn ServiceProvider>>,
    routes: Vec<RouteRegistrar>,
    commands: Vec<CommandRegistrar>,
    schedules: Vec<ScheduleRegistrar>,
    websocket_routes: Vec<WebSocketRouteRegistrar>,
    validation_rules: Vec<(ValidationRuleId, Arc<dyn ValidationRule>)>,
    middlewares: Vec<MiddlewareConfig>,
    middleware_groups: std::collections::HashMap<String, Vec<MiddlewareConfig>>,
    observability: Option<ObservabilityOptions>,
    spa_dir: Option<PathBuf>,
}

impl Default for AppBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AppBuilder {
    pub fn new() -> Self {
        Self {
            load_env: false,
            config_dir: None,
            plugins: Vec::new(),
            providers: Vec::new(),
            routes: Vec::new(),
            commands: Vec::new(),
            schedules: Vec::new(),
            websocket_routes: Vec::new(),
            validation_rules: Vec::new(),
            middlewares: Vec::new(),
            middleware_groups: std::collections::HashMap::new(),
            observability: None,
            spa_dir: None,
        }
    }

    /// Serve a SPA frontend from the given directory. All requests not matched
    /// by API routes will fall back to `{dir}/index.html` for client-side routing.
    ///
    /// ```ignore
    /// App::builder()
    ///     .register_routes(api::routes)
    ///     .serve_spa("dist/")
    ///     .run_http()?;
    /// ```
    pub fn serve_spa(mut self, dir: impl Into<PathBuf>) -> Self {
        self.spa_dir = Some(dir.into());
        self
    }

    pub fn load_env(mut self) -> Self {
        self.load_env = true;
        self
    }

    pub fn load_config_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_dir = Some(path.into());
        self
    }

    pub fn register_plugin<P>(mut self, plugin: P) -> Self
    where
        P: Plugin,
    {
        self.plugins.push(Arc::new(plugin));
        self
    }

    pub fn register_plugins<I, P>(mut self, plugins: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Plugin,
    {
        self.plugins.extend(
            plugins
                .into_iter()
                .map(|plugin| Arc::new(plugin) as Arc<dyn Plugin>),
        );
        self
    }

    pub fn register_provider<P>(mut self, provider: P) -> Self
    where
        P: ServiceProvider,
    {
        self.providers.push(Arc::new(provider));
        self
    }

    pub fn register_routes<F>(mut self, registrar: F) -> Self
    where
        F: Fn(&mut crate::http::HttpRegistrar) -> Result<()> + Send + Sync + 'static,
    {
        self.routes.push(Arc::new(registrar));
        self
    }

    pub fn register_commands<F>(mut self, registrar: F) -> Self
    where
        F: Fn(&mut crate::cli::CommandRegistry) -> Result<()> + Send + Sync + 'static,
    {
        self.commands.push(Arc::new(registrar));
        self
    }

    pub fn register_schedule<F>(mut self, registrar: F) -> Self
    where
        F: Fn(&mut crate::scheduler::ScheduleRegistry) -> Result<()> + Send + Sync + 'static,
    {
        self.schedules.push(Arc::new(registrar));
        self
    }

    pub fn register_websocket_routes<F>(mut self, registrar: F) -> Self
    where
        F: Fn(&mut crate::websocket::WebSocketRegistrar) -> Result<()> + Send + Sync + 'static,
    {
        self.websocket_routes.push(Arc::new(registrar));
        self
    }

    pub fn register_validation_rule<I, R>(mut self, id: I, rule: R) -> Self
    where
        I: Into<ValidationRuleId>,
        R: ValidationRule,
    {
        self.validation_rules.push((id.into(), Arc::new(rule)));
        self
    }

    pub fn register_middleware(mut self, config: MiddlewareConfig) -> Self {
        self.middlewares.push(config);
        self
    }

    /// Register a named middleware group for reuse on routes.
    ///
    /// ```ignore
    /// App::builder()
    ///     .middleware_group("api", vec![
    ///         RateLimit::new(100).per_minute().build(),
    ///         Compression::new().build(),
    ///     ])
    /// ```
    pub fn middleware_group(
        mut self,
        name: impl Into<String>,
        middlewares: Vec<MiddlewareConfig>,
    ) -> Self {
        self.middleware_groups.insert(name.into(), middlewares);
        self
    }

    pub fn enable_observability(mut self) -> Self {
        self.observability = Some(ObservabilityOptions::default());
        self
    }

    pub fn enable_observability_with(mut self, options: ObservabilityOptions) -> Self {
        self.observability = Some(options);
        self
    }

    pub fn run_http(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_http_async().await })
    }

    pub async fn run_http_async(self) -> Result<()> {
        let kernel = self.build_http_kernel().await?;
        let app = kernel.app().clone();
        let result = kernel.serve().await;
        app.shutdown_plugins().await?;
        result
    }

    pub fn run_cli(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_cli_async().await })
    }

    pub async fn run_cli_async(self) -> Result<()> {
        let kernel = self.build_cli_kernel().await?;
        let app = kernel.app().clone();
        let result = kernel.run().await;
        app.shutdown_plugins().await?;
        result
    }

    pub fn run_scheduler(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_scheduler_async().await })
    }

    pub async fn run_scheduler_async(self) -> Result<()> {
        let kernel = self.build_scheduler_kernel().await?;
        let app = kernel.app().clone();
        let result = kernel.run().await;
        app.shutdown_plugins().await?;
        result
    }

    pub fn run_worker(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_worker_async().await })
    }

    pub async fn run_worker_async(self) -> Result<()> {
        let kernel = self.build_worker_kernel().await?;
        let app = kernel.app().clone();
        let result = kernel.run().await;
        app.shutdown_plugins().await?;
        result
    }

    pub fn run_websocket(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_websocket_async().await })
    }

    pub async fn run_websocket_async(self) -> Result<()> {
        let kernel = self.build_websocket_kernel().await?;
        let app = kernel.app().clone();
        let result = kernel.serve().await;
        app.shutdown_plugins().await?;
        result
    }

    pub async fn build_http_kernel(self) -> Result<HttpKernel> {
        let boot = self.bootstrap().await?;
        Ok(HttpKernel::new(
            boot.app,
            boot.routes,
            boot.middlewares,
            boot.observability,
            boot.spa_dir,
        ))
    }

    pub async fn build_cli_kernel(self) -> Result<CliKernel> {
        let boot = self.bootstrap().await?;
        Ok(CliKernel::new(boot.app, boot.commands))
    }

    pub async fn build_scheduler_kernel(self) -> Result<SchedulerKernel> {
        let boot = self.bootstrap().await?;
        let mut registry = crate::scheduler::ScheduleRegistry::new();
        for registrar in boot.schedules {
            registrar(&mut registry)?;
        }
        SchedulerKernel::new(boot.app, registry)
    }

    pub async fn build_worker_kernel(self) -> Result<WorkerKernel> {
        let boot = self.bootstrap().await?;
        WorkerKernel::new(boot.app)
    }

    pub async fn build_websocket_kernel(self) -> Result<WebSocketKernel> {
        let boot = self.bootstrap().await?;
        Ok(WebSocketKernel::new(boot.app, boot.websocket_routes))
    }

    async fn bootstrap(self) -> Result<BootArtifacts> {
        let AppBuilder {
            load_env,
            config_dir,
            plugins,
            providers,
            routes,
            commands,
            schedules,
            websocket_routes,
            validation_rules,
            middlewares,
            middleware_groups,
            observability,
            spa_dir,
        } = self;

        if load_env {
            dotenvy::dotenv().ok();
        }

        let prepared_plugins = crate::plugin::prepare_plugins(&plugins)?;
        let config = match config_dir {
            Some(path) => ConfigRepository::from_dir_with_defaults(
                path,
                prepared_plugins.config_defaults.clone(),
            )?,
            None => ConfigRepository::with_env_overlay_and_defaults(
                prepared_plugins.config_defaults.clone(),
            )?,
        };
        set_runtime_model_defaults(config.database()?.models.clone());
        crate::logging::init(&config)?;

        let container = Container::new();
        let rules = RuleRegistry::new();
        for (name, rule) in prepared_plugins.validation_rules.iter() {
            rules.register_arc(name.clone(), rule.clone())?;
        }
        for (name, rule) in validation_rules {
            rules.register_arc(name, rule)?;
        }

        let event_registry = EventRegistryBuilder::shared();
        let job_registry = JobRegistryBuilder::shared();
        let job_middleware_registry = JobMiddlewareRegistryBuilder::shared();
        let migration_registry = MigrationRegistryBuilder::shared();
        let seeder_registry = SeederRegistryBuilder::shared();
        let guard_registry = GuardRegistryBuilder::shared();
        let policy_registry = PolicyRegistryBuilder::shared();
        let authenticatable_registry = AuthenticatableRegistryBuilder::shared();
        let readiness_registry = ReadinessRegistryBuilder::shared();
        let storage_driver_registry = StorageDriverRegistryBuilder::shared();
        let email_driver_registry = EmailDriverRegistryBuilder::shared();
        let mut registrar = ServiceRegistrar::new(
            container.clone(),
            config.clone(),
            event_registry.clone(),
            job_registry.clone(),
            job_middleware_registry.clone(),
            migration_registry.clone(),
            seeder_registry.clone(),
            guard_registry.clone(),
            policy_registry.clone(),
            authenticatable_registry.clone(),
            readiness_registry.clone(),
            storage_driver_registry.clone(),
            email_driver_registry.clone(),
        );
        for provider in &prepared_plugins.providers {
            provider.register(&mut registrar).await?;
        }
        // Apply plugin direct registrations (guards, jobs, events, etc.)
        for action in prepared_plugins.registrar_actions {
            action(&registrar)?;
        }
        for provider in &providers {
            provider.register(&mut registrar).await?;
        }

        // Register framework-internal jobs
        registrar.register_job::<SendQueuedEmailJob>()?;
        registrar.register_job::<crate::datatable::export_job::DatatableExportJob>()?;
        registrar.register_job::<crate::notifications::SendNotificationJob>()?;

        let app = AppContext::new(container, config, rules)?;
        let database = Arc::new(DatabaseManager::from_config(&app.config().database()?).await?);

        let auth_config = app.config().auth()?;
        let backend = RuntimeBackend::from_config(app.config())?;
        let backend_kind = backend.kind();
        let jobs_config = app.config().jobs()?;
        let redis = Arc::new(RedisManager::from_config(app.config())?);
        app.container().singleton_arc(Arc::new(backend.clone()))?;
        // Create distributed lock from the same backend
        let distributed_lock = Arc::new(crate::support::lock::DistributedLock::new(
            app.resolve::<RuntimeBackend>()?,
        ));
        app.container().singleton_arc(distributed_lock)?;

        // Auto-register guard authenticators from config before freezing
        let token_manager = Arc::new(crate::auth::token::TokenManager::new(
            database.clone(),
            auth_config.tokens.clone(),
        ));
        let session_manager = Arc::new(crate::auth::session::SessionManager::new(
            redis.clone(),
            auth_config.sessions.clone(),
        ));
        {
            let mut guards = guard_registry.lock().expect("guard registry lock poisoned");
            for (guard_name, driver_config) in &auth_config.guards {
                if guards.contains(guard_name) {
                    continue; // consumer-registered guard takes precedence
                }
                match driver_config.driver {
                    crate::config::GuardDriver::Token => {
                        guards.register_arc(
                            GuardId::owned(guard_name.clone()),
                            Arc::new(crate::auth::token::TokenAuthenticator::new(
                                token_manager.clone(),
                            )),
                        )?;
                    }
                    crate::config::GuardDriver::Session => {
                        guards.register_session(
                            GuardId::owned(guard_name.clone()),
                            session_manager.clone(),
                        )?;
                    }
                    crate::config::GuardDriver::Custom => {}
                }
            }
        }

        let auth_manager = Arc::new(AuthManager::new(
            auth_config,
            GuardRegistryBuilder::freeze_shared(guard_registry),
        ));
        let authorizer = Arc::new(Authorizer::new(
            app.clone(),
            PolicyRegistryBuilder::freeze_shared(policy_registry),
        ));
        let authenticatable_registry = Arc::new(AuthenticatableRegistryBuilder::freeze_shared(
            authenticatable_registry,
        ));
        register_builtin_readiness_checks(&readiness_registry, backend_kind)?;
        let diagnostics = Arc::new(RuntimeDiagnostics::new(
            backend_kind,
            ReadinessRegistryBuilder::freeze_shared(readiness_registry),
        ));
        let websocket_publisher = Arc::new(WebSocketPublisher::new(
            backend.clone(),
            diagnostics.clone(),
        ));
        let event_bus = Arc::new(EventBus::new(
            app.clone(),
            EventRegistryBuilder::freeze_shared(event_registry),
        ));
        let job_runtime = Arc::new(JobRuntime::new(
            backend,
            jobs_config.clone(),
            JobRegistryBuilder::freeze_shared(job_registry, &jobs_config),
        ));
        let job_dispatcher = Arc::new(JobDispatcher::new(job_runtime.clone(), diagnostics.clone()));
        let job_middleware_registry = Arc::new(JobMiddlewareRegistryBuilder::freeze_shared(
            registrar.job_middleware_registry(),
        ));
        let migration_registry =
            Arc::new(MigrationRegistryBuilder::freeze_shared(migration_registry)?);
        let seeder_registry = Arc::new(SeederRegistryBuilder::freeze_shared(seeder_registry)?);
        let datatable_registry = Arc::new(
            crate::datatable::registry::DatatableRegistryBuilder::freeze_shared(
                registrar.datatable_registry(),
            ),
        );

        // Auto-register built-in notification channels (consumer-registered ones take precedence)
        let ncr_handle = registrar.notification_channel_registry();
        {
            let mut ncr = ncr_handle.lock().expect("notification channel registry lock poisoned");
            if !ncr.contains(&crate::notifications::NOTIFY_EMAIL) {
                ncr.register(crate::notifications::NOTIFY_EMAIL, Arc::new(crate::notifications::EmailNotificationChannel))?;
            }
            if !ncr.contains(&crate::notifications::NOTIFY_DATABASE) {
                ncr.register(crate::notifications::NOTIFY_DATABASE, Arc::new(crate::notifications::DatabaseNotificationChannel))?;
            }
            if !ncr.contains(&crate::notifications::NOTIFY_BROADCAST) {
                ncr.register(crate::notifications::NOTIFY_BROADCAST, Arc::new(crate::notifications::BroadcastNotificationChannel))?;
            }
        }
        let notification_channel_registry = Arc::new(
            crate::notifications::NotificationChannelRegistryBuilder::freeze_shared(ncr_handle),
        );

        // Cache manager (needs redis before it's moved into container)
        let cache_config = app.config().cache()?;
        let cache_store: Arc<dyn crate::cache::CacheStore> = match cache_config.driver {
            crate::config::CacheDriver::Memory => {
                Arc::new(crate::cache::MemoryCacheStore::new(cache_config.max_entries))
            }
            crate::config::CacheDriver::Redis => Arc::new(crate::cache::RedisCacheStore::new(
                redis.clone(),
                cache_config.prefix.clone(),
            )),
        };
        let cache_manager = Arc::new(crate::cache::CacheManager::new(cache_store));

        let password_reset_manager = Arc::new(crate::auth::password_reset::PasswordResetManager::new(
            database.clone(),
            60, // 60 minutes expiry
        ));

        let email_verification_manager = Arc::new(
            crate::auth::email_verification::EmailVerificationManager::new(
                database.clone(),
                1440, // 24 hours expiry for email verification
            ),
        );

        app.container()
            .singleton_arc(prepared_plugins.registry.clone())?;
        app.container().singleton_arc(database)?;
        app.container().singleton_arc(redis)?;
        app.container().singleton_arc(auth_manager)?;
        app.container().singleton_arc(authorizer)?;
        app.container().singleton_arc(authenticatable_registry)?;
        app.container().singleton_arc(token_manager)?;
        app.container().singleton_arc(session_manager)?;
        app.container().singleton_arc(password_reset_manager)?;
        app.container().singleton_arc(email_verification_manager)?;
        app.container().singleton_arc(cache_manager)?;

        app.container().singleton_arc(diagnostics.clone())?;
        app.container().singleton_arc(websocket_publisher)?;
        app.container().singleton_arc(event_bus)?;
        app.container().singleton_arc(job_runtime)?;
        app.container().singleton_arc(job_dispatcher)?;
        app.container().singleton_arc(job_middleware_registry)?;
        app.container().singleton_arc(migration_registry)?;
        app.container().singleton_arc(seeder_registry)?;
        app.container().singleton_arc(datatable_registry)?;
        app.container()
            .singleton_arc(notification_channel_registry)?;

        // Register middleware groups for route-level resolution
        let groups = Arc::new(crate::http::middleware::MiddlewareGroups(middleware_groups));
        app.container().singleton_arc(groups)?;

        // Register i18n if configured
        if let Ok(i18n_config) = app.config().i18n() {
            if !i18n_config.resource_path.is_empty() {
                let i18n_manager = crate::i18n::I18nManager::load(&i18n_config)?;
                app.container().singleton_arc(Arc::new(i18n_manager))?;
            }
        }

        for provider in &prepared_plugins.providers {
            provider.boot(&app).await?;
        }
        for plugin in &prepared_plugins.instances {
            plugin.boot(&app).await?;
        }
        // Store plugin instances in reverse dependency order for shutdown
        let mut shutdown_order = prepared_plugins.instances.clone();
        shutdown_order.reverse();
        app.container()
            .singleton(PluginShutdownList(shutdown_order))?;

        for provider in &providers {
            provider.boot(&app).await?;
        }

        // Freeze storage driver registry and construct StorageManager
        let custom_storage_drivers =
            StorageDriverRegistryBuilder::freeze_shared(storage_driver_registry);
        let storage =
            Arc::new(StorageManager::from_config(app.config(), custom_storage_drivers).await?);
        app.container().singleton_arc(storage)?;

        // Freeze email driver registry and construct EmailManager
        let custom_email_drivers = EmailDriverRegistryBuilder::freeze_shared(email_driver_registry);
        let email = Arc::new(EmailManager::from_config(
            app.config(),
            custom_email_drivers,
            app.clone(),
        )?);
        app.container().singleton_arc(email)?;

        // Hash manager (argon2 password hashing)
        let hashing_config = app.config().hashing()?;
        let hash = Arc::new(HashManager::from_config(&hashing_config)?);
        app.container().singleton_arc(hash)?;

        // Crypt manager (AES-256-GCM encryption, optional)
        let crypt_config = app.config().crypt()?;
        if !crypt_config.key.is_empty() {
            let crypt = Arc::new(CryptManager::from_config(&crypt_config)?);
            app.container().singleton_arc(crypt)?;
        }

        diagnostics.mark_bootstrap_complete();

        let mut boot_routes = prepared_plugins.routes;
        boot_routes.extend(routes);

        let mut boot_commands = vec![
            crate::config::publish::config_publish_cli_registrar(),
            crate::config::api_docs::docs_api_cli_registrar(),
            crate::config::env_publish::env_publish_cli_registrar(),
            crate::http::maintenance_cli_registrar(),
        ];
        if app.config().value("database").is_some() {
            boot_commands.push(crate::database::builtin_cli_registrar());
            boot_commands.push(crate::auth::builtin_cli_registrar());
        }
        if !prepared_plugins.registry.is_empty() {
            boot_commands.push(crate::plugin::builtin_cli_registrar());
        }
        boot_commands.extend(prepared_plugins.commands);
        boot_commands.extend(commands);

        let mut boot_schedules = prepared_plugins.schedules;
        boot_schedules.extend(schedules);

        let mut boot_websocket_routes = prepared_plugins.websocket_routes;
        boot_websocket_routes.extend(websocket_routes);

        let mut boot_middlewares = prepared_plugins.middlewares;
        boot_middlewares.extend(middlewares);

        Ok(BootArtifacts {
            app,
            routes: boot_routes,
            commands: boot_commands,
            schedules: boot_schedules,
            websocket_routes: boot_websocket_routes,
            middlewares: boot_middlewares,
            observability,
            spa_dir,
        })
    }

    fn block_on<F, Fut>(self, runner: F) -> Result<()>
    where
        F: FnOnce(AppBuilder) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(Error::other)?;
        runtime.block_on(runner(self))
    }
}

struct BootArtifacts {
    app: AppContext,
    routes: Vec<RouteRegistrar>,
    commands: Vec<CommandRegistrar>,
    schedules: Vec<ScheduleRegistrar>,
    websocket_routes: Vec<WebSocketRouteRegistrar>,
    middlewares: Vec<MiddlewareConfig>,
    observability: Option<ObservabilityOptions>,
    spa_dir: Option<PathBuf>,
}

fn register_builtin_readiness_checks(
    registry: &ReadinessRegistryHandle,
    backend_kind: RuntimeBackendKind,
) -> Result<()> {
    let mut registry = registry.lock().expect("readiness registry lock poisoned");
    registry.register_arc(
        FRAMEWORK_BOOTSTRAP_PROBE,
        Arc::new(|app: &AppContext| {
            let app = app.clone();
            async move {
                match app.diagnostics() {
                    Ok(diagnostics) if diagnostics.bootstrap_complete() => {
                        Ok(ProbeResult::healthy(FRAMEWORK_BOOTSTRAP_PROBE))
                    }
                    Ok(_) => Ok(ProbeResult::unhealthy(
                        FRAMEWORK_BOOTSTRAP_PROBE,
                        "framework bootstrap not complete",
                    )),
                    Err(error) => Ok(ProbeResult::unhealthy(
                        FRAMEWORK_BOOTSTRAP_PROBE,
                        error.to_string(),
                    )),
                }
            }
        }),
    )?;
    registry.register_arc(
        RUNTIME_BACKEND_PROBE,
        Arc::new(|app: &AppContext| {
            let app = app.clone();
            async move {
                match app.resolve::<RuntimeBackend>() {
                    Ok(backend) => Ok(ProbeResult {
                        id: RUNTIME_BACKEND_PROBE,
                        state: crate::logging::ProbeState::Healthy,
                        message: Some(format!("{:?} backend active", backend.kind())),
                    }),
                    Err(error) => Ok(ProbeResult::unhealthy(
                        RUNTIME_BACKEND_PROBE,
                        error.to_string(),
                    )),
                }
            }
        }),
    )?;

    if matches!(backend_kind, RuntimeBackendKind::Redis) {
        registry.register_arc(
            REDIS_PING_PROBE,
            Arc::new(|app: &AppContext| {
                let app = app.clone();
                async move {
                    match app.resolve::<RuntimeBackend>() {
                        Ok(backend) => match backend.ping().await {
                            Ok(()) => Ok(ProbeResult::healthy(REDIS_PING_PROBE)),
                            Err(error) => {
                                Ok(ProbeResult::unhealthy(REDIS_PING_PROBE, error.to_string()))
                            }
                        },
                        Err(error) => {
                            Ok(ProbeResult::unhealthy(REDIS_PING_PROBE, error.to_string()))
                        }
                    }
                }
            }),
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::App;
    use crate::foundation::{AppContext, Result, ServiceProvider, ServiceRegistrar};

    struct TestProvider {
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl ServiceProvider for TestProvider {
        async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
            registrar.singleton::<String>("ready".to_string())?;
            self.order.lock().unwrap().push("register");
            Ok(())
        }

        async fn boot(&self, app: &AppContext) -> Result<()> {
            let value = app.resolve::<String>()?;
            assert_eq!(value.as_str(), "ready");
            self.order.lock().unwrap().push("boot");
            Ok(())
        }
    }

    #[tokio::test]
    async fn providers_register_before_boot() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let _kernel = App::builder()
            .register_provider(TestProvider {
                order: order.clone(),
            })
            .build_cli_kernel()
            .await
            .unwrap();

        assert_eq!(order.lock().unwrap().as_slice(), ["register", "boot"]);
    }

    #[tokio::test]
    async fn app_context_resolves_redis_manager() {
        let kernel = App::builder().build_cli_kernel().await.unwrap();
        let redis = kernel.app().redis().unwrap();

        assert_eq!(redis.namespace(), "forge:development");
    }
}
