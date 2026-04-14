# settings

[Back to index](../index.md)

## forge::settings

```rust
struct Setting
  async fn get(app: &AppContext, key: &str) -> Result<Option<Value>>
  async fn set(app: &AppContext, key: &str, value: Value) -> Result<()>
  async fn get_as<T: DeserializeOwned>( app: &AppContext, key: &str, ) -> Result<Option<T>>
  async fn get_or( app: &AppContext, key: &str, default: Value, ) -> Result<Value>
  async fn remove(app: &AppContext, key: &str) -> Result<bool>
  async fn exists(app: &AppContext, key: &str) -> Result<bool>
  async fn all(app: &AppContext) -> Result<Vec<Setting>>
  async fn by_prefix(app: &AppContext, prefix: &str) -> Result<Vec<Setting>>
```

