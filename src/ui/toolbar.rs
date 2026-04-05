use egui::{Button, Key, RichText, TextEdit, Ui};

use crate::storage::StoragePath;

/// Read-only state the toolbar needs to render itself.
pub struct ToolbarState<'a> {
    pub path_input: &'a mut String,
    pub can_back: bool,
    pub can_forward: bool,
    pub can_up: bool,
    pub dark_mode: bool,
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
}

pub fn show(ui: &mut Ui, state: ToolbarState<'_>) -> ToolbarResponse {
    let ToolbarState {
        path_input,
        can_back,
        can_forward,
        can_up,
        dark_mode,
    } = state;
    let mut resp = ToolbarResponse::default();

    ui.horizontal(|ui| {
        resp.go_back = ui
            .add_enabled(can_back, Button::new(RichText::new("◀").size(16.0)))
            .clicked();
        resp.go_forward = ui
            .add_enabled(can_forward, Button::new(RichText::new("▶").size(16.0)))
            .clicked();
        resp.go_up = ui
            .add_enabled(can_up, Button::new(RichText::new("⬆").size(16.0)))
            .on_hover_text("Go to parent directory")
            .clicked();
        resp.refresh = ui.button(RichText::new("⟳").size(18.0)).clicked();

        ui.separator();

        let path_response = ui.add(
            TextEdit::singleline(path_input)
                .desired_width(f32::INFINITY)
                .hint_text("Path or s3://bucket/prefix …"),
        );

        if path_response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            resp.navigate_to = Some(StoragePath::parse(path_input));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let icon = if dark_mode { "☀️" } else { "🌙" };
            let tooltip = if dark_mode {
                "Switch to light theme"
            } else {
                "Switch to dark theme"
            };
            if ui
                .button(RichText::new(icon).size(18.0))
                .on_hover_text(tooltip)
                .clicked()
            {
                resp.toggle_theme = true;
            }
        });
    });

    resp
}
