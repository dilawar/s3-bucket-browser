use egui::{Color32, RichText, Ui};

use crate::storage::StoragePath;

#[derive(Default)]
pub struct SidebarResponse {
    pub navigate_to: Option<StoragePath>,
    pub close_bucket: bool,
}

const INDENT: f32 = 14.0;

pub fn show(ui: &mut Ui, current_path: &StoragePath, connected: bool) -> SidebarResponse {
    let mut navigate_to = None;
    let mut close_bucket = false;

    ui.heading("Location");
    ui.separator();

    // Breadcrumb tree with indentation per depth level.
    let crumbs = current_path.breadcrumbs();
    let last_idx = crumbs.len().saturating_sub(1);
    for (depth, (label, path)) in crumbs.into_iter().enumerate() {
        let is_last = depth == last_idx;
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * INDENT);
            let glyph = if is_last { "└ " } else { "├ " };
            ui.label(RichText::new(glyph).color(Color32::GRAY).monospace());

            if is_last {
                // Current location: plain bold text, not interactive.
                ui.add(egui::Label::new(RichText::new(&label).strong()).truncate());
            } else if ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(&label).color(ui.visuals().hyperlink_color),
                    )
                    .truncate()
                    .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                navigate_to = Some(path);
            }
        });
    }

    // ── Close bucket button — pinned to the bottom of the panel ─────────────
    if connected {
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(6.0);
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(
                            format!("{}  Close bucket", egui_phosphor::regular::X)
                        )
                            .color(Color32::from_rgb(180, 40, 40))
                            .strong(),
                    )
                    .min_size(egui::vec2(ui.available_width(), 0.0)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                close_bucket = true;
            }
            ui.separator();
        });
    }

    SidebarResponse { navigate_to, close_bucket }
}
