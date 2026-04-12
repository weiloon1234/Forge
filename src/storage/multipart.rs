use std::collections::HashMap;
use std::path::PathBuf;

use axum::extract::FromRequest;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::foundation::{Error, Result};

/// Extractor that parses all fields from a multipart/form-data request.
///
/// File fields are collected into [`UploadedFile`] instances grouped by field
/// name; text fields are collected as plain strings.
///
/// # Handler usage
///
/// ```ignore
/// use forge::storage::{MultipartForm, UploadedFile};
///
/// async fn upload(form: MultipartForm) -> impl IntoResponse {
///     let avatar: &UploadedFile = form.file("avatar")?;
///     let display_name = form.text("name");
///     // ...
/// }
/// ```
#[derive(Debug)]
pub struct MultipartForm {
    files: HashMap<String, Vec<UploadedFile>>,
    texts: HashMap<String, String>,
}

/// Represents a single file received from a multipart request.
///
/// This is a mirror of [`super::upload::UploadedFile`] used within the
/// multipart module for constructing instances during request parsing.
#[derive(Debug)]
pub struct UploadedFile {
    pub field_name: String,
    pub original_name: Option<String>,
    pub content_type: Option<String>,
    pub size: u64,
    pub temp_path: PathBuf,
}

impl MultipartForm {
    /// Returns the first file uploaded under the given field name.
    ///
    /// Returns an error if no file was uploaded for that field.
    pub fn file(&self, name: &str) -> Result<&UploadedFile> {
        self.files
            .get(name)
            .and_then(|v| v.first())
            .ok_or_else(|| Error::message(format!("no file uploaded for field `{name}`")))
    }

    /// Returns all files uploaded under the given field name.
    pub fn files(&self, name: &str) -> &[UploadedFile] {
        self.files.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Returns the text value of a non-file field, or `None` if absent.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.texts.get(name).map(|s| s.as_str())
    }
}

impl<S> FromRequest<S> for MultipartForm
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

        let mut files: HashMap<String, Vec<UploadedFile>> = HashMap::new();
        let mut texts: HashMap<String, String> = HashMap::new();

        while let Ok(Some(field)) = multipart.next_field().await {
            let field_name = field.name().unwrap_or("").to_string();
            let original_name = field.file_name().map(|s| s.to_string());
            let content_type = field.content_type().map(|s| s.to_string());

            if original_name.is_some() {
                // File field — stream to a temp file.
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

                files.entry(field_name).or_default().push(UploadedFile {
                    field_name: String::new(), // redundant — key already stored
                    original_name,
                    content_type,
                    size,
                    temp_path,
                });
            } else {
                // Text field — collect the full value.
                let text = field
                    .text()
                    .await
                    .map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
                texts.insert(field_name, text);
            }
        }

        Ok(MultipartForm { files, texts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_form_file_returns_first_file() {
        let mut form = MultipartForm {
            files: HashMap::new(),
            texts: HashMap::new(),
        };

        let uploaded = UploadedFile {
            field_name: "avatar".to_string(),
            original_name: Some("photo.png".to_string()),
            content_type: Some("image/png".to_string()),
            size: 2048,
            temp_path: PathBuf::from("/tmp/test-upload"),
        };

        form.files
            .entry("avatar".to_string())
            .or_default()
            .push(uploaded);

        assert!(form.file("avatar").is_ok());
        assert_eq!(
            form.file("avatar").unwrap().original_name.as_deref(),
            Some("photo.png")
        );
        assert!(form.file("missing").is_err());
    }

    #[test]
    fn multipart_form_files_returns_slice() {
        let mut form = MultipartForm {
            files: HashMap::new(),
            texts: HashMap::new(),
        };

        let f1 = UploadedFile {
            field_name: String::new(),
            original_name: Some("a.txt".to_string()),
            content_type: None,
            size: 10,
            temp_path: PathBuf::from("/tmp/a"),
        };
        let f2 = UploadedFile {
            field_name: String::new(),
            original_name: Some("b.txt".to_string()),
            content_type: None,
            size: 20,
            temp_path: PathBuf::from("/tmp/b"),
        };

        form.files
            .entry("docs".to_string())
            .or_default()
            .extend([f1, f2]);

        assert_eq!(form.files("docs").len(), 2);
        assert!(form.files("missing").is_empty());
    }

    #[test]
    fn multipart_form_text_returns_value() {
        let mut form = MultipartForm {
            files: HashMap::new(),
            texts: HashMap::new(),
        };
        form.texts.insert("name".to_string(), "Forge".to_string());

        assert_eq!(form.text("name"), Some("Forge"));
        assert_eq!(form.text("missing"), None);
    }
}
