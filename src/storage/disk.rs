use std::path::Path;
use std::sync::Arc;

use crate::foundation::Result;
use crate::support::DateTime;

use super::adapter::{StorageAdapter, StorageVisibility};
use super::stored_file::StoredFile;

#[derive(Clone)]
pub struct StorageDisk {
    name: String,
    visibility: StorageVisibility,
    adapter: Arc<dyn StorageAdapter>,
}

impl std::fmt::Debug for StorageDisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageDisk")
            .field("name", &self.name)
            .field("visibility", &self.visibility)
            .finish()
    }
}

impl StorageDisk {
    pub(crate) fn new(
        name: String,
        visibility: StorageVisibility,
        adapter: Arc<dyn StorageAdapter>,
    ) -> Self {
        Self {
            name,
            visibility,
            adapter,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn visibility(&self) -> StorageVisibility {
        self.visibility
    }

    pub async fn put(&self, path: &str, contents: impl AsRef<[u8]>) -> Result<StoredFile> {
        let mut file = self
            .adapter
            .put_bytes(path, contents.as_ref(), None, self.visibility)
            .await?;
        file.disk = self.name.clone();
        Ok(file)
    }

    pub async fn put_bytes(&self, path: &str, bytes: impl AsRef<[u8]>) -> Result<StoredFile> {
        let mut file = self
            .adapter
            .put_bytes(path, bytes.as_ref(), None, self.visibility)
            .await?;
        file.disk = self.name.clone();
        Ok(file)
    }

    pub async fn put_file(
        &self,
        path: &str,
        temp_path: &Path,
        content_type: Option<&str>,
    ) -> Result<StoredFile> {
        let mut file = self
            .adapter
            .put_file(path, temp_path, content_type, self.visibility)
            .await?;
        file.disk = self.name.clone();
        Ok(file)
    }

    pub async fn get(&self, path: &str) -> Result<Vec<u8>> {
        self.adapter.get(path).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        self.adapter.delete(path).await
    }

    pub async fn exists(&self, path: &str) -> Result<bool> {
        self.adapter.exists(path).await
    }

    pub async fn copy(&self, from: &str, to: &str) -> Result<()> {
        self.adapter.copy(from, to).await
    }

    pub async fn move_to(&self, from: &str, to: &str) -> Result<()> {
        self.adapter.move_to(from, to).await
    }

    pub async fn url(&self, path: &str) -> Result<String> {
        self.adapter.url(path).await
    }

    pub async fn temporary_url(&self, path: &str, expires_at: DateTime) -> Result<String> {
        self.adapter.temporary_url(path, expires_at).await
    }
}
