use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;

use super::path::{StorageEntry, StoragePath};

#[async_trait]
pub trait Backend: Send + Sync + 'static {
    /// List the immediate children of `path`.
    async fn list(&self, path: &StoragePath) -> Result<Vec<StorageEntry>>;

    /// Download the full content of the object at `path`.
    async fn get(&self, path: &StoragePath) -> Result<Bytes>;

    /// Upload `data` to `path`, creating or replacing the object.
    async fn put(&self, path: &StoragePath, data: Bytes) -> Result<()>;

    /// Delete the object at `path`, or (for directory paths ending with `/`)
    /// recursively delete every object under that prefix.
    async fn delete(&self, path: &StoragePath) -> Result<()>;

    /// Return a public (unauthenticated) URL for `path`, if the backend
    /// can construct one without a network call. Returns `None` for backends
    /// that don't have a stable public URL (e.g. private buckets).
    fn public_url(&self, path: &StoragePath) -> Option<String> {
        let _ = path;
        None
    }

    /// Return a pre-signed GET URL for `path` valid for `expires`.
    /// Returns `Err` if the backend does not support presigning.
    async fn presign_url(&self, path: &StoragePath, expires: Duration) -> Result<String> {
        let _ = (path, expires);
        anyhow::bail!("presigned URLs are not supported by this storage backend")
    }

    /// Short human-readable name shown in the status bar.
    fn name(&self) -> &str;
}
