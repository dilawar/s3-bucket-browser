use std::sync::Arc;

use anyhow::Result;

use crate::async_rt::{SpawnContext, TransferHandle, base_name, spawn_transfer};
use crate::storage::{Backend, StoragePath};

// ── Individual file download ──────────────────────────────────────────────────

/// Spawn a download task.
/// - Single path → native save-file dialog.
/// - Multiple paths → native pick-folder dialog; each file is saved there by name.
pub fn spawn_download(sc: SpawnContext, paths: Vec<StoragePath>) -> TransferHandle {
    spawn_transfer(sc, move |backend| do_download(backend, paths))
}

async fn do_download(backend: Arc<dyn Backend>, paths: Vec<StoragePath>) -> Result<String> {
    if paths.len() == 1 {
        let path = paths.into_iter().next().unwrap();
        let file_name = base_name(&path.to_string());
        let save_path = tokio::task::spawn_blocking(move || {
            rfd::FileDialog::new().set_file_name(&file_name).save_file()
        })
        .await?;
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
            if name.is_empty() {
                continue;
            }
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

// ── ZIP download ──────────────────────────────────────────────────────────────

/// Estimated size threshold above which the caller should warn the user (500 MB).
pub const ZIP_WARN_BYTES: u64 = 500 * 1024 * 1024;

/// Recursively sum the sizes of all files reachable from `paths`.
/// Returns `None` for any file whose size is unknown.
pub async fn estimate_size(
    backend: Arc<dyn Backend>,
    paths: &[StoragePath],
) -> Result<Option<u64>> {
    let mut total: u64 = 0;
    for path in paths {
        if path.is_dir() {
            for entry in backend.list_recursive(path).await? {
                match entry.size {
                    Some(s) => total = total.saturating_add(s),
                    None => return Ok(None),
                }
            }
        } else {
            // For a plain file we don't have the size here without another
            // round-trip; conservatively return None so the caller may warn.
            return Ok(None);
        }
    }
    Ok(Some(total))
}

/// Spawn a task that collects all selected paths (expanding directories
/// recursively), then writes them into a single ZIP file chosen by the user.
pub fn spawn_download_zip(
    sc: SpawnContext,
    paths: Vec<StoragePath>,
    current_path: StoragePath,
) -> TransferHandle {
    spawn_transfer(sc, move |backend| do_download_zip(backend, paths, current_path))
}

async fn do_download_zip(
    backend: Arc<dyn Backend>,
    paths: Vec<StoragePath>,
    current_path: StoragePath,
) -> Result<String> {
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

/// Compute the path of a file inside the ZIP, relative to `current_dir`.
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
