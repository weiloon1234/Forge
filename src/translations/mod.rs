use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;

use crate::database::extensions::{
    current_extension_scope, uuid_array_from_ids, AnyModelExtension, ModelExtensionLoader,
    TranslationCacheShape,
};
use crate::database::{DbValue, QueryExecutor};
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

pub(crate) fn translated_field_extension_loader<M>(field: String) -> AnyModelExtension<M>
where
    M: HasTranslations + Send + Sync + 'static,
{
    Arc::new(TranslationExtensionLoader {
        shape: TranslationCacheShape::Field { field },
        _model: PhantomData,
    })
}

pub(crate) fn translations_for_extension_loader<M>(locale: String) -> AnyModelExtension<M>
where
    M: HasTranslations + Send + Sync + 'static,
{
    Arc::new(TranslationExtensionLoader {
        shape: TranslationCacheShape::Locale { locale },
        _model: PhantomData,
    })
}

pub(crate) fn all_translations_extension_loader<M>() -> AnyModelExtension<M>
where
    M: HasTranslations + Send + Sync + 'static,
{
    Arc::new(TranslationExtensionLoader {
        shape: TranslationCacheShape::All,
        _model: PhantomData,
    })
}

struct TranslationExtensionLoader<M> {
    shape: TranslationCacheShape,
    _model: PhantomData<fn() -> M>,
}

#[async_trait]
impl<M> ModelExtensionLoader<M> for TranslationExtensionLoader<M>
where
    M: HasTranslations + Send + Sync + 'static,
{
    async fn load(&self, executor: &dyn QueryExecutor, models: &[M]) -> Result<()> {
        let Some(scope) = current_extension_scope() else {
            return Ok(());
        };

        let ids = collect_unique_ids(models.iter().map(|model| model.translatable_id()));
        if ids.is_empty() {
            return Ok(());
        }

        let translatable_type = M::translatable_type();
        let missing_ids = scope.missing_translation_ids(translatable_type, &self.shape, &ids);
        if missing_ids.is_empty() {
            return Ok(());
        }

        let rows =
            load_translation_rows(executor, translatable_type, &self.shape, &missing_ids).await?;
        scope.store_translations(translatable_type, self.shape.clone(), &missing_ids, rows);
        Ok(())
    }
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
        invalidate_translation_cache(Self::translatable_type(), &self.translatable_id());
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
        let shape = TranslationCacheShape::Single {
            locale: locale.to_string(),
            field: field.to_string(),
        };
        if let Some(rows) = cached_translations_for_id(
            app,
            Self::translatable_type(),
            &self.translatable_id(),
            &shape,
        )
        .await?
        {
            return Ok(rows.into_iter().next().map(|row| row.value));
        }

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
        let shape = TranslationCacheShape::Locale {
            locale: locale.to_string(),
        };
        if let Some(rows) = cached_translations_for_id(
            app,
            Self::translatable_type(),
            &self.translatable_id(),
            &shape,
        )
        .await?
        {
            return Ok(rows.into_iter().map(|row| (row.field, row.value)).collect());
        }

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
        let shape = TranslationCacheShape::Field {
            field: field.to_string(),
        };
        if let Some(rows) = cached_translations_for_id(
            app,
            Self::translatable_type(),
            &self.translatable_id(),
            &shape,
        )
        .await?
        {
            return Ok(translated_fields_from_rows(app, rows));
        }

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
        let shape = TranslationCacheShape::All;
        if let Some(rows) = cached_translations_for_id(
            app,
            Self::translatable_type(),
            &self.translatable_id(),
            &shape,
        )
        .await?
        {
            return Ok(rows);
        }

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
        Ok(rows.iter().map(row_to_model_translation).collect())
    }

    async fn delete_translations(&self, app: &AppContext, locale: &str) -> Result<u64> {
        let db = app.database()?;
        let affected = db
            .raw_execute(
                "DELETE FROM model_translations \
             WHERE translatable_type = $1 AND translatable_id = $2::uuid AND locale = $3",
                &[
                    DbValue::Text(Self::translatable_type().to_string()),
                    DbValue::Text(self.translatable_id()),
                    DbValue::Text(locale.to_string()),
                ],
            )
            .await?;
        invalidate_translation_cache(Self::translatable_type(), &self.translatable_id());
        Ok(affected)
    }
}

async fn cached_translations_for_id(
    executor: &dyn QueryExecutor,
    translatable_type: &str,
    translatable_id: &str,
    shape: &TranslationCacheShape,
) -> Result<Option<Vec<ModelTranslation>>> {
    let Some(scope) = current_extension_scope() else {
        return Ok(None);
    };

    if let Some(rows) = scope.cached_translations(translatable_type, shape, translatable_id) {
        return Ok(Some(rows));
    }

    let missing_ids =
        scope.missing_translation_ids_for_known(translatable_type, shape, translatable_id);
    if !missing_ids.is_empty() {
        let rows = load_translation_rows(executor, translatable_type, shape, &missing_ids).await?;
        scope.store_translations(translatable_type, shape.clone(), &missing_ids, rows);
    }

    Ok(Some(
        scope
            .cached_translations(translatable_type, shape, translatable_id)
            .unwrap_or_default(),
    ))
}

async fn load_translation_rows(
    executor: &dyn QueryExecutor,
    translatable_type: &str,
    shape: &TranslationCacheShape,
    translatable_ids: &[String],
) -> Result<Vec<ModelTranslation>> {
    if translatable_ids.is_empty() {
        return Ok(Vec::new());
    }

    let ids = DbValue::UuidArray(uuid_array_from_ids(translatable_ids)?);
    let rows = match shape {
        TranslationCacheShape::Single { locale, field } => {
            executor
                .raw_query(
                    "SELECT id, translatable_type, translatable_id, locale, field, value \
                     FROM model_translations \
                     WHERE translatable_type = $1 AND translatable_id = ANY($2::uuid[]) \
                     AND locale = $3 AND field = $4 \
                     ORDER BY translatable_id, field, locale",
                    &[
                        DbValue::Text(translatable_type.to_string()),
                        ids,
                        DbValue::Text(locale.clone()),
                        DbValue::Text(field.clone()),
                    ],
                )
                .await?
        }
        TranslationCacheShape::Locale { locale } => {
            executor
                .raw_query(
                    "SELECT id, translatable_type, translatable_id, locale, field, value \
                     FROM model_translations \
                     WHERE translatable_type = $1 AND translatable_id = ANY($2::uuid[]) \
                     AND locale = $3 \
                     ORDER BY translatable_id, field, locale",
                    &[
                        DbValue::Text(translatable_type.to_string()),
                        ids,
                        DbValue::Text(locale.clone()),
                    ],
                )
                .await?
        }
        TranslationCacheShape::Field { field } => {
            executor
                .raw_query(
                    "SELECT id, translatable_type, translatable_id, locale, field, value \
                     FROM model_translations \
                     WHERE translatable_type = $1 AND translatable_id = ANY($2::uuid[]) \
                     AND field = $3 \
                     ORDER BY translatable_id, field, locale",
                    &[
                        DbValue::Text(translatable_type.to_string()),
                        ids,
                        DbValue::Text(field.clone()),
                    ],
                )
                .await?
        }
        TranslationCacheShape::All => {
            executor
                .raw_query(
                    "SELECT id, translatable_type, translatable_id, locale, field, value \
                     FROM model_translations \
                     WHERE translatable_type = $1 AND translatable_id = ANY($2::uuid[]) \
                     ORDER BY translatable_id, field, locale",
                    &[DbValue::Text(translatable_type.to_string()), ids],
                )
                .await?
        }
    };

    Ok(rows.iter().map(row_to_model_translation).collect())
}

fn row_to_model_translation(row: &crate::database::DbRecord) -> ModelTranslation {
    ModelTranslation {
        id: row.text_or_uuid("id"),
        translatable_type: row.text("translatable_type"),
        translatable_id: row.text_or_uuid("translatable_id"),
        locale: row.text("locale"),
        field: row.text("field"),
        value: row.text("value"),
    }
}

fn translated_fields_from_rows(app: &AppContext, rows: Vec<ModelTranslation>) -> TranslatedFields {
    let entries = rows
        .into_iter()
        .map(|row| (row.locale, row.value))
        .collect();
    let cur = current_locale(app);
    let default = app
        .i18n()
        .map(|m| m.default_locale().to_string())
        .unwrap_or_else(|_| "en".to_string());
    TranslatedFields::from_entries(entries, &cur, &default)
}

fn invalidate_translation_cache(translatable_type: &str, translatable_id: &str) {
    if let Some(scope) = current_extension_scope() {
        scope.invalidate_translations(translatable_type, translatable_id);
    }
}

fn collect_unique_ids(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    ids.into_iter()
        .filter(|id| !id.trim().is_empty())
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use uuid::Uuid;

    use crate::database::{
        scope_model_extensions, DbRecord, DbValue, QueryExecutionOptions, QueryExecutor,
    };

    use super::*;

    #[derive(Default)]
    struct CountingTranslationExecutor {
        query_count: AtomicUsize,
    }

    #[async_trait]
    impl QueryExecutor for CountingTranslationExecutor {
        async fn raw_query_with(
            &self,
            _sql: &str,
            bindings: &[DbValue],
            _options: QueryExecutionOptions,
        ) -> Result<Vec<DbRecord>> {
            self.query_count.fetch_add(1, Ordering::SeqCst);
            let translatable_type = match &bindings[0] {
                DbValue::Text(value) => value.clone(),
                _ => panic!("expected translatable_type binding"),
            };
            let ids = match &bindings[1] {
                DbValue::UuidArray(values) => values.clone(),
                _ => panic!("expected translatable_id uuid array binding"),
            };
            let field = match &bindings[2] {
                DbValue::Text(value) => value.clone(),
                _ => panic!("expected field binding"),
            };

            Ok(ids
                .into_iter()
                .map(|id| translation_record(&translatable_type, id, &field))
                .collect())
        }

        async fn raw_execute_with(
            &self,
            _sql: &str,
            _bindings: &[DbValue],
            _options: QueryExecutionOptions,
        ) -> Result<u64> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn lazy_translation_cache_batches_known_scope_ids() {
        let executor = CountingTranslationExecutor::default();
        let first_id = Uuid::now_v7().to_string();
        let second_id = Uuid::now_v7().to_string();
        let shape = TranslationCacheShape::Field {
            field: "name".to_string(),
        };

        scope_model_extensions(async {
            current_extension_scope()
                .unwrap()
                .register_model_ids("test_translatables", [first_id.clone(), second_id.clone()]);

            let first =
                cached_translations_for_id(&executor, "test_translatables", &first_id, &shape)
                    .await
                    .unwrap()
                    .unwrap();
            let second =
                cached_translations_for_id(&executor, "test_translatables", &second_id, &shape)
                    .await
                    .unwrap()
                    .unwrap();

            assert_eq!(executor.query_count.load(Ordering::SeqCst), 1);
            assert_eq!(first[0].translatable_id, first_id);
            assert_eq!(second[0].translatable_id, second_id);
        })
        .await;
    }

    fn translation_record(translatable_type: &str, translatable_id: Uuid, field: &str) -> DbRecord {
        let mut record = DbRecord::new();
        record.insert("id", DbValue::Uuid(Uuid::now_v7()));
        record.insert(
            "translatable_type",
            DbValue::Text(translatable_type.to_string()),
        );
        record.insert("translatable_id", DbValue::Uuid(translatable_id));
        record.insert("locale", DbValue::Text("en".to_string()));
        record.insert("field", DbValue::Text(field.to_string()));
        record.insert("value", DbValue::Text("Translated".to_string()));
        record
    }
}
