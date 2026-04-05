use std::cell::Cell;
use std::collections::HashSet;

use egui::{Button, Color32, Label, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};

use crate::storage::{EntryKind, StorageEntry, StoragePath, human_size};

// ── Sort state ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    #[default]
    Name,
    Size,
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

#[derive(Clone, Copy, Default)]
pub struct SortState {
    pub col: SortColumn,
    pub dir: SortDir,
}

impl SortState {
    /// Toggle direction if already on this column; otherwise select it (Asc).
    fn click(&mut self, col: SortColumn) {
        if self.col == col {
            self.dir = match self.dir {
                SortDir::Asc => SortDir::Desc,
                SortDir::Desc => SortDir::Asc,
            };
        } else {
            self.col = col;
            self.dir = SortDir::Asc;
        }
    }
}

#[derive(Default)]
pub struct FileListResponse {
    pub open_dir: Option<StoragePath>,
    /// Files/dirs to download (single or batch).
    pub download: Vec<StoragePath>,
    /// Selection to pack into a single ZIP file.
    pub download_zip: Vec<StoragePath>,
    /// Files/dirs to delete (single or batch).
    pub delete: Vec<StoragePath>,
    pub upload: bool,
    /// Path whose public URL should be copied to clipboard (sync, no network).
    pub copy_url: Option<StoragePath>,
    /// Path for which a 24-hour presigned URL should be generated and copied.
    pub presign: Option<StoragePath>,
    // Selection mutations — applied by app.rs after show() returns.
    pub sel_add: Option<StoragePath>,
    pub sel_remove: Option<StoragePath>,
    pub sel_clear: bool,
}

const FIXED_WIDTH: f32 = 24.0 + 28.0 + 80.0 + 130.0 + 28.0 + 48.0; // checkbox+icon+size+modified+copy
const LINE_HEIGHT: f32 = 18.0;
const ROW_V_PAD: f32 = 6.0;
const ROW_PADDING: f32 = ROW_V_PAD * 2.0;
const CHAR_WIDTH: f32 = 7.5;

fn row_height(name: &str, name_col_width: f32) -> f32 {
    let lines = ((name.len() as f32 * CHAR_WIDTH) / name_col_width)
        .ceil()
        .max(1.0);
    LINE_HEIGHT * lines + ROW_PADDING
}

fn file_icon(name: &str) -> &'static str {
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    match mime.type_().as_str() {
        "image" => "🖼",
        "audio" => "🎵",
        "video" => "🎬",
        "text" => "📝",
        _ => match mime.subtype().as_str() {
            "zip" | "gzip" | "x-tar" | "x-bzip2" | "x-xz" | "x-7z-compressed"
            | "x-rar-compressed" => "📦",
            "pdf" => "📕",
            _ => "📄",
        },
    }
}

pub fn show(
    ui: &mut Ui,
    entries: &[StorageEntry],
    filter: &mut String,
    sort: &mut SortState,
    selection: &HashSet<StoragePath>,
    loading: bool,
    error: Option<&str>,
    transfer_busy: bool,
) -> FileListResponse {
    // ── All output state ──────────────────────────────────────────────────────
    let upload = Cell::new(false);
    let open_dir: Cell<Option<StoragePath>> = Cell::new(None);
    let download: Cell<Vec<StoragePath>> = Cell::new(Vec::new());
    let download_zip: Cell<Vec<StoragePath>> = Cell::new(Vec::new());
    let delete: Cell<Vec<StoragePath>> = Cell::new(Vec::new());
    let copy_url: Cell<Option<StoragePath>> = Cell::new(None);
    let presign: Cell<Option<StoragePath>> = Cell::new(None);
    let sel_add: Cell<Option<StoragePath>> = Cell::new(None);
    let sel_remove: Cell<Option<StoragePath>> = Cell::new(None);
    let sel_clear = Cell::new(false);

    // ── Background right-click ────────────────────────────────────────────────
    let bg_resp = ui.interact(ui.max_rect(), ui.id().with("bg_ctx"), Sense::click());
    bg_resp.context_menu(|ui| {
        upload_item(ui, transfer_busy, &upload);
    });

    // ── Layout: upload link pinned to bottom ─────────────────────────────────
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
        ui.add_space(6.0);
        let color = if transfer_busy {
            Color32::from_gray(160)
        } else {
            Color32::from_rgb(37, 99, 235)
        };
        let resp = ui
            .add(
                Label::new(
                    RichText::new("+ Upload file")
                        .color(color)
                        .size(14.0)
                        .underline(),
                )
                .sense(Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Upload a file to the current location");
        if resp.clicked() && !transfer_busy {
            upload.set(true);
        }
        ui.add_space(4.0);
        ui.separator();

        // Main content (top-down)
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            // Filter bar
            ui.horizontal(|ui| {
                ui.label(RichText::new("🔍").size(18.0));
                ui.add(
                    egui::TextEdit::singleline(filter)
                        .hint_text("Filter…")
                        .desired_width(f32::INFINITY),
                );
            });

            // Selection action bar
            if !selection.is_empty() {
                ui.separator();
                let n = selection.len();
                let n_files = selection
                    .iter()
                    .filter(|p| {
                        entries
                            .iter()
                            .any(|e| &e.path == *p && e.kind == EntryKind::File)
                    })
                    .count();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{n} selected"))
                            .strong()
                            .color(Color32::from_rgb(100, 180, 255)),
                    );
                    ui.add_space(4.0);
                    if n_files > 0
                        && ui
                            .add_enabled(
                                !transfer_busy,
                                Button::new(format!("⬇ Download ({n_files})")),
                            )
                            .on_hover_text("Download selected files individually")
                            .clicked()
                    {
                        let paths: Vec<_> = selection
                            .iter()
                            .filter(|p| {
                                entries
                                    .iter()
                                    .any(|e| &e.path == *p && e.kind == EntryKind::File)
                            })
                            .cloned()
                            .collect();
                        download.set(paths);
                    }
                    if ui
                        .add_enabled(!transfer_busy, Button::new("⬇ Download as ZIP"))
                        .on_hover_text("Pack all selected items into a single ZIP file")
                        .clicked()
                    {
                        download_zip.set(selection.iter().cloned().collect());
                    }
                    if ui
                        .add_enabled(
                            !transfer_busy,
                            Button::new(format!("🗑 Delete ({n})"))
                                .fill(Color32::from_rgb(180, 40, 40)),
                        )
                        .on_hover_text("Delete all selected items")
                        .clicked()
                    {
                        delete.set(selection.iter().cloned().collect());
                    }
                    if ui
                        .button("✕ Clear")
                        .on_hover_text("Clear selection")
                        .clicked()
                    {
                        sel_clear.set(true);
                    }
                });
            }

            ui.separator();

            if loading {
                ui.centered_and_justified(|ui| {
                    ui.spinner();
                });
                return;
            }
            if let Some(msg) = error {
                ui.label(RichText::new(format!("✗  {msg}")).color(Color32::from_rgb(180, 30, 30)));
                return;
            }

            let filter_lc = filter.to_lowercase();
            let mut visible: Vec<&StorageEntry> = entries
                .iter()
                .filter(|e| filter_lc.is_empty() || e.name.to_lowercase().contains(&filter_lc))
                .collect();

            // Sort: directories always first, then by chosen column.
            visible.sort_by(|a, b| {
                use std::cmp::Ordering;
                // Dirs before files
                let kind_ord = match (&a.kind, &b.kind) {
                    (EntryKind::Directory, EntryKind::File) => return Ordering::Less,
                    (EntryKind::File, EntryKind::Directory) => return Ordering::Greater,
                    _ => Ordering::Equal,
                };
                let col_ord = match sort.col {
                    SortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    SortColumn::Size => a.size.cmp(&b.size),
                    SortColumn::Modified => a.last_modified.cmp(&b.last_modified),
                };
                let ord = kind_ord.then(col_ord);
                if sort.dir == SortDir::Desc {
                    ord.reverse()
                } else {
                    ord
                }
            });

            let name_col_width = (ui.available_width() - FIXED_WIDTH).max(80.0);

            // Capture sort clicks from headers; applied after the table renders.
            let mut sort_click: Option<SortColumn> = None;

            // Helper: render a sortable column header — entire cell width is clickable.
            let header_label = |ui: &mut Ui, label: &str, col: SortColumn, s: &SortState| {
                let arrow = if s.col == col {
                    if s.dir == SortDir::Asc { " ↑" } else { " ↓" }
                } else {
                    ""
                };
                // Interact with the full cell rect first.
                let resp = ui.interact(ui.max_rect(), ui.id().with(label), Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    ui.painter().rect_filled(
                        ui.max_rect(),
                        2.0,
                        Color32::from_rgba_premultiplied(0, 0, 0, 18),
                    );
                }
                let text = RichText::new(format!("{label}{arrow}")).strong();
                ui.label(text);
                let tooltip = if s.col == col && s.dir == SortDir::Asc {
                    format!("Sort by {label} descending")
                } else {
                    format!("Sort by {label} ascending")
                };
                resp.on_hover_text(tooltip).clicked()
            };

            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .auto_shrink(false)
                .column(Column::exact(24.0)) // checkbox
                .column(Column::exact(28.0)) // icon
                .column(Column::remainder().clip(false)) // name
                .column(Column::initial(80.0).resizable(true)) // size
                .column(Column::initial(130.0).resizable(true)) // modified
                .column(Column::exact(28.0)) // copy
                .header(22.0, |mut h| {
                    h.col(|_| {});
                    h.col(|_| {});
                    h.col(|ui| {
                        if header_label(ui, "Name", SortColumn::Name, sort) {
                            sort_click = Some(SortColumn::Name);
                        }
                    });
                    h.col(|ui| {
                        if header_label(ui, "Size", SortColumn::Size, sort) {
                            sort_click = Some(SortColumn::Size);
                        }
                    });
                    h.col(|ui| {
                        if header_label(ui, "Modified", SortColumn::Modified, sort) {
                            sort_click = Some(SortColumn::Modified);
                        }
                    });
                    h.col(|_| {});
                })
                .body(|body| {
                    let heights = visible.iter().map(|e| row_height(&e.name, name_col_width));
                    body.heterogeneous_rows(heights, |mut row| {
                        let entry = visible[row.index()];
                        let is_selected = selection.contains(&entry.path);

                        // ── checkbox ──────────────────────────────────────────
                        row.col(|ui| {
                            ui.add_space(ROW_V_PAD);
                            let mut checked = is_selected;
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked {
                                    sel_add.set(Some(entry.path.clone()));
                                } else {
                                    sel_remove.set(Some(entry.path.clone()));
                                }
                            }
                        });

                        // ── icon ──────────────────────────────────────────────
                        row.col(|ui| {
                            ui.add_space(ROW_V_PAD);
                            ui.label(
                                RichText::new(match &entry.kind {
                                    EntryKind::Directory => "📁",
                                    EntryKind::File => file_icon(&entry.name),
                                })
                                .size(18.0),
                            );
                        });

                        // ── name ──────────────────────────────────────────────
                        row.col(|ui| {
                            ui.add_space(ROW_V_PAD);
                            let color = if is_selected {
                                Color32::from_rgb(160, 210, 255)
                            } else if entry.kind == EntryKind::Directory {
                                Color32::from_rgb(100, 180, 255)
                            } else {
                                ui.visuals().text_color()
                            };
                            let resp = ui
                                .add(
                                    Label::new(RichText::new(&entry.name).color(color))
                                        .wrap()
                                        .sense(Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(entry.path.to_string());

                            if resp.clicked() {
                                match entry.kind {
                                    EntryKind::Directory => open_dir.set(Some(entry.path.clone())),
                                    EntryKind::File => download.set(vec![entry.path.clone()]),
                                }
                            }

                            // Right-click context menu
                            resp.context_menu(|ui| {
                                if entry.kind == EntryKind::File
                                    && ui.button("⬇ Download").clicked()
                                {
                                    download.set(vec![entry.path.clone()]);
                                    ui.close_menu();
                                }
                                let del_label = if is_selected && selection.len() > 1 {
                                    format!("🗑 Delete ({} selected)", selection.len())
                                } else {
                                    "🗑 Delete".to_owned()
                                };
                                if ui
                                    .add(
                                        Button::new(del_label).fill(Color32::from_rgb(180, 40, 40)),
                                    )
                                    .clicked()
                                {
                                    let paths = if is_selected && selection.len() > 1 {
                                        selection.iter().cloned().collect()
                                    } else {
                                        vec![entry.path.clone()]
                                    };
                                    delete.set(paths);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui
                                    .button("⎘  Copy URL")
                                    .on_hover_text("Copy the public URL to clipboard")
                                    .clicked()
                                {
                                    copy_url.set(Some(entry.path.clone()));
                                    ui.close_menu();
                                }
                                if entry.kind == EntryKind::File
                                    && ui
                                        .button("🔑  Copy presigned URL (24 h)")
                                        .on_hover_text(
                                            "Generate a presigned URL valid for 24 hours",
                                        )
                                        .clicked()
                                {
                                    presign.set(Some(entry.path.clone()));
                                    ui.close_menu();
                                }
                                ui.separator();
                                upload_item(ui, transfer_busy, &upload);
                            });
                        });

                        // ── size ──────────────────────────────────────────────
                        row.col(|ui| {
                            ui.add_space(ROW_V_PAD);
                            if let Some(sz) = entry.size {
                                ui.label(
                                    RichText::new(human_size(sz))
                                        .color(Color32::from_gray(90))
                                        .size(13.0),
                                );
                            }
                        });

                        // ── modified ──────────────────────────────────────────
                        row.col(|ui| {
                            ui.add_space(ROW_V_PAD);
                            if let Some(ts) = entry.last_modified {
                                ui.label(
                                    RichText::new(ts.format("%Y-%m-%d %H:%M").to_string())
                                        .color(Color32::from_gray(90))
                                        .size(13.0),
                                );
                            }
                        });

                        // ── copy path ─────────────────────────────────────────
                        row.col(|ui| {
                            ui.add_space(ROW_V_PAD);
                            let path_str = entry.path.to_string();
                            if ui
                                .button(RichText::new("⎘").size(16.0))
                                .on_hover_text(format!("Copy: {path_str}"))
                                .clicked()
                            {
                                ui.ctx().copy_text(path_str);
                            }
                        });
                    });
                });
            // Apply any column header click after the table is rendered.
            if let Some(col) = sort_click {
                sort.click(col);
            }
        });
    });

    FileListResponse {
        open_dir: open_dir.into_inner(),
        download: download.into_inner(),
        download_zip: download_zip.into_inner(),
        delete: delete.into_inner(),
        upload: upload.get(),
        copy_url: copy_url.into_inner(),
        presign: presign.into_inner(),
        sel_add: sel_add.into_inner(),
        sel_remove: sel_remove.into_inner(),
        sel_clear: sel_clear.get(),
    }
}

fn upload_item(ui: &mut Ui, transfer_busy: bool, upload: &Cell<bool>) {
    if ui
        .add_enabled(!transfer_busy, Button::new("+ Upload file"))
        .on_hover_text("Upload a file to the current location")
        .clicked()
    {
        upload.set(true);
        ui.close_menu();
    }
}
