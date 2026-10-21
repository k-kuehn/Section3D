//! Parts list and the selected piece's properties (cuts, flags, actions).

use eframe::egui;
use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2};
use s3d_core::edits::EditOp;
use s3d_core::partition::{Flag, Partition, PieceId};

use crate::app::S3dApp;
use crate::theme::{self, Palette};
use crate::widgets::{self, card, section_title};

pub fn flag_color(p: &Palette, f: &Flag) -> Color32 {
    match f {
        Flag::TooBig | Flag::InZone => p.bad,
        Flag::NeedsSupport(_) => p.bad,
        Flag::SkippedDowel => p.info,
        Flag::Tall | Flag::Small | Flag::NoAssemblyPath | Flag::Sharp | Flag::Thin | Flag::Aligned => p.warn,
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

/// How badly a flag needs attention (lower is worse); pin notes come last.
fn severity(f: &Flag) -> u8 {
    match f {
        Flag::TooBig => 0,
        Flag::InZone => 1,
        Flag::NeedsSupport(_) => 2,
        Flag::Tall => 3,
        Flag::Sharp => 4,
        Flag::Thin => 5,
        Flag::Small => 6,
        Flag::Aligned => 7,
        Flag::NoAssemblyPath => 8,
        Flag::SkippedDowel => 9,
    }
}

/// True when a piece has a problem beyond skipped pins.
pub fn needs_attention(q: &s3d_core::partition::Piece) -> bool {
    q.flags.iter().any(|f| *f != Flag::SkippedDowel)
}

const ROW_H: f32 = 30.0;
/// Height of the parts card with ten rows showing (heading, a note line,
/// rows and margins).
pub const LIST_MIN: f32 = ROW_H * 10.0 + 140.0;

pub fn list(app: &mut S3dApp, ui: &mut Ui) {
    let Some(split) = app.doc.split.clone() else { return };
    let p = app.palette();
    let part = &split.partition;
    let flagged = part.pieces.iter().filter(|q| needs_attention(q)).count();
    if flagged == 0 {
        app.view.attention = false;
    }
    let mut order: Vec<usize> = (0..part.pieces.len()).filter(|&i| !app.view.attention || needs_attention(&part.pieces[i])).collect();
    // Worst problems first, then by number.
    let worst = |q: &s3d_core::partition::Piece| q.flags.iter().map(severity).min().unwrap_or(10);
    order.sort_by_key(|&i| (worst(&part.pieces[i]), part.pieces[i].label));
    let pins = part.pieces.iter().filter(|q| q.flags.contains(&Flag::SkippedDowel)).count();
    card(ui, |ui| {
        ui.horizontal(|ui| {
            section_title(ui, "Parts");
            ui.label(RichText::new(format!("{} pieces", part.pieces.len())).color(p.text_strong));
            if flagged > 0 {
                let r = widgets::badge_button(ui, &format!("{flagged} NEED ATTENTION"), p.warn, app.view.attention);
                if r.on_hover_text(if app.view.attention { "Show all pieces" } else { "List only these pieces, worst first" }).clicked() {
                    app.view.attention = !app.view.attention;
                }
            }
            if pins > 0 {
                widgets::badge(ui, &format!("PINS SKIPPED ON {pins}"), p.info).on_hover_text("Walls are too thin for the dowel pins on these pieces. See the Features step.");
            }
            widgets::right(ui, |ui| {
                if !app.view.hidden.is_empty() || app.view.isolate.is_some() {
                    if ui.small_button("Show all").clicked() {
                        app.view.show_all();
                    }
                }
            });
        });
        if app.view.attention {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Showing the {flagged} pieces that need attention, worst first.")).color(p.text_weak).size(12.5));
                if ui.link("Show all").clicked() {
                    app.view.attention = false;
                }
            });
        }
        ui.add_space(4.0);
        // Fill the rest of the panel (sized for at least ten rows).
        let room = (ui.available_height() - 4.0).max(ROW_H * 10.0);
        egui::ScrollArea::vertical().id_salt("parts").max_height(room).auto_shrink([false, false]).show_rows(ui, ROW_H, order.len(), |ui, range| {
            for &i in &order[range] {
                let q = &part.pieces[i];
                row(app, ui, part, q.id, ROW_H, &p);
            }
        });
    });
}

fn row(app: &mut S3dApp, ui: &mut Ui, part: &Partition, id: PieceId, h: f32, p: &Palette) {
    let q = part.piece(id).unwrap();
    let sel = app.view.is_selected(id);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Part #{}", q.label)));
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
        if ui.input(|i| i.modifiers.shift || i.modifiers.command) {
            app.view.toggle_select(id);
        } else {
            app.view.select(Some(id));
        }
    }
    if resp.secondary_clicked() && !app.view.is_selected(id) {
        app.view.select(Some(id));
    }
    resp.context_menu(|ui| context_menu(app, ui));
}

/// True when the pieces touch each other as one connected group.
pub fn connected(part: &Partition, ids: &[PieceId]) -> bool {
    let Some(&first) = ids.first() else { return false };
    let mut seen = vec![first];
    let mut stack = vec![first];
    while let Some(id) = stack.pop() {
        for n in part.neighbours(id) {
            if ids.contains(&n) && !seen.contains(&n) {
                seen.push(n);
                stack.push(n);
            }
        }
    }
    seen.len() == ids.len()
}

/// Merges the selected pieces into one (they must touch and be unlocked).
fn merge_selection(app: &mut S3dApp) {
    let sel = app.view.selection();
    if sel.len() >= 2 {
        app.start_edit(EditOp::Merge(sel));
        app.view.select(None);
    }
}

/// Why the selection cannot be merged, if it cannot.
fn merge_blocker(part: &Partition, ids: &[PieceId]) -> Option<&'static str> {
    if ids.len() < 2 {
        Some("Shift-click more pieces to merge them")
    } else if ids.iter().any(|id| part.piece(*id).is_some_and(|q| q.locked)) {
        Some("A selected piece is locked")
    } else if !connected(part, ids) {
        Some("The selected pieces do not all touch")
    } else {
        None
    }
}

/// Right-click menu shared by the list and the 3D view. With several pieces
/// selected it offers to merge them; with one, to split it.
pub fn context_menu(app: &mut S3dApp, ui: &mut Ui) {
    let Some(split) = app.doc.split.clone() else { return };
    let sel = app.view.selection();
    let Some(&id) = sel.first() else { return };
    let busy = app.jobs.busy();
    if sel.len() > 1 {
        let blocker = merge_blocker(&split.partition, &sel);
        let r = ui.add_enabled(blocker.is_none() && !busy, egui::Button::new(format!("Merge {} pieces", sel.len())));
        let r = match blocker {
            Some(b) => r.on_disabled_hover_text(b),
            None => r,
        };
        if r.clicked() {
            merge_selection(app);
            ui.close();
        }
        ui.separator();
        if ui.button("Isolate selection").clicked() {
            app.view.isolate = Some(sel.clone());
            ui.close();
        }
        if ui.button("Hide selection").clicked() {
            app.view.hidden.extend(sel.iter().copied());
            app.view.select(None);
            ui.close();
        }
        if ui.button("Show all").clicked() {
            app.view.show_all();
            ui.close();
        }
        if ui.button("Clear selection").clicked() {
            app.view.select(None);
            ui.close();
        }
        return;
    }
    let locked = split.partition.piece(id).is_some_and(|q| q.locked);
    ui.add_enabled_ui(!busy && !locked, |ui| {
        ui.menu_button("Split", |ui| split_menu(app, ui, id));
    });
    let has_number = split.partition.piece(id).is_some_and(|q| q.label_place.is_some());
    if ui.add_enabled(!busy && has_number, egui::Button::new("Move number to the other side")).on_hover_text("Puts this piece's part number on the opposite face (convex or concave)").clicked() {
        app.start_flip_label(id);
        ui.close();
    }
    ui.separator();
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
        app.view.select(None);
        ui.close();
    }
    if ui.button("Show all").clicked() {
        app.view.show_all();
        ui.close();
    }
}

const BETTER_CUT: &str = "Moves a cut it shares with a neighbour a little, then tries other angles, then splits it in two or three; keeps the first change that makes the pieces print better";

fn split_menu(app: &mut S3dApp, ui: &mut Ui, id: PieceId) {
    if ui.button("Draw a cut line…").on_hover_text("Drag across the piece where it should be cut; the line is straightened and the cut angled for printing").clicked() {
        app.view.tab = crate::view::Tab::Model;
        app.view.start_cut_draw(id);
        ui.close();
    }
    if ui.button("Split in two automatically").on_hover_text("Finds the cut that prints best").clicked() {
        crate::view::split_piece_auto(app, id);
        ui.close();
    }
    if ui.button("Find a better cut automatically").on_hover_text(BETTER_CUT).clicked() {
        crate::view::find_better_cut(app, id);
        ui.close();
    }
}

/// Properties of the selected piece, or a summary of a multi-selection.
pub fn selected_card(app: &mut S3dApp, ui: &mut Ui) {
    let (Some(id), Some(split)) = (app.view.selected, app.doc.split.clone()) else { return };
    let part = &split.partition;
    let Some(q) = part.piece(id) else {
        app.view.select(None);
        return;
    };
    let p = app.palette();
    let busy = app.jobs.busy();
    if !app.view.multi.is_empty() {
        let sel = app.view.selection();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} pieces selected", sel.len())).font(theme::semibold(16.0)).color(p.text_strong));
                widgets::right(ui, |ui| {
                    if ui.small_button("✕").on_hover_text("Clear selection").clicked() {
                        app.view.select(None);
                    }
                });
            });
            let mut labels: Vec<u32> = sel.iter().filter_map(|i| part.piece(*i)).map(|q| q.label).collect();
            labels.sort_unstable();
            ui.label(RichText::new(labels.iter().map(|l| format!("#{l}")).collect::<Vec<_>>().join("  ")).color(p.text));
            let blocker = merge_blocker(part, &sel);
            ui.horizontal(|ui| {
                if ui.add_enabled(blocker.is_none() && !busy, egui::Button::new(RichText::new(format!("Merge {} pieces", sel.len())).color(p.accent_text)).fill(p.accent)).clicked() {
                    merge_selection(app);
                }
                if let Some(b) = blocker {
                    ui.label(RichText::new(b).color(p.warn).size(12.5));
                }
            });
            ui.label(widgets::weak(ui, "Shift-click pieces to add or remove them. Right-click for more."));
        });
        return;
    }
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
                    app.start_edit(EditOp::Lock(id, !q.locked));
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
            ui.label(widgets::weak(ui, "Touches"));
            let mut nb: Vec<u32> = part.neighbours(id).iter().filter_map(|n| part.piece(*n)).map(|o| o.label).collect();
            nb.sort_unstable();
            ui.label(if nb.is_empty() { "no other piece".to_string() } else { nb.iter().map(|l| format!("#{l}")).collect::<Vec<_>>().join(" ") });
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
        let drawing = app.view.cut_draw.as_ref().is_some_and(|c| c.piece == id);
        ui.horizontal_wrapped(|ui| {
            if drawing {
                if ui.button("Cancel cut line").clicked() {
                    app.view.cut_draw = None;
                }
            } else if ui.add_enabled(!busy && !q.locked, egui::Button::new("Draw cut line")).on_hover_text("Drag across the piece in the 3D view where it should be cut").clicked() {
                app.view.tab = crate::view::Tab::Model;
                app.view.start_cut_draw(id);
            }
            if ui.add_enabled(!busy && !q.locked, egui::Button::new("Split in two")).on_hover_text("Adds the cut that prints best").clicked() {
                crate::view::split_piece_auto(app, id);
            }
            if ui.add_enabled(!busy && !q.locked, egui::Button::new("Find a better cut")).on_hover_text(BETTER_CUT).clicked() {
                crate::view::find_better_cut(app, id);
            }
            if ui.add_enabled(!busy && q.label_place.is_some(), egui::Button::new("Number: other side")).on_hover_text("Moves the part number to the opposite face").clicked() {
                app.start_flip_label(id);
            }
        });
        ui.label(widgets::weak(ui, "Shift-click other pieces to select them for merging."));
    });
}