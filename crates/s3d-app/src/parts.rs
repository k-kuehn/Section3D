//! Parts list and the selected piece's properties (cuts, flags, actions).

use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2};
use s3d_core::edits::{EditOp, PieceRef, SeamRef};
use s3d_core::partition::{Flag, Partition, PieceId};

use crate::app::S3dApp;
use crate::theme::{self, Palette};
use crate::view::PendingSeam;
use crate::widgets::{self, card, section_title};

pub fn flag_color(p: &Palette, f: &Flag) -> Color32 {
    match f {
        Flag::TooBig => p.bad,
        Flag::NeedsSupport(_) => p.bad,
        Flag::SkippedDowel => p.info,
        Flag::Tall | Flag::Small | Flag::NoAssemblyPath => p.warn,
    }
}

pub fn piece_color(label: u32, dark: bool) -> [f32; 3] {
    // Golden-ratio hues, soft saturation so support red and selection stand out.
    let h = (label as f32 * 0.618_034).fract();
    let (s, v) = if dark { (0.32, 0.80) } else { (0.30, 0.86) };
    hsv(h, s, v)
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

pub fn swatch(c: [f32; 3]) -> Color32 {
    Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

pub fn list(app: &mut S3dApp, ui: &mut Ui) {
    let Some(split) = app.doc.split.clone() else { return };
    let p = app.palette();
    let part = &split.partition;
    let mut order: Vec<usize> = (0..part.pieces.len()).collect();
    // Flagged pieces first, then by label.
    let rank = |q: &s3d_core::partition::Piece| if q.flags.iter().any(|f| *f != Flag::SkippedDowel) { 0 } else if q.flags.is_empty() { 2 } else { 1 };
    order.sort_by_key(|&i| (rank(&part.pieces[i]), part.pieces[i].label));
    let flagged = part.pieces.iter().filter(|q| q.flags.iter().any(|f| *f != Flag::SkippedDowel)).count();
    let pins = part.pieces.iter().filter(|q| q.flags.contains(&Flag::SkippedDowel)).count();
    card(ui, |ui| {
        ui.horizontal(|ui| {
            section_title(ui, "Parts");
            ui.label(RichText::new(format!("{} pieces", part.pieces.len())).color(p.text_strong));
            if flagged > 0 {
                widgets::badge(ui, &format!("{flagged} NEED ATTENTION"), p.warn);
            }
            if pins > 0 {
                widgets::badge(ui, &format!("PINS SKIPPED ON {pins}"), p.info).on_hover_text("Walls are too thin for the dowel pins on these pieces. See the Connectors step.");
            }
            widgets::right(ui, |ui| {
                if !app.view.hidden.is_empty() || app.view.isolate.is_some() {
                    if ui.small_button("Show all").clicked() {
                        app.view.show_all();
                    }
                }
            });
        });
        ui.add_space(4.0);
        let row_h = 30.0;
        egui::ScrollArea::vertical().id_salt("parts").max_height(340.0).auto_shrink([false, true]).show_rows(ui, row_h, order.len(), |ui, range| {
            for &i in &order[range] {
                let q = &part.pieces[i];
                row(app, ui, part, q.id, row_h, &p);
            }
        });
    });
}

fn row(app: &mut S3dApp, ui: &mut Ui, part: &Partition, id: PieceId, h: f32, p: &Palette) {
    let q = part.piece(id).unwrap();
    let sel = app.view.selected == Some(id);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), egui::Sense::click());
    let resp = resp.on_hover_text(if q.flags.is_empty() { "Fits and prints without supports".to_string() } else { q.flags.iter().map(|f| f.explain()).collect::<Vec<_>>().join("\n") });
    if sel {
        ui.painter().rect(rect, CornerRadius::same(6), p.accent.gamma_multiply(0.12), Stroke::new(1.0, p.accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), p.field);
        app.view.hover = Some(id);
    }
    let mut x = rect.left() + 8.0;
    let cy = rect.center().y;
    // Colour swatch.
    ui.painter().rect_filled(egui::Rect::from_center_size(egui::pos2(x + 5.0, cy), Vec2::splat(10.0)), CornerRadius::same(3), swatch(piece_color(q.label, p.dark)));
    x += 18.0;
    let hidden = app.view.is_hidden(id);
    let tc = if hidden { p.text_weak } else { p.text_strong };
    ui.painter().text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, format!("#{}", q.label), theme::semibold(13.5), tc);
    x += 44.0;
    let size = q.orient.map(|o| o.size).unwrap_or(q.bbox.size());
    ui.painter().text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, format!("{:.0}×{:.0}×{:.0}", size.x, size.y, size.z), egui::FontId::proportional(12.5), p.text_weak);
    // Right side: status badge, lock, eye.
    let mut rx = rect.right() - 8.0;
    let eye = egui::Rect::from_center_size(egui::pos2(rx - 9.0, cy), Vec2::splat(18.0));
    let eye_resp = ui.interact(eye, ui.scope_id().with(("eye", id)), egui::Sense::click()).on_hover_text(if hidden { "Show" } else { "Hide" });
    ui.painter().text(eye.center(), egui::Align2::CENTER_CENTER, if hidden { "◌" } else { "●" }, egui::FontId::proportional(12.0), if hidden { p.text_weak } else { p.text });
    if eye_resp.clicked() {
        app.view.toggle_hidden(id);
    }
    rx -= 26.0;
    if q.locked {
        ui.painter().text(egui::pos2(rx - 8.0, cy), egui::Align2::CENTER_CENTER, "🔒", egui::FontId::proportional(12.0), p.text_weak);
        rx -= 22.0;
    }
    let (txt, col) = match q.flags.first() {
        None => ("OK".to_string(), p.good),
        Some(f) => (if q.flags.len() > 1 { format!("{} +{}", f.short(), q.flags.len() - 1) } else { f.short().to_string() }, flag_color(p, f)),
    };
    let font = theme::semibold(10.0);
    let g = ui.painter().layout_no_wrap(txt, font, col);
    let br = egui::Rect::from_min_size(egui::pos2(rx - g.size().x - 12.0, cy - 9.0), Vec2::new(g.size().x + 12.0, 18.0));
    ui.painter().rect(br, CornerRadius::same(255), col.gamma_multiply(0.14), Stroke::new(1.0, col.gamma_multiply(0.7)), egui::StrokeKind::Inside);
    ui.painter().galley(br.center() - g.size() / 2.0, g, col);
    if resp.clicked() {
        app.view.select(Some(id));
    }
    resp.context_menu(|ui| context_menu(app, ui, id));
}

/// Right-click menu shared by the list and the 3D view.
pub fn context_menu(app: &mut S3dApp, ui: &mut Ui, id: PieceId) {
    app.view.select(Some(id));
    let Some(split) = app.doc.split.clone() else { return };
    if ui.button("Isolate").clicked() {
        app.view.isolate = Some(vec![id]);
        ui.close();
    }
    if ui.button("Isolate with neighbours").clicked() {
        let mut v = split.partition.neighbours(id);
        v.push(id);
        app.view.isolate = Some(v);
        ui.close();
    }
    if ui.button("Hide").clicked() {
        app.view.hidden.insert(id);
        if app.view.selected == Some(id) {
            app.view.select(None);
        }
        ui.close();
    }
    if ui.button("Show all").clicked() {
        app.view.show_all();
        ui.close();
    }
    ui.separator();
    let needs = split.partition.piece(id).is_some_and(|q| q.flags.iter().any(|f| matches!(f, Flag::NeedsSupport(_) | Flag::Tall | Flag::TooBig)));
    ui.add_enabled_ui(needs && !app.jobs.busy(), |ui| {
        ui.menu_button("Fix support", |ui| fix_menu(app, ui, id));
    });
}

fn fix_menu(app: &mut S3dApp, ui: &mut Ui, id: PieceId) {
    if ui.button("Find a better cut automatically").on_hover_text("Tries many cuts through this piece and keeps the best").clicked() {
        crate::view::find_better_cut(app, id);
        ui.close();
    }
    if ui.button("Split this piece in two").clicked() {
        crate::view::split_piece_auto(app, id);
        ui.close();
    }
    if ui.button("Adjust one of its cuts").on_hover_text("Use the handles on the cuts in the 3D view").clicked() {
        app.view.select(Some(id));
        ui.close();
    }
}

/// Properties of the selected piece.
pub fn selected_card(app: &mut S3dApp, ui: &mut Ui) {
    let (Some(id), Some(split)) = (app.view.selected, app.doc.split.clone()) else { return };
    let part = &split.partition;
    let Some(q) = part.piece(id) else {
        app.view.select(None);
        return;
    };
    let p = app.palette();
    let busy = app.jobs.busy();
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.painter().rect_filled(egui::Rect::from_min_size(ui.cursor().min + Vec2::new(0.0, 4.0), Vec2::splat(12.0)), CornerRadius::same(3), swatch(piece_color(q.label, p.dark)));
            ui.add_space(18.0);
            ui.label(RichText::new(format!("Piece #{}", q.label)).font(theme::semibold(16.0)).color(p.text_strong));
            widgets::right(ui, |ui| {
                if ui.small_button("✕").on_hover_text("Deselect").clicked() {
                    app.view.select(None);
                }
                let lock_txt = if q.locked { "🔒 Locked" } else { "🔓 Lock" };
                if ui.add_enabled(!busy, egui::Button::new(lock_txt).small()).on_hover_text("Locked pieces never change, even when you split again").clicked() {
                    if let Some(r) = PieceRef::of(part, id) {
                        app.start_edit(EditOp::LockPiece { piece: r, locked: !q.locked });
                    }
                }
            });
        });
        let size = q.orient.map(|o| o.size).unwrap_or(q.bbox.size());
        let grams = q.volume / 1000.0 * app.doc.settings.export.density as f64;
        egui::Grid::new("pieceinfo").num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
            ui.label(widgets::weak(ui, "Print size"));
            ui.label(format!("{:.0} × {:.0} × {:.0} mm", size.x, size.y, size.z));
            ui.end_row();
            ui.label(widgets::weak(ui, "Volume / weight"));
            ui.label(format!("{:.1} cm³ · {:.0} g", q.volume / 1000.0, grams));
            ui.end_row();
        });
        if q.flags.is_empty() {
            ui.label(RichText::new("✓ Fits the printer and prints without supports.").color(p.good));
        }
        for f in &q.flags {
            ui.horizontal_wrapped(|ui| {
                widgets::badge(ui, f.short(), flag_color(&p, f));
                ui.label(RichText::new(f.explain()).color(p.text).size(12.5));
            });
        }
        ui.add_space(4.0);
        section_title(ui, "Cuts");
        let seams: Vec<_> = part.seams_of(id).cloned().collect();
        if seams.is_empty() {
            ui.label(widgets::weak(ui, "This piece has no cuts."));
        }
        for s in &seams {
            let other = part.piece(s.other(id)).map(|o| o.label).unwrap_or(0);
            let pending = app.view.pending.filter(|pd| pd.seam == s.id);
            let hovered = ui
                .horizontal(|ui| {
                    let mut pd = pending.unwrap_or(PendingSeam { seam: s.id, offset: 0.0, tilt: 0.0 });
                    let lbl = ui.label(RichText::new(format!("↔ #{other}")).color(p.text_strong)).on_hover_text(format!("Cut shared with piece #{other} ({:.0} cm²)", s.area / 100.0));
                    if s.locked {
                        ui.label("🔒");
                    }
                    ui.label(widgets::weak(ui, "move"));
                    let r1 = ui.add(egui::DragValue::new(&mut pd.offset).speed(0.5).suffix(" mm").range(-500.0..=500.0));
                    ui.label(widgets::weak(ui, "tilt"));
                    let r2 = ui.add(egui::DragValue::new(&mut pd.tilt).speed(0.25).suffix("°").range(-60.0..=60.0));
                    if r1.changed() || r2.changed() {
                        app.view.set_pending(pd);
                    }
                    widgets::right(ui, |ui| {
                        if ui.add_enabled(!busy && !s.locked, egui::Button::new("Merge").small()).on_hover_text(format!("Merge with piece #{other}")).clicked() {
                            if let (Some(a), Some(b)) = (PieceRef::of(part, id), PieceRef::of(part, s.other(id))) {
                                app.start_edit(EditOp::Merge { pieces: vec![a, b] });
                            }
                        }
                    });
                    lbl.hovered() || r1.hovered() || r2.hovered()
                })
                .inner;
            if hovered {
                app.view.hover_seam = Some(s.id);
            }
        }
        if let Some(pd) = app.view.pending {
            ui.horizontal(|ui| {
                let preview_ready = app.view.preview.as_ref().is_some_and(|pv| pv.pending == pd);
                if ui.add_enabled(!busy, egui::Button::new(RichText::new("Apply").color(p.accent_text)).fill(p.accent)).clicked() {
                    if let (Some(r), Some(plane)) = (SeamRef::of(part, pd.seam), app.view.pending_plane(part)) {
                        app.start_edit(EditOp::MoveSeam { seam: r, plane });
                        app.view.clear_pending();
                    }
                }
                if ui.button("Cancel").clicked() {
                    app.view.clear_pending();
                }
                if let Some(pv) = app.view.preview.as_ref().filter(|_| preview_ready) {
                    let worst = pv.support;
                    ui.label(RichText::new(if worst < s3d_core::orient::SUPPORT_NOISE { "✓ no support".to_string() } else { format!("support {worst:.0} mm²") }).color(if worst < s3d_core::orient::SUPPORT_NOISE { p.good } else { p.warn }).size(12.5));
                } else {
                    ui.label(widgets::weak(ui, "preview…"));
                }
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy && !q.locked, egui::Button::new("Split in two")).on_hover_text("Adds a cut through this piece").clicked() {
                crate::view::split_piece_auto(app, id);
            }
            let needs = q.flags.iter().any(|f| matches!(f, Flag::NeedsSupport(_) | Flag::Tall | Flag::TooBig));
            ui.add_enabled_ui(needs && !busy && !q.locked, |ui| {
                ui.menu_button("Fix support ▾", |ui| fix_menu(app, ui, id));
            });
        });
    });
}
