use std::sync::Arc;

use async_trait::async_trait;

use crate::auth::{BearerAuthenticator, GuardRegistryHandle, Policy, PolicyRegistryHandle};
use crate::config::ConfigRepository;
use crate::database::{MigrationFile, MigrationRegistryHandle, SeederFile, SeederRegistryHandle};
use crate::events::{Event, EventListener, EventRegistryHandle};
use crate::foundation::{Container, Result};
use crate::jobs::{Job, JobRegistryHandle};
use crate::logging::{ReadinessCheck, ReadinessRegistryHandle};
use crate::support::{GuardId, MigrationId, PolicyId, ProbeId, SeederId};

#[derive(Clone)]
pub struct ServiceRegistrar {
    container: Container,
    config: ConfigRepository,
    event_registry: EventRegistryHandle,
    job_registry: JobRegistryHandle,
    migration_registry: MigrationRegistryHandle,
    seeder_registry: SeederRegistryHandle,
    guard_registry: GuardRegistryHandle,
    policy_registry: PolicyRegistryHandle,
    readiness_registry: ReadinessRegistryHandle,
}

impl ServiceRegistrar {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        container: Container,
        config: ConfigRepository,
        event_registry: EventRegistryHandle,
        job_registry: JobRegistryHandle,
        migration_registry: MigrationRegistryHandle,
        seeder_registry: SeederRegistryHandle,
        guard_registry: GuardRegistryHandle,
        policy_registry: PolicyRegistryHandle,
        readiness_registry: ReadinessRegistryHandle,
    ) -> Self {
        Self {
            container,
            config,
            event_registry,
            job_registry,
            migration_registry,
            seeder_registry,
            guard_registry,
            policy_registry,
            readiness_registry,
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
        F: Fn(&Container) -> Result<T> + Send + Sync + 'static,
    {
        self.container.factory(factory)
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
