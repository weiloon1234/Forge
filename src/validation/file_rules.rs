use std::path::Path;

use crate::foundation::{Error, Result};
use crate::storage::UploadedFile;

/// Check if a file is an image by reading magic bytes.
pub async fn is_image(file: &UploadedFile) -> Result<bool> {
    let bytes = tokio::fs::read(&file.temp_path)
        .await
        .map_err(|e| Error::message(format!("failed to read uploaded file: {e}")))?;
    let is_img: bool = infer::is_image(&bytes);
    Ok(is_img)
}

/// Check if file size is within limit (in KB).
pub fn check_max_size(file: &UploadedFile, max_kb: u64) -> bool {
    file.size <= max_kb * 1024
}

/// Check if image dimensions are within limits.
/// Returns (width, height) if the file is a valid image.
pub async fn get_image_dimensions(file: &UploadedFile) -> Result<(u32, u32)> {
    let bytes = tokio::fs::read(&file.temp_path)
        .await
        .map_err(|e| Error::message(format!("failed to read uploaded file: {e}")))?;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::message(format!("failed to detect image format: {e}")))?;
    let dims = reader
        .into_dimensions()
        .map_err(|e| Error::message(format!("failed to read image dimensions: {e}")))?;
    Ok(dims)
}

/// Check if MIME type is in allowed list.
/// Checks magic bytes first (most reliable), then falls back to content-type header.
pub async fn check_allowed_mimes(file: &UploadedFile, allowed: &[String]) -> Result<bool> {
    // Try magic bytes first (most reliable)
    let bytes = tokio::fs::read(&file.temp_path)
        .await
        .map_err(|e| Error::message(format!("failed to read uploaded file: {e}")))?;
    if let Some(kind) = infer::get(&bytes) {
        let mime = kind.mime_type();
        return Ok(allowed.iter().any(|a| a == mime));
    }
    // Fallback to content-type header
    if let Some(ref ct) = file.content_type {
        return Ok(allowed.iter().any(|a| a == ct));
    }
    Ok(false)
}

/// Check if file extension is in allowed list.
pub fn check_allowed_extensions(file: &UploadedFile, allowed: &[String]) -> bool {
    if let Some(ref name) = file.original_name {
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());
        if let Some(ext) = ext {
            return allowed.iter().any(|a| a.to_lowercase() == ext);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file_with_size(size: u64) -> UploadedFile {
        let temp_dir = std::env::temp_dir().join("forge-test-file-rules");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let temp_path = temp_dir.join(format!("test-{}", uuid::Uuid::now_v7()));
        std::fs::write(&temp_path, vec![0u8; size as usize]).unwrap();

        UploadedFile {
            field_name: "file".to_string(),
            original_name: Some("test.png".to_string()),
            content_type: Some("image/png".to_string()),
            size,
            temp_path,
        }
    }

    fn make_file_with_content(content: &[u8], name: &str) -> UploadedFile {
        let temp_dir = std::env::temp_dir().join("forge-test-file-rules");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let temp_path = temp_dir.join(format!("test-{}", uuid::Uuid::now_v7()));
        std::fs::write(&temp_path, content).unwrap();

        UploadedFile {
            field_name: "file".to_string(),
            original_name: Some(name.to_string()),
            content_type: Some("application/octet-stream".to_string()),
            size: content.len() as u64,
            temp_path,
        }
    }

    #[test]
    fn check_max_size_within_limit() {
        let file = make_file_with_size(1024 * 100); // 100KB
        assert!(check_max_size(&file, 200)); // 200KB limit
    }

    #[test]
    fn check_max_size_over_limit() {
        let file = make_file_with_size(1024 * 300); // 300KB
        assert!(!check_max_size(&file, 200)); // 200KB limit
    }

    #[test]
    fn check_max_size_exact_limit() {
        let file = make_file_with_size(1024 * 200); // exactly 200KB
        assert!(check_max_size(&file, 200));
    }

    #[test]
    fn check_allowed_extensions_match() {
        let file = make_file_with_size(100);
        let allowed: Vec<String> = vec!["jpg".into(), "png".into(), "webp".into()];
        assert!(check_allowed_extensions(&file, &allowed));
    }

    #[test]
    fn check_allowed_extensions_no_match() {
        let mut file = make_file_with_size(100);
        file.original_name = Some("document.pdf".to_string());
        let allowed: Vec<String> = vec!["jpg".into(), "png".into()];
        assert!(!check_allowed_extensions(&file, &allowed));
    }

    #[test]
    fn check_allowed_extensions_case_insensitive() {
        let mut file = make_file_with_size(100);
        file.original_name = Some("photo.JPG".to_string());
        let allowed: Vec<String> = vec!["jpg".into()];
        assert!(check_allowed_extensions(&file, &allowed));
    }

    #[tokio::test]
    async fn is_image_detects_png() {
        // PNG magic bytes
        let png_header = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        let file = make_file_with_content(&png_header, "test.png");
        assert!(is_image(&file).await.unwrap());
    }

    #[tokio::test]
    async fn is_image_rejects_non_image() {
        let file = make_file_with_content(b"hello world", "test.txt");
        assert!(!is_image(&file).await.unwrap());
    }
}
