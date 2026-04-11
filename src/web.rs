//! WASM entry point — compiled only for `wasm32-unknown-unknown`.
#![cfg(target_arch = "wasm32")]

use std::sync::Arc;

use wasm_bindgen::prelude::*;

use crate::app::S3Explorer;
use crate::storage::{S3Backend, S3Config, StoragePath};

const CANVAS_ID: &str = "s3_explorer_canvas";

#[wasm_bindgen(start)]
pub fn start() {
    // Show Rust panics as readable messages in the browser console.
    console_error_panic_hook::set_once();

    // Route tracing log calls to console.log / console.warn / console.error.
    tracing_wasm::set_as_global_default();

    let app = resolve_startup();
    let options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async move {
        let canvas = web_sys::window()
            .expect("no global window")
            .document()
            .expect("no document on window")
            .get_element_by_id(CANVAS_ID)
            .expect("canvas element not found")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("element is not a canvas");

        eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(|cc| {
                    cc.egui_ctx.set_visuals(egui::Visuals::light());
                    crate::ui::font::setup_fonts(&cc.egui_ctx);
                    Ok(Box::new(app))
                }),
            )
            .await
            .expect("failed to start eframe WebRunner");
    });
}

/// Try to connect from saved localStorage credentials; fall back to config form.
fn resolve_startup() -> S3Explorer {
    if let Some(saved) = crate::credentials::CredentialStore::open()
        .ok()
        .and_then(|s| s.load())
    {
        if !saved.bucket.is_empty() && !saved.access_key.is_empty() {
            let endpoint = if saved.endpoint.is_empty() {
                None
            } else {
                Some(saved.endpoint.as_str())
            };
            if let Ok(backend) = S3Backend::with_credentials(S3Config {
                bucket: &saved.bucket,
                endpoint,
                access_key: &saved.access_key,
                secret_key: &saved.secret_key,
                region: &saved.region,
            }) {
                let start = StoragePath::s3_root(backend.bucket_name());
                return S3Explorer::new(Arc::new(backend), start);
            }
        }
    }
    S3Explorer::needs_config()
}
