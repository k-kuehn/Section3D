//! Help for every setting: a short explanation and a small vector diagram.

use eframe::egui;
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Ui, Vec2, pos2, vec2};

use crate::theme::{self, Palette};

pub struct Help {
    pub title: &'static str,
    pub text: &'static str,
    pub diagram: fn(&Painter, Rect, &Palette),
}

/// Every setting id used in the UI. A test checks each has help.
pub const IDS: &[&str] = &[
    "printer.volume",
    "printer.margin",
    "prepare.rotate",
    "prepare.scale",
    "prepare.mirror",
    "prepare.wall",
    "split.mode",
    "split.target_size",
    "split.min_piece",
    "split.overhang",
    "split.min_thickness",
    "split.min_angle",
    "split.time_limit",
    "split.zones",
    "split.grid",
    "conn.dowels",
    "conn.diameter",
    "conn.clearance",
    "conn.length",
    "conn.per_cut",
    "conn.labels",
    "conn.label_face",
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
        "printer.volume" => h("Printer", "Your printer's build volume (X × Y × Z). Every piece is sized to fit it, less the bed margin. Click to change it.", d_bed),
        "printer.margin" => h("Bed margin", "Space kept free around the edge of the build plate (and below the top of the build height). Pieces are sized to the bed minus this margin.", d_margin),
        "prepare.rotate" => h("Rotate", "Turns the model around the X, Y and Z axes before splitting. Z points up.", d_rotate),
        "prepare.scale" => h("Scale", "Resizes the model uniformly. 100% keeps the original size in millimetres.", d_scale),
        "prepare.mirror" => h("Mirror", "Flips the model across the chosen axis.", d_mirror),
        "prepare.wall" => h("Wall thickness", "Hollow removes the inside of the model and leaves a shell this thick. Saves filament and print time.", d_wall),
        "split.mode" => h("Split mode", "Shatter: a broken-glass pattern; cuts meet the surface square, three pieces meet at each corner, and the pieces are then improved until they print without supports (may use more pieces). Grid: planes square to X, Y and Z, evenly spaced or where you put them; the pieces are checked and flagged, but the cuts are never moved.", d_mode),
        "split.target_size" => h("Piece size", "Largest piece length. Max uses the biggest piece that fits your printer: fewer pieces, fewer seams. In Grid mode it sets the even spacing of the cuts.", d_target),
        "split.min_piece" => h("Smallest piece", "Fragments whose longest side is under about 2.5 times this are merged into a neighbour (Shatter) or flagged SMALL (Grid).", d_sliver),
        "split.overhang" => h("Overhang limit", "Surfaces that lean further than this from vertical need support. 45° suits most printers.", d_overhang),
        "split.min_thickness" => h("Thinnest part", "No piece may have a strip or tapering tip narrower than this. Shatter fixes them, Grid flags them THIN. Measured across the surface, so thin shell walls are fine.", d_thickness),
        "split.min_angle" => h("Sharpest corner", "Corners where a cut meets another cut or an edge of the model, and edges where a cut meets the surface, must stay at least this blunt. Sharper ones are fragile points and feather edges (flagged SHARP).", d_min_angle),
        "split.time_limit" => h("Max split time", "Shatter only: the longest the split keeps improving pieces. It stops earlier as soon as a round finds nothing better. The clock under the 3D view counts up towards it.", d_time),
        "split.zones" => h("No-cut zones", "Paint over details that must not be cut, like a face or lettering. The paint shows on the surface like marker. Shatter keeps every cut out of it; in Grid mode pieces whose cuts run through it are flagged.", d_zones),
        "split.grid" => h("Grid cuts", "How many cuts go along X, Y and Z, and where each one is (mm). Point at a value to highlight its plane on the model. A cell longer than the piece size is shown in orange. Reset spaces them evenly again.", d_mode),
        "conn.dowels" => h("Dowel pins", "Drills matching blind holes on both sides of every cut. Glue dowel pins into them to align and strengthen joints. Pins are optional: you can also push parts together without them.", d_dowel),
        "conn.diameter" => h("Pin diameter", "Diameter of the dowel pins you will use; type any size. A hole is skipped where the wall is too thin for it, and a smaller size that fits is suggested.", d_diameter),
        "conn.clearance" => h("Clearance", "Extra hole size so pins slide in. Increase if pins are too tight.", d_clearance),
        "conn.length" => h("Pin length", "Total pin length. Each side gets a hole half as deep, plus a little room for glue.", d_length),
        "conn.per_cut" => h("Pins per cut", "Most pins each cut face gets. Holes are kept away from edges, so small cut faces get fewer.", d_per_cut),
        "conn.labels" => h("Part numbers", "Embosses (raised) or debosses (sunken) each piece's number on its own surface. A raised number that only fits on the face the piece prints on is sunk there instead; a piece with no room anywhere else gets its number sunk into a cut face. The numbers are drawn on the pieces in the 3D view.", d_label),
        "conn.label_face" => h("Number face", "Convex puts numbers on the outside: facing away from the middle of the model and, of a wall's two sides, the one more open to view. Concave puts them on the inside, hidden once assembled. Where a side has no room the number is turned, then made smaller (down to half size, not under 3 mm), before it goes on the other side. Right-click a piece to move its number.", d_label),
        "conn.label_size" => h("Number size", "Height of the part number text.", d_label_size),
        "conn.label_depth" => h("Number depth", "How far the number is raised or sunk.", d_label_depth),
        "export.folder" => h("Destination", "Folder the print files are written to.", d_folder),
        "export.bed_name" => h("Bed file name", "Name for each bed's 3MF. Tokens: {project}, {bed}.", d_folder),
        "export.part_name" => h("Part file name", "Name for single-part STL files. Tokens: {project}, {part}.", d_folder),
        "export.single_stls" => h("Single STLs", "Also write one STL file per piece, in its print orientation.", d_folder),
        "export.viewer" => h("3D viewer", "Writes one HTML file that shows the assembled model, an exploded view and every assembly step in any modern browser, offline.", d_viewer),
        "export.guide" => h("Assembly guide", "Writes a PDF with a parts checklist and one assembly step per page.", d_guide),
        "export.spacing" => h("Spacing", "Gap between pieces on the same bed.", d_spacing),
        "export.keep_order" => h("Assembly order", "Ordered fills the beds in assembly order, so the first beds hold the pieces you join first. Random packs the pieces in whatever order fills the beds best.", d_order),
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
    // Left: broken-glass cells (three meet at each corner); right: grid.
    let cells: [&[(f32, f32)]; 5] = [
        &[(0.04, 0.4), (0.16, 0.36), (0.21, 0.55), (0.12, 0.66), (0.04, 0.62)],
        &[(0.16, 0.36), (0.32, 0.4), (0.34, 0.56), (0.21, 0.55)],
        &[(0.04, 0.62), (0.12, 0.66), (0.15, 0.86), (0.04, 0.86)],
        &[(0.12, 0.66), (0.21, 0.55), (0.34, 0.56), (0.38, 0.72), (0.27, 0.86), (0.15, 0.86)],
        &[(0.34, 0.56), (0.44, 0.6), (0.44, 0.86), (0.27, 0.86), (0.38, 0.72)],
    ];
    for c in cells {
        poly(pt, r, c, f, Stroke::new(1.0, p.accent));
    }
    text(pt, r, 0.24, 0.2, "Shatter", p.accent);
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

fn d_thickness(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.25), (0.62, 0.25), (0.62, 0.8), (0.1, 0.8)], f, s);
    poly(pt, r, &[(0.7, 0.25), (0.8, 0.25), (0.8, 0.8), (0.7, 0.8)], f, s);
    dim(pt, r, (0.7, 0.9), (0.8, 0.9), "min", p);
}

fn d_min_angle(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.12, 0.8), (0.9, 0.8), (0.9, 0.3)], f, s);
    let c = at(r, 0.12, 0.8);
    pt.add(Shape::line(
        (0..=10).map(|i| {
            let a = -(i as f32 / 10.0) * 0.57;
            c + Vec2::angled(a) * 46.0
        }).collect(),
        Stroke::new(1.5, p.accent),
    ));
    text(pt, r, 0.5, 0.88, "≥ min angle", p.accent);
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

fn d_zones(pt: &Painter, r: Rect, p: &Palette) {
    let (f, s) = part(p);
    poly(pt, r, &[(0.1, 0.2), (0.9, 0.2), (0.9, 0.85), (0.1, 0.85)], f, s);
    pt.circle(at(r, 0.42, 0.52), 18.0, Color32::from_rgb(64, 140, 255).gamma_multiply(0.45), Stroke::new(1.5, Color32::from_rgb(64, 140, 255)));
    pt.add(Shape::line(vec![at(r, 0.62, 0.12), at(r, 0.66, 0.4), at(r, 0.75, 0.6), at(r, 0.72, 0.95)], Stroke::new(1.5, p.accent)));
    text(pt, r, 0.42, 0.52, "keep", p.text_strong);
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
