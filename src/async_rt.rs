use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use crate::storage::{Backend, StorageEntry, StoragePath};

// ── SpawnContext ──────────────────────────────────────────────────────────────

/// Common context shared by every task-spawning operation.
#[derive(Clone)]
pub struct SpawnContext {
    pub backend: Arc<dyn Backend>,
    pub ctx: egui::Context,
    #[cfg(not(target_arch = "wasm32"))]
    pub rt: tokio::runtime::Handle,
}

// ── TaskHandle ────────────────────────────────────────────────────────────────

/// Platform-agnostic wrapper around a spawned async task.
enum TaskHandle {
    #[cfg(not(target_arch = "wasm32"))]
    Tokio(tokio::task::JoinHandle<()>),
    /// On WASM, `spawn_local` returns `()` — no handle, no cancellation.
    #[cfg(target_arch = "wasm32")]
    Wasm,
}

impl TaskHandle {
    fn is_finished(&self) -> bool {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Tokio(h) => h.is_finished(),
            #[cfg(target_arch = "wasm32")]
            Self::Wasm => false,
        }
    }

    fn abort(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Self::Tokio(h) = self;
            h.abort();
        }
    }
}

// ── platform_spawn ────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn platform_spawn(
    rt: &tokio::runtime::Handle,
    fut: impl std::future::Future<Output = ()> + Send + 'static,
) -> TaskHandle {
    TaskHandle::Tokio(rt.spawn(fut))
}

#[cfg(target_arch = "wasm32")]
fn platform_spawn(fut: impl std::future::Future<Output = ()> + 'static) -> TaskHandle {
    wasm_bindgen_futures::spawn_local(fut);
    TaskHandle::Wasm
}

// ── ListingHandle ─────────────────────────────────────────────────────────────

pub struct ListingHandle {
    slot: Arc<Mutex<Option<Result<Vec<StorageEntry>>>>>,
    join: TaskHandle,
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

    #[cfg(not(target_arch = "wasm32"))]
    let join = platform_spawn(&sc.rt, async move {
        let result = sc.backend.list(&path).await;
        *slot2.lock().unwrap() = Some(result);
        sc.ctx.request_repaint();
    });

    #[cfg(target_arch = "wasm32")]
    let join = platform_spawn(async move {
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
    join: TaskHandle,
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
        let msg = self.progress.lock().unwrap().clone();
        if let Some(space) = msg.find(' ')
            && msg[..space].contains('/')
        {
            return msg[space + 1..].to_owned();
        }
        msg
    }

    /// For folder uploads, returns (fraction 0.0–1.0, current filename).
    /// Returns None for single-file uploads or when total is unknown.
    pub fn upload_progress(&self) -> Option<(f32, String)> {
        let msg = self.progress.lock().unwrap().clone();
        let space = msg.find(' ')?;
        let (counts, rest) = msg.split_at(space);
        let (done, total) = counts.split_once('/')?;
        let done: f32 = done.parse().ok()?;
        let total: f32 = total.parse().ok()?;
        if total > 0.0 {
            Some((done / total, rest.trim_start().to_owned()))
        } else {
            None
        }
    }

    /// Abort the underlying task immediately (no-op on WASM).
    pub fn cancel(&self) {
        self.join.abort();
    }
}

// ── Delete ────────────────────────────────────────────────────────────────────

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

pub fn spawn_presign(sc: SpawnContext, path: StoragePath) -> TransferHandle {
    spawn_transfer(sc, move |backend| async move {
        backend.presign_url(&path, Duration::from_secs(86400)).await
    })
}

// ── Shared helpers ────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn spawn_transfer<F, Fut>(sc: SpawnContext, f: F) -> TransferHandle
where
    F: FnOnce(Arc<dyn Backend>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String>> + Send + 'static,
{
    let slot: Arc<Mutex<Option<Result<String>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let progress = Arc::new(Mutex::new(String::new()));
    let join = platform_spawn(&sc.rt, async move {
        let result = f(sc.backend).await;
        *slot2.lock().unwrap() = Some(result);
        sc.ctx.request_repaint();
    });
    TransferHandle { slot, progress, join }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn spawn_transfer<F, Fut>(sc: SpawnContext, f: F) -> TransferHandle
where
    F: FnOnce(Arc<dyn Backend>) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<String>> + 'static,
{
    let slot: Arc<Mutex<Option<Result<String>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let progress = Arc::new(Mutex::new(String::new()));
    let join = platform_spawn(async move {
        let result = f(sc.backend).await;
        *slot2.lock().unwrap() = Some(result);
        sc.ctx.request_repaint();
    });
    TransferHandle { slot, progress, join }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn spawn_transfer_uploading<F, Fut>(sc: SpawnContext, f: F) -> TransferHandle
where
    F: FnOnce(Arc<dyn Backend>, Arc<Mutex<String>>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String>> + Send + 'static,
{
    let slot: Arc<Mutex<Option<Result<String>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let progress: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let progress2 = Arc::clone(&progress);
    let join = platform_spawn(&sc.rt, async move {
        let result = f(sc.backend, progress2).await;
        *slot2.lock().unwrap() = Some(result);
        sc.ctx.request_repaint();
    });
    TransferHandle { slot, progress, join }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn spawn_transfer_uploading<F, Fut>(sc: SpawnContext, f: F) -> TransferHandle
where
    F: FnOnce(Arc<dyn Backend>, Arc<Mutex<String>>) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<String>> + 'static,
{
    let slot: Arc<Mutex<Option<Result<String>>>> = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let progress: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let progress2 = Arc::clone(&progress);
    let join = platform_spawn(async move {
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
