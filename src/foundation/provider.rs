use std::sync::Arc;

use async_trait::async_trait;

use crate::auth::{
    Authenticatable, AuthenticatableRegistryHandle, BearerAuthenticator, GuardRegistryHandle,
    Policy, PolicyRegistryHandle,
};
use crate::config::ConfigRepository;
use crate::database::{MigrationFile, MigrationRegistryHandle, SeederFile, SeederRegistryHandle};
use crate::datatable::registry::{DatatableRegistryBuilder, DatatableRegistryHandle};
use crate::email::{EmailDriverFactory, EmailDriverRegistryHandle};
use crate::events::{Event, EventListener, EventRegistryHandle};
use crate::foundation::{AppContext, Container, Result};
use crate::jobs::{Job, JobMiddleware, JobMiddlewareRegistryHandle, JobRegistryHandle};
use crate::logging::{ReadinessCheck, ReadinessRegistryHandle};
use crate::notifications::{
    NotificationChannel, NotificationChannelRegistryBuilder, NotificationChannelRegistryHandle,
};
use crate::storage::{StorageDriverFactory, StorageDriverRegistryHandle};
use crate::support::{GuardId, MigrationId, PolicyId, ProbeId, SeederId};
use crate::validation::RuleRegistry;

#[derive(Clone)]
pub struct ServiceRegistrar {
    container: Container,
    config: ConfigRepository,
    rules: RuleRegistry,
    event_registry: EventRegistryHandle,
    job_registry: JobRegistryHandle,
    job_middleware_registry: JobMiddlewareRegistryHandle,
    migration_registry: MigrationRegistryHandle,
    seeder_registry: SeederRegistryHandle,
    guard_registry: GuardRegistryHandle,
    policy_registry: PolicyRegistryHandle,
    authenticatable_registry: AuthenticatableRegistryHandle,
    readiness_registry: ReadinessRegistryHandle,
    storage_driver_registry: StorageDriverRegistryHandle,
    email_driver_registry: EmailDriverRegistryHandle,
    notification_channel_registry: NotificationChannelRegistryHandle,
    datatable_registry: DatatableRegistryHandle,
}

impl ServiceRegistrar {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        container: Container,
        config: ConfigRepository,
        rules: RuleRegistry,
        event_registry: EventRegistryHandle,
        job_registry: JobRegistryHandle,
        job_middleware_registry: JobMiddlewareRegistryHandle,
        migration_registry: MigrationRegistryHandle,
        seeder_registry: SeederRegistryHandle,
        guard_registry: GuardRegistryHandle,
        policy_registry: PolicyRegistryHandle,
        authenticatable_registry: AuthenticatableRegistryHandle,
        readiness_registry: ReadinessRegistryHandle,
        storage_driver_registry: StorageDriverRegistryHandle,
        email_driver_registry: EmailDriverRegistryHandle,
    ) -> Self {
        Self {
            container,
            config,
            rules,
            event_registry,
            job_registry,
            job_middleware_registry,
            migration_registry,
            seeder_registry,
            guard_registry,
            policy_registry,
            authenticatable_registry,
            readiness_registry,
            storage_driver_registry,
            email_driver_registry,
            notification_channel_registry: NotificationChannelRegistryBuilder::shared(),
            datatable_registry: DatatableRegistryBuilder::shared(),
        }
    }

    pub fn container(&self) -> &Container {
        &self.container
    }

    pub fn config(&self) -> &ConfigRepository {
        &self.config
    }

    pub fn singleton<T>(&self, value: T) -> Result<()>
    where
        T: Send + Sync + 'static,
    {
        self.container.singleton(value)
    }

    pub fn singleton_arc<T>(&self, value: Arc<T>) -> Result<()>
    where
        T: Send + Sync + 'static,
    {
        self.container.singleton_arc(value)
    }

    pub fn factory<T, F>(&self, factory: F) -> Result<()>
    where
        T: Send + Sync + 'static,
        F: Fn(&Container, &AppContext) -> Result<T> + Send + Sync + 'static,
    {
        let config = self.config.clone();
        let rules = self.rules.clone();

        self.container.factory(move |container| {
            let app = AppContext::new(container.clone(), config.clone(), rules.clone())?;
            factory(container, &app)
        })
    }

    pub fn resolve<T>(&self) -> Result<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.container.resolve::<T>()
    }

    pub fn listen_event<E, L>(&self, listener: L) -> Result<()>
    where
        E: Event,
        L: EventListener<E>,
    {
        self.event_registry
            .lock()
            .expect("event registry lock poisoned")
            .listen::<E, L>(listener);
        Ok(())
    }

    pub fn register_job<J>(&self) -> Result<()>
    where
        J: Job,
    {
        self.job_registry
            .lock()
            .expect("job registry lock poisoned")
            .register::<J>()
    }

    pub fn register_job_middleware<M: JobMiddleware>(&self, middleware: M) -> Result<()> {
        self.job_middleware_registry
            .lock()
            .expect("job middleware registry lock poisoned")
            .register(Arc::new(middleware));
        Ok(())
    }

    pub(crate) fn register_generated_migration_file<M>(
        &self,
        id: impl Into<MigrationId>,
    ) -> Result<()>
    where
        M: MigrationFile,
    {
        self.migration_registry
            .lock()
            .expect("migration registry lock poisoned")
            .register_file::<M>(id.into())
    }

    pub(crate) fn register_generated_seeder_file<S>(&self, id: impl Into<SeederId>) -> Result<()>
    where
        S: SeederFile,
    {
        self.seeder_registry
            .lock()
            .expect("seeder registry lock poisoned")
            .register_file::<S>(id.into())
    }

    pub fn register_guard<I, G>(&self, id: I, guard: G) -> Result<()>
    where
        I: Into<GuardId>,
        G: BearerAuthenticator,
    {
        self.guard_registry
            .lock()
            .expect("guard registry lock poisoned")
            .register_arc(id, Arc::new(guard))
    }

    pub fn register_policy<I, P>(&self, id: I, policy: P) -> Result<()>
    where
        I: Into<PolicyId>,
        P: Policy,
    {
        self.policy_registry
            .lock()
            .expect("policy registry lock poisoned")
            .register_arc(id, Arc::new(policy))
    }

    pub fn register_authenticatable<M>(&self) -> Result<()>
    where
        M: Authenticatable,
    {
        self.authenticatable_registry
            .lock()
            .expect("authenticatable registry lock poisoned")
            .register::<M>()
    }

    pub fn register_readiness_check<I, C>(&self, id: I, check: C) -> Result<()>
    where
        I: Into<ProbeId>,
        C: ReadinessCheck,
    {
        self.readiness_registry
            .lock()
            .expect("readiness registry lock poisoned")
            .register_arc(id, Arc::new(check))
    }

    pub fn register_storage_driver(&self, name: &str, factory: StorageDriverFactory) -> Result<()> {
        self.storage_driver_registry
            .lock()
            .expect("storage driver registry lock poisoned")
            .register(name.to_string(), factory)
    }

    pub fn register_email_driver(&self, name: &str, factory: EmailDriverFactory) -> Result<()> {
        self.email_driver_registry
            .lock()
            .expect("email driver registry lock poisoned")
            .register(name.to_string(), factory)
    }

    pub fn register_notification_channel<I, N>(&self, id: I, channel: N) -> Result<()>
    where
        I: Into<crate::support::NotificationChannelId>,
        N: NotificationChannel,
    {
        self.notification_channel_registry
            .lock()
            .expect("notification channel registry lock poisoned")
            .register(id, Arc::new(channel))
    }

    pub(crate) fn notification_channel_registry(&self) -> NotificationChannelRegistryHandle {
        self.notification_channel_registry.clone()
    }

    pub(crate) fn job_middleware_registry(&self) -> JobMiddlewareRegistryHandle {
        self.job_middleware_registry.clone()
    }

    pub fn register_datatable<D>(&self) -> Result<()>
    where
        D: crate::datatable::Datatable,
    {
        self.datatable_registry
            .lock()
            .expect("datatable registry lock poisoned")
            .register::<D>()
    }

    pub(crate) fn datatable_registry(&self) -> DatatableRegistryHandle {
        self.datatable_registry.clone()
    }
}

#[async_trait]
pub trait ServiceProvider: Send + Sync + 'static {
    async fn register(&self, _registrar: &mut ServiceRegistrar) -> Result<()> {
        Ok(())
    }

    async fn boot(&self, _app: &crate::foundation::AppContext) -> Result<()> {
        Ok(())
    }
}
