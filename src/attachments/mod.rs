use std::collections::BTreeSet;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;

use crate::database::extensions::{
    current_extension_scope, uuid_array_from_ids, AnyModelExtension, ModelExtensionLoader,
};
use crate::database::{DbType, DbValue, QueryExecutor};
use crate::foundation::{AppContext, Error, Result};
use crate::storage::UploadedFile;
use crate::support::DateTime;

const LOCALIZED_COLLECTION_SEPARATOR: &str = ":";

/// A file attachment record from the `attachments` table.
#[derive(Clone, Debug)]
pub struct Attachment {
    pub id: String,
    pub attachable_type: String,
    pub attachable_id: String,
    pub collection: String,
    pub disk: String,
    pub path: String,
    pub name: String,
    pub original_name: Option<String>,
    pub mime_type: Option<String>,
    pub size: i64,
    pub sort_order: i32,
    pub custom_properties: serde_json::Value,
}

impl Attachment {
    /// Start building an attachment upload pipeline.
    pub fn upload(file: UploadedFile) -> AttachmentUploadBuilder {
        AttachmentUploadBuilder {
            file,
            collection: "default".to_string(),
            disk: None,
            image_transforms: Vec::new(),
        }
    }

    pub fn is_image(&self) -> bool {
        self.mime_type
            .as_deref()
            .is_some_and(|m| m.starts_with("image/"))
    }

    pub fn is_video(&self) -> bool {
        self.mime_type
            .as_deref()
            .is_some_and(|m| m.starts_with("video/"))
    }

    pub fn is_audio(&self) -> bool {
        self.mime_type
            .as_deref()
            .is_some_and(|m| m.starts_with("audio/"))
    }

    pub fn is_document(&self) -> bool {
        self.mime_type.as_deref().is_some_and(|m| {
            matches!(
                m,
                "application/pdf"
                    | "application/msword"
                    | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    | "application/vnd.ms-excel"
                    | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                    | "text/csv"
                    | "text/plain"
            )
        })
    }

    pub fn extension(&self) -> Option<&str> {
        self.name
            .rsplit('.')
            .next()
            .filter(|ext| ext.len() < 10 && !ext.is_empty())
    }

    pub fn human_size(&self) -> String {
        let size = self.size as f64;
        if size < 1024.0 {
            return format!("{} B", self.size);
        }
        if size < 1024.0 * 1024.0 {
            return format!("{:.1} KB", size / 1024.0);
        }
        if size < 1024.0 * 1024.0 * 1024.0 {
            return format!("{:.1} MB", size / (1024.0 * 1024.0));
        }
        format!("{:.1} GB", size / (1024.0 * 1024.0 * 1024.0))
    }

    pub async fn url(&self, app: &AppContext) -> Result<String> {
        let storage = app.storage()?;
        let disk = storage.disk(&self.disk)?;
        disk.url(&self.path).await
    }

    pub async fn temporary_url(&self, app: &AppContext, expires_at: DateTime) -> Result<String> {
        let storage = app.storage()?;
        let disk = storage.disk(&self.disk)?;
        disk.temporary_url(&self.path, expires_at).await
    }

    /// Load this attachment's file into the image processing module.
    pub async fn image(&self, app: &AppContext) -> Result<crate::imaging::ImageProcessor> {
        let storage = app.storage()?;
        let disk = storage.disk(&self.disk)?;
        let bytes = disk.get(&self.path).await?;
        crate::imaging::ImageProcessor::from_bytes(&bytes)
    }
}

/// Build the concrete attachment collection name for a locale-specific asset.
///
/// This keeps localized assets in the existing `attachments.collection` column
/// without adding another table or duplicating locale configuration.
pub fn localized_attachment_collection(collection: &str, locale: &str) -> String {
    format!(
        "{}{}{}",
        collection.trim(),
        LOCALIZED_COLLECTION_SEPARATOR,
        locale.trim()
    )
}

/// Return the loaded i18n locales used by localized attachment helpers.
///
/// Locale folders under the configured i18n resource path are the source of
/// truth, matching `I18nManager::locale_list()`.
pub fn available_attachment_locales(app: &AppContext) -> Result<Vec<String>> {
    let manager = app.i18n().map_err(|_| {
        Error::http_with_code(
            400,
            "localized attachments require i18n to be configured",
            "i18n_not_configured",
        )
    })?;
    let mut locales = manager
        .locale_list()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    locales.sort();
    Ok(locales)
}

// ---------------------------------------------------------------------------
// Upload pipeline builder
// ---------------------------------------------------------------------------

enum ImageTransform {
    Resize(u32, u32),
    ResizeToFit(u32, u32),
    ResizeToFill(u32, u32),
    Quality(u8),
}

/// Chainable builder for uploading files as attachments.
///
/// ```ignore
/// Attachment::upload(file)
///     .collection("avatar")
///     .disk("s3")
///     .resize(800, 600)
///     .quality(80)
///     .store(&app, "users", &user.id.to_string())
///     .await?;
/// ```
pub struct AttachmentUploadBuilder {
    file: UploadedFile,
    collection: String,
    disk: Option<String>,
    image_transforms: Vec<ImageTransform>,
}

pub(crate) fn attachment_extension_loader<M>(collection: String) -> AnyModelExtension<M>
where
    M: HasAttachments + Send + Sync + 'static,
{
    Arc::new(AttachmentExtensionLoader {
        collection,
        _model: PhantomData,
    })
}

struct AttachmentExtensionLoader<M> {
    collection: String,
    _model: PhantomData<fn() -> M>,
}

#[async_trait]
impl<M> ModelExtensionLoader<M> for AttachmentExtensionLoader<M>
where
    M: HasAttachments + Send + Sync + 'static,
{
    async fn load(&self, executor: &dyn QueryExecutor, models: &[M]) -> Result<()> {
        let Some(scope) = current_extension_scope() else {
            return Ok(());
        };

        let ids = collect_unique_ids(models.iter().map(|model| model.attachable_id()));
        if ids.is_empty() {
            return Ok(());
        }

        let attachable_type = M::attachable_type();
        let missing_ids = scope.missing_attachment_ids(attachable_type, &self.collection, &ids);
        if missing_ids.is_empty() {
            return Ok(());
        }

        let rows =
            load_attachment_rows(executor, attachable_type, &self.collection, &missing_ids).await?;
        scope.store_attachments(attachable_type, &self.collection, &missing_ids, rows);
        Ok(())
    }
}

impl AttachmentUploadBuilder {
    pub fn collection(mut self, collection: impl Into<String>) -> Self {
        self.collection = collection.into();
        self
    }

    pub fn disk(mut self, disk: impl Into<String>) -> Self {
        self.disk = Some(disk.into());
        self
    }

    pub fn resize(mut self, width: u32, height: u32) -> Self {
        self.image_transforms
            .push(ImageTransform::Resize(width, height));
        self
    }

    pub fn resize_to_fit(mut self, max_width: u32, max_height: u32) -> Self {
        self.image_transforms
            .push(ImageTransform::ResizeToFit(max_width, max_height));
        self
    }

    pub fn resize_to_fill(mut self, width: u32, height: u32) -> Self {
        self.image_transforms
            .push(ImageTransform::ResizeToFill(width, height));
        self
    }

    pub fn quality(mut self, quality: u8) -> Self {
        self.image_transforms.push(ImageTransform::Quality(quality));
        self
    }

    /// Store the file and create the attachment record.
    pub async fn store(
        self,
        app: &AppContext,
        attachable_type: &str,
        attachable_id: &str,
    ) -> Result<Attachment> {
        let storage = app.storage()?;
        let db = app.database()?;

        let disk_name = self.disk.unwrap_or_else(|| {
            app.config()
                .storage()
                .map(|c| c.default.clone())
                .unwrap_or_else(|_| "local".to_string())
        });

        let dir = format!("attachments/{}/{}", attachable_type, self.collection);

        // Process image transforms if any, otherwise store directly
        let (path, name, size, content_type) = if !self.image_transforms.is_empty() {
            let bytes = tokio::fs::read(&self.file.temp_path)
                .await
                .map_err(Error::other)?;
            let mut processor = crate::imaging::ImageProcessor::from_bytes(&bytes)?;
            let mut quality_val = 85u8;

            for transform in &self.image_transforms {
                match transform {
                    ImageTransform::Resize(w, h) => processor = processor.resize(*w, *h),
                    ImageTransform::ResizeToFit(w, h) => {
                        processor = processor.resize_to_fit(*w, *h)
                    }
                    ImageTransform::ResizeToFill(w, h) => {
                        processor = processor.resize_to_fill(*w, *h)
                    }
                    ImageTransform::Quality(q) => quality_val = *q,
                }
            }
            processor = processor.quality(quality_val);

            let format = self
                .file
                .original_name
                .as_deref()
                .and_then(|n| n.rsplit('.').next())
                .and_then(crate::imaging::ImageFormat::from_extension)
                .unwrap_or(crate::imaging::ImageFormat::Jpeg);

            let output_bytes = processor.to_bytes(format)?;
            let size = output_bytes.len() as i64;
            let ext = format.extension();
            let storage_name = format!("{}.{}", uuid::Uuid::now_v7(), ext);
            let path = format!("{}/{}", dir, storage_name);
            let ct = format!("image/{}", ext);

            let disk = storage.disk(&disk_name)?;
            disk.put(&path, &output_bytes).await?;

            (path, storage_name, size, Some(ct))
        } else {
            let stored = self.file.store_on(app, &disk_name, &dir).await?;
            let size = self.file.size as i64;
            let ct = self.file.content_type.clone().or(stored.content_type);
            (stored.path, stored.name, size, ct)
        };

        let rows = db.raw_query(
            "INSERT INTO attachments \
             (id, attachable_type, attachable_id, collection, disk, path, name, original_name, mime_type, size, sort_order, custom_properties, created_at) \
             VALUES (gen_random_uuid(), $1, $2::uuid, $3, $4, $5, $6, $7, $8, $9, 0, '{}', NOW()) \
             RETURNING id",
            &[
                DbValue::Text(attachable_type.to_string()),
                DbValue::Text(attachable_id.to_string()),
                DbValue::Text(self.collection.clone()),
                DbValue::Text(disk_name.clone()),
                DbValue::Text(path.clone()),
                DbValue::Text(name.clone()),
                opt_text(&self.file.original_name),
                opt_text(&content_type),
                DbValue::Int64(size),
            ],
        )
        .await;

        // Clean up stored file if DB insert fails
        if let Err(e) = &rows {
            if let Ok(d) = storage.disk(&disk_name) {
                let _ = d.delete(&path).await;
            }
            return Err(crate::foundation::Error::message(format!(
                "failed to create attachment record: {e}"
            )));
        }

        let id = rows?
            .first()
            .and_then(|r| match r.get("id") {
                Some(DbValue::Uuid(u)) => Some(u.to_string()),
                _ => None,
            })
            .unwrap_or_default();

        Ok(Attachment {
            id,
            attachable_type: attachable_type.to_string(),
            attachable_id: attachable_id.to_string(),
            collection: self.collection,
            disk: disk_name,
            path,
            name,
            original_name: self.file.original_name,
            mime_type: content_type,
            size,
            sort_order: 0,
            custom_properties: serde_json::json!({}),
        })
    }
}

// ---------------------------------------------------------------------------
// HasAttachments trait
// ---------------------------------------------------------------------------

/// Trait for models that can have file attachments.
///
/// ```ignore
/// impl HasAttachments for User {
///     fn attachable_type() -> &'static str { "users" }
///     fn attachable_id(&self) -> String { self.id.to_string() }
/// }
///
/// user.attach(&app, "avatar", uploaded_file).await?;
/// let avatar = user.attachment(&app, "avatar").await?;
/// ```
#[async_trait::async_trait]
pub trait HasAttachments: Send + Sync {
    fn attachable_type() -> &'static str;
    fn attachable_id(&self) -> String;

    async fn attach(
        &self,
        app: &AppContext,
        collection: &str,
        file: UploadedFile,
    ) -> Result<Attachment> {
        let attachment = Attachment::upload(file)
            .collection(collection)
            .store(app, Self::attachable_type(), &self.attachable_id())
            .await?;
        invalidate_attachment_cache(
            Self::attachable_type(),
            &self.attachable_id(),
            Some(collection),
        );
        Ok(attachment)
    }

    /// Attach a file to a locale-specific collection.
    ///
    /// The locale must exist in `app.i18n()?.locale_list()`.
    async fn attach_localized(
        &self,
        app: &AppContext,
        collection: &str,
        locale: &str,
        file: UploadedFile,
    ) -> Result<Attachment> {
        let collection = localized_collection_for(app, collection, locale)?;
        self.attach(app, &collection, file).await
    }

    /// Replace the first-class localized asset for a collection and locale.
    ///
    /// The new file is stored before old files are removed, so a failed upload
    /// does not leave the locale without its previous asset.
    async fn replace_localized_attachment(
        &self,
        app: &AppContext,
        collection: &str,
        locale: &str,
        file: UploadedFile,
    ) -> Result<Attachment> {
        let collection = localized_collection_for(app, collection, locale)?;
        let existing = self.attachments(app, &collection).await?;
        let attachment = self.attach(app, &collection, file).await?;

        for old in existing {
            if old.id != attachment.id {
                self.detach(app, &old.id).await?;
            }
        }

        Ok(attachment)
    }

    /// Read the first attachment for an exact locale.
    async fn localized_attachment(
        &self,
        app: &AppContext,
        collection: &str,
        locale: &str,
    ) -> Result<Option<Attachment>> {
        let collection = localized_collection_for(app, collection, locale)?;
        self.attachment(app, &collection).await
    }

    /// Read all attachments for an exact locale.
    async fn localized_attachments(
        &self,
        app: &AppContext,
        collection: &str,
        locale: &str,
    ) -> Result<Vec<Attachment>> {
        let collection = localized_collection_for(app, collection, locale)?;
        self.attachments(app, &collection).await
    }

    /// Read a localized attachment, falling back to the i18n default locale.
    async fn localized_attachment_or_default(
        &self,
        app: &AppContext,
        collection: &str,
        locale: &str,
    ) -> Result<Option<Attachment>> {
        if let Some(attachment) = self.localized_attachment(app, collection, locale).await? {
            return Ok(Some(attachment));
        }

        let default_locale = app
            .i18n()
            .map_err(|_| {
                Error::http_with_code(
                    400,
                    "localized attachments require i18n to be configured",
                    "i18n_not_configured",
                )
            })?
            .default_locale()
            .to_string();

        if locale.trim() == default_locale {
            return Ok(None);
        }

        self.localized_attachment(app, collection, &default_locale)
            .await
    }

    /// Read a localized attachment for the current request locale, with default fallback.
    async fn current_localized_attachment(
        &self,
        app: &AppContext,
        collection: &str,
    ) -> Result<Option<Attachment>> {
        let locale = crate::translations::current_locale(app);
        self.localized_attachment_or_default(app, collection, &locale)
            .await
    }

    async fn attachment(&self, app: &AppContext, collection: &str) -> Result<Option<Attachment>> {
        if let Some(rows) = cached_attachments_for_id(
            app,
            Self::attachable_type(),
            &self.attachable_id(),
            collection,
        )
        .await?
        {
            return Ok(rows.into_iter().next());
        }

        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT id, attachable_type, attachable_id, collection, disk, path, name, \
                 original_name, mime_type, size, sort_order, custom_properties \
                 FROM attachments \
                 WHERE attachable_type = $1 AND attachable_id = $2::uuid AND collection = $3 \
                 ORDER BY sort_order, created_at LIMIT 1",
                &[
                    DbValue::Text(Self::attachable_type().to_string()),
                    DbValue::Text(self.attachable_id()),
                    DbValue::Text(collection.to_string()),
                ],
            )
            .await?;
        Ok(rows.first().map(row_to_attachment))
    }

    async fn attachments(&self, app: &AppContext, collection: &str) -> Result<Vec<Attachment>> {
        if let Some(rows) = cached_attachments_for_id(
            app,
            Self::attachable_type(),
            &self.attachable_id(),
            collection,
        )
        .await?
        {
            return Ok(rows);
        }

        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT id, attachable_type, attachable_id, collection, disk, path, name, \
                 original_name, mime_type, size, sort_order, custom_properties \
                 FROM attachments \
                 WHERE attachable_type = $1 AND attachable_id = $2::uuid AND collection = $3 \
                 ORDER BY sort_order, created_at",
                &[
                    DbValue::Text(Self::attachable_type().to_string()),
                    DbValue::Text(self.attachable_id()),
                    DbValue::Text(collection.to_string()),
                ],
            )
            .await?;
        Ok(rows.iter().map(row_to_attachment).collect())
    }

    /// Delete an attachment and its file from storage.
    async fn detach(&self, app: &AppContext, attachment_id: &str) -> Result<()> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT disk, path FROM attachments \
                 WHERE id = $1::uuid AND attachable_type = $2 AND attachable_id = $3::uuid",
                &[
                    DbValue::Text(attachment_id.to_string()),
                    DbValue::Text(Self::attachable_type().to_string()),
                    DbValue::Text(self.attachable_id()),
                ],
            )
            .await?;

        if let Some(row) = rows.first() {
            if let (Some(DbValue::Text(disk)), Some(DbValue::Text(path))) =
                (row.get("disk"), row.get("path"))
            {
                if let Ok(storage) = app.storage() {
                    if let Ok(d) = storage.disk(disk) {
                        let _ = d.delete(path).await;
                    }
                }
            }
        }

        db.raw_execute(
            "DELETE FROM attachments \
             WHERE id = $1::uuid AND attachable_type = $2 AND attachable_id = $3::uuid",
            &[
                DbValue::Text(attachment_id.to_string()),
                DbValue::Text(Self::attachable_type().to_string()),
                DbValue::Text(self.attachable_id()),
            ],
        )
        .await?;
        invalidate_attachment_cache(Self::attachable_type(), &self.attachable_id(), None);
        Ok(())
    }

    /// Delete attachment record but keep the file on storage.
    async fn detach_keep_file(&self, app: &AppContext, attachment_id: &str) -> Result<()> {
        let db = app.database()?;
        db.raw_execute(
            "DELETE FROM attachments \
             WHERE id = $1::uuid AND attachable_type = $2 AND attachable_id = $3::uuid",
            &[
                DbValue::Text(attachment_id.to_string()),
                DbValue::Text(Self::attachable_type().to_string()),
                DbValue::Text(self.attachable_id()),
            ],
        )
        .await?;
        invalidate_attachment_cache(Self::attachable_type(), &self.attachable_id(), None);
        Ok(())
    }

    /// Delete all attachments in a collection and their files.
    async fn detach_all(&self, app: &AppContext, collection: &str) -> Result<u64> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT disk, path FROM attachments \
                 WHERE attachable_type = $1 AND attachable_id = $2::uuid AND collection = $3",
                &[
                    DbValue::Text(Self::attachable_type().to_string()),
                    DbValue::Text(self.attachable_id()),
                    DbValue::Text(collection.to_string()),
                ],
            )
            .await?;

        if let Ok(storage) = app.storage() {
            for row in &rows {
                if let (Some(DbValue::Text(disk)), Some(DbValue::Text(path))) =
                    (row.get("disk"), row.get("path"))
                {
                    if let Ok(d) = storage.disk(disk) {
                        let _ = d.delete(path).await;
                    }
                }
            }
        }

        let affected = db
            .raw_execute(
                "DELETE FROM attachments \
             WHERE attachable_type = $1 AND attachable_id = $2::uuid AND collection = $3",
                &[
                    DbValue::Text(Self::attachable_type().to_string()),
                    DbValue::Text(self.attachable_id()),
                    DbValue::Text(collection.to_string()),
                ],
            )
            .await?;
        invalidate_attachment_cache(
            Self::attachable_type(),
            &self.attachable_id(),
            Some(collection),
        );
        Ok(affected)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn opt_text(value: &Option<String>) -> DbValue {
    match value {
        Some(s) => DbValue::Text(s.clone()),
        None => DbValue::Null(DbType::Text),
    }
}

fn localized_collection_for(app: &AppContext, collection: &str, locale: &str) -> Result<String> {
    let collection = collection.trim();
    if collection.is_empty() {
        return Err(Error::http_with_code(
            400,
            "attachment collection is required",
            "invalid_attachment_collection",
        ));
    }

    let locale = validate_attachment_locale(app, locale)?;
    Ok(localized_attachment_collection(collection, &locale))
}

fn validate_attachment_locale(app: &AppContext, locale: &str) -> Result<String> {
    let locale = locale.trim();
    if locale.is_empty() {
        return Err(Error::http_with_code(
            400,
            "locale is required",
            "invalid_locale",
        ));
    }

    let available = available_attachment_locales(app)?;
    if available.iter().any(|candidate| candidate == locale) {
        return Ok(locale.to_string());
    }

    let message = if available.is_empty() {
        format!("locale `{locale}` is not available because no i18n locales are loaded")
    } else {
        format!(
            "locale `{locale}` is not available; available locales: {}",
            available.join(", ")
        )
    };

    Err(Error::http_with_code(400, message, "invalid_locale"))
}

pub(crate) fn row_to_attachment(row: &crate::database::DbRecord) -> Attachment {
    Attachment {
        id: row.text_or_uuid("id"),
        attachable_type: row.text("attachable_type"),
        attachable_id: row.text_or_uuid("attachable_id"),
        collection: row.text("collection"),
        disk: row.text("disk"),
        path: row.text("path"),
        name: row.text("name"),
        original_name: row.optional_text("original_name"),
        mime_type: row.optional_text("mime_type"),
        size: match row.get("size") {
            Some(DbValue::Int64(n)) => *n,
            _ => 0,
        },
        sort_order: match row.get("sort_order") {
            Some(DbValue::Int32(n)) => *n,
            _ => 0,
        },
        custom_properties: match row.get("custom_properties") {
            Some(DbValue::Json(v)) => v.clone(),
            _ => serde_json::json!({}),
        },
    }
}

async fn cached_attachments_for_id(
    executor: &dyn QueryExecutor,
    attachable_type: &str,
    attachable_id: &str,
    collection: &str,
) -> Result<Option<Vec<Attachment>>> {
    let Some(scope) = current_extension_scope() else {
        return Ok(None);
    };

    if let Some(rows) = scope.cached_attachments(attachable_type, collection, attachable_id) {
        return Ok(Some(rows));
    }

    let missing_ids =
        scope.missing_attachment_ids_for_known(attachable_type, collection, attachable_id);
    if !missing_ids.is_empty() {
        let rows =
            load_attachment_rows(executor, attachable_type, collection, &missing_ids).await?;
        scope.store_attachments(attachable_type, collection, &missing_ids, rows);
    }

    Ok(Some(
        scope
            .cached_attachments(attachable_type, collection, attachable_id)
            .unwrap_or_default(),
    ))
}

async fn load_attachment_rows(
    executor: &dyn QueryExecutor,
    attachable_type: &str,
    collection: &str,
    attachable_ids: &[String],
) -> Result<Vec<Attachment>> {
    if attachable_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows = executor
        .raw_query(
            "SELECT id, attachable_type, attachable_id, collection, disk, path, name, \
             original_name, mime_type, size, sort_order, custom_properties \
             FROM attachments \
             WHERE attachable_type = $1 AND attachable_id = ANY($2::uuid[]) AND collection = $3 \
             ORDER BY attachable_id, sort_order, created_at",
            &[
                DbValue::Text(attachable_type.to_string()),
                DbValue::UuidArray(uuid_array_from_ids(attachable_ids)?),
                DbValue::Text(collection.to_string()),
            ],
        )
        .await?;
    Ok(rows.iter().map(row_to_attachment).collect())
}

fn invalidate_attachment_cache(
    attachable_type: &str,
    attachable_id: &str,
    collection: Option<&str>,
) {
    if let Some(scope) = current_extension_scope() {
        match collection {
            Some(collection) => {
                scope.invalidate_attachment_collection(attachable_type, attachable_id, collection)
            }
            None => scope.invalidate_attachments(attachable_type, attachable_id),
        }
    }
}

fn collect_unique_ids(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
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
    struct CountingAttachmentExecutor {
        query_count: AtomicUsize,
    }

    #[async_trait]
    impl QueryExecutor for CountingAttachmentExecutor {
        async fn raw_query_with(
            &self,
            _sql: &str,
            bindings: &[DbValue],
            _options: QueryExecutionOptions,
        ) -> Result<Vec<DbRecord>> {
            self.query_count.fetch_add(1, Ordering::SeqCst);
            let attachable_type = match &bindings[0] {
                DbValue::Text(value) => value.clone(),
                _ => panic!("expected attachable_type binding"),
            };
            let ids = match &bindings[1] {
                DbValue::UuidArray(values) => values.clone(),
                _ => panic!("expected attachable_id uuid array binding"),
            };
            let collection = match &bindings[2] {
                DbValue::Text(value) => value.clone(),
                _ => panic!("expected collection binding"),
            };

            Ok(ids
                .into_iter()
                .map(|id| attachment_record(&attachable_type, id, &collection))
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
    async fn lazy_attachment_cache_batches_known_scope_ids() {
        let executor = CountingAttachmentExecutor::default();
        let first_id = Uuid::now_v7().to_string();
        let second_id = Uuid::now_v7().to_string();

        scope_model_extensions(async {
            current_extension_scope()
                .unwrap()
                .register_model_ids("test_attachables", [first_id.clone(), second_id.clone()]);

            let first = cached_attachments_for_id(&executor, "test_attachables", &first_id, "logo")
                .await
                .unwrap()
                .unwrap();
            let second =
                cached_attachments_for_id(&executor, "test_attachables", &second_id, "logo")
                    .await
                    .unwrap()
                    .unwrap();

            assert_eq!(executor.query_count.load(Ordering::SeqCst), 1);
            assert_eq!(first[0].attachable_id, first_id);
            assert_eq!(second[0].attachable_id, second_id);
        })
        .await;
    }

    #[test]
    fn localized_attachment_collection_uses_stable_locale_suffix() {
        assert_eq!(
            localized_attachment_collection(" banner_image ", " ms "),
            "banner_image:ms"
        );
    }

    fn attachment_record(attachable_type: &str, attachable_id: Uuid, collection: &str) -> DbRecord {
        let mut record = DbRecord::new();
        record.insert("id", DbValue::Uuid(Uuid::now_v7()));
        record.insert(
            "attachable_type",
            DbValue::Text(attachable_type.to_string()),
        );
        record.insert("attachable_id", DbValue::Uuid(attachable_id));
        record.insert("collection", DbValue::Text(collection.to_string()));
        record.insert("disk", DbValue::Text("local".to_string()));
        record.insert("path", DbValue::Text("attachments/test.png".to_string()));
        record.insert("name", DbValue::Text("test.png".to_string()));
        record.insert("original_name", DbValue::Text("test.png".to_string()));
        record.insert("mime_type", DbValue::Text("image/png".to_string()));
        record.insert("size", DbValue::Int64(128));
        record.insert("sort_order", DbValue::Int32(0));
        record.insert("custom_properties", DbValue::Json(serde_json::json!({})));
        record
    }
}
