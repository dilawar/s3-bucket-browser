use std::collections::HashSet;
use std::sync::Arc;

use egui::{CentralPanel, Color32, RichText, SidePanel, TopBottomPanel};
use tracing::info;

use crate::async_rt::{self, ListingHandle, SpawnContext, TransferHandle};
use crate::download;
use crate::upload;
use crate::storage::{Backend, S3Config, StorageEntry, StoragePath};
use crate::ui::{config, file_list, file_list::SortState, sidebar, toolbar};

// ── Transfer status ───────────────────────────────────────────────────────────

#[derive(Default)]
enum TransferStatus {
    #[default]
    Idle,
    /// In-progress message (e.g. "Generating presigned URL…").
    Pending(String),
    Success(String),
    Error(String),
}

// ── Rename state ──────────────────────────────────────────────────────────────

struct RenameState {
    path: StoragePath,
    new_name: String,
}

// ── App mode ──────────────────────────────────────────────────────────────────

enum Mode {
    Configure {
        fields: config::ConfigFields,
        error: Option<String>,
    },
    Browse,
}

// ── App struct ────────────────────────────────────────────────────────────────

pub struct S3Explorer {
    mode: Mode,
    backend: Option<Arc<dyn Backend>>,
    current_path: StoragePath,
    entries: Vec<StorageEntry>,
    listing: Option<ListingHandle>,
    loading: bool,
    error: Option<String>,
    path_input: String,
    history: Vec<StoragePath>,
    history_pos: usize,
    filter: String,
    sort: SortState,
    selection: HashSet<StoragePath>,
    dark_mode: bool,
    needs_initial_load: bool,
    transfer: Option<TransferHandle>,
    transfer_status: TransferStatus,
    editing_path: bool,
    new_folder_name: Option<String>,
    rename_state: Option<RenameState>,
    /// Separate handle for presign tasks so they don't block uploads/downloads.
    presign: Option<TransferHandle>,
    /// Pending ZIP download that is awaiting user confirmation (large archive).
    zip_confirm: Option<ZipConfirm>,
    /// Whether the "close bucket" confirmation modal is open.
    confirming_close: bool,
    rt: tokio::runtime::Handle,
}

struct ZipConfirm {
    paths: Vec<StoragePath>,
    current_path: StoragePath,
    size_bytes: u64,
}

impl S3Explorer {
    /// Start immediately in browse mode (credentials already resolved).
    pub fn new(backend: Arc<dyn Backend>, start: StoragePath, rt: tokio::runtime::Handle) -> Self {
        info!("Opening {:?} with backend '{}'", start, backend.name());
        let path_input = start.to_string();
        Self {
            mode: Mode::Browse,
            backend: Some(backend),
            current_path: start.clone(),
            entries: vec![],
            listing: None,
            loading: false,
            error: None,
            path_input,
            history: vec![start],
            history_pos: 0,
            filter: String::new(),
            sort: SortState::default(),
            selection: HashSet::new(),
            dark_mode: false,
            needs_initial_load: true,
            transfer: None,
            transfer_status: TransferStatus::Idle,
            editing_path: false,
            new_folder_name: None,
            rename_state: None,
            presign: None,
            zip_confirm: None,
            confirming_close: false,
            rt,
        }
    }

    /// Start in configure mode; fields are pre-filled from env vars and saved credentials.
    pub fn needs_config(rt: tokio::runtime::Handle) -> Self {
        Self::needs_config_with_error(rt, None)
    }

    /// Like [`needs_config`] but pre-sets an error banner on the form.
    /// Used when a `.env` was loaded but the connection could not be built.
    pub fn needs_config_with_error(rt: tokio::runtime::Handle, error: Option<String>) -> Self {
        Self {
            mode: Mode::Configure {
                fields: config::ConfigFields::load(),
                error,
            },
            backend: None,
            current_path: StoragePath::default(),
            entries: vec![],
            listing: None,
            loading: false,
            error: None,
            path_input: String::new(),
            history: vec![],
            history_pos: 0,
            filter: String::new(),
            sort: SortState::default(),
            selection: HashSet::new(),
            dark_mode: false,
            needs_initial_load: false,
            transfer: None,
            transfer_status: TransferStatus::Idle,
            editing_path: false,
            new_folder_name: None,
            rename_state: None,
            presign: None,
            zip_confirm: None,
            confirming_close: false,
            rt,
        }
    }

    // ── listing ───────────────────────────────────────────────────────────────

    fn request_listing(&mut self, path: StoragePath, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        self.loading = true;
        self.error = None;
        self.filter.clear();
        self.path_input = path.to_string();
        self.current_path = path.clone();
        self.listing = Some(async_rt::spawn_listing(self.spawn_ctx(ctx), path));
    }

    fn poll_listing(&mut self) {
        if let Some(handle) = &self.listing
            && let Some(result) = handle.try_recv()
        {
            self.loading = false;
            self.listing = None;
            match result {
                Ok(entries) => self.entries = entries,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    // ── transfers ─────────────────────────────────────────────────────────────

    fn start_download(&mut self, paths: Vec<StoragePath>, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        self.transfer_status = TransferStatus::Idle;
        self.transfer = Some(download::spawn_download(self.spawn_ctx(ctx), paths));
    }

    fn start_delete(&mut self, paths: Vec<StoragePath>, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        self.transfer_status = TransferStatus::Idle;
        for p in &paths {
            self.selection.remove(p);
        }
        self.transfer = Some(async_rt::spawn_delete(self.spawn_ctx(ctx), paths));
    }

    fn start_upload(&mut self, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        self.transfer_status = TransferStatus::Idle;
        self.transfer = Some(upload::spawn_upload(
            self.spawn_ctx(ctx),
            self.current_path.clone(),
        ));
    }

    fn start_upload_folder(&mut self, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        self.transfer_status = TransferStatus::Idle;
        self.transfer = Some(upload::spawn_upload_folder(
            self.spawn_ctx(ctx),
            self.current_path.clone(),
        ));
    }

    fn poll_transfer(&mut self, ctx: &egui::Context) {
        if let Some(handle) = &self.transfer
            && let Some(result) = handle.try_recv()
        {
            self.transfer = None;
            match result {
                Ok(msg) => {
                    info!("{msg}");
                    self.transfer_status = TransferStatus::Success(msg);
                    let path = self.current_path.clone();
                    self.request_listing(path, ctx);
                }
                Err(e) => {
                    self.transfer_status = TransferStatus::Error(e.to_string());
                }
            }
        }
    }

    fn transfer_busy(&self) -> bool {
        self.transfer.as_ref().is_some_and(|h| h.is_running())
    }

    /// Build a [`SpawnContext`] from the current app state for the given frame.
    fn spawn_ctx(&self, ctx: &egui::Context) -> SpawnContext {
        SpawnContext {
            backend: Arc::clone(self.backend.as_ref().expect("backend is set in Browse mode")),
            ctx: ctx.clone(),
            rt: self.rt.clone(),
        }
    }

    // ── create dir / rename ───────────────────────────────────────────────────

    fn start_create_dir(&mut self, name: String, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        let path = self.current_path.child(&name);
        let sc = self.spawn_ctx(ctx);
        self.transfer_status = TransferStatus::Idle;
        self.transfer = Some(async_rt::spawn_transfer(sc, move |backend| async move {
            backend.create_dir(&path).await?;
            Ok(format!("Created folder '{name}'"))
        }));
    }

    fn start_rename(&mut self, from: StoragePath, new_name: String, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        let parent = from.parent().unwrap_or_else(|| self.current_path.clone());
        let to = parent.child_file(&new_name);
        let sc = self.spawn_ctx(ctx);
        self.transfer = Some(async_rt::spawn_transfer(sc, move |backend| async move {
            backend.rename(&from, &to).await?;
            Ok(format!("Renamed to '{new_name}'"))
        }));
    }

    // ── ZIP download with size pre-flight ─────────────────────────────────────

    fn start_zip_download(&mut self, paths: Vec<StoragePath>, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        let current = self.current_path.clone();
        self.transfer_status = TransferStatus::Idle;
        self.transfer = Some(download::spawn_download_zip(
            self.spawn_ctx(ctx),
            paths,
            current,
        ));
    }

    fn request_zip_download(&mut self, paths: Vec<StoragePath>, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        let current = self.current_path.clone();

        let known_total: Option<u64> = paths.iter().try_fold(0u64, |acc, p| {
            if p.is_dir() {
                None
            } else {
                self.entries
                    .iter()
                    .find(|e| &e.path == p)
                    .and_then(|e| e.size)
                    .map(|s| acc.saturating_add(s))
            }
        });

        if let Some(total) = known_total
            && total > download::ZIP_WARN_BYTES
        {
            self.zip_confirm = Some(ZipConfirm { paths, current_path: current, size_bytes: total });
            return;
        }
        self.start_zip_download(paths, ctx);
    }

    // ── presign ───────────────────────────────────────────────────────────────

    fn start_presign(&mut self, path: StoragePath, ctx: &egui::Context) {
        if self.backend.is_none() { return }
        self.presign = Some(async_rt::spawn_presign(self.spawn_ctx(ctx), path));
        self.transfer_status = TransferStatus::Pending("Generating presigned URL…".to_owned());
    }

    fn poll_presign(&mut self, ctx: &egui::Context) {
        if let Some(handle) = &self.presign
            && let Some(result) = handle.try_recv()
        {
            self.presign = None;
            match result {
                Ok(url) => {
                    ctx.copy_text(url);
                    self.transfer_status = TransferStatus::Success("Presigned URL copied to clipboard".to_owned());
                }
                Err(e) => {
                    self.transfer_status = TransferStatus::Error(e.to_string());
                }
            }
        }
    }

    // ── navigation ────────────────────────────────────────────────────────────

    fn navigate_to(&mut self, path: StoragePath, ctx: &egui::Context) {
        self.selection.clear();
        self.history.truncate(self.history_pos + 1);
        self.history.push(path.clone());
        self.history_pos = self.history.len() - 1;
        self.request_listing(path, ctx);
    }

    fn go_back(&mut self, ctx: &egui::Context) {
        if self.history_pos > 0 {
            self.history_pos -= 1;
            let path = self.history[self.history_pos].clone();
            self.request_listing(path, ctx);
        }
    }

    fn go_forward(&mut self, ctx: &egui::Context) {
        if self.history_pos + 1 < self.history.len() {
            self.history_pos += 1;
            let path = self.history[self.history_pos].clone();
            self.request_listing(path, ctx);
        }
    }

    fn go_up(&mut self, ctx: &egui::Context) {
        if let Some(parent) = self.current_path.parent() {
            self.navigate_to(parent, ctx);
        }
    }

    // ── connect ───────────────────────────────────────────────────────────────

    fn try_connect(&mut self, ctx: &egui::Context) {
        use crate::credentials::{CredentialStore, SavedCredentials};
        use crate::storage::S3Backend;

        let Mode::Configure { fields, error } = &mut self.mode else {
            return;
        };

        let endpoint = fields.resolved_endpoint();
        let region = if fields.region.is_empty() {
            "us-east-1".to_owned()
        } else {
            fields.region.clone()
        };

        match S3Backend::with_credentials(S3Config {
            bucket: &fields.bucket,
            endpoint: endpoint.as_deref(),
            access_key: &fields.access_key,
            secret_key: &fields.secret_key,
            region: &region,
        }) {
            Ok(backend) => {
                if fields.remember {
                    let creds = SavedCredentials {
                        bucket: fields.bucket.clone(),
                        endpoint: endpoint.clone().unwrap_or_default(),
                        access_key: fields.access_key.clone(),
                        secret_key: fields.secret_key.clone(),
                        region: fields.region.clone(),
                    };
                    if let Err(e) = CredentialStore::open().and_then(|s| s.save(&creds)) {
                        tracing::warn!("Failed to save credentials: {e}");
                    }
                }

                let start = StoragePath::s3_root(&fields.bucket);
                info!("Connected to S3 bucket '{}'", fields.bucket);
                self.backend = Some(Arc::new(backend));
                self.current_path = start.clone();
                self.path_input = start.to_string();
                self.history = vec![start];
                self.history_pos = 0;
                self.mode = Mode::Browse;
                let path = self.current_path.clone();
                self.request_listing(path, ctx);
            }
            Err(e) => {
                *error = Some(e.to_string());
            }
        }
    }
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for S3Explorer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match self.mode {
            Mode::Configure { .. } => self.draw_config(ctx),
            Mode::Browse => self.draw_browser(ctx),
        }
    }
}

impl S3Explorer {
    fn draw_config(&mut self, ctx: &egui::Context) {
        CentralPanel::default().show(ctx, |ui| {
            let Mode::Configure { fields, error } = &mut self.mode else {
                return;
            };
            let resp = config::show(ui, fields, error.as_deref());
            if resp.connect {
                self.try_connect(ctx);
            }
        });
    }

    fn show_close_bucket_modal(&mut self, ctx: &egui::Context) {
        if !self.confirming_close { return; }

        let mut remove_creds = false;
        let mut keep_creds   = false;
        let mut cancelled    = false;

        egui::Modal::new(egui::Id::new("close_bucket")).show(ctx, |ui| {
            ui.set_max_width(340.0);
            ui.heading("Close bucket");
            ui.add_space(8.0);
            ui.label("Also remove the stored credentials for this bucket?");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(egui::RichText::new("Remove & close").strong()).clicked() {
                    remove_creds = true;
                }
                if ui.button("Keep & close").clicked() {
                    keep_creds = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });

        if remove_creds || keep_creds {
            if remove_creds {
                if let Err(e) = crate::credentials::CredentialStore::open()
                    .and_then(|s| s.delete())
                {
                    tracing::warn!("Failed to delete stored credentials: {e}");
                }
            }
            self.confirming_close = false;
            self.backend = None;
            self.mode = Mode::Configure {
                fields: config::ConfigFields::load(),
                error: None,
            };
        } else if cancelled {
            self.confirming_close = false;
        }
    }

    fn show_zip_confirm_modal(&mut self, ctx: &egui::Context) {
        if self.zip_confirm.is_none() { return }
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Modal::new(egui::Id::new("zip_confirm")).show(ctx, |ui| {
            ui.set_max_width(360.0);
            let size_mb = self.zip_confirm.as_ref().unwrap().size_bytes as f64 / 1_048_576.0;
            ui.heading("Large download");
            ui.add_space(8.0);
            ui.label(format!(
                "The selected items total {size_mb:.0} MB.\n\
                 Downloading a large archive may take a while and use significant memory.\n\n\
                 Continue anyway?"
            ));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(egui::RichText::new("Download").strong()).clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });
        if confirmed {
            let ZipConfirm { paths, current_path, .. } = self.zip_confirm.take().unwrap();
            self.transfer_status = TransferStatus::Idle;
            self.transfer = Some(download::spawn_download_zip(self.spawn_ctx(ctx), paths, current_path));
        } else if cancelled {
            self.zip_confirm = None;
        }
    }

    fn show_new_folder_modal(&mut self, ctx: &egui::Context) {
        if self.new_folder_name.is_none() { return }
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Modal::new(egui::Id::new("new_folder")).show(ctx, |ui| {
            ui.set_max_width(320.0);
            ui.heading("New folder");
            ui.add_space(8.0);
            let name = self.new_folder_name.as_mut().unwrap();
            let resp = ui.add(
                egui::TextEdit::singleline(name)
                    .hint_text("Folder name…")
                    .desired_width(f32::INFINITY),
            );
            if !resp.has_focus() { resp.request_focus(); }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_create = !name.is_empty() && !name.contains('/');
                if ui.add_enabled(can_create, egui::Button::new(egui::RichText::new("Create").strong())).clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && can_create)
                {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancelled = true;
                }
            });
        });
        if confirmed {
            let name = self.new_folder_name.take().unwrap();
            self.start_create_dir(name, ctx);
        } else if cancelled {
            self.new_folder_name = None;
        }
    }

    fn show_rename_modal(&mut self, ctx: &egui::Context) {
        if self.rename_state.is_none() { return }
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Modal::new(egui::Id::new("rename")).show(ctx, |ui| {
            ui.set_max_width(320.0);
            ui.heading("Rename");
            ui.add_space(8.0);
            let state = self.rename_state.as_mut().unwrap();
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.new_name)
                    .hint_text("New name…")
                    .desired_width(f32::INFINITY),
            );
            if !resp.has_focus() { resp.request_focus(); }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_rename = !state.new_name.is_empty() && !state.new_name.contains('/');
                if ui.add_enabled(can_rename, egui::Button::new(egui::RichText::new("Rename").strong())).clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && can_rename)
                {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancelled = true;
                }
            });
        });
        if confirmed {
            let RenameState { path, new_name } = self.rename_state.take().unwrap();
            self.start_rename(path, new_name, ctx);
        } else if cancelled {
            self.rename_state = None;
        }
    }

    fn draw_browser(&mut self, ctx: &egui::Context) {
        if self.needs_initial_load {
            self.needs_initial_load = false;
            let path = self.current_path.clone();
            self.request_listing(path, ctx);
        }

        self.poll_listing();
        self.poll_presign(ctx);
        self.poll_transfer(ctx);

        // Modals (shown over everything else)
        self.show_close_bucket_modal(ctx);
        self.show_zip_confirm_modal(ctx);
        self.show_new_folder_modal(ctx);
        self.show_rename_modal(ctx);

        let can_back = self.history_pos > 0;
        let can_forward = self.history_pos + 1 < self.history.len();
        let can_up = self.current_path.parent().is_some();
        let busy = self.transfer_busy();

        // Keyboard shortcuts
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing {
            let delete_pressed = ctx.input(|i| i.key_pressed(egui::Key::Delete));
            let f5_pressed = ctx.input(|i| i.key_pressed(egui::Key::F5));
            let backspace_pressed = ctx.input(|i| i.key_pressed(egui::Key::Backspace));
            if f5_pressed {
                let p = self.current_path.clone();
                self.request_listing(p, ctx);
            }
            if backspace_pressed && can_up && !busy {
                self.go_up(ctx);
            }
            if delete_pressed && !self.selection.is_empty() && !busy {
                let paths: Vec<_> = self.selection.iter().cloned().collect();
                self.start_delete(paths, ctx);
            }
        }

        let mut cancel_clicked = false;

        TopBottomPanel::top("toolbar").show(ctx, |ui| {
            let resp = toolbar::show(
                ui,
                toolbar::ToolbarState {
                    path_input: &mut self.path_input,
                    can_back,
                    can_forward,
                    can_up,
                    dark_mode: self.dark_mode,
                    current_path: &self.current_path,
                    editing_path: self.editing_path,
                },
            );
            if let Some(editing) = resp.set_editing {
                self.editing_path = editing;
            }
            if resp.toggle_theme {
                self.dark_mode = !self.dark_mode;
                ctx.set_visuals(if self.dark_mode { egui::Visuals::dark() } else { egui::Visuals::light() });
            }
            if resp.go_back { self.go_back(ctx); }
            if resp.go_forward { self.go_forward(ctx); }
            if resp.go_up { self.go_up(ctx); }
            if resp.refresh {
                let p = self.current_path.clone();
                self.request_listing(p, ctx);
            }
            if let Some(p) = resp.navigate_to {
                self.editing_path = false;
                self.navigate_to(p, ctx);
            }
        });

        TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let backend_name = self.backend.as_ref().map_or("—", |b| b.name());
                let n = self.entries.len();
                let muted = Color32::from_gray(90);
                ui.label(
                    RichText::new(format!("{backend_name}  ·  {n} item{}", if n == 1 { "" } else { "s" }))
                        .size(13.0)
                        .color(muted),
                );

                if busy {
                    ui.separator();
                    ui.spinner();

                    // Check for folder upload progress
                    let upload_progress = self.transfer.as_ref().and_then(|h| h.upload_progress());
                    if let Some((fraction, filename)) = upload_progress {
                        ui.label(RichText::new(format!("Uploading  {filename}")).size(13.0).color(muted));
                        ui.add(egui::ProgressBar::new(fraction).desired_width(120.0));
                    } else {
                        let progress = self.transfer.as_ref()
                            .map(|h| h.progress_msg())
                            .filter(|s| !s.is_empty());
                        let label = match &progress {
                            Some(name) => format!("Uploading  {name}"),
                            None => "Working…".to_owned(),
                        };
                        ui.label(RichText::new(label).size(13.0).color(muted));
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("Cancel upload").size(13.0).color(Color32::WHITE))
                                    .fill(Color32::from_rgb(180, 40, 40)),
                            )
                            .on_hover_text("Abort the current upload")
                            .clicked()
                        {
                            cancel_clicked = true;
                        }
                    });
                } else {
                    match &self.transfer_status {
                        TransferStatus::Idle => {}
                        TransferStatus::Pending(msg) => {
                            ui.separator();
                            ui.spinner();
                            ui.label(RichText::new(msg.as_str()).size(13.0).color(muted));
                        }
                        TransferStatus::Success(msg) => {
                            ui.separator();
                            ui.label(RichText::new(format!("✓ {msg}")).size(13.0).color(Color32::from_rgb(20, 120, 60)));
                        }
                        TransferStatus::Error(msg) => {
                            ui.separator();
                            ui.label(RichText::new(format!("{} {msg}", egui_phosphor::regular::X_CIRCLE)).size(13.0).color(Color32::from_rgb(180, 30, 30)));
                        }
                    }
                }
            });
        });

        if cancel_clicked {
            if let Some(handle) = &self.transfer { handle.cancel(); }
            self.transfer = None;
            self.transfer_status = TransferStatus::Error("Upload cancelled.".to_owned());
        }

        let max_sidebar = ctx.screen_rect().width() / 2.0;
        SidePanel::left("sidebar")
            .resizable(true)
            .default_width(220.0)
            .width_range(80.0..=max_sidebar)
            .show(ctx, |ui| {
                let resp = sidebar::show(ui, &self.current_path, true);
                if let Some(path) = resp.navigate_to {
                    self.navigate_to(path, ctx);
                }
                if resp.close_bucket {
                    self.confirming_close = true;
                }
            });

        CentralPanel::default().show(ctx, |ui| {
            let resp = file_list::show(
                ui,
                file_list::FileListState {
                    entries: &self.entries,
                    filter: &mut self.filter,
                    sort: &mut self.sort,
                    selection: &self.selection,
                    loading: self.loading,
                    error: self.error.as_deref(),
                    transfer_busy: busy,
                },
            );
            if let Some(dir) = resp.open_dir { self.navigate_to(dir, ctx); }
            if let Some(p) = resp.sel_add { self.selection.insert(p); }
            if let Some(p) = resp.sel_remove { self.selection.remove(&p); }
            if resp.sel_clear { self.selection.clear(); }
            if let Some(path) = resp.copy_url {
                let url = self.backend.as_ref()
                    .and_then(|b| b.public_url(&path))
                    .unwrap_or_else(|| path.to_string());
                ctx.copy_text(url);
                self.transfer_status = TransferStatus::Success("URL copied to clipboard".to_owned());
            }
            if let Some(path) = resp.presign { self.start_presign(path, ctx); }
            if !resp.download.is_empty() && !busy { self.start_download(resp.download, ctx); }
            if !resp.download_zip.is_empty() && !busy { self.request_zip_download(resp.download_zip, ctx); }
            if !resp.delete.is_empty() && !busy { self.start_delete(resp.delete, ctx); }
            if resp.upload && !busy { self.start_upload(ctx); }
            if resp.upload_folder && !busy { self.start_upload_folder(ctx); }
            if resp.new_folder && !busy { self.new_folder_name = Some(String::new()); }
            if let Some(path) = resp.rename { self.rename_state = Some(RenameState { path, new_name: String::new() }); }
        });
    }
}
