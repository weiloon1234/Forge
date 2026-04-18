use std::collections::HashMap;

use crate::database::DbValue;
use crate::foundation::{AppContext, Result};

tokio::task_local! {
    /// The current request's locale, set automatically by request middleware.
    pub static CURRENT_LOCALE: String;
}

/// Resolve the current locale: task_local request locale → i18n default → "en".
pub fn current_locale(app: &AppContext) -> String {
    CURRENT_LOCALE.try_with(|l| l.clone()).unwrap_or_else(|_| {
        app.i18n()
            .map(|m| m.default_locale().to_string())
            .unwrap_or_else(|_| "en".to_string())
    })
}

/// A single translation record from the `model_translations` table.
#[derive(Clone, Debug)]
pub struct ModelTranslation {
    pub id: String,
    pub translatable_type: String,
    pub translatable_id: String,
    pub locale: String,
    pub field: String,
    pub value: String,
}

/// A field's translations across all locales, with a resolved current-locale value.
///
/// ```ignore
/// let tf = product.translated_field(&app, "name").await?;
/// tf.translated            // "Red Shirt" (current locale)
/// tf.values["zh"]          // "红色衬衫"
/// tf.get("ms")             // Some("Baju Merah")
/// ```
#[derive(Clone, Debug)]
pub struct TranslatedFields {
    /// All locale values: `{"en": "Red Shirt", "zh": "红色衬衫"}`
    pub values: HashMap<String, String>,
    /// The resolved translation for the current request locale (with fallback).
    pub translated: String,
}

impl TranslatedFields {
    /// Build from a list of (locale, value) pairs, resolving `translated`.
    pub fn from_entries(
        entries: Vec<(String, String)>,
        current_locale: &str,
        default_locale: &str,
    ) -> Self {
        let values: HashMap<String, String> = entries.into_iter().collect();
        let translated = values
            .get(current_locale)
            .or_else(|| values.get(default_locale))
            .or_else(|| values.values().next())
            .cloned()
            .unwrap_or_default();
        Self { values, translated }
    }

    /// Get a specific locale's value.
    pub fn get(&self, locale: &str) -> Option<&str> {
        self.values.get(locale).map(|s| s.as_str())
    }
}

/// Trait for models with translatable fields stored in the `model_translations` table.
///
/// ```ignore
/// impl HasTranslations for Product {
///     fn translatable_type() -> &'static str { "products" }
///     fn translatable_id(&self) -> String { self.id.to_string() }
/// }
///
/// product.set_translation(&app, "zh", "name", "红色衬衫").await?;
/// let name = product.translated_field(&app, "name").await?;
/// name.translated  // current locale value
/// ```
#[async_trait::async_trait]
pub trait HasTranslations: Send + Sync {
    fn translatable_type() -> &'static str;
    fn translatable_id(&self) -> String;

    async fn set_translation(
        &self,
        app: &AppContext,
        locale: &str,
        field: &str,
        value: &str,
    ) -> Result<()> {
        let db = app.database()?;
        db.raw_execute(
            "INSERT INTO model_translations (id, translatable_type, translatable_id, locale, field, value, created_at) \
             VALUES (gen_random_uuid(), $1, $2::uuid, $3, $4, $5, NOW()) \
             ON CONFLICT (translatable_type, translatable_id, locale, field) \
             DO UPDATE SET value = $5, updated_at = NOW()",
            &[
                DbValue::Text(Self::translatable_type().to_string()),
                DbValue::Text(self.translatable_id()),
                DbValue::Text(locale.to_string()),
                DbValue::Text(field.to_string()),
                DbValue::Text(value.to_string()),
            ],
        )
        .await?;
        Ok(())
    }

    async fn set_translations(
        &self,
        app: &AppContext,
        locale: &str,
        values: &[(&str, &str)],
    ) -> Result<()> {
        for (field, value) in values {
            self.set_translation(app, locale, field, value).await?;
        }
        Ok(())
    }

    async fn translation(
        &self,
        app: &AppContext,
        locale: &str,
        field: &str,
    ) -> Result<Option<String>> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT value FROM model_translations \
                 WHERE translatable_type = $1 AND translatable_id = $2::uuid \
                 AND locale = $3 AND field = $4",
                &[
                    DbValue::Text(Self::translatable_type().to_string()),
                    DbValue::Text(self.translatable_id()),
                    DbValue::Text(locale.to_string()),
                    DbValue::Text(field.to_string()),
                ],
            )
            .await?;
        match rows.first() {
            Some(row) => match row.get("value") {
                Some(DbValue::Text(s)) => Ok(Some(s.clone())),
                _ => Ok(None),
            },
            None => Ok(None),
        }
    }

    async fn translations_for(
        &self,
        app: &AppContext,
        locale: &str,
    ) -> Result<HashMap<String, String>> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT field, value FROM model_translations \
                 WHERE translatable_type = $1 AND translatable_id = $2::uuid AND locale = $3",
                &[
                    DbValue::Text(Self::translatable_type().to_string()),
                    DbValue::Text(self.translatable_id()),
                    DbValue::Text(locale.to_string()),
                ],
            )
            .await?;
        let mut map = HashMap::new();
        for row in &rows {
            if let (Some(DbValue::Text(field)), Some(DbValue::Text(value))) =
                (row.get("field"), row.get("value"))
            {
                map.insert(field.clone(), value.clone());
            }
        }
        Ok(map)
    }

    /// Get a `TranslatedFields` for a specific field across all locales.
    ///
    /// The `translated` value is resolved using the current request locale
    /// (via `CURRENT_LOCALE` task_local), falling back to the i18n default locale.
    async fn translated_field(&self, app: &AppContext, field: &str) -> Result<TranslatedFields> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT locale, value FROM model_translations \
                 WHERE translatable_type = $1 AND translatable_id = $2::uuid AND field = $3",
                &[
                    DbValue::Text(Self::translatable_type().to_string()),
                    DbValue::Text(self.translatable_id()),
                    DbValue::Text(field.to_string()),
                ],
            )
            .await?;
        let entries: Vec<(String, String)> = rows
            .iter()
            .filter_map(|row| match (row.get("locale"), row.get("value")) {
                (Some(DbValue::Text(locale)), Some(DbValue::Text(value))) => {
                    Some((locale.clone(), value.clone()))
                }
                _ => None,
            })
            .collect();
        let cur = current_locale(app);
        let default = app
            .i18n()
            .map(|m| m.default_locale().to_string())
            .unwrap_or_else(|_| "en".to_string());
        Ok(TranslatedFields::from_entries(entries, &cur, &default))
    }

    async fn all_translations(&self, app: &AppContext) -> Result<Vec<ModelTranslation>> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT id, translatable_type, translatable_id, locale, field, value \
                 FROM model_translations \
                 WHERE translatable_type = $1 AND translatable_id = $2::uuid \
                 ORDER BY field, locale",
                &[
                    DbValue::Text(Self::translatable_type().to_string()),
                    DbValue::Text(self.translatable_id()),
                ],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|row| ModelTranslation {
                id: row.text_or_uuid("id"),
                translatable_type: row.text("translatable_type"),
                translatable_id: row.text_or_uuid("translatable_id"),
                locale: row.text("locale"),
                field: row.text("field"),
                value: row.text("value"),
            })
            .collect())
    }

    async fn delete_translations(&self, app: &AppContext, locale: &str) -> Result<u64> {
        let db = app.database()?;
        db.raw_execute(
            "DELETE FROM model_translations \
             WHERE translatable_type = $1 AND translatable_id = $2::uuid AND locale = $3",
            &[
                DbValue::Text(Self::translatable_type().to_string()),
                DbValue::Text(self.translatable_id()),
                DbValue::Text(locale.to_string()),
            ],
        )
        .await
    }
}
