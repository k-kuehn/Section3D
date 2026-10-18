//! Visual theme: colours, fonts and spacing with Section3D branding.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub card_stroke: Color32,
    pub field: Color32,
    pub field_stroke: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub text_strong: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_text: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub info: Color32,
    pub viewport_bg: [f32; 3],
    pub dark: bool,
}

pub const ORANGE: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x2C);

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x0E, 0x11, 0x16),
    panel: Color32::from_rgb(0x14, 0x18, 0x1F),
    card: Color32::from_rgb(0x1A, 0x1F, 0x28),
    card_stroke: Color32::from_rgb(0x2A, 0x31, 0x3D),
    field: Color32::from_rgb(0x0F, 0x13, 0x19),
    field_stroke: Color32::from_rgb(0x34, 0x3C, 0x49),
    text: Color32::from_rgb(0xD6, 0xDD, 0xE7),
    text_weak: Color32::from_rgb(0x8C, 0x98, 0xA9),
    text_strong: Color32::from_rgb(0xF4, 0xF6, 0xF9),
    accent: ORANGE,
    accent_hover: Color32::from_rgb(0xFF, 0x87, 0x52),
    accent_text: Color32::from_rgb(0x15, 0x0A, 0x04),
    good: Color32::from_rgb(0x34, 0xC7, 0x6F),
    warn: Color32::from_rgb(0xF2, 0xB1, 0x34),
    bad: Color32::from_rgb(0xF0, 0x52, 0x52),
    info: Color32::from_rgb(0x5B, 0x9C, 0xF5),
    viewport_bg: [0.075, 0.085, 0.105],
    dark: true,
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xE9, 0xED, 0xF2),
    panel: Color32::from_rgb(0xF4, 0xF6, 0xF9),
    card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    card_stroke: Color32::from_rgb(0xD9, 0xDF, 0xE7),
    field: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    field_stroke: Color32::from_rgb(0xC7, 0xCF, 0xDA),
    text: Color32::from_rgb(0x27, 0x30, 0x3D),
    text_weak: Color32::from_rgb(0x64, 0x70, 0x80),
    text_strong: Color32::from_rgb(0x0F, 0x17, 0x2A),
    accent: Color32::from_rgb(0xF0, 0x5A, 0x1A),
    accent_hover: Color32::from_rgb(0xFF, 0x6B, 0x2C),
    accent_text: Color32::WHITE,
    good: Color32::from_rgb(0x16, 0xA3, 0x4A),
    warn: Color32::from_rgb(0xC2, 0x84, 0x0C),
    bad: Color32::from_rgb(0xDC, 0x26, 0x26),
    info: Color32::from_rgb(0x25, 0x63, 0xEB),
    viewport_bg: [0.90, 0.92, 0.95],
    dark: false,
};

pub fn palette(ctx: &egui::Context) -> Palette {
    if ctx.global_style().visuals.dark_mode { DARK } else { LIGHT }
}

pub fn ui_palette(ui: &egui::Ui) -> Palette {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), FontData::from_static(include_bytes!("../assets/Inter-400.ttf")).into());
    fonts.font_data.insert("inter-semibold".into(), FontData::from_static(include_bytes!("../assets/Inter-600.ttf")).into());
    fonts.font_data.insert("inter-bold".into(), FontData::from_static(include_bytes!("../assets/Inter-700.ttf")).into());
    fonts.font_data.insert("mono".into(), FontData::from_static(include_bytes!("../assets/JetBrainsMono-400.ttf")).into());
    // Small Noto subsets for UI symbols (see tools/make_symbol_fonts.py).
    fonts.font_data.insert("symbols".into(), FontData::from_static(include_bytes!("../assets/Symbols.ttf")).into());
    fonts.font_data.insert("math".into(), FontData::from_static(include_bytes!("../assets/MathSymbols.ttf")).into());
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "inter".into());
    fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "mono".into());
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        let f = fonts.families.get_mut(&fam).unwrap();
        f.push("symbols".into());
        f.push("math".into());
    }
    let fallback: Vec<String> = fonts.families[&FontFamily::Proportional].iter().skip(1).cloned().collect();
    let mut semi = vec!["inter-semibold".to_string()];
    semi.extend(fallback.iter().cloned());
    let mut bold = vec!["inter-bold".to_string()];
    bold.extend(fallback);
    fonts.families.insert(FontFamily::Name("semibold".into()), semi);
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    let r = CornerRadius::same(6);
    v.panel_fill = p.panel;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.card_stroke);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.card;
    v.code_bg_color = p.field;
    v.override_text_color = None;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.bad;
    v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.45 } else { 0.30 });
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.slider_trailing_fill = true;
    v.striped = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_stroke);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;

    let btn = if p.dark { Color32::from_rgb(0x23, 0x29, 0x33) } else { Color32::from_rgb(0xF4, 0xF6, 0xF9) };
    let btn_hover = if p.dark { Color32::from_rgb(0x2C, 0x33, 0x40) } else { Color32::from_rgb(0xE8, 0xEC, 0xF1) };
    w.inactive.bg_fill = p.field;
    w.inactive.weak_bg_fill = btn;
    w.inactive.bg_stroke = Stroke::new(1.0, p.field_stroke);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = p.field;
    w.hovered.weak_bg_fill = btn_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.8));
    w.hovered.fg_stroke = Stroke::new(1.5, p.text_strong);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.field;
    w.active.weak_bg_fill = btn_hover;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.text_strong);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.weak_bg_fill = btn_hover;
    w.open.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.corner_radius = r;
    v
}

pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = if dark { DARK } else { LIGHT };
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    let theme = if dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.style_mut_of(theme, |s| {
        s.visuals = visuals(&p);
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 5.0);
        s.spacing.interact_size = egui::vec2(36.0, 28.0);
        s.spacing.window_margin = Margin::same(14);
        s.spacing.menu_margin = Margin::same(6);
        s.spacing.combo_width = 120.0;
        s.spacing.slider_width = 140.0;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.0));
        s.text_styles.insert(TextStyle::Heading, semibold(17.0));
        s.interaction.tooltip_delay = 0.35;
    });
}
