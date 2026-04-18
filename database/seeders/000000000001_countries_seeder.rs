use async_trait::async_trait;
use forge::prelude::*;

pub struct Entry;

#[async_trait]
impl SeederFile for Entry {
    async fn run(ctx: &SeederContext<'_>) -> Result<()> {
        let count = forge::countries::seed_countries_with(ctx).await?;

        println!("  seeded {count} countries");
        Ok(())
    }
}
