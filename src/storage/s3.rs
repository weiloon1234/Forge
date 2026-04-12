use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStoreExt;

use crate::foundation::{Error, Result};
use crate::support::DateTime;

use super::adapter::{StorageAdapter, StorageVisibility};
use super::config::ResolvedS3Config;
use super::stored_file::StoredFile;

pub struct S3StorageAdapter {
    inner: Arc<object_store::aws::AmazonS3>,
    bucket: String,
    region: String,
    url_prefix: Option<String>,
}

impl S3StorageAdapter {
    pub fn from_config(config: &ResolvedS3Config) -> Result<Self> {
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(&config.bucket)
            .with_region(&config.region)
            .with_access_key_id(&config.key)
            .with_secret_access_key(&config.secret);

        if let Some(endpoint) = &config.endpoint {
            if !endpoint.is_empty() {
                builder = builder.with_endpoint(endpoint);
            }
        }
        if config.use_path_style {
            builder = builder.with_virtual_hosted_style_request(false);
        }

        let store = builder.build().map_err(Error::other)?;
        Ok(Self {
            inner: Arc::new(store),
            bucket: config.bucket.clone(),
            region: config.region.clone(),
            url_prefix: config.url.clone(),
        })
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
impl StorageAdapter for S3StorageAdapter {
    async fn put_bytes(
        &self,
        path: &str,
        bytes: &[u8],
        content_type: Option<&str>,
        _visibility: StorageVisibility,
    ) -> Result<StoredFile> {
        let object_path = ObjectPath::from(path);
        self.inner
            .put(&object_path, bytes.to_vec().into())
            .await
            .map_err(Error::other)?;

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
        visibility: StorageVisibility,
    ) -> Result<StoredFile> {
        let bytes = tokio::fs::read(temp_path).await.map_err(Error::other)?;
        self.put_bytes(path, &bytes, content_type, visibility).await
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>> {
        let object_path = ObjectPath::from(path);
        let result = self.inner.get(&object_path).await.map_err(Error::other)?;
        let bytes = result.bytes().await.map_err(Error::other)?;
        Ok(bytes.to_vec())
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let object_path = ObjectPath::from(path);
        self.inner.delete(&object_path).await.map_err(Error::other)
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let object_path = ObjectPath::from(path);
        match self.inner.head(&object_path).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(Error::other(e)),
        }
    }

    async fn copy(&self, from: &str, to: &str) -> Result<()> {
        let from_path = ObjectPath::from(from);
        let to_path = ObjectPath::from(to);
        self.inner
            .copy(&from_path, &to_path)
            .await
            .map_err(Error::other)
    }

    async fn move_to(&self, from: &str, to: &str) -> Result<()> {
        let from_path = ObjectPath::from(from);
        let to_path = ObjectPath::from(to);
        self.inner
            .rename(&from_path, &to_path)
            .await
            .map_err(Error::other)
    }

    async fn url(&self, path: &str) -> Result<String> {
        match &self.url_prefix {
            Some(prefix) => Ok(format!("{prefix}/{path}")),
            None => Ok(format!(
                "https://{}.s3.{}.amazonaws.com/{path}",
                self.bucket, self.region
            )),
        }
    }

    async fn temporary_url(&self, path: &str, expires_at: DateTime) -> Result<String> {
        use object_store::signer::Signer;
        use std::time::Duration;

        let now_ms = DateTime::now().timestamp_millis();
        let expires_ms = expires_at.timestamp_millis();
        let secs = (expires_ms - now_ms) / 1000;
        if secs <= 0 {
            return Err(Error::message("expiration must be in the future"));
        }

        let object_path = ObjectPath::from(path);
        let url = self
            .inner
            .signed_url(
                reqwest::Method::GET,
                &object_path,
                Duration::from_secs(secs as u64),
            )
            .await
            .map_err(Error::other)?;

        Ok(url.to_string())
    }
}
