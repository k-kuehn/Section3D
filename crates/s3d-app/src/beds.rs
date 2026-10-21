//! Beds tab: the pieces as they print, in 3D or as a plan to rearrange.

use std::sync::Arc;

use eframe::egui;
use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2, pos2};
use glam::{Mat4, Vec3};
use s3d_core::mesh::{Aabb, Mesh};
use s3d_core::pack::Layout;
use s3d_core::partition::PieceId;

use crate::app::S3dApp;
use crate::parts::{self, piece_color};
use crate::theme::{self, Palette};
use crate::view::{ViewState, arrange_key, projection_switch, rgba};
use crate::viewport::renderer::{FLAG_SELECTED, FLAG_SUPPORT};
use crate::viewport::{DrawItem, LineSeg, MeshRef, SceneFrame, TriVert, ViewPreset};
use crate::widgets;

fn footprint(view: &mut ViewState, key: u64, mesh: &Mesh, print: Mat4) -> Arc<Vec<glam::Vec2>> {
    view.footprints
        .entry(key)
        .or_insert_with(|| {
            let pts: Vec<glam::DVec2> = mesh.verts.iter().map(|v| {
                let q = print.transform_point3(*v);
                glam::DVec2::new(q.x as f64, q.y as f64)
            }).collect();
            Arc::new(s3d_core::orient::hull2d(pts).into_iter().map(|p| p.as_vec2()).collect())
        })
        .clone()
}

pub fn beds_view(app: &mut S3dApp, ui: &mut Ui, p: &Palette) {
    let Some(split) = app.doc.split.clone() else { return };
    if app.view.beds.beds.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.4);
            ui.label(widgets::weak(ui, "Pieces are not arranged on beds yet."));
            ui.add_space(6.0);
            if ui.add_enabled(!app.jobs.busy(), egui::Button::new("Arrange beds")).clicked() {
                app.start_arrange();
            }
        });
        return;
    }
    let stale = app.view.beds_key != Some(arrange_key(&app.doc));
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(format!("{} beds", app.view.beds.beds.len())).font(theme::semibold(14.0)).color(p.text_strong));
        ui.spacing_mut().item_spacing.x = 4.0;
        for (v, label) in [(true, "3D"), (false, "Plan")] {
            if ui.add(egui::Button::new(label).selected(app.view.beds_3d == v)).clicked() {
                app.view.beds_3d = v;
            }
        }
        if app.view.beds_3d {
            ui.label(widgets::weak(ui, "· pieces as they print · switch to Plan to move pieces"));
            if ui.small_button("Fit").clicked() {
                app.view.beds_viewport.pending_fit = true;
            }
        } else {
            ui.label(widgets::weak(ui, "· drag pieces to move them, right-click to rotate 90°"));
        }
        if stale {
            ui.label(RichText::new("· out of date").color(p.warn));
            if ui.add_enabled(!app.jobs.busy(), egui::Button::new("Arrange again").small()).clicked() {
                app.start_arrange();
            }
        }
        if app.view.beds_3d {
            widgets::right(ui, |ui| {
                ui.add_space(10.0);
                projection_switch(app, ui);
            });
        }
    });
    if app.view.beds_3d {
        beds_3d(app, ui, p, &split);
        return;
    }
    let bed = app.view.beds.bed;
    let avail = ui.available_size();
    let cols = ((avail.x / 300.0).floor() as usize).clamp(1, 6);
    let cell_w = (avail.x - 24.0) / cols as f32 - 12.0;
    let scale = cell_w / bed[0].max(bed[1]);
    let card_h = bed[1] * scale + 34.0;
    let n = app.view.beds.beds.len();
    let mut drop: Option<(PieceId, usize, glam::Vec2)> = None;
    let mut rotate: Option<PieceId> = None;
    let pointer = ui.ctx().pointer_interact_pos();
    let released = ui.input(|i| i.pointer.any_released());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(6.0);
        let mut bed_rects = Vec::new();
        for row in 0..n.div_ceil(cols) {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                for c in 0..cols {
                    let b = row * cols + c;
                    if b >= n {
                        break;
                    }
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(cell_w, card_h), egui::Sense::hover());
                    let painter = ui.painter_at(rect.expand(2.0));
                    painter.text(rect.left_top() + Vec2::new(2.0, 2.0), egui::Align2::LEFT_TOP, format!("Bed {}", b + 1), theme::semibold(13.0), p.text_strong);
                    painter.text(rect.right_top() + Vec2::new(-2.0, 2.0), egui::Align2::RIGHT_TOP, format!("{} pieces", app.view.beds.beds[b].len()), egui::FontId::proportional(12.0), p.text_weak);
                    let plate = egui::Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 24.0), Vec2::new(bed[0] * scale, bed[1] * scale));
                    painter.rect(plate, CornerRadius::same(4), p.field, Stroke::new(1.0, p.field_stroke), egui::StrokeKind::Inside);
                    bed_rects.push((b, plate));
                    let to_screen = |x: f32, y: f32| pos2(plate.left() + x * scale, plate.bottom() - y * scale);
                    for pl in app.view.beds.beds[b].clone() {
                        let Some(q) = split.partition.piece(pl.piece) else { continue };
                        let print = q.orient.map(|o| o.matrix()).unwrap_or(Mat4::IDENTITY);
                        let fp = footprint(&mut app.view, split.mesh_id(q.rev), q.display_mesh(), print);
                        let (s, co) = pl.angle.sin_cos();
                        let dragging = app.view.bed_drag.is_some_and(|d| d.0 == pl.piece);
                        let shift = if dragging { app.view.bed_drag.unwrap().1 } else { Vec2::ZERO };
                        let pts: Vec<egui::Pos2> = fp.iter().map(|v| to_screen(pl.x + co * v.x - s * v.y, pl.y + s * v.x + co * v.y) + shift).collect();
                        let col = parts::swatch(piece_color(q.label, p.dark));
                        let sel = app.view.selected == Some(q.id);
                        painter.add(egui::Shape::convex_polygon(pts.clone(), col.gamma_multiply(if dragging { 0.6 } else { 0.85 }), Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { p.accent } else { col.gamma_multiply(0.5) })));
                        let c = to_screen(pl.x, pl.y) + shift;
                        painter.text(c, egui::Align2::CENTER_CENTER, format!("{}", q.label), theme::semibold(11.0), Color32::from_rgb(0x15, 0x18, 0x1F));
                        // Interaction on the footprint's bounding box.
                        let bbox = egui::Rect::from_points(&pts);
                        let id = ui.scope_id().with(("bedpiece", pl.piece));
                        let r = ui.interact(bbox, id, egui::Sense::click_and_drag());
                        if r.drag_started() {
                            app.view.bed_drag = Some((pl.piece, Vec2::ZERO));
                            app.view.select(Some(pl.piece));
                        }
                        if r.dragged() {
                            if let Some(d) = &mut app.view.bed_drag {
                                d.1 += r.drag_delta();
                            }
                        }
                        if r.clicked() {
                            app.view.select(Some(pl.piece));
                        }
                        if r.secondary_clicked() {
                            rotate = Some(pl.piece);
                        }
                        r.on_hover_text(format!("Piece #{}", q.label));
                    }
                    ui.add_space(12.0);
                }
            });
            ui.add_space(10.0);
        }
        if released {
            if let (Some((id, _)), Some(pos)) = (app.view.bed_drag, pointer) {
                if let Some((b, plate)) = bed_rects.iter().find(|(_, r)| r.contains(pos)) {
                    drop = Some((id, *b, glam::Vec2::new((pos.x - plate.left()) / scale, (plate.bottom() - pos.y) / scale)));
                }
            }
            app.view.bed_drag = None;
        }
    });
    if let Some((id, b, at)) = drop {
        move_piece(&mut app.view.beds, id, b, at);
        app.doc.dirty = true;
    }
    if let Some(id) = rotate {
        for bedv in &mut app.view.beds.beds {
            for pl in bedv.iter_mut().filter(|pl| pl.piece == id) {
                pl.angle = (pl.angle + std::f32::consts::FRAC_PI_2) % std::f32::consts::TAU;
            }
        }
        app.doc.dirty = true;
    }
}

/// Origin of bed `b` when all beds are laid out in a grid in 3D.
fn bed_origin(b: usize, n: usize, bed: [f32; 2]) -> Vec3 {
    let cols = (n as f32).sqrt().ceil().max(1.0) as usize;
    let gap = bed[0].max(bed[1]) * 0.18;
    Vec3::new((b % cols) as f32 * (bed[0] + gap), -((b / cols) as f32) * (bed[1] + gap), 0.0)
}

/// All beds as build plates in 3D, each piece in its print pose.
fn beds_3d(app: &mut S3dApp, ui: &mut Ui, p: &Palette, split: &crate::doc::SplitState) {
    let mut frame = SceneFrame {
        overhang_sin: app.doc.settings.split.overhang.to_radians().sin(),
        cap_color: rgba(p.accent, 1.0),
        support_color: [0.95, 0.18, 0.15, 1.0],
        bg: p.viewport_bg,
        ..Default::default()
    };
    let layout = app.view.beds.clone();
    let bed = layout.bed;
    let n = layout.beds.len();
    let mut bb = Aabb::EMPTY;
    let plate = if p.dark { [0.20, 0.23, 0.28, 1.0] } else { [0.86, 0.88, 0.91, 1.0] };
    let grid = if p.dark { [1.0, 1.0, 1.0, 0.07] } else { [0.0, 0.0, 0.0, 0.08] };
    let edge = rgba(p.accent, 0.8);
    let mut labels: Vec<(Vec3, String, bool)> = Vec::new();
    for (b, list) in layout.beds.iter().enumerate() {
        let o = bed_origin(b, n, bed);
        let c = |x: f32, y: f32| o + Vec3::new(x, y, -0.3);
        let q = [c(0.0, 0.0), c(bed[0], 0.0), c(bed[0], bed[1]), c(0.0, bed[1])];
        frame.tris.extend([0usize, 1, 2, 0, 2, 3].iter().map(|&i| TriVert { pos: q[i].into(), color: plate }));
        for i in 0..4 {
            frame.lines.push(LineSeg::new(q[i], q[(i + 1) % 4], edge, 1.5));
        }
        let step = 50.0;
        let mut x = step;
        while x < bed[0] {
            frame.lines.push(LineSeg::new(c(x, 0.0) + Vec3::Z * 0.1, c(x, bed[1]) + Vec3::Z * 0.1, grid, 1.0));
            x += step;
        }
        let mut y = step;
        while y < bed[1] {
            frame.lines.push(LineSeg::new(c(0.0, y) + Vec3::Z * 0.1, c(bed[0], y) + Vec3::Z * 0.1, grid, 1.0));
            y += step;
        }
        bb.grow(q[0]);
        bb.grow(q[2]);
        labels.push((c(0.0, bed[1]), format!("Bed {} · {} pieces", b + 1, list.len()), true));
        for pl in list {
            let Some(q) = split.partition.piece(pl.piece) else { continue };
            let print = q.orient.map(|o| o.matrix()).unwrap_or(Mat4::IDENTITY);
            let model = Mat4::from_translation(o + Vec3::new(pl.x, pl.y, 0.0)) * Mat4::from_rotation_z(pl.angle) * print;
            let id = split.mesh_id(q.rev);
            frame.meshes.push(MeshRef { id, rev: q.rev, mesh: q.display_mesh().clone() });
            let color = piece_color(q.label, p.dark);
            let sel = app.view.selected == Some(q.id);
            frame.items.push(DrawItem {
                mesh: id,
                model,
                print,
                color: [color[0], color[1], color[2], 1.0],
                highlight: if sel { rgba(p.accent, 1.0) } else { [0.0; 4] },
                flags: FLAG_SUPPORT | if sel { FLAG_SELECTED } else { 0 },
                planes: Vec::new(),
            });
            if sel {
                app.view.outline_edges(id, q.display_mesh(), model, rgba(p.accent, 1.0), 2.0, &mut frame.lines);
            }
            let pb = q.display_mesh().transformed_bbox(&model);
            bb = bb.union(&pb);
            labels.push((Vec3::new(pb.center().x, pb.center().y, pb.max.z), format!("#{}", q.label), false));
        }
    }
    if bb.is_empty() {
        return;
    }
    let vp = &mut app.view.beds_viewport;
    if vp.pending_fit {
        let aspect = vp.aspect();
        vp.camera.preset(ViewPreset::Iso);
        vp.camera.fit(&bb, aspect);
        vp.pending_fit = false;
    }
    let input = vp.interact(ui, false, &|_| None);
    let rect = input.rect;
    vp.paint(ui, rect, frame);
    // Bed names once a plate is big enough on screen, piece numbers once
    // zoomed in further.
    let painter = ui.painter_at(rect);
    let vp = &app.view.beds_viewport;
    let plate_px = match (vp.to_screen(rect, bed_origin(0, n, bed)), vp.to_screen(rect, bed_origin(0, n, bed) + Vec3::new(bed[0], 0.0, 0.0))) {
        (Some(a), Some(b)) => a.distance(b),
        _ => 0.0,
    };
    for (at, text, is_bed) in &labels {
        if plate_px < if *is_bed { 110.0 } else { 260.0 } {
            continue;
        }
        if let Some(sp) = app.view.beds_viewport.to_screen(rect, *at) {
            if !rect.contains(sp) {
                continue;
            }
            if *is_bed {
                painter.text(sp + Vec2::new(0.0, -4.0), egui::Align2::LEFT_BOTTOM, text, theme::semibold(13.0), p.text_strong);
            } else {
                let g = painter.layout_no_wrap(text.clone(), theme::semibold(11.0), p.text_strong);
                let r = egui::Rect::from_center_size(sp, g.size() + Vec2::new(8.0, 2.0));
                painter.rect(r, CornerRadius::same(3), p.card.gamma_multiply(0.8), Stroke::NONE, egui::StrokeKind::Inside);
                painter.galley(r.min + Vec2::new(4.0, 1.0), g, p.text_strong);
            }
        }
    }
    painter.text(rect.left_bottom() + Vec2::new(10.0, -8.0), egui::Align2::LEFT_BOTTOM, "drag to orbit · right-drag to pan · scroll to zoom", egui::FontId::monospace(11.0), p.text_weak);
}

fn move_piece(l: &mut Layout, id: PieceId, bed: usize, at: glam::Vec2) {
    let mut pl = None;
    for b in &mut l.beds {
        if let Some(i) = b.iter().position(|p| p.piece == id) {
            pl = Some(b.remove(i));
        }
    }
    if let Some(mut pl) = pl {
        pl.x = at.x;
        pl.y = at.y;
        l.beds[bed].push(pl);
    }
    l.beds.retain(|b| !b.is_empty());
}
