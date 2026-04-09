use egui::{Button, Color32, Key, Label, RichText, Sense, TextEdit, Ui};

use crate::storage::StoragePath;

/// Read-only state the toolbar needs to render itself.
pub struct ToolbarState<'a> {
    pub path_input: &'a mut String,
    pub can_back: bool,
    pub can_forward: bool,
    pub can_up: bool,
    pub dark_mode: bool,
    pub current_path: &'a StoragePath,
    pub editing_path: bool,
}

/// Actions the toolbar produced this frame.
#[derive(Default)]
pub struct ToolbarResponse {
    pub navigate_to: Option<StoragePath>,
    pub go_back: bool,
    pub go_forward: bool,
    pub go_up: bool,
    pub refresh: bool,
    pub toggle_theme: bool,
    /// Some(true) = enter path-edit mode; Some(false) = leave it.
    pub set_editing: Option<bool>,
}

pub fn show(ui: &mut Ui, state: ToolbarState<'_>) -> ToolbarResponse {
    let ToolbarState {
        path_input,
        can_back,
        can_forward,
        can_up,
        dark_mode,
        current_path,
        editing_path,
    } = state;
    let mut resp = ToolbarResponse::default();

    use egui_phosphor::regular as ph;
    ui.horizontal(|ui| {
        resp.go_back = ui
            .add_enabled(can_back, Button::new(RichText::new(ph::ARROW_LEFT).size(16.0)))
            .on_hover_text("Go back")
            .clicked();
        resp.go_forward = ui
            .add_enabled(can_forward, Button::new(RichText::new(ph::ARROW_RIGHT).size(16.0)))
            .on_hover_text("Go forward")
            .clicked();
        resp.go_up = ui
            .add_enabled(can_up, Button::new(RichText::new(ph::ARROW_UP).size(16.0)))
            .on_hover_text("Go to parent directory")
            .clicked();
        resp.refresh = ui
            .button(RichText::new(ph::ARROWS_CLOCKWISE).size(18.0))
            .on_hover_text("Refresh")
            .clicked();

        ui.separator();

        // Theme button on the far right — add it first in a right_to_left sub-layout
        // so the middle section can take the remaining space.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let icon = if dark_mode { ph::SUN } else { ph::MOON };
            let tooltip = if dark_mode { "Switch to light theme" } else { "Switch to dark theme" };
            if ui
                .button(RichText::new(icon).size(18.0))
                .on_hover_text(tooltip)
                .clicked()
            {
                resp.toggle_theme = true;
            }

            ui.separator();

            // Middle section: breadcrumbs or text input
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if editing_path {
                    let text_resp = ui.add(
                        TextEdit::singleline(path_input)
                            .desired_width(f32::INFINITY)
                            .hint_text("Path or s3://bucket/prefix …"),
                    );
                    if !text_resp.has_focus() {
                        text_resp.request_focus();
                    }
                    if text_resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        resp.navigate_to = Some(StoragePath::parse(path_input));
                        resp.set_editing = Some(false);
                    }
                    if ui.input(|i| i.key_pressed(Key::Escape)) {
                        resp.set_editing = Some(false);
                    }
                } else {
                    // Breadcrumbs
                    let crumbs = current_path.breadcrumbs();
                    let muted = Color32::from_gray(130);
                    for (i, (label, path)) in crumbs.iter().enumerate() {
                        if i > 0 {
                            ui.label(RichText::new("›").color(muted));
                        }
                        let link = ui.add(
                            Label::new(RichText::new(label).strong())
                                .sense(Sense::click()),
                        );
                        if link.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if link.clicked() {
                            resp.navigate_to = Some(path.clone());
                            resp.set_editing = Some(false);
                        }
                    }
                    // Edit button
                    if ui
                        .add(Label::new(RichText::new(ph::PENCIL).color(muted)).sense(Sense::click()))
                        .on_hover_text("Edit path manually")
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        *path_input = current_path.to_string();
                        resp.set_editing = Some(true);
                    }
                }
            });
        });
    });

    resp
}
