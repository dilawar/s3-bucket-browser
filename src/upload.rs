use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::async_rt::{SpawnContext, TransferHandle, spawn_transfer_uploading};
use crate::storage::StoragePath;

// ── Single-file upload ────────────────────────────────────────────────────────

pub fn spawn_upload(sc: SpawnContext, dest: StoragePath) -> TransferHandle {
    spawn_transfer_uploading(sc, move |backend, progress| {
        do_upload(backend, dest, progress)
    })
}

async fn do_upload(
    backend: Arc<dyn crate::storage::Backend>,
    current_path: StoragePath,
    progress: Arc<Mutex<String>>,
) -> Result<String> {
    let local_path =
        tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_file()).await?;
    let Some(local_path) = local_path else {
        return Ok("Upload cancelled.".to_owned());
    };
    let file_name = local_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "upload".to_owned());
    *progress.lock().unwrap() = file_name.clone();
    let dest = current_path.child_file(&file_name);
    let data = bytes::Bytes::from(tokio::fs::read(&local_path).await?);
    let size = data.len();
    backend.put(&dest, data).await?;
    Ok(format!("Uploaded {file_name} ({size} bytes) → {dest}"))
}

// ── Folder upload ─────────────────────────────────────────────────────────────

pub fn spawn_upload_folder(sc: SpawnContext, dest: StoragePath) -> TransferHandle {
    spawn_transfer_uploading(sc, move |backend, progress| {
        do_upload_folder(backend, dest, progress)
    })
}

async fn do_upload_folder(
    backend: Arc<dyn crate::storage::Backend>,
    current_path: StoragePath,
    progress: Arc<Mutex<String>>,
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
        *progress.lock().unwrap() = rel_key.clone();
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

/// Recursively collect all files under `dir` into `out` as
/// `(absolute_path, s3_key_relative_to_strip_base)`.
fn collect_files_sync(
    dir: &Path,
    strip_base: &Path,
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
