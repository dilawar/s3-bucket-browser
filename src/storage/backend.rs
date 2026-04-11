use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;

use super::path::{StorageEntry, StoragePath};

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
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

    /// Recursively list all *files* under `path` (directories are expanded).
    /// The default implementation calls `list()` repeatedly; backends may
    /// override this with a more efficient single-request approach.
    async fn list_recursive(&self, path: &StoragePath) -> Result<Vec<StorageEntry>> {
        use super::path::EntryKind;
        let mut files = Vec::new();
        let mut dirs = vec![path.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in self.list(&dir).await? {
                match entry.kind {
                    EntryKind::Directory => dirs.push(entry.path),
                    EntryKind::File => files.push(entry),
                }
            }
        }
        Ok(files)
    }

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

    /// Create a virtual directory. Default: put a zero-byte `.keep` placeholder.
    async fn create_dir(&self, path: &StoragePath) -> Result<()> {
        self.put(&path.child_file(".keep"), bytes::Bytes::new()).await
    }

    /// Rename a file: copy to new path then delete original.
    /// Only for files; directory rename is not supported.
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<()> {
        let data = self.get(from).await?;
        self.put(to, data).await?;
        self.delete(from).await?;
        Ok(())
    }

    /// Short human-readable name shown in the status bar.
    fn name(&self) -> &str;
}
