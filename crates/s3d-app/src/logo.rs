//! The Section3D logo, drawn with egui shapes (no SVG renderer needed).
//! Geometry copied from section3d-logo-*.svg (820 x 240 viewBox).

use eframe::egui;
use egui::{Color32, Pos2, Rect, RichText, Shape, Stroke, Ui, Vec2, pos2, vec2};

use crate::theme;

/// (shade, quad) in SVG order. Shades: 0-2 slate top/left/right, 3-5 orange.
const CUBES: [(u8, [(f32, f32); 4]); 24] = [
    (0, [(140.00, 76.00), (181.57, 100.00), (140.00, 124.00), (98.43, 100.00)]),
    (1, [(98.43, 148.00), (140.00, 172.00), (140.00, 124.00), (98.43, 100.00)]),
    (2, [(181.57, 148.00), (140.00, 172.00), (140.00, 124.00), (181.57, 100.00)]),
    (0, [(98.43, 100.00), (140.00, 124.00), (98.43, 148.00), (56.86, 124.00)]),
    (1, [(56.86, 172.00), (98.43, 196.00), (98.43, 148.00), (56.86, 124.00)]),
    (2, [(140.00, 172.00), (98.43, 196.00), (98.43, 148.00), (140.00, 124.00)]),
    (0, [(181.57, 100.00), (223.14, 124.00), (181.57, 148.00), (140.00, 124.00)]),
    (1, [(140.00, 172.00), (181.57, 196.00), (181.57, 148.00), (140.00, 124.00)]),
    (2, [(223.14, 172.00), (181.57, 196.00), (181.57, 148.00), (223.14, 124.00)]),
    (0, [(140.00, 28.00), (181.57, 52.00), (140.00, 76.00), (98.43, 52.00)]),
    (1, [(98.43, 100.00), (140.00, 124.00), (140.00, 76.00), (98.43, 52.00)]),
    (2, [(181.57, 100.00), (140.00, 124.00), (140.00, 76.00), (181.57, 52.00)]),
    (0, [(140.00, 124.00), (181.57, 148.00), (140.00, 172.00), (98.43, 148.00)]),
    (1, [(98.43, 196.00), (140.00, 220.00), (140.00, 172.00), (98.43, 148.00)]),
    (2, [(181.57, 196.00), (140.00, 220.00), (140.00, 172.00), (181.57, 148.00)]),
    (0, [(98.43, 52.00), (140.00, 76.00), (98.43, 100.00), (56.86, 76.00)]),
    (1, [(56.86, 124.00), (98.43, 148.00), (98.43, 100.00), (56.86, 76.00)]),
    (2, [(140.00, 124.00), (98.43, 148.00), (98.43, 100.00), (140.00, 76.00)]),
    (0, [(181.57, 52.00), (223.14, 76.00), (181.57, 100.00), (140.00, 76.00)]),
    (1, [(140.00, 124.00), (181.57, 148.00), (181.57, 100.00), (140.00, 76.00)]),
    (2, [(223.14, 124.00), (181.57, 148.00), (181.57, 100.00), (223.14, 76.00)]),
    (3, [(146.24, 22.00), (187.80, 46.00), (146.24, 70.00), (104.67, 46.00)]),
    (4, [(104.67, 94.00), (146.24, 118.00), (146.24, 70.00), (104.67, 46.00)]),
    (5, [(187.80, 94.00), (146.24, 118.00), (146.24, 70.00), (187.80, 46.00)]),
];

fn shade_color(shade: u8, dark: bool) -> Color32 {
    match (shade, dark) {
        (0, true) => Color32::from_rgb(0x64, 0x74, 0x8B),
        (1, true) => Color32::from_rgb(0x47, 0x55, 0x69),
        (2, true) => Color32::from_rgb(0x33, 0x41, 0x55),
        (0, false) => Color32::from_rgb(0xCB, 0xD5, 0xE1),
        (1, false) => Color32::from_rgb(0x94, 0xA3, 0xB8),
        (2, false) => Color32::from_rgb(0x64, 0x74, 0x8B),
        (3, _) => Color32::from_rgb(0xFF, 0xB0, 0x88),
        (4, _) => Color32::from_rgb(0xFF, 0x6B, 0x2C),
        _ => Color32::from_rgb(0xD9, 0x48, 0x0F),
    }
}

/// Paints the cube mark so that the SVG box (56..224, 22..220) fills `rect`.
pub fn paint_mark(ui: &Ui, rect: Rect, dark: bool, stroke: Color32) {
    let (x0, y0, w, h) = (56.0, 20.0, 168.0, 202.0);
    let s = (rect.width() / w).min(rect.height() / h);
    let off = rect.center() - vec2(w, h) * s * 0.5;
    let map = |(x, y): (f32, f32)| -> Pos2 { off + vec2((x - x0) * s, (y - y0) * s) };
    for (shade, q) in CUBES.iter() {
        let mut pts: Vec<Pos2> = q.iter().map(|p| map(*p)).collect();
        // egui feathers convex polygons correctly only when clockwise on screen.
        let area: f32 = (0..4).map(|i| pts[i].x * pts[(i + 1) % 4].y - pts[(i + 1) % 4].x * pts[i].y).sum();
        if area < 0.0 {
            pts.reverse();
        }
        ui.painter().add(Shape::convex_polygon(pts, shade_color(*shade, dark), Stroke::new((2.88 * s).max(0.6), stroke)));
    }
}

/// Small mark + "Section3D" for the top bar.
pub fn wordmark(ui: &mut Ui, height: f32, dark: bool) {
    let p = theme::ui_palette(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(height * 0.84, height), egui::Sense::hover());
    paint_mark(ui, rect, dark, p.bg);
    ui.add_space(8.0);
    ui.label(RichText::new("Section").font(theme::bold(height * 0.7)).color(p.text_strong));
    ui.label(RichText::new("3D").font(theme::bold(height * 0.7)).color(theme::ORANGE));
}

/// Full logo with tagline for the start screen.
pub fn full(ui: &mut Ui, width: f32, dark: bool) {
    let p = theme::ui_palette(ui);
    let h = width * 0.29;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), egui::Sense::hover());
    let mark = Rect::from_min_size(rect.min, Vec2::new(h * 0.84, h));
    paint_mark(ui, mark, dark, p.panel);
    let tx = mark.right() + h * 0.18;
    let big = theme::bold(h * 0.42);
    let g1 = ui.painter().layout_no_wrap("Section".into(), big.clone(), p.text_strong);
    let y = rect.top() + h * 0.18;
    let w1 = g1.size().x;
    ui.painter().galley(pos2(tx, y), g1, p.text_strong);
    let g2 = ui.painter().layout_no_wrap("3D".into(), big, theme::ORANGE);
    ui.painter().galley(pos2(tx + w1, y), g2, theme::ORANGE);
    ui.painter().text(pos2(tx + 2.0, rect.top() + h * 0.74), egui::Align2::LEFT_CENTER, "Split. Connect. Print. Assemble.", theme::semibold(h * 0.11), p.text_weak);
}
