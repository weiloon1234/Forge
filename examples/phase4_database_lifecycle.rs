use async_trait::async_trait;
use forge::prelude::*;

#[derive(forge::Model)]
#[forge(model = "users")]
struct User {
    id: ModelId<User>,
    email: String,
    created_at: DateTime,
    updated_at: DateTime,
}

#[derive(Clone)]
struct DatabaseLifecycleProvider;

#[async_trait]
impl ServiceProvider for DatabaseLifecycleProvider {
    async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
        forge::register_generated_database!(registrar)?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let _builder = App::builder()
        .load_env()
        .load_config_dir("config")
        .register_provider(DatabaseLifecycleProvider);

    let _ = User::query();

    Ok(())
}
