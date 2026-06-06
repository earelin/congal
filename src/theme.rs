//! Tema visual inspirado nas Apple Human Interface Guidelines.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    TextStyle, Vec2, Visuals,
};
use std::sync::Arc;

/// Carga a fonte Inter e define a xerarquía tipográfica.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-medium".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Medium.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );

    // Corpo: Inter Regular como fonte proporcional principal.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());

    // Familias con nome para pesos específicos.
    fonts
        .families
        .insert(FontFamily::Name("medium".into()), vec!["inter-medium".to_owned()]);
    fonts.families.insert(
        FontFamily::Name("semibold".into()),
        vec!["inter-semibold".to_owned()],
    );

    ctx.set_fonts(fonts);
}

/// Aplica o tema completo (fontes, tipografía, espazado e cores).
pub fn apply(ctx: &egui::Context, dark: bool) {
    let mut style = egui::Style::default();

    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(21.0, FontFamily::Name("semibold".into())),
        ),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.5, FontFamily::Name("medium".into())),
        ),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
    ]
    .into();

    // Ritmo e espazado xenerosos (retícula base ~8px).
    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 8.0);
    s.button_padding = Vec2::new(12.0, 7.0);
    s.interact_size.y = 30.0;
    s.window_margin = Margin::same(14);
    s.menu_margin = Margin::same(8);
    s.indent = 18.0;
    s.scroll.bar_width = 9.0;

    style.visuals = if dark { dark_visuals() } else { light_visuals() };
    ctx.set_global_style(style);
}

const ACCENT_LIGHT: Color32 = Color32::from_rgb(0, 122, 255); // #007AFF
const ACCENT_DARK: Color32 = Color32::from_rgb(10, 132, 255); // #0A84FF

fn round(v: &mut Visuals, r: u8) {
    let cr = CornerRadius::same(r);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = cr;
    }
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
}

fn light_visuals() -> Visuals {
    let mut v = Visuals::light();
    let text = Color32::from_rgb(29, 29, 31); // #1D1D1F
    let surface = Color32::from_rgb(255, 255, 255);
    let panel = Color32::from_rgb(245, 245, 247); // #F5F5F7
    let separator = Color32::from_rgb(222, 222, 226);

    v.override_text_color = Some(text);
    v.panel_fill = panel;
    v.window_fill = surface;
    v.extreme_bg_color = surface; // fondo de campos de texto
    v.faint_bg_color = Color32::from_rgb(248, 248, 250); // raias de táboa
    v.hyperlink_color = ACCENT_LIGHT;

    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, separator);
    v.widgets.inactive.weak_bg_fill = surface;
    v.widgets.inactive.bg_fill = surface;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, separator);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(238, 238, 242);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(200, 200, 206));
    v.widgets.active.weak_bg_fill = Color32::from_rgb(228, 228, 234);

    v.selection.bg_fill = ACCENT_LIGHT.gamma_multiply(0.25);
    v.selection.stroke = Stroke::new(1.0, ACCENT_LIGHT);

    round(&mut v, 8);
    v
}

fn dark_visuals() -> Visuals {
    let mut v = Visuals::dark();
    let text = Color32::from_rgb(245, 245, 247);
    let surface = Color32::from_rgb(44, 44, 46); // #2C2C2E
    let panel = Color32::from_rgb(30, 30, 32);
    let separator = Color32::from_rgb(64, 64, 67);

    v.override_text_color = Some(text);
    v.panel_fill = panel;
    v.window_fill = surface;
    v.extreme_bg_color = Color32::from_rgb(24, 24, 26);
    v.faint_bg_color = Color32::from_rgb(38, 38, 40);
    v.hyperlink_color = ACCENT_DARK;

    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, separator);
    v.widgets.inactive.weak_bg_fill = surface;
    v.widgets.inactive.bg_fill = surface;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, separator);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 58, 60);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(80, 80, 84));
    v.widgets.active.weak_bg_fill = Color32::from_rgb(68, 68, 70);

    v.selection.bg_fill = ACCENT_DARK.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT_DARK);

    round(&mut v, 8);
    v
}

/// Color de acento segundo o modo.
pub fn accent(dark: bool) -> Color32 {
    if dark { ACCENT_DARK } else { ACCENT_LIGHT }
}
