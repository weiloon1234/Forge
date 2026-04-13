# Caching & Redis Guide

Forge provides a cache abstraction backed by Redis or in-memory storage, and a namespaced Redis client for direct key-value operations.

---

## Cache

### Quick Start

```rust
let cache = app.cache()?;

// Store a value (TTL: 1 hour)
cache.put("user:123", &user, Duration::from_secs(3600)).await?;

// Retrieve
let user: Option<User> = cache.get("user:123").await?;

// Get or compute (cache-aside pattern)
let user = cache.remember("user:123", Duration::from_secs(3600), || async {
    User::model_query().where_col(User::ID, "123").first(&*db).await
}).await?;
```

### Config

```toml
# config/cache.toml
[cache]
driver = "redis"       # "redis" or "memory"
prefix = "cache:"      # key prefix
ttl_seconds = 3600     # default TTL (used by framework, not enforced on put())
max_entries = 10000    # memory driver only — evicts oldest when full
```

### Methods

```rust
let cache = app.cache()?;

// Get — returns None if missing or expired
let value: Option<User> = cache.get("key").await?;

// Put — stores with explicit TTL
cache.put("key", &value, Duration::from_secs(300)).await?;

// Remember — get from cache, or compute + store if missing
let value = cache.remember("key", Duration::from_secs(3600), || async {
    expensive_computation().await
}).await?;

// Forget — remove a key
cache.forget("key").await?;

// Flush — clear entire cache
cache.flush().await?;
```

All values are serialized as JSON. Any type implementing `Serialize + DeserializeOwned` works.

### Cache-Aside Pattern

The most common pattern — avoid repeated expensive queries:

```rust
async fn get_dashboard_stats(app: &AppContext) -> Result<DashboardStats> {
    let cache = app.cache()?;

    cache.remember("dashboard:stats", Duration::from_secs(60), || async {
        let db = app.database()?;
        let total_users = User::model_query().count(&*db).await?;
        let total_orders = Order::model_query().count(&*db).await?;
        Ok(DashboardStats { total_users, total_orders })
    }).await
}
```

### Cache Invalidation

```rust
// After creating a user, invalidate the cached stats
User::model_create()
    .set(User::EMAIL, &email)
    .execute(&*db).await?;

app.cache()?.forget("dashboard:stats").await?;
```

---

## Redis

For operations beyond caching — counters, pub/sub, sets, hashes, distributed state.

### Namespacing

All keys are automatically prefixed with your app namespace:

```
App name: "my-app", Environment: "production"
Namespace: "my-app:production"

redis.key("user:123")  →  full key: "my-app:production:user:123"
redis.key("count")     →  full key: "my-app:production:count"
```

This prevents key collisions when multiple apps share the same Redis server.

### Basic Operations

```rust
let redis = app.redis()?;
let mut conn = redis.connection().await?;

// String get/set
let key = redis.key("user:123:name");
conn.set(&key, "Alice").await?;
let name: String = conn.get(&key).await?;

// Set with expiry (seconds)
conn.set_ex(&key, "Alice", 3600).await?;

// Delete
conn.del(&key).await?;

// Check existence
if conn.exists(&key).await? {
    // key exists
}

// Set expiry on existing key
conn.expire(&key, 300).await?;

// Atomic increment
let count: i64 = conn.incr(&redis.key("page:views")).await?;
```

### Hash Operations

```rust
let key = redis.key("user:123");

conn.hset(&key, "email", "alice@example.com").await?;
conn.hset(&key, "name", "Alice").await?;

let email: String = conn.hget(&key, "email").await?;
```

### Set Operations

```rust
let key = redis.key("post:123:likes");

conn.sadd(&key, "user-1").await?;
conn.sadd(&key, "user-2").await?;
conn.srem(&key, "user-1").await?;

let members: Vec<String> = conn.smembers(&key).await?;
```

### Pub/Sub

```rust
let channel = redis.channel("events:new-order");
conn.publish(&channel, serde_json::json!({ "order_id": "ORD-123" }).to_string()).await?;
```

### Cross-App Access

When one app needs to read another app's keys:

```rust
let redis = app.redis()?;

// Read from another app's namespace
let foreign_key = redis.key_in_namespace("analytics:production", "daily:visitors");
let mut conn = redis.connection().await?;
let visitors: i64 = conn.get(&foreign_key).await?;

// Subscribe to another app's channel
let foreign_channel = redis.channel_in_namespace("payments:production", "events");
conn.publish(&foreign_channel, "ping").await?;
```

### Config

```toml
# config/redis.toml
[redis]
url = "redis://127.0.0.1/"
# namespace = "my-app"    # auto-derived from app.name:app.environment if not set
```

---

## Cache vs Redis — When to Use Which

| Use case | Use `cache` | Use `redis` |
|----------|------------|-------------|
| Cache a DB query result | x | |
| Cache an API response | x | |
| Rate limiting counter | | x |
| Page view counter | | x |
| User session data | | x |
| Pub/sub messaging | | x |
| Leaderboard (sorted sets) | | x |
| Feature flags | x | |
| Distributed lock | | (use `app.lock()`) |

`cache` is a high-level abstraction (get/put/remember with serialization). `redis` is the low-level client (any Redis command). Cache uses Redis as its backend when configured.
