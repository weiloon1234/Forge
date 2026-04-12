use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::foundation::{Error, Result};
use crate::support::DateTime;

use super::adapter::{StorageAdapter, StorageVisibility};
use super::config::ResolvedLocalConfig;
use super::stored_file::StoredFile;

pub struct LocalStorageAdapter {
    root: PathBuf,
    url: Option<String>,
}

impl LocalStorageAdapter {
    pub fn from_config(config: &ResolvedLocalConfig) -> Result<Self> {
        Ok(Self {
            root: PathBuf::from(&config.root),
            url: config.url.clone(),
        })
    }

    fn full_path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    fn file_name(path: &str) -> String {
        Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_string()
    }
}

#[async_trait]
impl StorageAdapter for LocalStorageAdapter {
    async fn put_bytes(
        &self,
        path: &str,
        bytes: &[u8],
        content_type: Option<&str>,
        _visibility: StorageVisibility,
    ) -> Result<StoredFile> {
        let full = self.full_path(path);

        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(Error::other)?;
        }

        tokio::fs::write(&full, bytes).await.map_err(Error::other)?;

        Ok(StoredFile {
            disk: String::new(),
            path: path.to_string(),
            name: Self::file_name(path),
            size: bytes.len() as u64,
            content_type: content_type.map(|s| s.to_string()),
            url: None,
        })
    }

    async fn put_file(
        &self,
        path: &str,
        temp_path: &Path,
        content_type: Option<&str>,
        _visibility: StorageVisibility,
    ) -> Result<StoredFile> {
        let full = self.full_path(path);

        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(Error::other)?;
        }

        let metadata = tokio::fs::copy(temp_path, &full)
            .await
            .map_err(Error::other)?;

        Ok(StoredFile {
            disk: String::new(),
            path: path.to_string(),
            name: Self::file_name(path),
            size: metadata,
            content_type: content_type.map(|s| s.to_string()),
            url: None,
        })
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>> {
        let full = self.full_path(path);
        tokio::fs::read(&full)
            .await
            .map_err(|e| Error::message(format!("Failed to read file '{path}': {e}")))
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let full = self.full_path(path);
        tokio::fs::remove_file(&full)
            .await
            .map_err(|e| Error::message(format!("Failed to delete file '{path}': {e}")))
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let full = self.full_path(path);
        match tokio::fs::metadata(&full).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::other(e)),
        }
    }

    async fn copy(&self, from: &str, to: &str) -> Result<()> {
        let src = self.full_path(from);
        let dst = self.full_path(to);

        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(Error::other)?;
        }

        tokio::fs::copy(&src, &dst)
            .await
            .map_err(|e| Error::message(format!("Failed to copy '{from}' to '{to}': {e}")))?;

        Ok(())
    }

    async fn move_to(&self, from: &str, to: &str) -> Result<()> {
        let src = self.full_path(from);
        let dst = self.full_path(to);

        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(Error::other)?;
        }

        if let Err(e) = tokio::fs::rename(&src, &dst).await {
            if e.raw_os_error() == Some(18)
                || e.to_string().contains("cross-device")
                || e.to_string().contains("Invalid cross-device link")
            {
                let data = tokio::fs::read(&src).await.map_err(Error::other)?;
                tokio::fs::write(&dst, &data).await.map_err(Error::other)?;
                tokio::fs::remove_file(&src).await.map_err(Error::other)?;
            } else {
                return Err(Error::message(format!(
                    "Failed to move '{from}' to '{to}': {e}"
                )));
            }
        }

        Ok(())
    }

    async fn url(&self, path: &str) -> Result<String> {
        match &self.url {
            Some(base) => Ok(format!("{base}/{path}")),
            None => Err(Error::message(
                "URL generation not supported for this disk (no url configured)",
            )),
        }
    }

    async fn temporary_url(&self, _path: &str, _expires_at: DateTime) -> Result<String> {
        Err(Error::message(
            "Temporary URLs are not supported for local disk",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tempfile::TempDir;

    use super::*;

    fn make_adapter(dir: &TempDir) -> LocalStorageAdapter {
        LocalStorageAdapter {
            root: dir.path().to_path_buf(),
            url: None,
        }
    }

    fn make_adapter_with_url(dir: &TempDir, url: &str) -> LocalStorageAdapter {
        LocalStorageAdapter {
            root: dir.path().to_path_buf(),
            url: Some(url.to_string()),
        }
    }

    #[tokio::test]
    async fn put_bytes_and_read_back() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        let file = adapter
            .put_bytes(
                "hello.txt",
                b"hello world",
                None,
                StorageVisibility::Private,
            )
            .await
            .unwrap();

        assert_eq!(file.path, "hello.txt");
        assert_eq!(file.name, "hello.txt");
        assert_eq!(file.size, 11);
        assert!(file.disk.is_empty());

        let data = adapter.get("hello.txt").await.unwrap();
        assert_eq!(data, b"hello world");
    }

    #[tokio::test]
    async fn put_file_and_read_back() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        let temp = TempDir::new().unwrap();
        let temp_file_path = temp.path().join("upload.bin");
        {
            let mut f = std::fs::File::create(&temp_file_path).unwrap();
            f.write_all(b"file contents").unwrap();
        }

        let file = adapter
            .put_file(
                "uploads/file.bin",
                &temp_file_path,
                Some("application/octet-stream"),
                StorageVisibility::Public,
            )
            .await
            .unwrap();

        assert_eq!(file.path, "uploads/file.bin");
        assert_eq!(file.name, "file.bin");
        assert_eq!(file.size, 13);
        assert_eq!(
            file.content_type.as_deref(),
            Some("application/octet-stream")
        );

        let data = adapter.get("uploads/file.bin").await.unwrap();
        assert_eq!(data, b"file contents");
    }

    #[tokio::test]
    async fn delete_file() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        adapter
            .put_bytes("to_delete.txt", b"bye", None, StorageVisibility::Private)
            .await
            .unwrap();

        adapter.delete("to_delete.txt").await.unwrap();

        assert!(!adapter.exists("to_delete.txt").await.unwrap());
    }

    #[tokio::test]
    async fn exists_returns_true_for_existing_file() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        adapter
            .put_bytes("exists.txt", b"data", None, StorageVisibility::Private)
            .await
            .unwrap();

        assert!(adapter.exists("exists.txt").await.unwrap());
    }

    #[tokio::test]
    async fn exists_returns_false_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        assert!(!adapter.exists("missing.txt").await.unwrap());
    }

    #[tokio::test]
    async fn copy_file() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        adapter
            .put_bytes("original.txt", b"copy me", None, StorageVisibility::Private)
            .await
            .unwrap();

        adapter.copy("original.txt", "copy.txt").await.unwrap();

        let original = adapter.get("original.txt").await.unwrap();
        let copy = adapter.get("copy.txt").await.unwrap();
        assert_eq!(original, copy);
    }

    #[tokio::test]
    async fn move_file() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        adapter
            .put_bytes("src.txt", b"move me", None, StorageVisibility::Private)
            .await
            .unwrap();

        adapter.move_to("src.txt", "dst.txt").await.unwrap();

        assert!(!adapter.exists("src.txt").await.unwrap());
        let data = adapter.get("dst.txt").await.unwrap();
        assert_eq!(data, b"move me");
    }

    #[tokio::test]
    async fn url_returns_url_when_configured() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter_with_url(&dir, "http://localhost/storage");

        let url = adapter.url("images/photo.jpg").await.unwrap();
        assert_eq!(url, "http://localhost/storage/images/photo.jpg");
    }

    #[tokio::test]
    async fn url_returns_error_when_not_configured() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        let result = adapter.url("test.txt").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("URL"));
    }

    #[tokio::test]
    async fn temporary_url_always_errors() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        let result = adapter.temporary_url("test.txt", DateTime::now()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Temporary"));
    }

    #[tokio::test]
    async fn parent_directories_are_auto_created() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        adapter
            .put_bytes(
                "a/b/c/deep.txt",
                b"nested",
                None,
                StorageVisibility::Private,
            )
            .await
            .unwrap();

        let data = adapter.get("a/b/c/deep.txt").await.unwrap();
        assert_eq!(data, b"nested");
    }

    #[tokio::test]
    async fn delete_missing_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        let result = adapter.delete("nonexistent.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn from_config_creates_adapter() {
        let config = ResolvedLocalConfig {
            root: "/tmp/test-storage".to_string(),
            url: Some("http://example.com/files".to_string()),
            visibility: StorageVisibility::Public,
        };

        let adapter = LocalStorageAdapter::from_config(&config).unwrap();
        assert_eq!(adapter.root, PathBuf::from("/tmp/test-storage"));
        assert_eq!(adapter.url.as_deref(), Some("http://example.com/files"));
    }

    #[tokio::test]
    async fn put_bytes_with_content_type() {
        let dir = TempDir::new().unwrap();
        let adapter = make_adapter(&dir);

        let file = adapter
            .put_bytes(
                "data.json",
                b"{}",
                Some("application/json"),
                StorageVisibility::Private,
            )
            .await
            .unwrap();

        assert_eq!(file.content_type.as_deref(), Some("application/json"));
    }
}
