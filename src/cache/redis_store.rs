use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::foundation::Result;
use crate::redis::RedisManager;

use super::CacheStore;

pub struct RedisCacheStore {
    redis: Arc<RedisManager>,
    prefix: String,
}

impl RedisCacheStore {
    pub fn new(redis: Arc<RedisManager>, prefix: String) -> Self {
        Self { redis, prefix }
    }
}

#[async_trait]
impl CacheStore for RedisCacheStore {
    async fn get_raw(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.redis.connection().await?;
        let redis_key = self.redis.key(format!("{}{}", self.prefix, key));
        match conn.get::<String>(&redis_key).await {
            Ok(value) if value.is_empty() => Ok(None),
            Ok(value) => Ok(Some(value)),
            Err(_) => Ok(None),
        }
    }

    async fn put_raw(&self, key: &str, value: &str, ttl: Duration) -> Result<()> {
        let mut conn = self.redis.connection().await?;
        let redis_key = self.redis.key(format!("{}{}", self.prefix, key));
        conn.set_ex(&redis_key, value, ttl.as_secs()).await
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        let mut conn = self.redis.connection().await?;
        let redis_key = self.redis.key(format!("{}{}", self.prefix, key));
        let deleted = conn.del(&redis_key).await?;
        Ok(deleted > 0)
    }

    async fn flush(&self) -> Result<()> {
        Err(crate::foundation::Error::message(
            "cache flush is not supported on Redis store; use specific forget() calls",
        ))
    }
}
