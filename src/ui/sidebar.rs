use egui::{Color32, RichText, Ui};

use crate::storage::StoragePath;

#[derive(Default)]
pub struct SidebarResponse {
    pub navigate_to: Option<StoragePath>,
    /// User clicked the bucket name — caller should open the credential form.
    pub open_config: bool,
}

const INDENT: f32 = 14.0;

pub fn show(ui: &mut Ui, current_path: &StoragePath) -> SidebarResponse {
    let mut navigate_to = None;
    let mut open_config = false;

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

            if depth == 0 {
                // Bucket name: always clickable — opens the credential/config form.
                let style = if is_last {
                    // At root: bold + link colour so it's clearly interactive.
                    egui::RichText::new(&label)
                        .strong()
                        .color(ui.visuals().hyperlink_color)
                } else {
                    egui::RichText::new(&label).color(ui.visuals().hyperlink_color)
                };
                if ui
                    .add(egui::Label::new(style).truncate().sense(egui::Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Click to edit connection / credentials")
                    .clicked()
                {
                    open_config = true;
                }
            } else if is_last {
                // Current folder: plain bold text, not interactive.
                ui.add(egui::Label::new(RichText::new(&label).strong()).truncate());
            } else {
                // Ancestor folder: clickable, navigates to that path.
                if ui
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
            }
        });
    }

    SidebarResponse { navigate_to, open_config }
}
