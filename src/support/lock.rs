use std::sync::Arc;
use std::time::Duration;

use crate::foundation::{Error, Result};
use crate::support::runtime::RuntimeBackend;

/// Distributed lock for coordinating concurrent workers.
///
/// ```ignore
/// if let Some(guard) = app.lock()?.acquire("payment:123", Duration::from_secs(30)).await? {
///     process_payment(123).await?;
///     // auto-releases on drop
/// }
/// ```
pub struct DistributedLock {
    backend: Arc<RuntimeBackend>,
}

impl DistributedLock {
    pub(crate) fn new(backend: Arc<RuntimeBackend>) -> Self {
        Self { backend }
    }

    /// Try to acquire a lock. Returns `Some(guard)` if acquired, `None` if already held.
    pub async fn acquire(&self, key: &str, ttl: Duration) -> Result<Option<LockGuard>> {
        let lock_key = format!("lock:{key}");
        let owner = uuid::Uuid::now_v7().to_string();
        let ttl_secs = ttl.as_secs().max(1);

        let acquired = self
            .backend
            .set_nx_value(&lock_key, &owner, ttl_secs)
            .await?;
        if acquired {
            Ok(Some(LockGuard {
                backend: self.backend.clone(),
                key: lock_key,
                owner,
            }))
        } else {
            Ok(None)
        }
    }

    /// Block until a lock is acquired, with a timeout.
    pub async fn block(
        &self,
        key: &str,
        ttl: Duration,
        wait_timeout: Duration,
    ) -> Result<LockGuard> {
        let deadline = tokio::time::Instant::now() + wait_timeout;
        loop {
            if let Some(guard) = self.acquire(key, ttl).await? {
                return Ok(guard);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::message(format!(
                    "failed to acquire lock '{key}' within {}ms",
                    wait_timeout.as_millis()
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Guard that releases the distributed lock on drop.
pub struct LockGuard {
    backend: Arc<RuntimeBackend>,
    key: String,
    owner: String,
}

impl LockGuard {
    /// Explicitly release the lock (instead of waiting for drop).
    pub async fn release(self) -> Result<bool> {
        self.backend.del_if_value(&self.key, &self.owner).await
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let backend = self.backend.clone();
        let key = std::mem::take(&mut self.key);
        let owner = std::mem::take(&mut self.owner);
        if !key.is_empty() {
            // Use try_current to avoid panic if the runtime is shutting down
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = backend.del_if_value(&key, &owner).await;
                });
            }
        }
    }
}
