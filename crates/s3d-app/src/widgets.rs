//! Reusable widgets: cards, labelled numeric fields with
//! units and `?` help, primary buttons, badges and toggles.

use egui::{Align, Color32, CornerRadius, Layout, Response, RichText, Sense, Stroke, Ui, Vec2};

use crate::theme::{self, Palette};
use crate::tooltips;

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = theme::ui_palette(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_stroke))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Small uppercase heading used inside cards ("FEATURES").
pub fn section_title(ui: &mut Ui, text: &str) {
    let p = theme::ui_palette(ui);
    ui.label(RichText::new(text.to_uppercase()).font(theme::semibold(12.5)).color(p.text_weak).extra_letter_spacing(1.0));
}

/// Label with a `?` marker that shows the setting's help (text + diagram).
pub fn field_label(ui: &mut Ui, text: &str, help_id: &str) -> Response {
    let p = theme::ui_palette(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(RichText::new(text).color(p.text_weak));
        help_marker(ui, help_id)
    })
    .inner
}

pub fn help_marker(ui: &mut Ui, help_id: &str) -> Response {
    let p = theme::ui_palette(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
    let hovered = resp.hovered();
    let c = if hovered { p.accent } else { p.text_weak.gamma_multiply(0.8) };
    ui.painter().circle_stroke(rect.center(), 6.5, Stroke::new(1.0, c));
    ui.painter().text(rect.center() + egui::vec2(0.0, 0.5), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(10.0), c);
    let id = help_id.to_string();
    resp.on_hover_ui(move |ui| tooltips::show(ui, &id))
}

/// Numeric input with a unit suffix, styled as a spin field.
pub fn num(ui: &mut Ui, value: &mut f32, unit: &str, range: std::ops::RangeInclusive<f32>, decimals: usize) -> Response {
    let speed = ((*range.end() - *range.start()) / 400.0).max(0.01);
    let suffix = if unit.is_empty() { String::new() } else { format!(" {unit}") };
    ui.add_sized(
        [ui.available_width().min(170.0), 28.0],
        egui::DragValue::new(value).range(range).speed(speed).max_decimals(decimals).suffix(suffix),
    )
}

pub fn num_u32(ui: &mut Ui, value: &mut u32, range: std::ops::RangeInclusive<u32>) -> Response {
    ui.add_sized([ui.available_width().min(170.0), 28.0], egui::DragValue::new(value).range(range).speed(0.05))
}

/// One labelled setting cell: label + `?` above the control.
pub fn setting<R>(ui: &mut Ui, label: &str, help_id: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        field_label(ui, label, help_id);
        control(ui)
    })
    .inner
}

/// Lays out setting cells in a grid of `cols` equal columns.
pub fn setting_grid(ui: &mut Ui, id: &str, cols: usize, add: impl FnOnce(&mut Ui)) {
    let w = (ui.available_width() - (cols as f32 - 1.0) * 12.0) / cols as f32;
    egui::Grid::new(id).num_columns(cols).spacing([12.0, 10.0]).min_col_width(w).max_col_width(w).show(ui, add);
}

pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let p = theme::ui_palette(ui);
    let fill = if enabled { p.accent } else { p.accent.gamma_multiply(0.35) };
    let btn = egui::Button::new(RichText::new(text).font(theme::semibold(14.5)).color(p.accent_text))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(7))
        .min_size(Vec2::new(ui.available_width(), 36.0));
    let r = ui.add_enabled(enabled, btn);
    if r.hovered() && enabled {
        ui.painter().rect_stroke(r.rect, CornerRadius::same(7), Stroke::new(1.0, p.accent_hover), egui::StrokeKind::Inside);
    }
    r
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = theme::semibold(10.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = galley.size() + Vec2::new(14.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(255), color.gamma_multiply(0.14), Stroke::new(1.0, color.gamma_multiply(0.7)), egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = theme::ui_palette(ui);
    let size = Vec2::new(36.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, ""));
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let off_fill = if p.dark { Color32::from_rgb(0x3A, 0x42, 0x50) } else { Color32::from_rgb(0xC9, 0xD1, 0xDB) };
    let fill = lerp_color(off_fill, p.accent, t);
    ui.painter().rect_filled(rect, CornerRadius::same(255), fill);
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 7.5, Color32::WHITE);
    resp
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Horizontal segmented control (e.g. Triangle | Grid).
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> Response {
    let p = theme::ui_palette(ui);
    let mut changed = None;
    let r = egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0, p.field_stroke))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let w = (ui.available_width() - 2.0 * (options.len() as f32 - 1.0)) / options.len() as f32;
            ui.horizontal(|ui| {
                for (v, label) in options {
                    let sel = *value == *v;
                    let text = RichText::new(*label).color(if sel { p.accent_text } else { p.text });
                    let b = egui::Button::new(if sel { text.font(theme::semibold(13.5)) } else { text })
                        .fill(if sel { p.accent } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(5))
                        .min_size(Vec2::new(w, 26.0));
                    let r = ui.add(b);
                    if r.clicked() && !sel {
                        changed = Some(*v);
                    }
                }
            });
        });
    let mut resp = r.response;
    if let Some(v) = changed {
        *value = v;
        resp.mark_changed();
    }
    resp
}

/// Coloured status dot.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// Right-aligned row helper.
pub fn right<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(Layout::right_to_left(Align::Center), add).inner
}

pub fn weak(ui: &Ui, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(theme::ui_palette(ui).text_weak)
}

pub fn palette_color_for_status(p: &Palette, ok: bool, warn: bool) -> Color32 {
    if !ok { p.bad } else if warn { p.warn } else { p.good }
}
