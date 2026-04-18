use async_trait::async_trait;
use forge::prelude::*;

pub struct Entry;

#[async_trait]
impl SeederFile for Entry {
    async fn run(ctx: &SeederContext<'_>) -> Result<()> {
        let count = forge::countries::seed_countries_with(ctx).await?;
        ctx.raw_execute(
            "UPDATE countries SET is_default = true, status = 'enabled', updated_at = NOW() WHERE iso2 = 'MY'",
            &[],
        )
        .await?;

        println!("  seeded {count} countries");
        Ok(())
    }
}
