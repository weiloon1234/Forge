# countries

Built-in country data (250 countries)

[Back to index](../index.md)

## forge::countries

```rust
struct Country
  async fn find(app: &AppContext, iso2: &str) -> Result<Option<Country>>
  async fn all(app: &AppContext) -> Result<Vec<Country>>
  async fn by_status(app: &AppContext, status: &str) -> Result<Vec<Country>>
  async fn enabled(app: &AppContext) -> Result<Vec<Country>>
  async fn exists(app: &AppContext, iso2: &str) -> Result<bool>
struct CountryCurrency
struct CountrySeed
fn load_seed() -> Result<Vec<CountrySeed>>
fn async fn seed_countries(app: &AppContext) -> Result<u64>
```

