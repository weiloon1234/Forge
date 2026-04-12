use serde::{de::DeserializeOwned, Serialize};

use crate::database::DbValue;
use crate::foundation::{AppContext, Error, Result};

/// A metadata record — polymorphic key-value store.
#[derive(Clone, Debug)]
pub struct ModelMeta {
    pub id: String,
    pub metadatable_type: String,
    pub metadatable_id: String,
    pub key: String,
    pub value: Option<serde_json::Value>,
}

/// Trait for models that can have arbitrary key-value metadata.
///
/// ```ignore
/// impl HasMetadata for User {
///     fn metadatable_type() -> &'static str { "users" }
///     fn metadatable_id(&self) -> String { self.id.to_string() }
/// }
///
/// user.set_meta(&app, "theme", json!("dark")).await?;
/// let theme: String = user.get_meta(&app, "theme").await?.unwrap();
/// ```
#[async_trait::async_trait]
pub trait HasMetadata: Send + Sync {
    fn metadatable_type() -> &'static str;
    fn metadatable_id(&self) -> String;

    async fn set_meta(
        &self,
        app: &AppContext,
        key: &str,
        value: impl Serialize + Send,
    ) -> Result<()> {
        let db = app.database()?;
        let json_val = serde_json::to_value(value).map_err(Error::other)?;
        db.raw_execute(
            "INSERT INTO metadata (id, metadatable_type, metadatable_id, key, value, created_at) \
             VALUES (gen_random_uuid(), $1, $2::uuid, $3, $4, NOW()) \
             ON CONFLICT (metadatable_type, metadatable_id, key) \
             DO UPDATE SET value = $4, updated_at = NOW()",
            &[
                DbValue::Text(Self::metadatable_type().to_string()),
                DbValue::Text(self.metadatable_id()),
                DbValue::Text(key.to_string()),
                DbValue::Json(json_val),
            ],
        )
        .await?;
        Ok(())
    }

    async fn get_meta<T: DeserializeOwned>(
        &self,
        app: &AppContext,
        key: &str,
    ) -> Result<Option<T>> {
        match self.get_meta_raw(app, key).await? {
            Some(v) => Ok(Some(serde_json::from_value(v).map_err(Error::other)?)),
            None => Ok(None),
        }
    }

    async fn get_meta_raw(
        &self,
        app: &AppContext,
        key: &str,
    ) -> Result<Option<serde_json::Value>> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT value FROM metadata \
                 WHERE metadatable_type = $1 AND metadatable_id = $2::uuid AND key = $3",
                &[
                    DbValue::Text(Self::metadatable_type().to_string()),
                    DbValue::Text(self.metadatable_id()),
                    DbValue::Text(key.to_string()),
                ],
            )
            .await?;
        match rows.first() {
            Some(row) => match row.get("value") {
                Some(DbValue::Json(v)) => Ok(Some(v.clone())),
                _ => Ok(None),
            },
            None => Ok(None),
        }
    }

    async fn forget_meta(&self, app: &AppContext, key: &str) -> Result<bool> {
        let db = app.database()?;
        let affected = db
            .raw_execute(
                "DELETE FROM metadata \
                 WHERE metadatable_type = $1 AND metadatable_id = $2::uuid AND key = $3",
                &[
                    DbValue::Text(Self::metadatable_type().to_string()),
                    DbValue::Text(self.metadatable_id()),
                    DbValue::Text(key.to_string()),
                ],
            )
            .await?;
        Ok(affected > 0)
    }

    async fn has_meta(&self, app: &AppContext, key: &str) -> Result<bool> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT 1 FROM metadata \
                 WHERE metadatable_type = $1 AND metadatable_id = $2::uuid AND key = $3",
                &[
                    DbValue::Text(Self::metadatable_type().to_string()),
                    DbValue::Text(self.metadatable_id()),
                    DbValue::Text(key.to_string()),
                ],
            )
            .await?;
        Ok(!rows.is_empty())
    }

    async fn all_meta(&self, app: &AppContext) -> Result<Vec<ModelMeta>> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT id, metadatable_type, metadatable_id, key, value FROM metadata \
                 WHERE metadatable_type = $1 AND metadatable_id = $2::uuid ORDER BY key",
                &[
                    DbValue::Text(Self::metadatable_type().to_string()),
                    DbValue::Text(self.metadatable_id()),
                ],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|row| ModelMeta {
                id: row.text_or_uuid("id"),
                metadatable_type: row.text("metadatable_type"),
                metadatable_id: row.text_or_uuid("metadatable_id"),
                key: row.text("key"),
                value: match row.get("value") {
                    Some(DbValue::Json(v)) => Some(v.clone()),
                    _ => None,
                },
            })
            .collect())
    }
}

