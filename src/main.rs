use std::sync::Arc;

use anyhow::Result;
use s3_explorer::app::S3Explorer;
use s3_explorer::storage::{S3Backend, StoragePath};

const APP_TITLE: &str = "S3 Compatible Bucket Browser";

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("s3_explorer=debug".parse()?),
        )
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let rt_handle = rt.handle().clone();

    let app = resolve_startup(rt_handle);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_inner_size([1100.0, 700.0]),
        ..Default::default()
    };

    // Keep the runtime alive for the duration of the process.
    let _rt = rt;

    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            s3_explorer::ui::font::setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}

/// Determine the startup mode:
///
/// 1. `.env` found → load it and connect directly (no login screen, no stored creds).
///    If the file exists but variables are incomplete, show the config form with an
///    error banner explaining what is missing — still no silent fall-through to login.
/// 2. Required `S3_*` vars already in the environment → connect directly.
/// 3. CLI arg is a local path → open the local browser.
/// 4. Otherwise → show the credentials config form.
///
/// Variables recognised (in `.env` or the shell environment):
///   S3_BUCKET            – bucket name (required)
///   S3_ACCESS_KEY_ID     – access key ID (required)
///   S3_SECRET_ACCESS_KEY – secret access key (required)
///   S3_ENDPOINT_URL      – custom endpoint, e.g. https://s3.us-west-004.backblazeb2.com
///   S3_REGION            – region, e.g. us-east-1  (default: us-east-1)
fn resolve_startup(rt: tokio::runtime::Handle) -> S3Explorer {
    use s3_explorer::storage::LocalBackend;

    // ── Step 1: load .env if present ─────────────────────────────────────────
    // dotenvy does NOT override variables already set in the shell environment.
    let dotenv_loaded = match dotenvy::dotenv() {
        Ok(path) => {
            tracing::info!("Loaded .env from {path:?}");
            true
        }
        Err(dotenvy::Error::Io(_)) => false, // no .env file — that's fine
        Err(e) => {
            tracing::warn!(".env parse error: {e}");
            false
        }
    };

    // ── Step 2: connect from env vars (set directly or loaded from .env) ─────
    match S3Backend::from_env() {
        Ok(backend) => {
            let start = StoragePath::s3_root(backend.bucket_name());
            tracing::info!("Auto-connecting to '{}'", backend.bucket_name());
            return S3Explorer::new(Arc::new(backend), start, rt);
        }
        Err(e) if dotenv_loaded => {
            // .env was found but it doesn't supply all required variables.
            // Show the config form pre-filled with whatever was loaded, and
            // explain the problem — do NOT silently fall through to an empty form.
            tracing::warn!("Could not connect using .env: {e}");
            return S3Explorer::needs_config_with_error(
                rt,
                Some(format!(
                    ".env loaded but connection failed: {e}. \
                     Make sure S3_BUCKET, S3_ACCESS_KEY_ID and S3_SECRET_ACCESS_KEY are set."
                )),
            );
        }
        Err(_) => {} // no .env and no env vars — continue below
    }

    // ── Step 3: explicit local path as CLI argument ───────────────────────────
    if let Some(arg) = std::env::args().nth(1) {
        let path = StoragePath::parse(&arg);
        if let StoragePath::Local(ref pb) = path
            && pb.exists()
        {
            return S3Explorer::new(Arc::new(LocalBackend), path, rt);
        }
    }

    // ── Step 4: show the credentials config form ──────────────────────────────
    S3Explorer::needs_config(rt)
}
