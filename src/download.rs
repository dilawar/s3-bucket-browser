use std::sync::Arc;

use anyhow::Result;

use crate::async_rt::{SpawnContext, TransferHandle, base_name, spawn_transfer};
use crate::storage::{Backend, StoragePath};

// ── Individual file download ──────────────────────────────────────────────────

pub fn spawn_download(sc: SpawnContext, paths: Vec<StoragePath>) -> TransferHandle {
    spawn_transfer(sc, move |backend| do_download(backend, paths))
}

#[cfg(not(target_arch = "wasm32"))]
async fn do_download(backend: Arc<dyn Backend>, paths: Vec<StoragePath>) -> Result<String> {
    if paths.len() == 1 {
        let path = paths.into_iter().next().unwrap();
        let file_name = base_name(&path.to_string());
        let handle = rfd::AsyncFileDialog::new()
            .set_file_name(&file_name)
            .save_file()
            .await;
        let Some(handle) = handle else {
            return Ok("Download cancelled.".to_owned());
        };
        let data = backend.get(&path).await?;
        tokio::fs::write(handle.path(), &data).await?;
        Ok(format!("Saved to {}", handle.path().display()))
    } else {
        let handle = rfd::AsyncFileDialog::new()
            .set_title("Choose download folder")
            .pick_folder()
            .await;
        let Some(folder) = handle else {
            return Ok("Download cancelled.".to_owned());
        };
        let mut saved = 0usize;
        for path in &paths {
            let name = base_name(&path.to_string());
            if name.is_empty() {
                continue;
            }
            let data = backend.get(path).await?;
            tokio::fs::write(folder.path().join(&name), &data).await?;
            saved += 1;
        }
        Ok(format!(
            "Downloaded {saved} file{} to {}",
            if saved == 1 { "" } else { "s" },
            folder.path().display()
        ))
    }
}

#[cfg(target_arch = "wasm32")]
async fn do_download(backend: Arc<dyn Backend>, paths: Vec<StoragePath>) -> Result<String> {
    for path in &paths {
        let name = base_name(&path.to_string());
        if name.is_empty() {
            continue;
        }
        let data = backend.get(path).await?;
        browser_download(&name, &data)?;
    }
    Ok(format!(
        "Downloaded {} file{}",
        paths.len(),
        if paths.len() == 1 { "" } else { "s" }
    ))
}

// ── ZIP download ──────────────────────────────────────────────────────────────

/// Estimated size threshold above which the caller should warn the user (500 MB).
pub const ZIP_WARN_BYTES: u64 = 500 * 1024 * 1024;

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
            return Ok(None);
        }
    }
    Ok(Some(total))
}

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

    #[cfg(not(target_arch = "wasm32"))]
    {
        let handle = rfd::AsyncFileDialog::new()
            .set_file_name("download.zip")
            .add_filter("ZIP archive", &["zip"])
            .save_file()
            .await;
        let Some(handle) = handle else {
            return Ok("Download cancelled.".to_owned());
        };
        tokio::fs::write(handle.path(), &zip_buf).await?;
        Ok(format!("Zipped {n} file{} → {}", if n == 1 { "" } else { "s" }, handle.path().display()))
    }

    #[cfg(target_arch = "wasm32")]
    {
        browser_download("download.zip", &zip_buf)?;
        Ok(format!("Zipped {n} file{} downloaded", if n == 1 { "" } else { "s" }))
    }
}

fn zip_entry_name(file: &StoragePath, current_dir: &StoragePath) -> String {
    match (file, current_dir) {
        (StoragePath::S3 { prefix: fp, .. }, StoragePath::S3 { prefix: cp, .. }) => {
            fp.strip_prefix(cp.as_str()).unwrap_or(fp).to_owned()
        }
        #[cfg(not(target_arch = "wasm32"))]
        (StoragePath::Local(fp), StoragePath::Local(cp)) => fp
            .strip_prefix(cp)
            .unwrap_or(fp)
            .to_string_lossy()
            .into_owned(),
        _ => base_name(&file.to_string()),
    }
}

// ── Browser download helper (WASM only) ───────────────────────────────────────

#[cfg(target_arch = "wasm32")]
fn browser_download(filename: &str, data: &[u8]) -> Result<()> {
    use js_sys::{Array, Uint8Array};
    use wasm_bindgen::JsCast;
    use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, Url};

    let window = web_sys::window().ok_or_else(|| anyhow::anyhow!("no window"))?;
    let document = window.document().ok_or_else(|| anyhow::anyhow!("no document"))?;

    let uint8 = Uint8Array::from(data);
    let array = Array::new();
    array.push(&uint8);

    let bag = BlobPropertyBag::new();
    bag.set_type("application/octet-stream");
    let blob = Blob::new_with_u8_array_sequence_and_options(&array, &bag)
        .map_err(|_| anyhow::anyhow!("Blob creation failed"))?;
    let url = Url::create_object_url_with_blob(&blob)
        .map_err(|_| anyhow::anyhow!("createObjectURL failed"))?;

    let anchor: HtmlAnchorElement = document
        .create_element("a")
        .map_err(|_| anyhow::anyhow!("createElement failed"))?
        .dyn_into()
        .map_err(|_| anyhow::anyhow!("dyn_into HtmlAnchorElement failed"))?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    anchor.click();
    Url::revoke_object_url(&url).ok();
    Ok(())
}
