use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::async_rt::{SpawnContext, TransferHandle, spawn_transfer_uploading};
use crate::storage::StoragePath;

// ── Single-file upload ────────────────────────────────────────────────────────

pub fn spawn_upload(sc: SpawnContext, dest: StoragePath) -> TransferHandle {
    spawn_transfer_uploading(sc, move |backend, progress| do_upload(backend, dest, progress))
}

async fn do_upload(
    backend: Arc<dyn crate::storage::Backend>,
    current_path: StoragePath,
    progress: Arc<Mutex<String>>,
) -> Result<String> {
    let handle = rfd::AsyncFileDialog::new().pick_file().await;
    let Some(handle) = handle else {
        return Ok("Upload cancelled.".to_owned());
    };
    let file_name = handle.file_name();
    *progress.lock().unwrap() = file_name.clone();
    let dest = current_path.child_file(&file_name);
    // AsyncFileHandle::read() works on both native (reads from disk) and WASM
    // (reads the in-memory File object — no tokio::fs needed).
    let data = bytes::Bytes::from(handle.read().await);
    let size = data.len();
    backend.put(&dest, data).await?;
    Ok(format!("Uploaded {file_name} ({size} bytes) → {dest}"))
}

// ── Folder upload (native only) ───────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_upload_folder(sc: SpawnContext, dest: StoragePath) -> TransferHandle {
    spawn_transfer_uploading(sc, move |backend, progress| {
        do_upload_folder(backend, dest, progress)
    })
}

/// WASM stub — folder upload is not supported in the browser.
#[cfg(target_arch = "wasm32")]
pub fn spawn_upload_folder(_sc: SpawnContext, _dest: StoragePath) -> TransferHandle {
    use crate::async_rt::spawn_transfer;
    spawn_transfer(_sc, |_backend| async {
        Err(anyhow::anyhow!("Folder upload is not supported in the browser"))
    })
}

#[cfg(not(target_arch = "wasm32"))]
async fn do_upload_folder(
    backend: Arc<dyn crate::storage::Backend>,
    current_path: StoragePath,
    progress: Arc<Mutex<String>>,
) -> Result<String> {
    let handle = rfd::AsyncFileDialog::new().pick_folder().await;
    let Some(folder_handle) = handle else {
        return Ok("Upload cancelled.".to_owned());
    };
    let local_folder = folder_handle.path().to_path_buf();

    let folder = local_folder.clone();
    let file_list: Vec<(std::path::PathBuf, String)> =
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
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
    for (idx, (local_path, rel_key)) in file_list.iter().enumerate() {
        *progress.lock().unwrap() = format!("{}/{} {}", idx + 1, total, rel_key);
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

#[cfg(not(target_arch = "wasm32"))]
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
                .replace('\\', "/");
            out.push((path, rel));
        }
    }
    Ok(())
}
