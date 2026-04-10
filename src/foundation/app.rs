use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::auth::{AuthManager, Authorizer, GuardRegistryBuilder, PolicyRegistryBuilder};
use crate::cli::CommandRegistrar;
use crate::config::ConfigRepository;
use crate::database::{
    DatabaseManager, DatabaseTransaction, MigrationRegistryBuilder, ModelWriteExecutor,
    QueryExecutionOptions, QueryExecutor, SeederRegistryBuilder,
};
use crate::events::{EventBus, EventRegistryBuilder};
use crate::foundation::{Container, Error, Result, ServiceProvider, ServiceRegistrar};
use crate::http::RouteRegistrar;
use crate::jobs::{JobDispatcher, JobRegistryBuilder, JobRuntime};
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
use crate::scheduler::ScheduleRegistrar;
use crate::support::runtime::RuntimeBackend;
use crate::support::ValidationRuleId;
use crate::validation::{RuleRegistry, ValidationRule};
use crate::websocket::{WebSocketPublisher, WebSocketRouteRegistrar};

#[derive(Clone)]
pub struct AppContext {
    container: Container,
    config: ConfigRepository,
    rules: RuleRegistry,
}

pub struct AppTransaction {
    app: AppContext,
    transaction: DatabaseTransaction,
}

impl AppContext {
    pub fn new(container: Container, config: ConfigRepository, rules: RuleRegistry) -> Self {
        Self {
            container,
            config,
            rules,
        }
    }

    pub fn container(&self) -> &Container {
        &self.container
    }

    pub fn config(&self) -> &ConfigRepository {
        &self.config
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

    pub async fn begin_transaction(&self) -> Result<AppTransaction> {
        let database = self.database()?;
        let transaction = database.begin().await?;
        Ok(AppTransaction {
            app: self.clone(),
            transaction,
        })
    }

    pub fn diagnostics(&self) -> Result<Arc<RuntimeDiagnostics>> {
        self.resolve::<RuntimeDiagnostics>()
    }

    pub fn plugins(&self) -> Result<Arc<PluginRegistry>> {
        self.resolve::<PluginRegistry>()
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

    pub async fn commit(self) -> Result<()> {
        self.transaction.commit().await
    }

    pub async fn rollback(self) -> Result<()> {
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
}

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
    observability: Option<ObservabilityOptions>,
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
            observability: None,
        }
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
        self.build_http_kernel().await?.serve().await
    }

    pub fn run_cli(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_cli_async().await })
    }

    pub async fn run_cli_async(self) -> Result<()> {
        self.build_cli_kernel().await?.run().await
    }

    pub fn run_scheduler(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_scheduler_async().await })
    }

    pub async fn run_scheduler_async(self) -> Result<()> {
        self.build_scheduler_kernel().await?.run().await
    }

    pub fn run_worker(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_worker_async().await })
    }

    pub async fn run_worker_async(self) -> Result<()> {
        self.build_worker_kernel().await?.run().await
    }

    pub fn run_websocket(self) -> Result<()> {
        self.block_on(|builder| async move { builder.run_websocket_async().await })
    }

    pub async fn run_websocket_async(self) -> Result<()> {
        self.build_websocket_kernel().await?.serve().await
    }

    pub async fn build_http_kernel(self) -> Result<HttpKernel> {
        let boot = self.bootstrap().await?;
        Ok(HttpKernel::new(boot.app, boot.routes, boot.observability))
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
            observability,
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
        let migration_registry = MigrationRegistryBuilder::shared();
        let seeder_registry = SeederRegistryBuilder::shared();
        let guard_registry = GuardRegistryBuilder::shared();
        let policy_registry = PolicyRegistryBuilder::shared();
        let readiness_registry = ReadinessRegistryBuilder::shared();
        let mut registrar = ServiceRegistrar::new(
            container.clone(),
            config.clone(),
            event_registry.clone(),
            job_registry.clone(),
            migration_registry.clone(),
            seeder_registry.clone(),
            guard_registry.clone(),
            policy_registry.clone(),
            readiness_registry.clone(),
        );
        for provider in &prepared_plugins.providers {
            provider.register(&mut registrar).await?;
        }
        for provider in &providers {
            provider.register(&mut registrar).await?;
        }

        let app = AppContext::new(container, config, rules);
        let database = Arc::new(DatabaseManager::from_config(&app.config().database()?).await?);
        let auth_manager = Arc::new(AuthManager::new(
            app.config().auth()?,
            GuardRegistryBuilder::freeze_shared(guard_registry),
        ));
        let authorizer = Arc::new(Authorizer::new(
            app.clone(),
            PolicyRegistryBuilder::freeze_shared(policy_registry),
        ));
        let backend = RuntimeBackend::from_config(app.config())?;
        let backend_kind = backend.kind();
        let jobs_config = app.config().jobs()?;
        app.container().singleton_arc(Arc::new(backend.clone()))?;
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
        let migration_registry =
            Arc::new(MigrationRegistryBuilder::freeze_shared(migration_registry)?);
        let seeder_registry = Arc::new(SeederRegistryBuilder::freeze_shared(seeder_registry)?);

        app.container()
            .singleton_arc(prepared_plugins.registry.clone())?;
        app.container().singleton_arc(database)?;
        app.container().singleton_arc(auth_manager)?;
        app.container().singleton_arc(authorizer)?;
        app.container().singleton_arc(diagnostics.clone())?;
        app.container().singleton_arc(websocket_publisher)?;
        app.container().singleton_arc(event_bus)?;
        app.container().singleton_arc(job_runtime)?;
        app.container().singleton_arc(job_dispatcher)?;
        app.container().singleton_arc(migration_registry)?;
        app.container().singleton_arc(seeder_registry)?;

        for provider in &prepared_plugins.providers {
            provider.boot(&app).await?;
        }
        for plugin in &prepared_plugins.instances {
            plugin.boot(&app).await?;
        }
        for provider in &providers {
            provider.boot(&app).await?;
        }
        diagnostics.mark_bootstrap_complete();

        let mut boot_routes = prepared_plugins.routes;
        boot_routes.extend(routes);

        let mut boot_commands = Vec::new();
        if app.config().value("database").is_some() {
            boot_commands.push(crate::database::builtin_cli_registrar());
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

        Ok(BootArtifacts {
            app,
            routes: boot_routes,
            commands: boot_commands,
            schedules: boot_schedules,
            websocket_routes: boot_websocket_routes,
            observability,
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
    observability: Option<ObservabilityOptions>,
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
}
