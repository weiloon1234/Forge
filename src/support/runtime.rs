use std::collections::{HashMap, HashSet, VecDeque};
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
            Self::Memory(runtime) => {
                let now = std::time::Instant::now();
                let ttl = std::time::Duration::from_secs(ttl_secs);
                let mut counters = runtime.counters.lock().await;
                let entry = counters.entry(key.to_string()).or_insert(MemoryCounter {
                    count: 0,
                    expires_at: now + ttl,
                });
                // Reset if expired
                if now >= entry.expires_at {
                    entry.count = 0;
                    entry.expires_at = now + ttl;
                }
                entry.count += 1;
                Ok(entry.count)
            }
        }
    }

    /// Add a member to a set.
    pub async fn sadd(&self, key: &str, member: &str) -> Result<()> {
        match self {
            Self::Redis(runtime) => {
                let full_key = runtime.namespaced_key(key);
                let mut conn = runtime
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(Error::other)?;
                let _: () = conn.sadd(full_key, member).await.map_err(Error::other)?;
                Ok(())
            }
            Self::Memory(runtime) => {
                let mut sets = runtime.sets.lock().await;
                sets.entry(key.to_string())
                    .or_default()
                    .insert(member.to_string());
                Ok(())
            }
        }
    }

    /// Remove a member from a set.
    pub async fn srem(&self, key: &str, member: &str) -> Result<()> {
        match self {
            Self::Redis(runtime) => {
                let full_key = runtime.namespaced_key(key);
                let mut conn = runtime
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(Error::other)?;
                let _: () = conn.srem(full_key, member).await.map_err(Error::other)?;
                Ok(())
            }
            Self::Memory(runtime) => {
                let mut sets = runtime.sets.lock().await;
                if let Some(set) = sets.get_mut(key) {
                    set.remove(member);
                    if set.is_empty() {
                        sets.remove(key);
                    }
                }
                Ok(())
            }
        }
    }

    /// Return all members of a set.
    pub async fn smembers(&self, key: &str) -> Result<Vec<String>> {
        match self {
            Self::Redis(runtime) => {
                let full_key = runtime.namespaced_key(key);
                let mut conn = runtime
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(Error::other)?;
                let members: Vec<String> =
                    conn.smembers(full_key).await.map_err(Error::other)?;
                Ok(members)
            }
            Self::Memory(runtime) => {
                let sets = runtime.sets.lock().await;
                Ok(sets
                    .get(key)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default())
            }
        }
    }

    /// Return the number of members in a set.
    pub async fn scard(&self, key: &str) -> Result<usize> {
        match self {
            Self::Redis(runtime) => {
                let full_key = runtime.namespaced_key(key);
                let mut conn = runtime
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(Error::other)?;
                let count: usize = conn.scard(full_key).await.map_err(Error::other)?;
                Ok(count)
            }
            Self::Memory(runtime) => {
                let sets = runtime.sets.lock().await;
                Ok(sets.get(key).map(|s| s.len()).unwrap_or(0))
            }
        }
    }

    /// Set a key only if it does not already exist, with a TTL.
    ///
    /// Returns `true` if the key was set (did not exist), `false` if
    /// it already existed (duplicate).
    pub async fn set_if_absent(&self, key: &str, ttl_secs: u64) -> Result<bool> {
        match self {
            Self::Redis(runtime) => {
                let full_key = runtime.namespaced_key(key);
                let mut conn = runtime
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(Error::other)?;
                // SET key 1 NX EX ttl — returns OK if set, nil if already exists
                let result: Option<String> = ::redis::cmd("SET")
                    .arg(&full_key)
                    .arg(1)
                    .arg("NX")
                    .arg("EX")
                    .arg(ttl_secs as i64)
                    .query_async(&mut conn)
                    .await
                    .map_err(Error::other)?;
                Ok(result.is_some())
            }
            Self::Memory(runtime) => {
                let now = std::time::Instant::now();
                let ttl = std::time::Duration::from_secs(ttl_secs);
                let mut unique_keys = runtime.unique_keys.lock().await;
                // Evict expired entry
                if let Some(expires_at) = unique_keys.get(key) {
                    if now >= *expires_at {
                        unique_keys.remove(key);
                    }
                }
                if unique_keys.contains_key(key) {
                    Ok(false)
                } else {
                    unique_keys.insert(key.to_string(), now + ttl);
                    Ok(true)
                }
            }
        }
    }

    /// Push a value to the head of a list and trim to a maximum length (circular buffer).
    ///
    /// Equivalent to `LPUSH key value` followed by `LTRIM key 0 (max_len - 1)`.
    pub async fn lpush_capped(&self, key: &str, value: &str, max_len: usize) -> Result<()> {
        match self {
            Self::Redis(runtime) => {
                let full_key = runtime.namespaced_key(key);
                let mut conn = runtime
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(Error::other)?;
                let _: () = conn.lpush(&full_key, value).await.map_err(Error::other)?;
                let _: () = conn
                    .ltrim(&full_key, 0, max_len as isize - 1)
                    .await
                    .map_err(Error::other)?;
                Ok(())
            }
            Self::Memory(runtime) => {
                let mut lists = runtime.lists.lock().await;
                let list = lists.entry(key.to_string()).or_default();
                list.push_front(value.to_string());
                while list.len() > max_len {
                    list.pop_back();
                }
                Ok(())
            }
        }
    }

    /// Return a range of elements from a list.
    ///
    /// Equivalent to `LRANGE key start stop`.
    pub async fn lrange(&self, key: &str, start: i64, stop: i64) -> Result<Vec<String>> {
        match self {
            Self::Redis(runtime) => {
                let full_key = runtime.namespaced_key(key);
                let mut conn = runtime
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(Error::other)?;
                let values: Vec<String> = conn
                    .lrange(&full_key, start as isize, stop as isize)
                    .await
                    .map_err(Error::other)?;
                Ok(values)
            }
            Self::Memory(runtime) => {
                let lists = runtime.lists.lock().await;
                let Some(list) = lists.get(key) else {
                    return Ok(Vec::new());
                };
                let len = list.len() as i64;
                // Normalize negative indices like Redis does.
                let s = if start < 0 {
                    (len + start).max(0) as usize
                } else {
                    start as usize
                };
                let e = if stop < 0 {
                    (len + stop).max(0) as usize
                } else {
                    stop as usize
                };
                if s > e || s >= list.len() {
                    return Ok(Vec::new());
                }
                let end = e.min(list.len() - 1);
                Ok(list.iter().skip(s).take(end - s + 1).cloned().collect())
            }
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

/// Batch metadata stored in the memory backend.
#[derive(Clone, Debug)]
pub(crate) struct MemoryBatchMeta {
    pub total: u64,
    pub completed: u64,
    pub on_complete_job: Option<String>,
    pub on_complete_queue: Option<String>,
}

pub(crate) struct MemoryRuntime {
    pub(crate) ws_tx: broadcast::Sender<PubSubMessage>,
    pub(crate) ready_queues: Mutex<HashMap<QueueId, VecDeque<String>>>,
    pub(crate) scheduled_jobs: Mutex<HashMap<QueueId, Vec<ScheduledJobToken>>>,
    pub(crate) leased_jobs: Mutex<HashMap<QueueId, Vec<LeasedJobToken>>>,
    pub(crate) payloads: Mutex<HashMap<String, String>>,
    pub(crate) dead_letters: Mutex<HashMap<QueueId, Vec<String>>>,
    pub(crate) scheduler_leader: Mutex<Option<LeadershipLease>>,
    pub(crate) batches: Mutex<HashMap<String, MemoryBatchMeta>>,
    pub(crate) counters: Mutex<HashMap<String, MemoryCounter>>,
    pub(crate) unique_keys: Mutex<HashMap<String, std::time::Instant>>,
    pub(crate) sets: Mutex<HashMap<String, HashSet<String>>>,
    pub(crate) lists: Mutex<HashMap<String, VecDeque<String>>>,
    pub(crate) notify: Notify,
}

/// In-memory counter with TTL-based expiration.
pub(crate) struct MemoryCounter {
    pub count: u64,
    pub expires_at: std::time::Instant,
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
            batches: Mutex::new(HashMap::new()),
            counters: Mutex::new(HashMap::new()),
            unique_keys: Mutex::new(HashMap::new()),
            sets: Mutex::new(HashMap::new()),
            lists: Mutex::new(HashMap::new()),
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
