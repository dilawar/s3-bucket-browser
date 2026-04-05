use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use crate::storage::{Backend, StorageEntry, StoragePath};

// ── SpawnContext ──────────────────────────────────────────────────────────────

/// Common context shared by every task-spawning operation.
///
/// Bundles the three parameters that every spawn call needs — backend, egui
/// repaint handle, and tokio runtime — so call sites don't repeat them.
#[derive(Clone)]
pub struct SpawnContext {
    pub backend: Arc<dyn Backend>,
    pub ctx: egui::Context,
    pub rt: tokio::runtime::Handle,
}

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

pub fn spawn_listing(sc: SpawnContext, path: StoragePath) -> ListingHandle {
    let slot: Arc<Mutex<Option<Result<Vec<StorageEntry>>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let join = sc.rt.spawn(async move {
        let result = sc.backend.list(&path).await;
        *slot2.lock().unwrap() = Some(result);
        sc.ctx.request_repaint();
    });
    ListingHandle { slot, join }
}

// ── TransferHandle ────────────────────────────────────────────────────────────

pub struct TransferHandle {
    slot: Arc<Mutex<Option<Result<String>>>>,
    /// Current file/operation being processed; empty when not applicable.
    progress: Arc<Mutex<String>>,
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

    /// Current filename/operation being processed, or empty string if unknown.
    pub fn progress_msg(&self) -> String {
        self.progress.lock().unwrap().clone()
    }

    /// Abort the underlying task immediately.
    pub fn cancel(&self) {
        self.join.abort();
    }
}

// ── Delete ────────────────────────────────────────────────────────────────────

/// Spawn a delete task that removes every path in `paths` sequentially.
/// Directories (S3 prefixes ending with `/`) are deleted recursively.
pub fn spawn_delete(sc: SpawnContext, paths: Vec<StoragePath>) -> TransferHandle {
    spawn_transfer(sc, move |backend| do_delete(backend, paths))
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
pub fn spawn_presign(sc: SpawnContext, path: StoragePath) -> TransferHandle {
    spawn_transfer(sc, move |backend| async move {
        backend.presign_url(&path, Duration::from_secs(86400)).await
    })
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Spawn a transfer task with no progress reporting (delete, download, presign).
///
/// The factory `f` receives the backend Arc and returns a future that produces
/// a status string.
pub(crate) fn spawn_transfer<F, Fut>(sc: SpawnContext, f: F) -> TransferHandle
where
    F: FnOnce(Arc<dyn Backend>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String>> + Send + 'static,
{
    let slot: Arc<Mutex<Option<Result<String>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let progress = Arc::new(Mutex::new(String::new()));
    let join = sc.rt.spawn(async move {
        let result = f(sc.backend).await;
        *slot2.lock().unwrap() = Some(result);
        sc.ctx.request_repaint();
    });
    TransferHandle { slot, progress, join }
}

/// Spawn a transfer task that receives a progress Arc to report current filename.
///
/// The factory `f` receives both the backend Arc and a progress slot it can
/// write to on each file processed.
pub(crate) fn spawn_transfer_uploading<F, Fut>(sc: SpawnContext, f: F) -> TransferHandle
where
    F: FnOnce(Arc<dyn Backend>, Arc<Mutex<String>>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String>> + Send + 'static,
{
    let slot: Arc<Mutex<Option<Result<String>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let progress: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let progress2 = Arc::clone(&progress);
    let join = sc.rt.spawn(async move {
        let result = f(sc.backend, progress2).await;
        *slot2.lock().unwrap() = Some(result);
        sc.ctx.request_repaint();
    });
    TransferHandle { slot, progress, join }
}

/// Extract the last path segment (filename) from an S3 key or local path string.
pub(crate) fn base_name(s: &str) -> String {
    s.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned()
}
