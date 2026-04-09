use egui::FontId;

pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Phosphor icon font — registered as a proportional fallback so icon
    // codepoints render from this font while Latin text uses the system font.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    ctx.set_fonts(fonts);

    // Enforce minimum readable sizes — nothing smaller than 13 px (≈ 10 pt).
    ctx.style_mut(|style| {
        use egui::{FontFamily::Proportional, TextStyle::*};
        style
            .text_styles
            .insert(Body, FontId::new(14.0, Proportional));
        style
            .text_styles
            .insert(Button, FontId::new(14.0, Proportional));
        // "Small" is used for hints and secondary labels; floor at 13 px.
        style
            .text_styles
            .insert(Small, FontId::new(13.0, Proportional));
        style
            .text_styles
            .insert(Heading, FontId::new(22.0, Proportional));
        style
            .text_styles
            .insert(Monospace, FontId::new(13.0, egui::FontFamily::Monospace));
    });
}
