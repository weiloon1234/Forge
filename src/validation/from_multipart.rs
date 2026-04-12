use async_trait::async_trait;

use crate::foundation::Result;

/// Trait for extracting a typed struct from a multipart form-data request.
///
/// This trait is automatically implemented by the `#[derive(Validate)]` macro
/// when the struct contains `UploadedFile` or `Option<UploadedFile>` fields.
///
/// Text fields are extracted as strings and parsed into the target type via
/// `FromStr`. File fields are streamed to a temporary file and wrapped in
/// `UploadedFile`.
#[async_trait]
pub trait FromMultipart: Send + Sized {
    /// Extract fields from a multipart stream.
    ///
    /// The implementation iterates over all fields, matching by name, and
    /// populates the struct fields accordingly.
    async fn from_multipart(multipart: &mut axum::extract::Multipart) -> Result<Self>;
}
