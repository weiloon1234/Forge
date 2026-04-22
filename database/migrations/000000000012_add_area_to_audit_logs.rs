use async_trait::async_trait;
use forge::prelude::*;

pub struct Entry;

#[async_trait]
impl MigrationFile for Entry {
    async fn up(ctx: &MigrationContext<'_>) -> Result<()> {
        ctx.raw_execute(
            "ALTER TABLE audit_logs ADD COLUMN IF NOT EXISTS area TEXT",
            &[],
        )
        .await?;

        ctx.raw_execute(
            "CREATE INDEX IF NOT EXISTS idx_audit_logs_area_created_at ON audit_logs (area, created_at)",
            &[],
        )
        .await?;

        Ok(())
    }

    async fn down(ctx: &MigrationContext<'_>) -> Result<()> {
        ctx.raw_execute("DROP INDEX IF EXISTS idx_audit_logs_area_created_at", &[])
            .await?;
        ctx.raw_execute("ALTER TABLE audit_logs DROP COLUMN IF EXISTS area", &[])
            .await?;
        Ok(())
    }
}
