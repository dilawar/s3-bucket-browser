use egui::{Color32, RichText, Ui};

use crate::storage::StoragePath;

pub struct SidebarResponse {
    pub navigate_to: Option<StoragePath>,
}

const INDENT: f32 = 14.0;

pub fn show(ui: &mut Ui, current_path: &StoragePath, filter: &mut String) -> SidebarResponse {
    let mut navigate_to = None;

    ui.heading("Location");
    ui.separator();

    // Filter above the tree.
    ui.label("Filter:");
    ui.add(egui::TextEdit::singleline(filter).desired_width(f32::INFINITY));
    ui.add_space(6.0);

    // Breadcrumb tree with indentation per depth level.
    let crumbs = current_path.breadcrumbs();
    let last_idx = crumbs.len().saturating_sub(1);
    for (depth, (label, path)) in crumbs.into_iter().enumerate() {
        let is_last = depth == last_idx;
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * INDENT);
            // Connector glyph
            let glyph = if is_last { "└ " } else { "├ " };
            ui.label(RichText::new(glyph).color(Color32::GRAY).monospace());
            // Leaf is shown as plain text; ancestors are clickable links.
            // Both are truncated so they never push the panel wider.
            if is_last {
                ui.add(egui::Label::new(RichText::new(&label).strong()).truncate());
            } else if ui
                .add(egui::Label::new(egui::RichText::new(&label).color(ui.visuals().hyperlink_color)).truncate().sense(egui::Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                navigate_to = Some(path);
            }
        });
    }

    SidebarResponse { navigate_to }
}
