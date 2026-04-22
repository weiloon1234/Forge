use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Serialize;

use crate::auth::Actor;
use crate::foundation::{AppContext, Error, Result};
use crate::jobs::Job;
use crate::support::EventId;
use crate::websocket::ServerMessage;

pub trait Event: Clone + Serialize + Send + Sync + 'static {
    const ID: EventId;
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventOrigin {
    pub actor: Option<Actor>,
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub request_id: Option<String>,
}

impl EventOrigin {
    pub fn new(
        actor: Option<Actor>,
        ip: Option<IpAddr>,
        user_agent: Option<String>,
        request_id: Option<String>,
    ) -> Self {
        Self {
            actor,
            ip,
            user_agent,
            request_id,
        }
    }

    pub fn from_request(
        actor: Option<Actor>,
        request: Option<&crate::logging::CurrentRequest>,
    ) -> Option<Self> {
        match (actor, request) {
            (None, None) => None,
            (actor, request) => Some(Self::new(
                actor,
                request.and_then(|value| value.ip),
                request.and_then(|value| value.user_agent.clone()),
                request.and_then(|value| value.request_id.clone()),
            )),
        }
    }
}

#[derive(Clone)]
pub struct EventContext {
    app: AppContext,
    origin: Option<EventOrigin>,
}

impl EventContext {
    pub(crate) fn new(app: AppContext, origin: Option<EventOrigin>) -> Self {
        Self { app, origin }
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub fn origin(&self) -> Option<&EventOrigin> {
        self.origin.as_ref()
    }

    pub fn actor(&self) -> Option<&Actor> {
        self.origin().and_then(|origin| origin.actor.as_ref())
    }

    pub fn ip(&self) -> Option<IpAddr> {
        self.origin().and_then(|origin| origin.ip)
    }

    pub fn user_agent(&self) -> Option<&str> {
        self.origin()
            .and_then(|origin| origin.user_agent.as_deref())
    }

    pub fn request_id(&self) -> Option<&str> {
        self.origin()
            .and_then(|origin| origin.request_id.as_deref())
    }
}

#[async_trait]
pub trait EventListener<E: Event>: Send + Sync + 'static {
    async fn handle(&self, context: &EventContext, event: &E) -> Result<()>;
}

#[async_trait]
trait DynEventListener: Send + Sync {
    async fn handle_boxed(
        &self,
        context: &EventContext,
        event: &(dyn Any + Send + Sync),
    ) -> Result<()>;
}

struct ListenerAdapter<E, L> {
    listener: L,
    marker: PhantomData<E>,
}

#[async_trait]
impl<E, L> DynEventListener for ListenerAdapter<E, L>
where
    E: Event,
    L: EventListener<E>,
{
    async fn handle_boxed(
        &self,
        context: &EventContext,
        event: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let event = event
            .downcast_ref::<E>()
            .ok_or_else(|| Error::message(format!("failed to downcast event `{}`", E::ID)))?;
        self.listener.handle(context, event).await
    }
}

pub(crate) type EventRegistryHandle = Arc<Mutex<EventRegistryBuilder>>;

#[derive(Default)]
pub(crate) struct EventRegistryBuilder {
    listeners: HashMap<TypeId, Vec<Arc<dyn DynEventListener>>>,
}

impl EventRegistryBuilder {
    pub(crate) fn shared() -> EventRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn listen<E, L>(&mut self, listener: L)
    where
        E: Event,
        L: EventListener<E>,
    {
        self.listeners
            .entry(TypeId::of::<E>())
            .or_default()
            .push(Arc::new(ListenerAdapter::<E, L> {
                listener,
                marker: PhantomData,
            }));
    }

    pub(crate) fn freeze_shared(handle: EventRegistryHandle) -> EventRegistrySnapshot {
        let mut builder = handle.lock().expect("event registry lock poisoned");
        EventRegistrySnapshot {
            listeners: std::mem::take(&mut builder.listeners),
        }
    }
}

pub(crate) struct EventRegistrySnapshot {
    listeners: HashMap<TypeId, Vec<Arc<dyn DynEventListener>>>,
}

#[derive(Clone)]
pub struct EventBus {
    app: AppContext,
    registry: Arc<EventRegistrySnapshot>,
}

impl EventBus {
    pub(crate) fn new(app: AppContext, registry: EventRegistrySnapshot) -> Self {
        Self {
            app,
            registry: Arc::new(registry),
        }
    }

    pub async fn dispatch<E>(&self, event: E) -> Result<()>
    where
        E: Event,
    {
        self.dispatch_with_origin(event, None).await
    }

    pub async fn dispatch_with_origin<E>(&self, event: E, origin: Option<EventOrigin>) -> Result<()>
    where
        E: Event,
    {
        let context = EventContext::new(self.app.clone(), origin);
        if let Some(listeners) = self.registry.listeners.get(&TypeId::of::<E>()) {
            for listener in listeners {
                listener.handle_boxed(&context, &event).await?;
            }
        }
        Ok(())
    }
}

pub struct JobDispatchListener<E, J, F> {
    mapper: F,
    marker: PhantomData<(E, J)>,
}

pub fn dispatch_job<E, J, F>(mapper: F) -> JobDispatchListener<E, J, F>
where
    E: Event,
    J: Job,
    F: Fn(&E) -> J + Send + Sync + 'static,
{
    JobDispatchListener {
        mapper,
        marker: PhantomData,
    }
}

#[async_trait]
impl<E, J, F> EventListener<E> for JobDispatchListener<E, J, F>
where
    E: Event,
    J: Job,
    F: Fn(&E) -> J + Send + Sync + 'static,
{
    async fn handle(&self, context: &EventContext, event: &E) -> Result<()> {
        context.app().jobs()?.dispatch((self.mapper)(event)).await
    }
}

pub struct WebSocketPublishListener<E, F> {
    mapper: F,
    marker: PhantomData<E>,
}

pub fn publish_websocket<E, F>(mapper: F) -> WebSocketPublishListener<E, F>
where
    E: Event,
    F: Fn(&E) -> ServerMessage + Send + Sync + 'static,
{
    WebSocketPublishListener {
        mapper,
        marker: PhantomData,
    }
}

#[async_trait]
impl<E, F> EventListener<E> for WebSocketPublishListener<E, F>
where
    E: Event,
    F: Fn(&E) -> ServerMessage + Send + Sync + 'static,
{
    async fn handle(&self, context: &EventContext, event: &E) -> Result<()> {
        context
            .app()
            .websocket()?
            .publish_message((self.mapper)(event))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{Event, EventBus, EventContext, EventListener, EventOrigin, EventRegistryBuilder};
    use crate::auth::Actor;
    use crate::config::ConfigRepository;
    use crate::foundation::{AppContext, Container};
    use crate::support::EventId;
    use crate::support::GuardId;
    use crate::validation::RuleRegistry;

    #[derive(Clone, serde::Serialize)]
    struct TestEvent;

    impl Event for TestEvent {
        const ID: EventId = EventId::new("test.event");
    }

    struct PushListener {
        target: Arc<Mutex<Vec<&'static str>>>,
        name: &'static str,
    }

    #[async_trait]
    impl EventListener<TestEvent> for PushListener {
        async fn handle(&self, _context: &EventContext, _event: &TestEvent) -> crate::Result<()> {
            self.target.lock().unwrap().push(self.name);
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatches_listeners_in_registration_order() {
        let target = Arc::new(Mutex::new(Vec::new()));
        let registry = EventRegistryBuilder::shared();
        registry
            .lock()
            .unwrap()
            .listen::<TestEvent, _>(PushListener {
                target: target.clone(),
                name: "first",
            });
        registry
            .lock()
            .unwrap()
            .listen::<TestEvent, _>(PushListener {
                target: target.clone(),
                name: "second",
            });

        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        let bus = EventBus::new(app, EventRegistryBuilder::freeze_shared(registry));
        bus.dispatch(TestEvent).await.unwrap();

        assert_eq!(target.lock().unwrap().as_slice(), ["first", "second"]);
    }

    struct OriginListener {
        target: Arc<Mutex<Option<(String, Option<String>, Option<IpAddr>)>>>,
    }

    #[async_trait]
    impl EventListener<TestEvent> for OriginListener {
        async fn handle(&self, context: &EventContext, _event: &TestEvent) -> crate::Result<()> {
            let actor = context
                .actor()
                .map(|actor| actor.id.clone())
                .unwrap_or_default();
            let request_id = context.request_id().map(ToOwned::to_owned);
            let ip = context.ip();
            *self.target.lock().unwrap() = Some((actor, request_id, ip));
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatch_with_origin_exposes_actor_and_request_metadata() {
        let target = Arc::new(Mutex::new(None));
        let registry = EventRegistryBuilder::shared();
        registry
            .lock()
            .unwrap()
            .listen::<TestEvent, _>(OriginListener {
                target: target.clone(),
            });

        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        let bus = EventBus::new(app, EventRegistryBuilder::freeze_shared(registry));
        bus.dispatch_with_origin(
            TestEvent,
            Some(EventOrigin::new(
                Some(Actor::new("admin-1", GuardId::new("admin"))),
                Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5))),
                Some("ForgeTest/1.0".to_string()),
                Some("req-events".to_string()),
            )),
        )
        .await
        .unwrap();

        assert_eq!(
            *target.lock().unwrap(),
            Some((
                "admin-1".to_string(),
                Some("req-events".to_string()),
                Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5))),
            ))
        );
    }
}
