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
pub(crate) struct RegistryHub {
    pub(crate) event: EventRegistryHandle,
    pub(crate) job: JobRegistryHandle,
    pub(crate) job_middleware: JobMiddlewareRegistryHandle,
    pub(crate) migration: MigrationRegistryHandle,
    pub(crate) seeder: SeederRegistryHandle,
    pub(crate) guard: GuardRegistryHandle,
    pub(crate) policy: PolicyRegistryHandle,
    pub(crate) authenticatable: AuthenticatableRegistryHandle,
    pub(crate) readiness: ReadinessRegistryHandle,
    pub(crate) storage_driver: StorageDriverRegistryHandle,
    pub(crate) email_driver: EmailDriverRegistryHandle,
    pub(crate) notification_channel: NotificationChannelRegistryHandle,
    pub(crate) datatable: DatatableRegistryHandle,
}

impl RegistryHub {
    pub(crate) fn new() -> Self {
        Self {
            event: crate::events::EventRegistryBuilder::shared(),
            job: crate::jobs::JobRegistryBuilder::shared(),
            job_middleware: crate::jobs::JobMiddlewareRegistryBuilder::shared(),
            migration: crate::database::MigrationRegistryBuilder::shared(),
            seeder: crate::database::SeederRegistryBuilder::shared(),
            guard: crate::auth::GuardRegistryBuilder::shared(),
            policy: crate::auth::PolicyRegistryBuilder::shared(),
            authenticatable: crate::auth::AuthenticatableRegistryBuilder::shared(),
            readiness: crate::logging::ReadinessRegistryBuilder::shared(),
            storage_driver: crate::storage::StorageDriverRegistryBuilder::shared(),
            email_driver: crate::email::EmailDriverRegistryBuilder::shared(),
            notification_channel: NotificationChannelRegistryBuilder::shared(),
            datatable: DatatableRegistryBuilder::shared(),
        }
    }
}

#[derive(Clone)]
pub struct ServiceRegistrar {
    container: Container,
    config: ConfigRepository,
    rules: RuleRegistry,
    registries: RegistryHub,
}

impl ServiceRegistrar {
    pub(crate) fn new(
        container: Container,
        config: ConfigRepository,
        rules: RuleRegistry,
        registries: RegistryHub,
    ) -> Self {
        Self {
            container,
            config,
            rules,
            registries,
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
        self.registries
            .event
            .lock()
            .expect("event registry lock poisoned")
            .listen::<E, L>(listener);
        Ok(())
    }

    pub fn register_job<J>(&self) -> Result<()>
    where
        J: Job,
    {
        self.registries
            .job
            .lock()
            .expect("job registry lock poisoned")
            .register::<J>()
    }

    pub fn register_job_middleware<M: JobMiddleware>(&self, middleware: M) -> Result<()> {
        self.registries
            .job_middleware
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
        self.registries
            .migration
            .lock()
            .expect("migration registry lock poisoned")
            .register_file::<M>(id.into())
    }

    pub(crate) fn register_generated_seeder_file<S>(&self, id: impl Into<SeederId>) -> Result<()>
    where
        S: SeederFile,
    {
        self.registries
            .seeder
            .lock()
            .expect("seeder registry lock poisoned")
            .register_file::<S>(id.into())
    }

    pub fn register_guard<I, G>(&self, id: I, guard: G) -> Result<()>
    where
        I: Into<GuardId>,
        G: BearerAuthenticator,
    {
        self.registries
            .guard
            .lock()
            .expect("guard registry lock poisoned")
            .register_arc(id, Arc::new(guard))
    }

    pub fn register_policy<I, P>(&self, id: I, policy: P) -> Result<()>
    where
        I: Into<PolicyId>,
        P: Policy,
    {
        self.registries
            .policy
            .lock()
            .expect("policy registry lock poisoned")
            .register_arc(id, Arc::new(policy))
    }

    pub fn register_authenticatable<M>(&self) -> Result<()>
    where
        M: Authenticatable,
    {
        self.registries
            .authenticatable
            .lock()
            .expect("authenticatable registry lock poisoned")
            .register::<M>()
    }

    pub fn register_readiness_check<I, C>(&self, id: I, check: C) -> Result<()>
    where
        I: Into<ProbeId>,
        C: ReadinessCheck,
    {
        self.registries
            .readiness
            .lock()
            .expect("readiness registry lock poisoned")
            .register_arc(id, Arc::new(check))
    }

    pub fn register_storage_driver(&self, name: &str, factory: StorageDriverFactory) -> Result<()> {
        self.registries
            .storage_driver
            .lock()
            .expect("storage driver registry lock poisoned")
            .register(name.to_string(), factory)
    }

    pub fn register_email_driver(&self, name: &str, factory: EmailDriverFactory) -> Result<()> {
        self.registries
            .email_driver
            .lock()
            .expect("email driver registry lock poisoned")
            .register(name.to_string(), factory)
    }

    pub fn register_notification_channel<I, N>(&self, id: I, channel: N) -> Result<()>
    where
        I: Into<crate::support::NotificationChannelId>,
        N: NotificationChannel,
    {
        self.registries
            .notification_channel
            .lock()
            .expect("notification channel registry lock poisoned")
            .register(id, Arc::new(channel))
    }

    pub(crate) fn notification_channel_registry(&self) -> NotificationChannelRegistryHandle {
        self.registries.notification_channel.clone()
    }

    pub(crate) fn job_middleware_registry(&self) -> JobMiddlewareRegistryHandle {
        self.registries.job_middleware.clone()
    }

    pub fn register_datatable<D>(&self) -> Result<()>
    where
        D: crate::datatable::Datatable,
    {
        self.registries
            .datatable
            .lock()
            .expect("datatable registry lock poisoned")
            .register::<D>()
    }

    pub(crate) fn datatable_registry(&self) -> DatatableRegistryHandle {
        self.registries.datatable.clone()
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
