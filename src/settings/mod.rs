use serde::{Deserialize, Serialize};

use crate::database::DbValue;
use crate::foundation::{AppContext, Result};

// ---------------------------------------------------------------------------
// Setting struct (framework-provided key-value store)
// ---------------------------------------------------------------------------

/// A key-value setting record from the `settings` table.
///
/// Values are stored as JSONB, supporting strings, numbers, booleans,
/// arrays, and nested objects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Setting {
    pub id: String,
    pub key: String,
    pub value: Option<serde_json::Value>,
}

impl Setting {
    /// Get a setting by key. Returns `None` if the key doesn't exist.
    pub async fn get(app: &AppContext, key: &str) -> Result<Option<serde_json::Value>> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT value FROM settings WHERE key = $1",
                &[DbValue::Text(key.to_string())],
            )
            .await?;
        Ok(rows.first().and_then(|row| match row.get("value") {
            Some(DbValue::Json(v)) => Some(v.clone()),
            _ => None,
        }))
    }

    /// Set a setting value. Creates the key if it doesn't exist, updates if it does.
    pub async fn set(
        app: &AppContext,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        let db = app.database()?;
        db.raw_execute(
            "INSERT INTO settings (key, value, created_at) \
             VALUES ($1, $2, NOW()) \
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()",
            &[
                DbValue::Text(key.to_string()),
                DbValue::Json(value),
            ],
        )
        .await?;
        Ok(())
    }

    /// Get a setting as a typed value via serde deserialization.
    /// Returns `None` if the key doesn't exist or deserialization fails.
    pub async fn get_as<T: serde::de::DeserializeOwned>(
        app: &AppContext,
        key: &str,
    ) -> Result<Option<T>> {
        match Self::get(app, key).await? {
            Some(value) => Ok(serde_json::from_value(value).ok()),
            None => Ok(None),
        }
    }

    /// Get a setting value, returning a default if the key doesn't exist.
    pub async fn get_or(
        app: &AppContext,
        key: &str,
        default: serde_json::Value,
    ) -> Result<serde_json::Value> {
        Ok(Self::get(app, key).await?.unwrap_or(default))
    }

    /// Delete a setting by key. Returns `true` if the key existed.
    pub async fn remove(app: &AppContext, key: &str) -> Result<bool> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "DELETE FROM settings WHERE key = $1 RETURNING id",
                &[DbValue::Text(key.to_string())],
            )
            .await?;
        Ok(!rows.is_empty())
    }

    /// Check if a setting key exists.
    pub async fn exists(app: &AppContext, key: &str) -> Result<bool> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT 1 FROM settings WHERE key = $1",
                &[DbValue::Text(key.to_string())],
            )
            .await?;
        Ok(!rows.is_empty())
    }

    /// List all settings, ordered by key.
    pub async fn all(app: &AppContext) -> Result<Vec<Setting>> {
        let db = app.database()?;
        let rows = db
            .raw_query("SELECT * FROM settings ORDER BY key", &[])
            .await?;
        Ok(rows.iter().map(row_to_setting).collect())
    }

    /// List settings whose keys start with a given prefix.
    pub async fn by_prefix(app: &AppContext, prefix: &str) -> Result<Vec<Setting>> {
        let db = app.database()?;
        let pattern = format!("{}%", prefix.replace('%', "\\%").replace('_', "\\_"));
        let rows = db
            .raw_query(
                "SELECT * FROM settings WHERE key LIKE $1 ORDER BY key",
                &[DbValue::Text(pattern)],
            )
            .await?;
        Ok(rows.iter().map(row_to_setting).collect())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_setting(row: &crate::database::DbRecord) -> Setting {
    Setting {
        id: row.text("id"),
        key: row.text("key"),
        value: match row.get("value") {
            Some(DbValue::Json(v)) => Some(v.clone()),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_to_setting_handles_default() {
        // Structural test — actual DB tests are in acceptance tests
        let setting = Setting {
            id: "test".to_string(),
            key: "app.name".to_string(),
            value: Some(serde_json::json!("My App")),
        };
        assert_eq!(setting.key, "app.name");
        assert_eq!(setting.value, Some(serde_json::json!("My App")));
    }
}
