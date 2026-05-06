use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::IpAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::FutureExt;
use serde::Serialize;

use crate::auth::Actor;
use crate::foundation::{AppContext, Error, Result};
use crate::jobs::Job;
use crate::logging::panic_payload_message;
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
        match AssertUnwindSafe(self.listener.handle(context, event))
            .catch_unwind()
            .await
        {
            Ok(result) => result,
            Err(panic) => {
                let message = panic_payload_message(panic);
                tracing::error!(
                    event = %E::ID,
                    panic = %message,
                    "Event listener panicked"
                );
                Err(Error::message(format!(
                    "event listener panicked: {message}"
                )))
            }
        }
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
        let job = run_event_mapper("event dispatch_job mapper", event, |event| {
            (self.mapper)(event)
        })?;
        context.app().jobs()?.dispatch(job).await
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
        let message = run_event_mapper("event publish_websocket mapper", event, |event| {
            (self.mapper)(event)
        })?;
        context.app().websocket()?.publish_message(message).await
    }
}

fn run_event_mapper<E, T>(
    subject: &'static str,
    event: &E,
    mapper: impl FnOnce(&E) -> T,
) -> Result<T>
where
    E: Event,
{
    match catch_unwind(AssertUnwindSafe(|| mapper(event))) {
        Ok(value) => Ok(value),
        Err(panic) => Err(event_mapper_panic_error::<E>(subject, panic)),
    }
}

fn event_mapper_panic_error<E: Event>(
    subject: &'static str,
    panic: Box<dyn std::any::Any + Send>,
) -> Error {
    let message = panic_payload_message(panic);
    tracing::error!(
        target: "forge.events",
        event = %E::ID,
        subject = subject,
        panic = %message,
        "Event helper mapper panicked"
    );
    Error::message(format!("{subject} panicked: {message}"))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{Event, EventBus, EventContext, EventListener, EventOrigin, EventRegistryBuilder};
    use crate::auth::Actor;
    use crate::config::ConfigRepository;
    use crate::foundation::{AppContext, Container};
    use crate::jobs::{Job, JobContext};
    use crate::support::GuardId;
    use crate::support::{ChannelEventId, ChannelId, EventId, JobId};
    use crate::validation::RuleRegistry;

    type OriginSnapshot = Option<(String, Option<String>, Option<IpAddr>)>;

    #[derive(Clone, serde::Serialize)]
    struct TestEvent;

    impl Event for TestEvent {
        const ID: EventId = EventId::new("test.event");
    }

    #[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
    struct TestJob;

    #[async_trait]
    impl Job for TestJob {
        const ID: JobId = JobId::new("test.job");

        async fn handle(&self, _context: JobContext) -> crate::Result<()> {
            Ok(())
        }
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

    struct PanicListener {
        target: Arc<Mutex<Vec<&'static str>>>,
        name: &'static str,
    }

    #[async_trait]
    impl EventListener<TestEvent> for PanicListener {
        async fn handle(&self, _context: &EventContext, _event: &TestEvent) -> crate::Result<()> {
            self.target.lock().unwrap().push(self.name);
            panic!("listener explode")
        }
    }

    struct PanicOnceListener {
        panicked: Arc<AtomicBool>,
        target: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl EventListener<TestEvent> for PanicOnceListener {
        async fn handle(&self, _context: &EventContext, _event: &TestEvent) -> crate::Result<()> {
            if !self.panicked.swap(true, Ordering::SeqCst) {
                panic!("one-time listener explode");
            }

            self.target.lock().unwrap().push("recovered");
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

    #[tokio::test]
    async fn listener_panic_becomes_dispatch_error_and_stops_later_listeners() {
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
            .listen::<TestEvent, _>(PanicListener {
                target: target.clone(),
                name: "panic",
            });
        registry
            .lock()
            .unwrap()
            .listen::<TestEvent, _>(PushListener {
                target: target.clone(),
                name: "after",
            });

        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        let bus = EventBus::new(app, EventRegistryBuilder::freeze_shared(registry));
        let error = bus.dispatch(TestEvent).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "event listener panicked: listener explode"
        );
        assert_eq!(target.lock().unwrap().as_slice(), ["first", "panic"]);
    }

    #[tokio::test]
    async fn bus_remains_healthy_after_caught_listener_panic() {
        let panicked = Arc::new(AtomicBool::new(false));
        let target = Arc::new(Mutex::new(Vec::new()));
        let registry = EventRegistryBuilder::shared();
        registry
            .lock()
            .unwrap()
            .listen::<TestEvent, _>(PanicOnceListener {
                panicked,
                target: target.clone(),
            });

        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        let bus = EventBus::new(app, EventRegistryBuilder::freeze_shared(registry));

        let error = bus.dispatch(TestEvent).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "event listener panicked: one-time listener explode"
        );

        bus.dispatch(TestEvent).await.unwrap();
        assert_eq!(target.lock().unwrap().as_slice(), ["recovered"]);
    }

    #[tokio::test]
    async fn dispatch_job_mapper_panic_becomes_helper_error() {
        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        let context = EventContext::new(app, None);
        let listener = super::dispatch_job::<TestEvent, TestJob, _>(|_| {
            panic!("job mapper explode");
        });

        let error = listener.handle(&context, &TestEvent).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "event dispatch_job mapper panicked: job mapper explode"
        );
    }

    #[tokio::test]
    async fn publish_websocket_mapper_panic_becomes_helper_error() {
        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        let context = EventContext::new(app, None);
        let listener = super::publish_websocket::<TestEvent, _>(|_| {
            panic!("websocket mapper explode");
            #[allow(unreachable_code)]
            crate::websocket::ServerMessage {
                channel: ChannelId::new("chat"),
                event: ChannelEventId::new("created"),
                room: None,
                payload: serde_json::Value::Null,
            }
        });

        let error = listener.handle(&context, &TestEvent).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "event publish_websocket mapper panicked: websocket mapper explode"
        );
    }

    struct OriginListener {
        target: Arc<Mutex<OriginSnapshot>>,
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
