mod memory;
mod redis_store;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};

use crate::foundation::{Error, Result};

pub use memory::MemoryCacheStore;
pub use redis_store::RedisCacheStore;

/// Trait for cache store backends.
#[async_trait]
pub trait CacheStore: Send + Sync + 'static {
    async fn get_raw(&self, key: &str) -> Result<Option<String>>;
    async fn put_raw(&self, key: &str, value: &str, ttl: Duration) -> Result<()>;
    async fn forget(&self, key: &str) -> Result<bool>;
    async fn flush(&self) -> Result<()>;
}

/// Framework cache manager, accessible via `app.cache()`.
pub struct CacheManager {
    store: Arc<dyn CacheStore>,
}

impl CacheManager {
    pub(crate) fn new(store: Arc<dyn CacheStore>) -> Self {
        Self { store }
    }

    /// Get a value from cache. Returns None if not found or expired.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.store.get_raw(key).await? {
            Some(raw) => Ok(Some(serde_json::from_str(&raw).map_err(Error::other)?)),
            None => Ok(None),
        }
    }

    /// Store a value in cache with a TTL.
    pub async fn put<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) -> Result<()> {
        let raw = serde_json::to_string(value).map_err(Error::other)?;
        self.store.put_raw(key, &raw, ttl).await
    }

    /// Get from cache, or compute + store with TTL.
    pub async fn remember<T, F, Fut>(&self, key: &str, ttl: Duration, f: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        if let Some(cached) = self.get::<T>(key).await? {
            return Ok(cached);
        }
        let value = f().await?;
        self.put(key, &value, ttl).await?;
        Ok(value)
    }

    /// Remove a value from cache.
    pub async fn forget(&self, key: &str) -> Result<bool> {
        self.store.forget(key).await
    }

    /// Clear all cached values.
    pub async fn flush(&self) -> Result<()> {
        self.store.flush().await
    }
}
