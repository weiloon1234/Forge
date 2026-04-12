use std::path::{Path, PathBuf};

use axum::extract::FromRequest;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::foundation::{AppContext, Result};
use serde::de::{self, Deserialize, Deserializer};

use super::stored_file::StoredFile;
use super::StorageManager;

/// Represents a file received from an HTTP request (multipart upload).
///
/// Contains metadata about the upload (original name, content type, size)
/// and the temporary path where the file body was written by the HTTP layer.
///
/// Helper methods generate safe storage names and paths.
#[derive(Debug)]
pub struct UploadedFile {
    pub field_name: String,
    pub original_name: Option<String>,
    pub content_type: Option<String>,
    pub size: u64,
    pub temp_path: PathBuf,
}

/// `UploadedFile` cannot be deserialized from JSON — it is populated
/// exclusively via multipart extraction (`FromMultipart`). This impl
/// exists to satisfy `Deserialize` bounds on structs that contain both
/// text fields and file fields.
impl<'de> Deserialize<'de> for UploadedFile {
    fn deserialize<D>(_deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(de::Error::custom(
            "UploadedFile cannot be deserialized from JSON; use multipart/form-data",
        ))
    }
}

impl UploadedFile {
    /// Generates a UUIDv7-based filename, preserving a safe normalized extension.
    pub fn generate_storage_name(&self) -> String {
        let uuid = uuid::Uuid::now_v7().to_string();
        match self.original_extension() {
            Some(ext) => format!("{uuid}.{ext}"),
            None => uuid,
        }
    }

    /// Extracts and normalizes the file extension from the original filename.
    ///
    /// Returns `None` if there is no extension, or if the extension contains
    /// dangerous characters (path separators) or exceeds 32 characters.
    pub fn original_extension(&self) -> Option<String> {
        self.original_name
            .as_ref()
            .and_then(|n| Path::new(n).extension())
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_lowercase())
            .filter(|ext| !ext.contains('/') && !ext.contains('\\') && ext.len() <= 32)
    }

    /// Normalizes a user-provided filename by stripping any path components,
    /// keeping only the final file name segment.
    pub fn normalize_name(name: &str) -> String {
        Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(name)
            .to_string()
    }

    /// Builds a storage path from a directory and filename.
    fn storage_path(dir: &str, name: &str) -> String {
        format!("{dir}/{name}")
    }

    /// Stores the uploaded file on the default disk in the given directory.
    ///
    /// Generates a unique filename (UUIDv7-based) preserving the original extension.
    pub async fn store(&self, app: &AppContext, dir: &str) -> Result<StoredFile> {
        let storage = app.resolve::<StorageManager>()?;
        let disk = storage.default_disk()?;
        let name = self.generate_storage_name();
        let path = Self::storage_path(dir, &name);
        disk.put_file(&path, &self.temp_path, self.content_type.as_deref())
            .await
    }

    /// Stores the uploaded file on a named disk in the given directory.
    ///
    /// Generates a unique filename (UUIDv7-based) preserving the original extension.
    pub async fn store_on(
        &self,
        app: &AppContext,
        disk_name: &str,
        dir: &str,
    ) -> Result<StoredFile> {
        let storage = app.resolve::<StorageManager>()?;
        let disk = storage.disk(disk_name)?;
        let name = self.generate_storage_name();
        let path = Self::storage_path(dir, &name);
        disk.put_file(&path, &self.temp_path, self.content_type.as_deref())
            .await
    }

    /// Stores the uploaded file on the default disk with a custom filename.
    ///
    /// The name is normalized (path components stripped) before storage.
    pub async fn store_as(&self, app: &AppContext, dir: &str, name: &str) -> Result<StoredFile> {
        let storage = app.resolve::<StorageManager>()?;
        let disk = storage.default_disk()?;
        let safe_name = Self::normalize_name(name);
        let path = Self::storage_path(dir, &safe_name);
        disk.put_file(&path, &self.temp_path, self.content_type.as_deref())
            .await
    }

    /// Stores the uploaded file on a named disk with a custom filename.
    ///
    /// The name is normalized (path components stripped) before storage.
    pub async fn store_as_on(
        &self,
        app: &AppContext,
        disk_name: &str,
        dir: &str,
        name: &str,
    ) -> Result<StoredFile> {
        let storage = app.resolve::<StorageManager>()?;
        let disk = storage.disk(disk_name)?;
        let safe_name = Self::normalize_name(name);
        let path = Self::storage_path(dir, &safe_name);
        disk.put_file(&path, &self.temp_path, self.content_type.as_deref())
            .await
    }
}

/// Extracts the first file field from a multipart request.
///
/// Returns `400 Bad Request` if no file field is found in the request body.
impl<S> FromRequest<S> for UploadedFile
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(
        req: axum::http::Request<axum::body::Body>,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        let mut multipart = axum::extract::Multipart::from_request(req, state)
            .await
            .map_err(|rejection| rejection.into_response())?;

        while let Ok(Some(field)) = multipart.next_field().await {
            let field_name = field.name().unwrap_or("").to_string();
            let original_name = field.file_name().map(|s| s.to_string());
            let content_type = field.content_type().map(|s| s.to_string());

            if original_name.is_some() {
                let temp_id = uuid::Uuid::now_v7().to_string();
                let temp_path = std::env::temp_dir().join(format!("forge-upload-{temp_id}"));

                let mut file = tokio::fs::File::create(&temp_path)
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

                let mut size: u64 = 0;
                let mut field = field;
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| StatusCode::BAD_REQUEST.into_response())?
                {
                    size += chunk.len() as u64;
                    tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                        .await
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
                }

                return Ok(UploadedFile {
                    field_name,
                    original_name,
                    content_type,
                    size,
                    temp_path,
                });
            }
        }

        Err(StatusCode::BAD_REQUEST.into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_upload(original_name: Option<&str>) -> UploadedFile {
        UploadedFile {
            field_name: "file".to_string(),
            original_name: original_name.map(|s| s.to_string()),
            content_type: Some("image/png".to_string()),
            size: 1024,
            temp_path: PathBuf::from("/tmp/upload123"),
        }
    }

    #[test]
    fn generate_storage_name_produces_uuid_with_extension() {
        let upload = make_upload(Some("photo.JPG"));
        let name = upload.generate_storage_name();

        // UUIDv7 format: 8-4-4-4-12 hex chars, then .jpg
        let parts: Vec<&str> = name.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], "jpg"); // normalized to lowercase

        let uuid_part = parts[0];
        let segments: Vec<&str> = uuid_part.split('-').collect();
        assert_eq!(segments.len(), 5);
        assert_eq!(segments[0].len(), 8);
        assert_eq!(segments[1].len(), 4);
        assert_eq!(segments[2].len(), 4);
        assert_eq!(segments[3].len(), 4);
        assert_eq!(segments[4].len(), 12);
    }

    #[test]
    fn generate_storage_name_without_original_name_is_just_uuid() {
        let upload = make_upload(None);
        let name = upload.generate_storage_name();

        // No extension — just the UUID
        assert!(!name.contains('.'));
        let segments: Vec<&str> = name.split('-').collect();
        assert_eq!(segments.len(), 5);
        assert_eq!(segments[0].len(), 8);
    }

    #[test]
    fn original_extension_normalizes_to_lowercase() {
        let upload = make_upload(Some("document.PDF"));
        assert_eq!(upload.original_extension(), Some("pdf".to_string()));
    }

    #[test]
    fn original_extension_strips_dangerous_slash() {
        let upload = make_upload(Some("file.sh/evil"));
        assert_eq!(upload.original_extension(), None);
    }

    #[test]
    fn original_extension_strips_dangerous_backslash() {
        let upload = make_upload(Some("file.exe\\evil"));
        assert_eq!(upload.original_extension(), None);
    }

    #[test]
    fn original_extension_returns_none_for_no_extension() {
        let upload = make_upload(Some("README"));
        assert_eq!(upload.original_extension(), None);
    }

    #[test]
    fn original_extension_returns_none_for_none_original_name() {
        let upload = make_upload(None);
        assert_eq!(upload.original_extension(), None);
    }

    #[test]
    fn original_extension_rejects_overly_long_extension() {
        let long_ext = "a".repeat(33);
        let upload = make_upload(Some(&format!("file.{long_ext}")));
        assert_eq!(upload.original_extension(), None);
    }

    #[test]
    fn original_extension_accepts_max_length_extension() {
        let ext = "a".repeat(32);
        let upload = make_upload(Some(&format!("file.{ext}")));
        assert_eq!(upload.original_extension(), Some(ext));
    }

    #[test]
    fn normalize_name_strips_path_components() {
        // Unix-style paths are stripped by std::path::Path::file_name
        assert_eq!(UploadedFile::normalize_name("/etc/passwd"), "passwd");
        assert_eq!(
            UploadedFile::normalize_name("subdir/photo.jpg"),
            "photo.jpg"
        );
        assert_eq!(UploadedFile::normalize_name("simple.txt"), "simple.txt");
    }

    #[test]
    fn normalize_name_returns_input_for_bare_name() {
        assert_eq!(UploadedFile::normalize_name("photo.jpg"), "photo.jpg");
    }

    #[test]
    fn storage_path_combines_dir_and_name() {
        let path = UploadedFile::storage_path("avatars", "uuid.png");
        assert_eq!(path, "avatars/uuid.png");
    }
}
