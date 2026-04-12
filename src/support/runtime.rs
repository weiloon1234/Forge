use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};

use ::redis::AsyncCommands;
use futures_util::StreamExt;
use tokio::sync::{broadcast, mpsc, Mutex, Notify};

use crate::config::ConfigRepository;
use crate::foundation::{Error, Result};
use crate::logging::RuntimeBackendKind;
use crate::redis::namespaced_value;
use crate::support::QueueId;

#[derive(Clone, Debug)]
pub(crate) struct PubSubMessage {
    pub topic: String,
    pub payload: String,
}

pub(crate) struct BackendSubscription {
    receiver: mpsc::UnboundedReceiver<PubSubMessage>,
}

impl BackendSubscription {
    pub async fn recv(&mut self) -> Option<PubSubMessage> {
        self.receiver.recv().await
    }
}

#[derive(Clone)]
pub(crate) enum RuntimeBackend {
    Redis(RedisRuntime),
    Memory(Arc<MemoryRuntime>),
}

impl RuntimeBackend {
    #[cfg(test)]
    pub fn memory(namespace: &str) -> Self {
        Self::Memory(shared_memory_runtime(namespace))
    }

    pub fn from_config(config: &ConfigRepository) -> Result<Self> {
        let redis = config.redis()?;
        let force_memory = std::env::var("FORGE_INTERNAL_RUNTIME_BACKEND")
            .ok()
            .as_deref()
            == Some("memory");

        if force_memory || redis.url.trim().is_empty() {
            return Ok(Self::Memory(shared_memory_runtime(&redis.namespace)));
        }

        Ok(Self::Redis(RedisRuntime {
            client: ::redis::Client::open(redis.url.as_str()).map_err(Error::other)?,
            namespace: redis.namespace,
        }))
    }

    pub fn kind(&self) -> RuntimeBackendKind {
        match self {
            Self::Redis(_) => RuntimeBackendKind::Redis,
            Self::Memory(_) => RuntimeBackendKind::Memory,
        }
    }

    pub async fn ping(&self) -> Result<()> {
        match self {
            Self::Redis(runtime) => runtime.ping().await,
            Self::Memory(_) => Ok(()),
        }
    }

    pub async fn publish_ws(&self, topic: &str, payload: &str) -> Result<()> {
        match self {
            Self::Redis(runtime) => runtime.publish_ws(topic, payload).await,
            Self::Memory(runtime) => runtime.publish_ws(topic, payload).await,
        }
    }

    pub async fn subscribe_ws(&self, topics: &[String]) -> Result<BackendSubscription> {
        match self {
            Self::Redis(runtime) => runtime.subscribe_ws(topics).await,
            Self::Memory(runtime) => runtime.subscribe_ws(topics).await,
        }
    }

    /// Atomically increment a counter and set TTL on first creation.
    ///
    /// The key is automatically prefixed with the app's Redis namespace:
    /// `{namespace}:{key}`.
    ///
    /// Returns the current count after increment. If the key didn't exist
    /// before this call, it is created with value `1` and the given TTL.
    pub async fn incr_with_ttl(&self, key: &str, ttl_secs: u64) -> Result<u64> {
        match self {
            Self::Redis(runtime) => {
                let full_key = runtime.namespaced_key(key);
                let mut conn = runtime
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(Error::other)?;
                let count: i64 = ::redis::cmd("INCR")
                    .arg(&full_key)
                    .query_async(&mut conn)
                    .await
                    .map_err(Error::other)?;
                if count == 1 {
                    let _: () = ::redis::cmd("EXPIRE")
                        .arg(&full_key)
                        .arg(ttl_secs as i64)
                        .query_async(&mut conn)
                        .await
                        .map_err(Error::other)?;
                }
                Ok(count as u64)
            }
            Self::Memory(_) => Err(Error::other(anyhow::anyhow!(
                "memory backend does not support incr_with_ttl"
            ))),
        }
    }
}

#[derive(Clone)]
pub(crate) struct RedisRuntime {
    pub(crate) client: ::redis::Client,
    pub(crate) namespace: String,
}

impl RedisRuntime {
    fn websocket_topic(&self, topic: &str) -> String {
        namespaced_value(&self.namespace, &format!("ws:{topic}"))
    }

    /// Build a namespaced key: `{namespace}:{suffix}`.
    fn namespaced_key(&self, suffix: &str) -> String {
        namespaced_value(&self.namespace, suffix)
    }

    async fn ping(&self) -> Result<()> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(Error::other)?;
        let _: String = ::redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(Error::other)?;
        Ok(())
    }

    async fn publish_ws(&self, topic: &str, payload: &str) -> Result<()> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(Error::other)?;
        let redis_topic = self.websocket_topic(topic);
        let _: () = conn
            .publish(redis_topic, payload)
            .await
            .map_err(Error::other)?;
        Ok(())
    }

    async fn subscribe_ws(&self, topics: &[String]) -> Result<BackendSubscription> {
        let (tx, rx) = mpsc::unbounded_channel();
        if topics.is_empty() {
            return Ok(BackendSubscription { receiver: rx });
        }

        let mut pubsub = self.client.get_async_pubsub().await.map_err(Error::other)?;
        for topic in topics {
            pubsub
                .subscribe(self.websocket_topic(topic))
                .await
                .map_err(Error::other)?;
        }

        let mut stream = pubsub.into_on_message();
        tokio::spawn(async move {
            while let Some(message) = stream.next().await {
                let payload = match message.get_payload::<String>() {
                    Ok(payload) => payload,
                    Err(_) => continue,
                };
                let raw_topic = message.get_channel_name().to_string();
                let topic = raw_topic
                    .rsplit(':')
                    .next()
                    .map(ToOwned::to_owned)
                    .unwrap_or(raw_topic);
                let _ = tx.send(PubSubMessage { topic, payload });
            }
        });

        Ok(BackendSubscription { receiver: rx })
    }
}

pub(crate) struct MemoryRuntime {
    pub(crate) ws_tx: broadcast::Sender<PubSubMessage>,
    pub(crate) ready_queues: Mutex<HashMap<QueueId, VecDeque<String>>>,
    pub(crate) scheduled_jobs: Mutex<HashMap<QueueId, Vec<ScheduledJobToken>>>,
    pub(crate) leased_jobs: Mutex<HashMap<QueueId, Vec<LeasedJobToken>>>,
    pub(crate) payloads: Mutex<HashMap<String, String>>,
    pub(crate) dead_letters: Mutex<HashMap<QueueId, Vec<String>>>,
    pub(crate) scheduler_leader: Mutex<Option<LeadershipLease>>,
    pub(crate) notify: Notify,
}

#[derive(Clone)]
pub(crate) struct ScheduledJobToken {
    pub run_at_millis: i64,
    pub token: String,
}

#[derive(Clone)]
pub(crate) struct LeasedJobToken {
    pub expires_at_millis: i64,
    pub token: String,
}

#[derive(Clone)]
pub(crate) struct LeadershipLease {
    pub owner_id: String,
    pub expires_at_millis: i64,
}

impl MemoryRuntime {
    fn new() -> Self {
        let (ws_tx, _) = broadcast::channel(1024);
        Self {
            ws_tx,
            ready_queues: Mutex::new(HashMap::new()),
            scheduled_jobs: Mutex::new(HashMap::new()),
            leased_jobs: Mutex::new(HashMap::new()),
            payloads: Mutex::new(HashMap::new()),
            dead_letters: Mutex::new(HashMap::new()),
            scheduler_leader: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    async fn publish_ws(&self, topic: &str, payload: &str) -> Result<()> {
        let _ = self.ws_tx.send(PubSubMessage {
            topic: topic.to_string(),
            payload: payload.to_string(),
        });
        Ok(())
    }

    async fn subscribe_ws(&self, topics: &[String]) -> Result<BackendSubscription> {
        let topics = topics.to_vec();
        let mut receiver = self.ws_tx.subscribe();
        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(message) => {
                        if topics.iter().any(|topic| topic == &message.topic) {
                            let _ = tx.send(message);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(BackendSubscription { receiver: rx })
    }
}

fn shared_memory_runtime(namespace: &str) -> Arc<MemoryRuntime> {
    static REGISTRY: OnceLock<StdMutex<HashMap<String, Weak<MemoryRuntime>>>> = OnceLock::new();

    let registry = REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut registry = registry.lock().expect("runtime registry lock poisoned");

    if let Some(existing) = registry.get(namespace).and_then(Weak::upgrade) {
        return existing;
    }

    let runtime = Arc::new(MemoryRuntime::new());
    registry.insert(namespace.to_string(), Arc::downgrade(&runtime));
    runtime
}
