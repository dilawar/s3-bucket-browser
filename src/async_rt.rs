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

// ── Download ──────────────────────────────────────────────────────────────────

/// Spawn a download task.
/// - Single path → native save-file dialog.
/// - Multiple paths → native pick-folder dialog; each file is saved there by name.
pub fn spawn_download(
    backend: Arc<dyn Backend>,
    paths: Vec<StoragePath>,
    ctx: egui::Context,
    rt: &tokio::runtime::Handle,
) -> TransferHandle {
    spawn_transfer(rt, ctx, move || do_download(backend, paths))
}

async fn do_download(backend: Arc<dyn Backend>, paths: Vec<StoragePath>) -> Result<String> {
    if paths.len() == 1 {
        let path = paths.into_iter().next().unwrap();
        let file_name = base_name(&path.to_string());
        let save_path = tokio::task::spawn_blocking(move || {
            rfd::FileDialog::new().set_file_name(&file_name).save_file()
        })
        .await?;  // unwrap JoinError only; result is Option<PathBuf>
        let Some(save_path) = save_path else {
            return Ok("Download cancelled.".to_owned());
        };
        let data = backend.get(&path).await?;
        tokio::fs::write(&save_path, &data).await?;
        Ok(format!("Saved to {}", save_path.display()))
    } else {
        let folder = tokio::task::spawn_blocking(|| {
            rfd::FileDialog::new()
                .set_title("Choose download folder")
                .pick_folder()
        })
        .await?;
        let Some(folder) = folder else {
            return Ok("Download cancelled.".to_owned());
        };
        let mut saved = 0usize;
        for path in &paths {
            let name = base_name(&path.to_string());
            if name.is_empty() { continue; }
            let data = backend.get(path).await?;
            tokio::fs::write(folder.join(&name), &data).await?;
            saved += 1;
        }
        Ok(format!(
            "Downloaded {saved} file{} to {}",
            if saved == 1 { "" } else { "s" },
            folder.display()
        ))
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

// ── Download as ZIP ───────────────────────────────────────────────────────────

/// Spawn a task that collects all selected paths (expanding directories
/// recursively), then writes them into a single ZIP file chosen by the user.
pub fn spawn_download_zip(
    backend: Arc<dyn Backend>,
    paths: Vec<StoragePath>,
    current_path: StoragePath,
    ctx: egui::Context,
    rt: &tokio::runtime::Handle,
) -> TransferHandle {
    spawn_transfer(rt, ctx, move || do_download_zip(backend, paths, current_path))
}

async fn do_download_zip(
    backend: Arc<dyn Backend>,
    paths: Vec<StoragePath>,
    current_path: StoragePath,
) -> anyhow::Result<String> {
    let save_path = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_file_name("download.zip")
            .add_filter("ZIP archive", &["zip"])
            .save_file()
    })
    .await?;
    let Some(save_path) = save_path else {
        return Ok("Download cancelled.".to_owned());
    };

    // Expand directories; collect (storage_path, zip_entry_name) pairs.
    let mut entries: Vec<(StoragePath, String)> = Vec::new();
    for path in &paths {
        if path.is_dir() {
            for entry in backend.list_recursive(path).await? {
                let name = zip_entry_name(&entry.path, &current_path);
                entries.push((entry.path, name));
            }
        } else {
            let name = zip_entry_name(path, &current_path);
            entries.push((path.clone(), name));
        }
    }

    let n = entries.len();
    let mut zip_buf: Vec<u8> = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut zip_buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (path, name) in &entries {
            let data = backend.get(path).await?;
            zip.start_file(name, options)?;
            std::io::Write::write_all(&mut zip, &data)?;
        }
        zip.finish()?;
    }

    tokio::fs::write(&save_path, &zip_buf).await?;
    Ok(format!(
        "Zipped {n} file{} → {}",
        if n == 1 { "" } else { "s" },
        save_path.display()
    ))
}

/// Compute the path of a file inside the ZIP relative to `current_dir`.
/// e.g. current = "projects/", file = "projects/code/main.rs" → "code/main.rs"
fn zip_entry_name(file: &StoragePath, current_dir: &StoragePath) -> String {
    match (file, current_dir) {
        (StoragePath::S3 { prefix: fp, .. }, StoragePath::S3 { prefix: cp, .. }) => {
            fp.strip_prefix(cp.as_str()).unwrap_or(fp).to_owned()
        }
        (StoragePath::Local(fp), StoragePath::Local(cp)) => fp
            .strip_prefix(cp)
            .unwrap_or(fp)
            .to_string_lossy()
            .into_owned(),
        _ => base_name(&file.to_string()),
    }
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

fn spawn_transfer<F, Fut>(
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
fn base_name(s: &str) -> String {
    s.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_owned()
}
