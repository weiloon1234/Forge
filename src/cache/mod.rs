mod memory;
mod redis_store;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};

use crate::foundation::{Error, Result};
use crate::logging::{catch_async_panic, panic_payload_message};

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
        let value = run_cache_remember_callback(key, f).await?;
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

async fn run_cache_remember_callback<T, F, Fut>(key: &str, callback: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    match catch_async_panic(callback).await {
        Ok(result) => result,
        Err(panic) => Err(cache_remember_panic_error(key, panic)),
    }
}

fn cache_remember_panic_error(key: &str, panic: Box<dyn std::any::Any + Send>) -> Error {
    let message = panic_payload_message(panic);
    tracing::error!(
        target: "forge.cache",
        key = key,
        panic = %message,
        "cache remember callback panicked"
    );
    Error::message(format!("cache remember callback panicked: {message}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{CacheManager, MemoryCacheStore};
    use crate::foundation::Error;

    fn manager() -> CacheManager {
        CacheManager::new(Arc::new(MemoryCacheStore::new(100)))
    }

    #[tokio::test]
    async fn remember_computes_and_stores_missing_value() {
        let cache = manager();

        let value = cache
            .remember("remember.success", Duration::from_secs(60), || async {
                Ok::<_, Error>("computed".to_string())
            })
            .await
            .unwrap();

        assert_eq!(value, "computed");
        assert_eq!(
            cache.get::<String>("remember.success").await.unwrap(),
            Some("computed".to_string())
        );
    }

    #[tokio::test]
    async fn remember_cache_hit_skips_callback() {
        let cache = manager();
        cache
            .put(
                "remember.hit",
                &"cached".to_string(),
                Duration::from_secs(60),
            )
            .await
            .unwrap();

        let value = cache
            .remember("remember.hit", Duration::from_secs(60), || async {
                panic!("remember callback should not run");
                #[allow(unreachable_code)]
                Ok::<_, Error>("computed".to_string())
            })
            .await
            .unwrap();

        assert_eq!(value, "cached");
    }

    #[tokio::test]
    async fn remember_callback_error_remains_unchanged() {
        let cache = manager();

        let error = cache
            .remember("remember.error", Duration::from_secs(60), || async {
                Err::<String, _>(Error::message("compute failed"))
            })
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "compute failed");
        assert!(cache
            .get::<String>("remember.error")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn remember_factory_panic_becomes_error() {
        let cache = manager();

        let error = cache
            .remember(
                "remember.factory-panic",
                Duration::from_secs(60),
                || -> std::future::Ready<crate::Result<String>> {
                    panic!("remember factory explode")
                },
            )
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "cache remember callback panicked: remember factory explode"
        );
        assert!(cache
            .get::<String>("remember.factory-panic")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn remember_future_panic_becomes_error() {
        let cache = manager();

        let error = cache
            .remember("remember.future-panic", Duration::from_secs(60), || async {
                panic!("remember future explode");
                #[allow(unreachable_code)]
                Ok::<_, Error>("computed".to_string())
            })
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "cache remember callback panicked: remember future explode"
        );
        assert!(cache
            .get::<String>("remember.future-panic")
            .await
            .unwrap()
            .is_none());
    }
}
