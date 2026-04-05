use std::path::PathBuf;

use anyhow::{Result, bail};
use async_trait::async_trait;
use bytes::Bytes;
use tracing::{debug, warn};

use super::backend::Backend;
use super::path::{EntryKind, StorageEntry, StoragePath, sort_entries};

pub struct LocalBackend;

#[async_trait]
impl Backend for LocalBackend {
    async fn list(&self, path: &StoragePath) -> Result<Vec<StorageEntry>> {
        let StoragePath::Local(dir) = path else {
            bail!("LocalBackend cannot handle {path:?}");
        };
        Ok(list_dir(dir, path))
    }

    async fn get(&self, path: &StoragePath) -> Result<Bytes> {
        let StoragePath::Local(p) = path else {
            bail!("LocalBackend cannot handle {path:?}");
        };
        Ok(Bytes::from(tokio::fs::read(p).await?))
    }

    async fn put(&self, path: &StoragePath, data: Bytes) -> Result<()> {
        let StoragePath::Local(p) = path else {
            bail!("LocalBackend cannot handle {path:?}");
        };
        tokio::fs::write(p, data).await?;
        Ok(())
    }

    async fn delete(&self, path: &StoragePath) -> Result<()> {
        let StoragePath::Local(p) = path else {
            bail!("LocalBackend cannot handle {path:?}");
        };
        if p.is_dir() {
            tokio::fs::remove_dir_all(p).await?;
        } else {
            tokio::fs::remove_file(p).await?;
        }
        Ok(())
    }

    fn public_url(&self, path: &StoragePath) -> Option<String> {
        let StoragePath::Local(p) = path else { return None; };
        // file:// URL so it can be opened directly from a browser or file manager.
        Some(format!("file://{}", p.display()))
    }

    fn name(&self) -> &str {
        "Local"
    }
}

fn list_dir(dir: &PathBuf, parent: &StoragePath) -> Vec<StorageEntry> {
    debug!("Listing {:?}", dir);
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        warn!("Cannot read {:?}", dir);
        return vec![];
    };

    let mut entries: Vec<StorageEntry> = read_dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let meta = e.metadata().ok()?;
            let kind = if meta.is_dir() {
                EntryKind::Directory
            } else {
                EntryKind::File
            };
            let size = kind.is_file().then_some(meta.len());
            let last_modified = meta.modified().ok().map(|t| t.into());
            let path = parent.child(&name);
            // For local files the child() appends "/" for dirs; strip trailing slash from name
            Some(StorageEntry {
                name,
                path,
                kind,
                size,
                last_modified,
            })
        })
        .collect();

    sort_entries(&mut entries);
    entries
}
