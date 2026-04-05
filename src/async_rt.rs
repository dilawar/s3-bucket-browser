use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use crate::storage::{Backend, StorageEntry, StoragePath};

// ── ListingHandle ─────────────────────────────────────────────────────────────

pub struct ListingHandle {
    slot: Arc<Mutex<Option<Result<Vec<StorageEntry>>>>>,
    join: tokio::task::JoinHandle<()>,
}

impl ListingHandle {
    pub fn try_recv(&self) -> Option<Result<Vec<StorageEntry>>> {
        if let Some(result) = self.slot.lock().unwrap().take() {
            return Some(result);
        }
        if self.join.is_finished() {
            return Some(Err(anyhow::anyhow!(
                "listing task panicked — check S3 credentials and endpoint"
            )));
        }
        None
    }
}

pub fn spawn_listing(
    backend: Arc<dyn Backend>,
    path: StoragePath,
    ctx: egui::Context,
    rt: &tokio::runtime::Handle,
) -> ListingHandle {
    let slot: Arc<Mutex<Option<Result<Vec<StorageEntry>>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let join = rt.spawn(async move {
        let result = backend.list(&path).await;
        *slot2.lock().unwrap() = Some(result);
        ctx.request_repaint();
    });
    ListingHandle { slot, join }
}

// ── TransferHandle ────────────────────────────────────────────────────────────

pub struct TransferHandle {
    slot: Arc<Mutex<Option<Result<String>>>>,
    join: tokio::task::JoinHandle<()>,
}

impl TransferHandle {
    pub fn try_recv(&self) -> Option<Result<String>> {
        if let Some(result) = self.slot.lock().unwrap().take() {
            return Some(result);
        }
        if self.join.is_finished() {
            return Some(Err(anyhow::anyhow!("transfer task panicked")));
        }
        None
    }

    pub fn is_running(&self) -> bool {
        !self.join.is_finished() && self.slot.lock().unwrap().is_none()
    }
}

// ── Upload ────────────────────────────────────────────────────────────────────

pub fn spawn_upload(
    backend: Arc<dyn Backend>,
    current_path: StoragePath,
    ctx: egui::Context,
    rt: &tokio::runtime::Handle,
) -> TransferHandle {
    spawn_transfer(rt, ctx, move || do_upload(backend, current_path))
}

async fn do_upload(backend: Arc<dyn Backend>, current_path: StoragePath) -> Result<String> {
    let local_path = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_file()).await?;
    let Some(local_path) = local_path else {
        return Ok("Upload cancelled.".to_owned());
    };
    let file_name = local_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "upload".to_owned());
    let dest = current_path.child_file(&file_name);
    let data = bytes::Bytes::from(tokio::fs::read(&local_path).await?);
    let size = data.len();
    backend.put(&dest, data).await?;
    Ok(format!("Uploaded {file_name} ({size} bytes) → {dest}"))
}

// ── Delete ────────────────────────────────────────────────────────────────────

/// Spawn a delete task that removes every path in `paths` sequentially.
/// Directories (S3 prefixes ending with `/`) are deleted recursively.
pub fn spawn_delete(
    backend: Arc<dyn Backend>,
    paths: Vec<StoragePath>,
    ctx: egui::Context,
    rt: &tokio::runtime::Handle,
) -> TransferHandle {
    spawn_transfer(rt, ctx, move || do_delete(backend, paths))
}

async fn do_delete(backend: Arc<dyn Backend>, paths: Vec<StoragePath>) -> Result<String> {
    let n = paths.len();
    for path in &paths {
        backend.delete(path).await?;
    }
    Ok(format!("Deleted {n} item{}", if n == 1 { "" } else { "s" }))
}

// ── Presign ───────────────────────────────────────────────────────────────────

/// Spawn a task that generates a pre-signed GET URL valid for 24 hours.
pub fn spawn_presign(
    backend: Arc<dyn Backend>,
    path: StoragePath,
    ctx: egui::Context,
    rt: &tokio::runtime::Handle,
) -> TransferHandle {
    spawn_transfer(rt, ctx, move || async move {
        backend.presign_url(&path, Duration::from_secs(86400)).await
    })
}

// ── Shared helpers ────────────────────────────────────────────────────────────

pub(crate) fn spawn_transfer<F, Fut>(
    rt: &tokio::runtime::Handle,
    ctx: egui::Context,
    f: F,
) -> TransferHandle
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String>> + Send + 'static,
{
    let slot: Arc<Mutex<Option<Result<String>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let join = rt.spawn(async move {
        let result = f().await;
        *slot2.lock().unwrap() = Some(result);
        ctx.request_repaint();
    });
    TransferHandle { slot, join }
}

/// Extract the last path segment (filename) from an S3 key or local path string.
pub(crate) fn base_name(s: &str) -> String {
    s.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_owned()
}
