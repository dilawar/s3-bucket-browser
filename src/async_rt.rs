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

// ── Upload folder ─────────────────────────────────────────────────────────────

pub fn spawn_upload_folder(
    backend: Arc<dyn Backend>,
    current_path: StoragePath,
    ctx: egui::Context,
    rt: &tokio::runtime::Handle,
) -> TransferHandle {
    spawn_transfer(rt, ctx, move || do_upload_folder(backend, current_path))
}

async fn do_upload_folder(
    backend: Arc<dyn Backend>,
    current_path: StoragePath,
) -> Result<String> {
    // Pick folder on the blocking thread (rfd is sync).
    let local_folder =
        tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_folder()).await?;
    let Some(local_folder) = local_folder else {
        return Ok("Upload cancelled.".to_owned());
    };

    // Walk all files under the folder (sync, on blocking thread).
    let folder = local_folder.clone();
    let file_list: Vec<(std::path::PathBuf, String)> =
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            // Strip the folder's *parent* so the folder name is preserved in the key.
            let strip_base = folder.parent().unwrap_or(folder.as_path()).to_path_buf();
            collect_files_sync(&folder, &strip_base, &mut out)?;
            Ok::<_, anyhow::Error>(out)
        })
        .await??;

    if file_list.is_empty() {
        return Ok("No files found in the selected folder.".to_owned());
    }

    let total = file_list.len();
    let mut errors = 0usize;
    for (local_path, rel_key) in &file_list {
        let dest = current_path.child_file(rel_key);
        match tokio::fs::read(local_path).await {
            Ok(data) => {
                if let Err(e) = backend.put(&dest, bytes::Bytes::from(data)).await {
                    tracing::warn!("Upload failed for {rel_key}: {e}");
                    errors += 1;
                }
            }
            Err(e) => {
                tracing::warn!("Read failed for {}: {e}", local_path.display());
                errors += 1;
            }
        }
    }

    if errors == 0 {
        Ok(format!(
            "Uploaded {total} file{} from folder",
            if total == 1 { "" } else { "s" }
        ))
    } else {
        Ok(format!(
            "Uploaded {}/{total} files ({errors} failed — see log for details)",
            total - errors
        ))
    }
}

/// Recursively collect all files under `dir`, recording each as
/// `(absolute_local_path, relative_key_string)` where the key is the path
/// relative to `strip_base` with forward-slash separators.
fn collect_files_sync(
    dir: &std::path::Path,
    strip_base: &std::path::Path,
    out: &mut Vec<(std::path::PathBuf, String)>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            collect_files_sync(&path, strip_base, out)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(strip_base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/"); // normalise Windows paths
            out.push((path, rel));
        }
    }
    Ok(())
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
    s.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned()
}
