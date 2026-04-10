use async_trait::async_trait;
use forge::prelude::*;

pub struct Entry;

#[async_trait]
impl SeederFile for Entry {
    async fn run(ctx: &SeederContext<'_>) -> Result<()> {
        ctx.raw_execute(
            r#"
            INSERT INTO users (id, email)
            VALUES (1, 'forge@example.com')
            ON CONFLICT (id) DO NOTHING
            "#,
            &[],
        )
        .await?;
        Ok(())
    }
}
