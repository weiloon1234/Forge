use crate::database::{DbType, DbValue};
use crate::foundation::{AppContext, Error, Result};
use crate::storage::UploadedFile;
use crate::support::DateTime;

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
        self.image_transforms.push(ImageTransform::Resize(width, height));
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
        self.image_transforms
            .push(ImageTransform::Quality(quality));
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
        Attachment::upload(file)
            .collection(collection)
            .store(app, Self::attachable_type(), &self.attachable_id())
            .await
    }

    async fn attachment(
        &self,
        app: &AppContext,
        collection: &str,
    ) -> Result<Option<Attachment>> {
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

    async fn attachments(
        &self,
        app: &AppContext,
        collection: &str,
    ) -> Result<Vec<Attachment>> {
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

        db.raw_execute(
            "DELETE FROM attachments \
             WHERE attachable_type = $1 AND attachable_id = $2::uuid AND collection = $3",
            &[
                DbValue::Text(Self::attachable_type().to_string()),
                DbValue::Text(self.attachable_id()),
                DbValue::Text(collection.to_string()),
            ],
        )
        .await
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

fn row_to_attachment(row: &crate::database::DbRecord) -> Attachment {
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

