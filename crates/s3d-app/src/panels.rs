//! Left panel: step tabs, each step's settings card and action button, the
//! parts list and the selected piece's properties.

use eframe::egui;
use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2};
use s3d_core::settings::{LabelFace, LabelMode, SplitMode};

use crate::app::S3dApp;
use crate::doc::{Status, Step};
use crate::theme::{self, Palette};
use crate::widgets::{self, card, num, num_u32, primary_button, section_title, setting, setting_grid, toggle};
use crate::{parts, view};

pub fn show(app: &mut S3dApp, ui: &mut Ui) {
    step_tabs(app, ui);
    ui.add_space(10.0);
    let pinned = matches!(app.step, Step::Split | Step::Features | Step::Export);
    let parts = app.doc.split.is_some() && pinned;
    // The parts list keeps the bottom of the panel, at least ten rows, with
    // the step's button above it; the selected piece's card and the step's
    // settings scroll above those.
    let below = if parts { parts::LIST_MIN } else { 0.0 } + if pinned { 56.0 } else { 0.0 };
    egui::ScrollArea::vertical().auto_shrink([false, parts]).max_height((ui.available_height() - below).max(80.0)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        if parts {
            // A newly picked piece's card comes into view.
            let shown = egui::Id::unique("selected_card_for");
            if ui.ctx().data(|d| d.get_temp::<Option<u32>>(shown)) != Some(app.view.selected) {
                ui.ctx().data_mut(|d| d.insert_temp(shown, app.view.selected));
                ui.scroll_to_cursor(Some(egui::Align::TOP));
            }
            parts::selected_card(app, ui);
        }
        match app.step {
            Step::Import => import(app, ui),
            Step::Prepare => prepare(app, ui),
            Step::Split => split(app, ui),
            Step::Features => features(app, ui),
            Step::Export => export(app, ui),
        }
    });
    if pinned {
        ui.add_space(10.0);
        action(app, ui);
    }
    if parts {
        ui.add_space(10.0);
        parts::list(app, ui);
    }
}

fn status_color(p: &Palette, s: Status) -> Color32 {
    match s {
        Status::Done => p.good,
        Status::Stale => p.warn,
        Status::NotRun => p.text_weak.gamma_multiply(0.5),
    }
}

fn step_tabs(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    egui::Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.card_stroke)).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let n = Step::ALL.len() as f32;
            let w = (ui.available_width() - 2.0 * (n - 1.0)) / n;
            for (i, step) in Step::ALL.iter().enumerate() {
                let enabled = *step == Step::Import || app.doc.source.is_some();
                let sel = app.step == *step;
                let st = app.status(*step);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 46.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
                let resp = resp.on_hover_text(match st {
                    Status::Done => format!("{} — done", step.label()),
                    Status::Stale => format!("{} — out of date", step.label()),
                    Status::NotRun => format!("{} — not run yet", step.label()),
                });
                if sel {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), if p.dark { Color32::from_rgb(0x25, 0x2C, 0x37) } else { Color32::from_rgb(0xF1, 0xF4, 0xF8) });
                    ui.painter().hline(rect.x_range().shrink(10.0), rect.bottom() - 1.5, Stroke::new(2.5, p.accent));
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), p.field);
                }
                let col = if !enabled { p.text_weak.gamma_multiply(0.5) } else if sel { p.text_strong } else { p.text_weak };
                ui.painter().text(rect.center_top() + egui::vec2(0.0, 15.0), egui::Align2::CENTER_CENTER, format!("{}", i + 1), theme::semibold(11.0), col.gamma_multiply(0.8));
                ui.painter().text(rect.center_top() + egui::vec2(0.0, 31.0), egui::Align2::CENTER_CENTER, step.label().to_uppercase(), theme::semibold(10.5), col);
                ui.painter().circle_filled(rect.right_top() + egui::vec2(-9.0, 10.0), 3.5, status_color(&p, st));
                if resp.clicked() && enabled {
                    app.step = *step;
                }
            }
        });
    });
}

/// Banner shown when a step's result no longer matches its settings.
fn stale_banner(app: &S3dApp, ui: &mut Ui, text: &str) -> bool {
    let p = app.palette();
    let mut clicked = false;
    egui::Frame::new().fill(p.warn.gamma_multiply(0.12)).stroke(Stroke::new(1.0, p.warn.gamma_multiply(0.6))).corner_radius(CornerRadius::same(7)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("⚠").color(p.warn));
            ui.label(RichText::new(text).color(p.text));
            widgets::right(ui, |ui| {
                clicked = ui.add_enabled(!app.jobs.busy(), egui::Button::new("Update")).clicked();
            });
        });
    });
    clicked
}

fn fmt_size(v: glam::Vec3) -> String {
    format!("{:.0} × {:.0} × {:.0} mm", v.x, v.y, v.z)
}

fn import(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    card(ui, |ui| {
        section_title(ui, "Model");
        match &app.doc.source {
            None => {
                ui.label(widgets::weak(ui, "No model loaded. Open an STL, OBJ or 3MF file."));
            }
            Some(src) => {
                let src = src.clone();
                ui.label(RichText::new(&src.name).font(theme::semibold(15.0)).color(p.text_strong));
                let bb = src.mesh.bbox();
                egui::Grid::new("modelinfo").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    ui.label(widgets::weak(ui, "Size"));
                    ui.label(fmt_size(bb.size()));
                    ui.end_row();
                    ui.label(widgets::weak(ui, "Triangles"));
                    ui.label(format!("{}", src.mesh.tris.len()));
                    ui.end_row();
                    ui.label(widgets::weak(ui, "Volume"));
                    ui.label(format!("{:.1} cm³", src.mesh.volume() / 1000.0));
                    ui.end_row();
                    ui.label(widgets::weak(ui, "Health"));
                    let (c, t) = match &src.health {
                        s3d_core::repair::Health::Clean => (p.good, "CLEAN"),
                        s3d_core::repair::Health::Repaired { .. } => (p.info, "REPAIRED"),
                        s3d_core::repair::Health::Rebuilt { .. } => (p.warn, "REBUILT"),
                        s3d_core::repair::Health::Unresolved { .. } => (p.bad, "UNRESOLVED"),
                    };
                    widgets::badge(ui, t, c);
                    ui.end_row();
                });
                match &src.health {
                    s3d_core::repair::Health::Repaired { notes } | s3d_core::repair::Health::Rebuilt { notes } => {
                        for n in notes {
                            ui.label(RichText::new(format!("• {n}")).color(p.text_weak).size(12.5));
                        }
                    }
                    s3d_core::repair::Health::Unresolved { reason } => {
                        ui.label(RichText::new(reason).color(p.bad));
                    }
                    _ => {}
                }
            }
        }
        ui.add_space(6.0);
        if primary_button(ui, if app.doc.source.is_some() { "Open another model…" } else { "Open model…" }, !app.jobs.busy()).clicked() {
            app.request(crate::app::PendingAction::OpenModel(None));
        }
        if app.doc.source.is_some() {
            ui.horizontal(|ui| {
                ui.label(widgets::weak(ui, "Next:"));
                if ui.link("Prepare →").clicked() {
                    app.step = Step::Prepare;
                }
            });
        }
    });
}

fn prepare(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    let busy = app.jobs.busy();
    card(ui, |ui| {
        section_title(ui, "Transform");
        let pr = &mut app.doc.settings.prepare;
        setting_grid(ui, "rot", 3, |ui| {
            for (i, ax) in ["X", "Y", "Z"].iter().enumerate() {
                setting(ui, &format!("Rotate {ax}"), "prepare.rotate", |ui| num(ui, &mut pr.rotate[i], "°", -360.0..=360.0, 1));
            }
            ui.end_row();
            setting(ui, "Scale", "prepare.scale", |ui| num(ui, &mut pr.scale, "%", 1.0..=10000.0, 2));
            setting(ui, "Mirror", "prepare.mirror", |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, ax) in ["X", "Y", "Z"].iter().enumerate() {
                        let b = egui::Button::new(*ax).selected(pr.mirror[i]).min_size(Vec2::new(30.0, 28.0));
                        if ui.add(b).clicked() {
                            pr.mirror[i] = !pr.mirror[i];
                        }
                    }
                });
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            for (label, r) in [("↻ X 90°", 0usize), ("↻ Y 90°", 1), ("↻ Z 90°", 2)] {
                if ui.small_button(label).clicked() {
                    pr.rotate[r] = (pr.rotate[r] + 90.0) % 360.0;
                }
            }
            if ui.small_button("Reset").clicked() {
                *pr = s3d_core::settings::PrepareSettings { wall: pr.wall, ..Default::default() };
            }
        });
        if let Some((m, _)) = app.doc.prepared_mesh() {
            let size = m.bbox().size();
            let bed = app.doc.settings.printer.usable();
            let est = ((size.x / bed[0]).ceil() * (size.y / bed[1]).ceil() * (size.z / bed[2]).ceil()).max(1.0);
            ui.label(RichText::new(format!("Model size {}  ·  about {} bed-sized blocks", fmt_size(size), est)).color(p.text_weak).size(12.5));
        }
    });
    let st = app.status(Step::Prepare);
    if st == Status::Stale && stale_banner(app, ui, "Hollowing is out of date.") {
        app.start_hollow();
    }
    card(ui, |ui| {
        section_title(ui, "Hollow (optional)");
        ui.label(widgets::weak(ui, "Makes the model a shell to save material. Runs only when you press the button."));
        setting_grid(ui, "hollow", 2, |ui| {
            setting(ui, "Wall thickness", "prepare.wall", |ui| num(ui, &mut app.doc.settings.prepare.wall, "mm", 0.8..=50.0, 1));
            ui.end_row();
        });
        let has = app.doc.hollow.is_some();
        if primary_button(ui, if has { "Hollow again" } else { "Hollow model" }, !busy).clicked() {
            app.start_hollow();
        }
        if has && ui.link("Remove hollowing").clicked() {
            app.doc.hollow = None;
        }
    });
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(widgets::weak(ui, "Next:"));
            if ui.link("Split →").clicked() {
                app.step = Step::Split;
            }
        });
    });
}

fn split(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    if app.status(Step::Split) == Status::Stale && stale_banner(app, ui, "Settings changed since the last split.") {
        app.start_split();
    }
    card(ui, |ui| {
        section_title(ui, "Split");
        let s = &mut app.doc.settings.split;
        setting(ui, "Mode", "split.mode", |ui| widgets::segmented(ui, &mut s.mode, &[(SplitMode::Shatter, "Shatter"), (SplitMode::Grid, "Grid")]));
        let shatter = s.mode == SplitMode::Shatter;
        ui.add_space(2.0);
        setting_grid(ui, "split", 2, |ui| {
            setting(ui, "Piece size", "split.target_size", |ui| {
                ui.horizontal(|ui| {
                    let mut max = s.target_size.is_none();
                    if ui.add(egui::Button::new("Max").selected(max).min_size(Vec2::new(44.0, 28.0))).on_hover_text("Largest piece that fits the printer").clicked() {
                        max = !max;
                        let bed = app.doc.settings.printer.usable();
                        s.target_size = if max { None } else { Some(bed[0].min(bed[1]).round()) };
                        // Grid cuts follow the piece size until moved by hand.
                        s.grid = None;
                    }
                    if let Some(t) = &mut s.target_size {
                        if num(ui, t, "mm", 20.0..=2000.0, 0).changed() {
                            s.grid = None;
                        }
                    }
                });
            });
            setting(ui, "Smallest piece", "split.min_piece", |ui| num(ui, &mut s.min_piece, "mm", 0.0..=200.0, 0));
            ui.end_row();
            setting(ui, "Overhang limit", "split.overhang", |ui| num(ui, &mut s.overhang, "°", 20.0..=80.0, 0));
            if shatter {
                setting(ui, "Max split time", "split.time_limit", |ui| num(ui, &mut s.time_limit, "s", 2.0..=600.0, 0));
            }
            ui.end_row();
            setting(ui, "Thinnest part", "split.min_thickness", |ui| num(ui, &mut s.min_thickness, "mm", 0.0..=100.0, 0));
            setting(ui, "Sharpest corner", "split.min_angle", |ui| num(ui, &mut s.min_angle, "°", 5.0..=60.0, 0));
            ui.end_row();
        });
        if !shatter {
            ui.add_space(4.0);
            grid_editor(app, ui);
        }
        ui.add_space(4.0);
        // No-cut zone painting.
        ui.horizontal(|ui| {
            widgets::field_label(ui, "No-cut zones", "split.zones");
        });
        ui.horizontal(|ui| {
            let mut on = app.view.paint;
            if toggle(ui, &mut on).changed() {
                app.view.paint = on;
                if on {
                    app.view.tab = view::Tab::Model;
                    app.view.cut_draw = None;
                    app.view.measure = false;
                }
            }
            ui.label(if app.view.paint { "Painting — drag on the model" } else if app.doc.settings.split.mode == SplitMode::Grid { "Paint areas to check (grid cuts are not moved)" } else { "Paint areas cuts must avoid" });
        });
        if app.view.paint {
            ui.horizontal(|ui| {
                ui.label(widgets::weak(ui, "Brush"));
                num(ui, &mut app.view.brush, "mm", 1.0..=500.0, 0);
                ui.checkbox(&mut app.view.erase, "Erase");
            });
        }
        if !app.doc.zones.is_empty() {
            ui.horizontal(|ui| {
                let what = if app.doc.settings.split.mode == SplitMode::Grid { "pieces cut through them are flagged" } else { "cuts keep out of them" };
                ui.label(widgets::weak(ui, format!("{} painted spots · {what}", app.doc.zones.dabs.len())));
                if ui.small_button("Clear").clicked() {
                    app.doc.zones.dabs.clear();
                }
            });
        }
        let note = if app.doc.settings.split.mode == SplitMode::Grid {
            "Grid cuts exactly where the planes are, then checks the pieces; it does not move cuts. It also adds the features from the Features step. Splitting again starts over and discards manual edits."
        } else {
            "Split also adds the features from the Features step, so pieces come out finished. Splitting again starts over and discards manual edits."
        };
        ui.label(RichText::new(note).color(p.text_weak).size(12.0));
    });
    edits_card(app, ui);
}

/// Grid mode: how many cuts along each axis and where, before cutting.
fn grid_editor(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    let Some((mesh, _)) = app.doc.prepared_mesh() else { return };
    let bb = mesh.bbox();
    let grid = s3d_core::split::grid::cuts(&bb, &app.doc.settings);
    let mut cuts = grid.clone();
    let cell = {
        let bed = app.doc.settings.printer.usable();
        let t = app.doc.settings.split.target_size;
        [0, 1, 2].map(|a| t.map_or(bed[a], |t| t.min(bed[a])))
    };
    let mut hover = None;
    ui.horizontal(|ui| {
        widgets::field_label(ui, "Grid cuts", "split.grid");
        widgets::right(ui, |ui| {
            if ui.small_button("Reset").on_hover_text("Evenly spaced, as few cuts as the piece size allows").clicked() {
                app.doc.settings.split.grid = None;
            }
            if ui.small_button("Space evenly").on_hover_text("Keep the number of cuts, spread them evenly").clicked() {
                for a in 0..3 {
                    cuts[a] = s3d_core::split::grid::even(bb.min[a], bb.max[a], cuts[a].len());
                }
            }
        });
    });
    for (a, name) in ["X", "Y", "Z"].iter().enumerate() {
        let (lo, hi) = (bb.min[a], bb.max[a]);
        ui.horizontal(|ui| {
            ui.label(RichText::new(*name).font(theme::semibold(13.0)).color(p.text_strong));
            let mut n = cuts[a].len() as u32;
            if ui.small_button("−").on_hover_text("One cut fewer").clicked() && n > 0 {
                n -= 1;
            }
            ui.label(RichText::new(format!("{n}")).monospace());
            if ui.small_button("+").on_hover_text("One cut more").clicked() && n < 40 {
                n += 1;
            }
            if n as usize != cuts[a].len() {
                cuts[a] = s3d_core::split::grid::even(lo, hi, n as usize);
            }
            ui.label(widgets::weak(ui, if n == 1 { "cut" } else { "cuts" }));
            // Longest cell along this axis.
            let mut edges: Vec<f32> = cuts[a].iter().copied().filter(|x| *x > lo && *x < hi).collect();
            edges.extend([lo, hi]);
            edges.sort_by(f32::total_cmp);
            let longest = edges.windows(2).map(|w| w[1] - w[0]).fold(0.0f32, f32::max);
            let txt = RichText::new(format!("cells up to {longest:.0} mm")).size(12.0);
            ui.label(if longest > cell[a] + 0.5 { txt.color(p.warn) } else { txt.color(p.text_weak) }).on_hover_text(format!("The model spans {lo:.0} to {hi:.0} mm along {name}; pieces may be up to {:.0} mm.", cell[a]));
        });
        if !cuts[a].is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(18.0);
                for (i, v) in cuts[a].iter_mut().enumerate() {
                    let r = ui.add(egui::DragValue::new(v).range(lo..=hi).speed(1.0).max_decimals(1).suffix(" mm"));
                    if r.hovered() || r.dragged() || r.has_focus() {
                        hover = Some((a, i));
                    }
                }
            });
        }
    }
    app.view.grid_hover = hover;
    if cuts != grid {
        app.doc.settings.split.grid = Some(cuts);
    }
}

/// Manual edits made since the split, newest first. Clicking one selects the
/// pieces it produced.
fn edits_card(app: &mut S3dApp, ui: &mut Ui) {
    if app.doc.edits.is_empty() {
        return;
    }
    let p = app.palette();
    let part = app.doc.split.clone();
    let mut pick: Option<Vec<s3d_core::partition::PieceId>> = None;
    card(ui, |ui| {
        ui.horizontal(|ui| {
            section_title(ui, "Manual edits");
            ui.label(RichText::new(format!("{}", app.doc.edits.len())).color(p.text_weak));
        });
        ui.label(widgets::weak(ui, "Click an edit to select its pieces. Undo (Ctrl+Z) takes back the last one; splitting again discards them all."));
        egui::ScrollArea::vertical().id_salt("edits").max_height(170.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, e) in app.doc.edits.iter().enumerate().rev() {
                let live: Vec<_> = e.pieces.iter().copied().filter(|id| part.as_ref().is_some_and(|s| s.partition.piece(*id).is_some())).collect();
                let text = RichText::new(format!("{}.  {}", i + 1, e.text)).color(if live.is_empty() { p.text_weak } else { p.text });
                let r = ui.add_enabled(!live.is_empty(), egui::Button::new(text).frame(false));
                let r = if live.is_empty() { r.on_disabled_hover_text("Changed again by a later edit") } else { r.on_hover_text("Select these pieces") };
                if r.clicked() {
                    pick = Some(live);
                }
            }
        });
    });
    if let Some(ids) = pick {
        app.view.select(ids.first().copied());
        app.view.multi = ids.into_iter().skip(1).collect();
        app.view.tab = view::Tab::Model;
    }
}

fn features(app: &mut S3dApp, ui: &mut Ui) {
    let busy = app.jobs.busy();
    if app.doc.split.is_none() {
        card(ui, |ui| {
            ui.label(widgets::weak(ui, "Split the model first. These features are added to the pieces when splitting, and can be changed here afterwards."));
        });
    } else if app.status(Step::Features) == Status::Stale && stale_banner(app, ui, "Feature settings changed.") {
        app.start_features();
    }
    if let Some(a) = app.doc.split.as_ref().and_then(|s| s.pin_advice) {
        let p = app.palette();
        let skipped = app.doc.split.as_ref().map(|s| s.partition.pieces.iter().filter(|q| q.flags.contains(&s3d_core::partition::Flag::SkippedDowel)).count()).unwrap_or(0);
        let mut apply = false;
        egui::Frame::new().fill(p.info.gamma_multiply(0.12)).stroke(Stroke::new(1.0, p.info.gamma_multiply(0.6))).corner_radius(CornerRadius::same(7)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("Walls are too thin for these pins on {skipped} pieces.")).color(p.text_strong));
            ui.label(RichText::new(format!("{:.0} × {:.0} mm pins fit on about {:.0}% of cuts.", a.diameter, a.length, a.coverage * 100.0)).color(p.text));
            apply = ui.add_enabled(!busy, egui::Button::new(format!("Use {:.0} × {:.0} mm pins", a.diameter, a.length))).clicked();
        });
        if apply {
            app.doc.settings.connectors.diameter = a.diameter;
            app.doc.settings.connectors.length = a.length;
            app.start_features();
        }
    }
    card(ui, |ui| {
        section_title(ui, "Dowel pins");
        let c = &mut app.doc.settings.connectors;
        ui.horizontal(|ui| {
            toggle(ui, &mut c.dowels);
            widgets::field_label(ui, "Add dowel pin holes", "conn.dowels");
        });
        if c.dowels {
            setting_grid(ui, "dowels", 2, |ui| {
                setting(ui, "Pin diameter", "conn.diameter", |ui| num(ui, &mut c.diameter, "mm", 1.0..=30.0, 1));
                setting(ui, "Pin length", "conn.length", |ui| num(ui, &mut c.length, "mm", 4.0..=200.0, 0));
                ui.end_row();
                setting(ui, "Clearance", "conn.clearance", |ui| num(ui, &mut c.clearance, "mm", 0.0..=2.0, 2));
                setting(ui, "Pins per cut", "conn.per_cut", |ui| num_u32(ui, &mut c.per_cut, 1..=12));
                ui.end_row();
            });
        }
    });
    card(ui, |ui| {
        section_title(ui, "Part numbers");
        let c = &mut app.doc.settings.connectors;
        setting(ui, "Style", "conn.labels", |ui| widgets::segmented(ui, &mut c.labels, &[(LabelMode::Off, "Off"), (LabelMode::Emboss, "Emboss"), (LabelMode::Deboss, "Deboss")]));
        if c.labels != LabelMode::Off {
            setting(ui, "Face", "conn.label_face", |ui| widgets::segmented(ui, &mut c.label_face, &[(LabelFace::Convex, "Convex (outside)"), (LabelFace::Concave, "Concave (inside)")]));
            setting_grid(ui, "labels", 2, |ui| {
                setting(ui, "Size", "conn.label_size", |ui| num(ui, &mut c.label_size, "mm", 3.0..=50.0, 1));
                setting(ui, "Depth", "conn.label_depth", |ui| num(ui, &mut c.label_depth, "mm", 0.2..=5.0, 1));
                ui.end_row();
            });
            ui.label(widgets::weak(ui, "Right-click a piece to move its number to the other side."));
        }
    });
}

fn export(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    let busy = app.jobs.busy();
    if app.doc.split.is_none() {
        card(ui, |ui| {
            ui.label(widgets::weak(ui, "Split the model first."));
        });
        return;
    }
    card(ui, |ui| {
        section_title(ui, "Beds");
        let e = &mut app.doc.settings.export;
        setting_grid(ui, "beds", 2, |ui| {
            setting(ui, "Spacing", "export.spacing", |ui| num(ui, &mut e.spacing, "mm", 0.5..=50.0, 1));
            setting(ui, "Assembly order", "export.keep_order", |ui| widgets::segmented(ui, &mut e.keep_assembly_order, &[(true, "Ordered"), (false, "Random")]));
            ui.end_row();
        });
        let n = app.view.beds.beds.len();
        let stale = app.view.arrange_needed || app.view.beds_key != Some(view::arrange_key(&app.doc));
        if n > 0 {
            ui.label(RichText::new(format!("{} piece(s) on {} bed(s){}", app.view.beds.beds.iter().map(|b| b.len()).sum::<usize>(), n, if app.view.beds.unplaced.is_empty() { String::new() } else { format!(" · {} do not fit", app.view.beds.unplaced.len()) })).color(p.text));
        }
        if stale && n > 0 {
            ui.label(RichText::new("Pieces or settings changed — arrange again.").color(p.warn).size(12.5));
        }
        if ui.add_enabled(!busy, egui::Button::new(if n == 0 { "Arrange beds" } else { "Arrange again" }).min_size(Vec2::new(ui.available_width(), 30.0))).clicked() {
            app.start_arrange();
            app.view.tab = view::Tab::Beds;
        }
    });
    card(ui, |ui| {
        section_title(ui, "Files");
        let e = &mut app.doc.settings.export;
        setting(ui, "Destination", "export.folder", |ui| {
            ui.horizontal(|ui| {
                let label = e.folder.clone().unwrap_or_else(|| "Choose a folder…".into());
                if ui.add(egui::Button::new(RichText::new(label).size(12.5)).truncate().min_size(Vec2::new(ui.available_width(), 28.0))).clicked() {
                    if let Some(f) = rfd::FileDialog::new().set_title("Export folder").pick_folder() {
                        e.folder = Some(f.display().to_string());
                    }
                }
            });
        });
        setting(ui, "Bed file name", "export.bed_name", |ui| ui.add(egui::TextEdit::singleline(&mut e.bed_name).desired_width(f32::INFINITY)));
        setting(ui, "Part file name", "export.part_name", |ui| ui.add(egui::TextEdit::singleline(&mut e.part_name).desired_width(f32::INFINITY)));
        setting_grid(ui, "exportopts", 2, |ui| {
            setting(ui, "Shrinkage X/Y", "export.shrink_xy", |ui| num(ui, &mut e.shrink_xy, "%", -5.0..=5.0, 2));
            setting(ui, "Shrinkage Z", "export.shrink_z", |ui| num(ui, &mut e.shrink_z, "%", -5.0..=5.0, 2));
            ui.end_row();
            setting(ui, "Filament density", "export.density", |ui| num(ui, &mut e.density, "g/cm³", 0.5..=3.0, 2));
            ui.end_row();
        });
        for (v, label, id) in [(&mut e.single_stls, "Single-part STLs", "export.single_stls"), (&mut e.guide, "Assembly guide (PDF)", "export.guide"), (&mut e.viewer, "3D viewer (HTML)", "export.viewer")] {
            ui.horizontal(|ui| {
                toggle(ui, v);
                widgets::field_label(ui, label, id);
            });
        }
    });
}

/// The step's main button, kept in view between its settings and the
/// parts list.
fn action(app: &mut S3dApp, ui: &mut Ui) {
    let busy = app.jobs.busy();
    match app.step {
        Step::Split => {
            let label = if app.doc.split.is_some() { "Split again" } else { "Split" };
            if primary_button(ui, label, !busy && app.doc.source.is_some()).on_hover_text("Cuts the model, then adds the features (pins, part numbers) set in Features").clicked() {
                app.start_split();
            }
        }
        Step::Features => {
            if primary_button(ui, "Apply features", !busy && app.doc.split.is_some()).clicked() {
                app.start_features();
            }
        }
        Step::Export => {
            if primary_button(ui, "Export files", !busy && app.doc.split.is_some()).on_hover_text("Writes one 3MF per bed, the print list, guide and viewer").clicked() {
                let folder = app.doc.settings.export.folder.clone().map(std::path::PathBuf::from).or_else(|| rfd::FileDialog::new().set_title("Export folder").pick_folder());
                if let Some(f) = folder {
                    app.doc.settings.export.folder = Some(f.display().to_string());
                    app.start_export(f);
                }
            }
        }
        Step::Import | Step::Prepare => {}
    }
}

/// Printer size in the top bar; opens the Settings window to change it.
pub fn printer_button(app: &mut S3dApp, ui: &mut Ui) {
    let v = app.doc.settings.printer.volume;
    let r = ui.button(format!("🖨 {:.0} × {:.0} × {:.0} mm", v[0], v[1], v[2]));
    if r.clicked() {
        app.show_settings = !app.show_settings;
    }
    r.on_hover_ui(|ui| crate::tooltips::show(ui, "printer.volume"));
}

/// The Settings window: printer build volume and bed margin.
pub fn app_settings(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    ui.set_width(340.0);
    section_title(ui, "Printer");
    let pr = &mut app.doc.settings.printer;
    setting_grid(ui, "printer", 3, |ui| {
        for (i, ax) in ["X", "Y", "Z"].iter().enumerate() {
            setting(ui, &format!("Build {ax}"), "printer.volume", |ui| num(ui, &mut pr.volume[i], "mm", 50.0..=2000.0, 0));
        }
        ui.end_row();
        setting(ui, "Bed margin", "printer.margin", |ui| num(ui, &mut pr.margin, "mm", 0.0..=50.0, 1));
        ui.end_row();
    });
    let u = pr.usable();
    ui.label(RichText::new(format!("Usable volume: {:.0} × {:.0} × {:.0} mm", u[0], u[1], u[2])).color(p.text_weak));
}
