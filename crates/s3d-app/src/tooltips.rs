//! Help for every setting: a short explanation and a small vector diagram.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Ui, Vec2, pos2, vec2};

use crate::theme::{self, Palette};

pub struct Help {
    pub title: &'static str,
    pub text: &'static str,
    pub diagram: fn(&Painter, Rect, &Palette),
}

/// Every setting id used in the UI. A test checks each has help.
pub const IDS: &[&str] = &[
    "printer.model",
    "printer.margin",
    "prepare.rotate",
    "prepare.scale",
    "prepare.mirror",
    "prepare.wall",
    "split.mode",
    "split.target_size",
    "split.min_piece",
    "split.overhang",
    "split.side_angle",
    "split.time_limit",
    "conn.dowels",
    "conn.diameter",
    "conn.clearance",
    "conn.length",
    "conn.per_cut",
    "conn.labels",
    "conn.label_size",
    "conn.label_depth",
    "export.folder",
    "export.bed_name",
    "export.part_name",
    "export.single_stls",
    "export.viewer",
    "export.guide",
    "export.spacing",
    "export.keep_order",
    "export.shrink_xy",
    "export.shrink_z",
    "export.density",
];

pub fn get(id: &str) -> Option<Help> {
    let h = |title, text, diagram| Some(Help { title, text, diagram });
    match id {
        "printer.model" => h("Printer", "Pick your printer so every piece fits its build volume. Choose Custom to type your own size.", d_bed),
        "printer.margin" => h("Bed margin", "Space kept free around the edge of the build plate. Pieces are sized to the bed minus this margin.", d_margin),
        "prepare.rotate" => h("Rotate", "Turns the model around the X, Y and Z axes before splitting. Z points up.", d_rotate),
        "prepare.scale" => h("Scale", "Resizes the model uniformly. 100% keeps the original size in millimetres.", d_scale),
        "prepare.mirror" => h("Mirror", "Flips the model across the chosen axis.", d_mirror),
        "prepare.wall" => h("Wall thickness", "Hollow removes the inside of the model and leaves a shell this thick. Saves filament and print time.", d_wall),
        "split.mode" => h("Split mode", "Triangle: big brick-like triangular pieces that print without supports (recommended). Grid: simple boxes the size of the bed.", d_mode),
        "split.target_size" => h("Piece size", "Largest piece length. Leave on Max to use the biggest piece that fits your printer: fewer pieces, fewer seams.", d_target),
        "split.min_piece" => h("Smallest piece", "Pieces thinner than this are merged into a neighbour so you never print tiny slivers.", d_sliver),
        "split.overhang" => h("Overhang limit", "Surfaces that lean further than this from vertical need support. 45° suits most printers.", d_overhang),
        "split.side_angle" => h("Side angle", "Angle of the two side cuts of each triangle. 45° keeps pieces low and support-free.", d_side_angle),
        "split.time_limit" => h("Search time", "How long Triangle mode keeps improving the layout. A first result appears within a couple of seconds.", d_time),
        "conn.dowels" => h("Dowel pins", "Adds matching blind holes on both sides of every cut. Glue dowel pins into them to align and strengthen joints.", d_dowel),
        "conn.diameter" => h("Pin diameter", "Diameter of the dowel pins you will use.", d_diameter),
        "conn.clearance" => h("Clearance", "Extra hole size so pins slide in. Increase if pins are too tight.", d_clearance),
        "conn.length" => h("Pin length", "Total pin length. Each side gets a hole half as deep, plus a little room for glue.", d_length),
        "conn.per_cut" => h("Pins per cut", "How many pins each cut face gets. Holes are spread evenly and kept away from edges.", d_per_cut),
        "conn.labels" => h("Part numbers", "Embosses (raised) or debosses (sunken) each piece's number on an outside face, never on a cut face.", d_label),
        "conn.label_size" => h("Number size", "Height of the part number text.", d_label_size),
        "conn.label_depth" => h("Number depth", "How far the number is raised or sunk.", d_label_depth),
        "export.folder" => h("Destination", "Folder the print files are written to.", d_folder),
        "export.bed_name" => h("Bed file name", "Name for each bed's 3MF. Tokens: {project}, {bed}.", d_folder),
        "export.part_name" => h("Part file name", "Name for single-part STL files. Tokens: {project}, {part}.", d_folder),
        "export.single_stls" => h("Single STLs", "Also write one STL file per piece.", d_folder),
        "export.viewer" => h("3D viewer", "Writes an HTML file that shows the assembled model and every piece in any browser, offline.", d_viewer),
        "export.guide" => h("Assembly guide", "Writes a PDF with a parts checklist and one assembly step per page.", d_guide),
        "export.spacing" => h("Spacing", "Gap between pieces on the same bed.", d_spacing),
        "export.keep_order" => h("Keep assembly order", "Fill beds in assembly order, so the first beds hold the first pieces you need.", d_order),
        "export.shrink_xy" => h("Shrinkage X/Y", "Enlarges exported parts horizontally to make up for material shrinkage.", d_shrink),
        "export.shrink_z" => h("Shrinkage Z", "Enlarges exported parts vertically to make up for material shrinkage.", d_shrink),
        "export.density" => h("Filament density", "Used to estimate each piece's weight. PLA ≈ 1.24 g/cm³, PETG ≈ 1.27.", d_weight),
        _ => None,
    }
}

pub fn show(ui: &mut Ui, id: &str) {
    let Some(help) = get(id) else {
        ui.label(id);
        return;
    };
    let p = theme::ui_palette(ui);
    ui.set_max_width(260.0);
    ui.label(egui::RichText::new(help.title).font(theme::semibold(14.0)).color(p.text_strong));
    ui.label(egui::RichText::new(help.text).color(p.text));
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(240.0, 110.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 6.0, p.field);
    (help.diagram)(ui.painter(), rect.shrink(8.0), &p);
}

// ---------------------------------------------------------------------------
// Diagram helpers. All drawing is relative to the rect: (u, v) in 0..1.

fn at(r: Rect, u: f32, v: f32) -> Pos2 {
    pos2(r.left() + u * r.width(), r.top() + v * r.height())
}

fn line(pt: &Painter, r: Rect, a: (f32, f32), b: (f32, f32), s: Stroke) {
    pt.line_segment([at(r, a.0, a.1), at(r, b.0, b.1)], s);
}

fn poly(pt: &Painter, r: Rect, pts: &[(f32, f32)], fill: Color32, s: Stroke) {
    pt.add(Shape::convex_polygon(pts.iter().map(|&(u, v)| at(r, u, v)).collect(), fill, s));
}

fn text(pt: &Painter, r: Rect, u: f32, v: f32, s: &str, c: Color32) {
    pt.text(at(r, u, v), egui::Align2::CENTER_CENTER, s, egui::FontId::proportional(11.0), c);
}

fn dashed(pt: &Painter, r: Rect, a: (f32, f32), b: (f32, f32), c: Color32) {
    pt.extend(Shape::dashed_line(&[at(r, a.0, a.1), at(r, b.0, b.1)], Stroke::new(1.0, c), 4.0, 3.0));
}

fn arrow(pt: &Painter, r: Rect, a: (f32, f32), b: (f32, f32), c: Color32) {
    let (pa, pb) = (at(r, a.0, a.1), at(r, b.0, b.1));
    pt.arrow(pa, pb - pa, Stroke::new(1.5, c));
}

fn dim(pt: &Painter, r: Rect, a: (f32, f32), b: (f32, f32), label: &str, p: &Palette) {
    let (pa, pb) = (at(r, a.0, a.1), at(r, b.0, b.1));
    let s = Stroke::new(1.0, p.accent);
    pt.line_segment([pa, pb], s);
    let d = (pb - pa).normalized().rot90() * 4.0;
    pt.line_segment([pa - d, pa + d], s);
    pt.line_segment([pb - d, pb + d], s);
    let mid = pa + (pb - pa) * 0.5 + d * 2.5;
    pt.text(mid, egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), p.accent);
}

fn part(p: &Palette) -> (Color32, Stroke) {
    (p.text_weak.gamma_multiply(0.35), Stroke::new(1.0, p.text_weak))
}

fn bed(pt: &Painter, r: Rect, p: &Palette) {
    poly(pt, r, &[(0.05, 0.9), (0.95, 0.9), (0.95, 0.96), (0.05, 0.96)], p.text_weak.gamma_multiply(0.5), Stroke::NONE);
}

fn d_bed(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.2, 0.15), (0.8, 0.15), (0.8, 0.85), (0.2, 0.85)], Color32::TRANSPARENT, Stroke::new(1.0, p.text_weak));
    poly(pt, r, &[(0.3, 0.45), (0.6, 0.45), (0.6, 0.85), (0.3, 0.85)], f, s);
    bed(pt, r, p);
    dim(pt, r, (0.2, 0.08), (0.8, 0.08), "build volume", p);
}

fn d_margin(pt: &Painter, r: Rect, p: &Palette) {
    poly(pt, r, &[(0.1, 0.1), (0.9, 0.1), (0.9, 0.9), (0.1, 0.9)], p.text_weak.gamma_multiply(0.2), Stroke::new(1.0, p.text_weak));
    poly(pt, r, &[(0.2, 0.25), (0.8, 0.25), (0.8, 0.75), (0.2, 0.75)], Color32::TRANSPARENT, Stroke::new(1.0, p.accent));
    dim(pt, r, (0.1, 0.5), (0.2, 0.5), "", p);
    text(pt, r, 0.5, 0.5, "usable area", p.text);
}

fn d_rotate(pt: &Painter, r: Rect, p: &Palette) {
    let c = at(r, 0.5, 0.55);
    pt.circle_stroke(c, 30.0, Stroke::new(1.0, p.text_weak));
    arrow(pt, r, (0.5, 0.55), (0.5, 0.05), p.good);
    text(pt, r, 0.56, 0.08, "Z", p.good);
    pt.arrow(c + vec2(30.0, 0.0), vec2(0.0, -12.0), Stroke::new(1.5, p.accent));
}

fn d_scale(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.15, 0.55), (0.35, 0.55), (0.35, 0.85), (0.15, 0.85)], f, s);
    poly(pt, r, &[(0.5, 0.25), (0.9, 0.25), (0.9, 0.85), (0.5, 0.85)], f, Stroke::new(1.0, p.accent));
    arrow(pt, r, (0.36, 0.6), (0.48, 0.4), p.accent);
}

fn d_mirror(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.8), (0.4, 0.8), (0.4, 0.3)], f, s);
    poly(pt, r, &[(0.6, 0.8), (0.9, 0.8), (0.6, 0.3)], f, Stroke::new(1.0, p.accent));
    dashed(pt, r, (0.5, 0.1), (0.5, 0.95), p.text_weak);
}

fn d_wall(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.15, 0.15), (0.85, 0.15), (0.85, 0.85), (0.15, 0.85)], f, s);
    poly(pt, r, &[(0.27, 0.32), (0.73, 0.32), (0.73, 0.68), (0.27, 0.68)], p.field, s);
    dim(pt, r, (0.15, 0.5), (0.27, 0.5), "", p);
    text(pt, r, 0.5, 0.5, "hollow", p.text_weak);
}

fn d_mode(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    // Left: triangle bricks; right: grid.
    let tri = |a: f32, up: bool| {
        if up {
            vec![(a, 0.85), (a + 0.2, 0.85), (a + 0.1, 0.45)]
        } else {
            vec![(a, 0.45), (a + 0.2, 0.45), (a + 0.1, 0.85)]
        }
    };
    for (i, a) in [0.02, 0.12, 0.22].iter().enumerate() {
        poly(pt, r, &tri(*a, i % 2 == 0), f, Stroke::new(1.0, p.accent));
    }
    text(pt, r, 0.22, 0.2, "Triangle", p.accent);
    for i in 0..3 {
        for j in 0..2 {
            let (u, v) = (0.58 + i as f32 * 0.12, 0.45 + j as f32 * 0.2);
            poly(pt, r, &[(u, v), (u + 0.12, v), (u + 0.12, v + 0.2), (u, v + 0.2)], f, s);
        }
    }
    text(pt, r, 0.76, 0.2, "Grid", p.text);
}

fn d_target(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.8), (0.9, 0.8), (0.5, 0.3)], f, s);
    dim(pt, r, (0.1, 0.92), (0.9, 0.92), "piece size", p);
}

fn d_sliver(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.3), (0.62, 0.3), (0.62, 0.8), (0.1, 0.8)], f, s);
    poly(pt, r, &[(0.62, 0.3), (0.7, 0.3), (0.7, 0.8), (0.62, 0.8)], p.bad.gamma_multiply(0.4), Stroke::new(1.0, p.bad));
    arrow(pt, r, (0.8, 0.55), (0.71, 0.55), p.bad);
    text(pt, r, 0.85, 0.2, "merged", p.text_weak);
}

fn d_overhang(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.2, 0.85), (0.4, 0.85), (0.75, 0.3), (0.55, 0.3)], f, s);
    dashed(pt, r, (0.4, 0.85), (0.4, 0.15), p.text_weak);
    let c = at(r, 0.4, 0.85);
    pt.add(Shape::line(
        (0..=10).map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + (i as f32 / 10.0) * 0.56;
            c + Vec2::angled(a) * 40.0
        }).collect(),
        Stroke::new(1.5, p.accent),
    ));
    text(pt, r, 0.52, 0.18, "≤ limit", p.accent);
    bed(pt, r, p);
}

fn d_side_angle(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.85), (0.9, 0.85), (0.5, 0.25)], f, s);
    text(pt, r, 0.22, 0.75, "45°", p.accent);
    text(pt, r, 0.78, 0.75, "45°", p.accent);
    bed(pt, r, p);
}

fn d_time(pt: &Painter, r: Rect, p: &Palette) {
    let pts: Vec<Pos2> = (0..=20).map(|i| {
        let t = i as f32 / 20.0;
        at(r, 0.1 + t * 0.8, 0.85 - 0.6 * (1.0 - (-4.0 * t).exp()))
    }).collect();
    pt.add(Shape::line(pts, Stroke::new(2.0, p.accent)));
    line(pt, r, (0.1, 0.9), (0.9, 0.9), Stroke::new(1.0, p.text_weak));
    text(pt, r, 0.5, 0.97, "time → better layout", p.text_weak);
}

fn d_dowel(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.05, 0.2), (0.46, 0.2), (0.46, 0.8), (0.05, 0.8)], f, s);
    poly(pt, r, &[(0.54, 0.2), (0.95, 0.2), (0.95, 0.8), (0.54, 0.8)], f, s);
    poly(pt, r, &[(0.3, 0.44), (0.7, 0.44), (0.7, 0.56), (0.3, 0.56)], p.accent.gamma_multiply(0.6), Stroke::new(1.0, p.accent));
    text(pt, r, 0.5, 0.1, "pin", p.accent);
}

fn d_diameter(pt: &Painter, r: Rect, p: &Palette) {
    pt.circle(at(r, 0.5, 0.5), 28.0, p.accent.gamma_multiply(0.3), Stroke::new(1.0, p.accent));
    dim(pt, r, (0.36, 0.5), (0.64, 0.5), "Ø", p);
}

fn d_clearance(pt: &Painter, r: Rect, p: &Palette) {
    pt.circle(at(r, 0.5, 0.5), 34.0, p.field, Stroke::new(1.0, p.text_weak));
    pt.circle(at(r, 0.5, 0.5), 28.0, p.accent.gamma_multiply(0.3), Stroke::new(1.0, p.accent));
    text(pt, r, 0.5, 0.06, "hole = pin + clearance", p.text_weak);
}

fn d_length(pt: &Painter, r: Rect, p: &Palette) {
    poly(pt, r, &[(0.15, 0.42), (0.85, 0.42), (0.85, 0.58), (0.15, 0.58)], p.accent.gamma_multiply(0.4), Stroke::new(1.0, p.accent));
    dim(pt, r, (0.15, 0.8), (0.85, 0.8), "length", p);
    dashed(pt, r, (0.5, 0.2), (0.5, 0.9), p.text_weak);
}

fn d_per_cut(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.2), (0.9, 0.2), (0.9, 0.8), (0.1, 0.8)], f, s);
    for u in [0.33, 0.67] {
        pt.circle(at(r, u, 0.5), 7.0, p.field, Stroke::new(1.0, p.accent));
    }
    text(pt, r, 0.5, 0.1, "cut face", p.text_weak);
}

fn d_label(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.2), (0.9, 0.2), (0.9, 0.8), (0.1, 0.8)], f, s);
    pt.text(at(r, 0.5, 0.5), egui::Align2::CENTER_CENTER, "12", egui::FontId::proportional(30.0), p.accent);
}

fn d_label_size(pt: &Painter, r: Rect, p: &Palette) {
    d_label(pt, r, p);
    dim(pt, r, (0.72, 0.32), (0.72, 0.68), "", p);
}

fn d_label_depth(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.5), (0.9, 0.5), (0.9, 0.85), (0.1, 0.85)], f, s);
    poly(pt, r, &[(0.4, 0.5), (0.6, 0.5), (0.6, 0.62), (0.4, 0.62)], p.field, Stroke::new(1.0, p.accent));
    dim(pt, r, (0.68, 0.5), (0.68, 0.62), "", p);
}

fn d_folder(pt: &Painter, r: Rect, p: &Palette) {
    poly(pt, r, &[(0.2, 0.3), (0.45, 0.3), (0.5, 0.38), (0.8, 0.38), (0.8, 0.8), (0.2, 0.8)], p.accent.gamma_multiply(0.25), Stroke::new(1.0, p.accent));
    text(pt, r, 0.5, 0.6, "bed1.3mf", p.text);
}

fn d_viewer(pt: &Painter, r: Rect, p: &Palette) {
    poly(pt, r, &[(0.1, 0.15), (0.9, 0.15), (0.9, 0.85), (0.1, 0.85)], p.card, Stroke::new(1.0, p.text_weak));
    line(pt, r, (0.1, 0.27), (0.9, 0.27), Stroke::new(1.0, p.text_weak));
    poly(pt, r, &[(0.35, 0.75), (0.65, 0.75), (0.5, 0.4)], p.accent.gamma_multiply(0.4), Stroke::new(1.0, p.accent));
}

fn d_guide(pt: &Painter, r: Rect, p: &Palette) {
    for (i, u) in [0.15, 0.4, 0.65].iter().enumerate() {
        poly(pt, r, &[(*u, 0.15), (u + 0.2, 0.15), (u + 0.2, 0.85), (*u, 0.85)], p.card, Stroke::new(1.0, p.text_weak));
        text(pt, r, u + 0.1, 0.5, &format!("{}", i + 1), p.accent);
    }
}

fn d_spacing(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.3), (0.42, 0.3), (0.42, 0.8), (0.1, 0.8)], f, s);
    poly(pt, r, &[(0.58, 0.3), (0.9, 0.3), (0.9, 0.8), (0.58, 0.8)], f, s);
    dim(pt, r, (0.42, 0.55), (0.58, 0.55), "", p);
}

fn d_order(pt: &Painter, r: Rect, p: &Palette) {
    for (i, u) in [0.1, 0.4, 0.7].iter().enumerate() {
        poly(pt, r, &[(*u, 0.3), (u + 0.22, 0.3), (u + 0.22, 0.75), (*u, 0.75)], p.card, Stroke::new(1.0, p.text_weak));
        text(pt, r, u + 0.11, 0.52, &format!("{}", i + 1), p.accent);
    }
}

fn d_shrink(pt: &Painter, r: Rect, p: &Palette) {
    let (f, _) = part(p);
    poly(pt, r, &[(0.2, 0.25), (0.8, 0.25), (0.8, 0.85), (0.2, 0.85)], Color32::TRANSPARENT, Stroke::new(1.0, p.accent));
    poly(pt, r, &[(0.25, 0.3), (0.75, 0.3), (0.75, 0.85), (0.25, 0.85)], f, Stroke::new(1.0, p.text_weak));
    text(pt, r, 0.5, 0.12, "+ %", p.accent);
}

fn d_weight(pt: &Painter, r: Rect, p: &Palette) {
    poly(pt, r, &[(0.3, 0.3), (0.7, 0.3), (0.85, 0.85), (0.15, 0.85)], p.text_weak.gamma_multiply(0.3), Stroke::new(1.0, p.text_weak));
    text(pt, r, 0.5, 0.62, "g", p.accent);
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_setting_has_help() {
        for id in super::IDS {
            assert!(super::get(id).is_some(), "missing help for {id}");
        }
    }
}
