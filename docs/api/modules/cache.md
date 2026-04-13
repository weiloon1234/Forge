# cache

In-memory and Redis-backed caching (CacheManager)

[Back to index](../index.md)

## forge::cache

```rust
struct CacheManager
  async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>>
  async fn put<T: Serialize>( &self, key: &str, value: &T, ttl: Duration, ) -> Result<()>
  async fn remember<T, F, Fut>( &self, key: &str, ttl: Duration, f: F, ) -> Result<T>
  async fn forget(&self, key: &str) -> Result<bool>
  async fn flush(&self) -> Result<()>
struct MemoryCacheStore
  fn new(max_entries: usize) -> Self
struct RedisCacheStore
  fn new(redis: Arc<RedisManager>, prefix: String) -> Self
trait CacheStore
  fn get_raw<'life0, 'life1, 'async_trait>(
  fn put_raw<'life0, 'life1, 'life2, 'async_trait>(
  fn forget<'life0, 'life1, 'async_trait>(
  fn flush<'life0, 'async_trait>(
```

