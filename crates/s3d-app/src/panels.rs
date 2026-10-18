//! Left panel: step tabs, each step's settings card and action button, the
//! parts list and the selected piece's properties.

use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2};
use s3d_core::edits::EditStatus;
use s3d_core::settings::{LabelMode, SplitMode};

use crate::app::S3dApp;
use crate::doc::{Status, Step};
use crate::theme::{self, Palette};
use crate::widgets::{self, card, num, num_u32, primary_button, section_title, setting, setting_grid, toggle};
use crate::{parts, view};

pub fn show(app: &mut S3dApp, ui: &mut Ui) {
    step_tabs(app, ui);
    ui.add_space(10.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        match app.step {
            Step::Import => import(app, ui),
            Step::Prepare => prepare(app, ui),
            Step::Split => split(app, ui),
            Step::Connectors => connectors(app, ui),
            Step::Export => export(app, ui),
        }
        if app.doc.split.is_some() && matches!(app.step, Step::Split | Step::Connectors | Step::Export) {
            parts::selected_card(app, ui);
            parts::list(app, ui);
        }
    });
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
    let busy = app.jobs.busy();
    if app.status(Step::Split) == Status::Stale && stale_banner(app, ui, "Settings changed since the last split.") {
        app.start_split();
    }
    card(ui, |ui| {
        section_title(ui, "Split");
        let s = &mut app.doc.settings.split;
        setting(ui, "Mode", "split.mode", |ui| widgets::segmented(ui, &mut s.mode, &[(SplitMode::Triangle, "Triangle"), (SplitMode::Grid, "Grid")]));
        ui.add_space(2.0);
        setting_grid(ui, "split", 2, |ui| {
            setting(ui, "Piece size", "split.target_size", |ui| {
                ui.horizontal(|ui| {
                    let mut max = s.target_size.is_none();
                    if ui.add(egui::Button::new("Max").selected(max).min_size(Vec2::new(44.0, 28.0))).on_hover_text("Largest piece that fits the printer").clicked() {
                        max = !max;
                        let bed = app.doc.settings.printer.usable();
                        s.target_size = if max { None } else { Some(bed[0].min(bed[1]).round()) };
                    }
                    if let Some(t) = &mut s.target_size {
                        num(ui, t, "mm", 20.0..=2000.0, 0);
                    }
                });
            });
            setting(ui, "Smallest piece", "split.min_piece", |ui| num(ui, &mut s.min_piece, "mm", 0.0..=200.0, 0));
            ui.end_row();
            setting(ui, "Overhang limit", "split.overhang", |ui| num(ui, &mut s.overhang, "°", 20.0..=80.0, 0));
            if s.mode == SplitMode::Triangle {
                setting(ui, "Search time", "split.time_limit", |ui| num(ui, &mut s.time_limit, "s", 2.0..=600.0, 0));
                ui.end_row();
                setting(ui, "Side angle", "split.side_angle", |ui| num(ui, &mut s.side_angle, "°", 45.0..=60.0, 0));
            }
            ui.end_row();
        });
        ui.add_space(4.0);
        // No-cut zone painting.
        ui.horizontal(|ui| {
            widgets::field_label(ui, "No-cut zones", "split.mode");
        });
        ui.horizontal(|ui| {
            let mut on = app.view.paint;
            if toggle(ui, &mut on).changed() {
                app.view.paint = on;
            }
            ui.label(if app.view.paint { "Painting — drag on the model" } else { "Paint areas cuts must avoid" });
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
                ui.label(widgets::weak(ui, format!("{} painted spots", app.doc.zones.dabs.len())));
                if ui.small_button("Clear").clicked() {
                    app.doc.zones.dabs.clear();
                }
            });
        }
        ui.add_space(6.0);
        let label = if app.doc.split.is_some() { "Split again" } else { "Split" };
        if primary_button(ui, label, !busy && app.doc.source.is_some()).on_hover_text("Cuts the model, then adds connectors and part numbers").clicked() {
            app.start_split();
        }
        ui.label(RichText::new("Split also adds connectors and part numbers, so pieces come out finished.").color(p.text_weak).size(12.0));
    });
    edits_card(app, ui);
}

fn edits_card(app: &mut S3dApp, ui: &mut Ui) {
    if app.doc.edits.is_empty() {
        return;
    }
    let p = app.palette();
    let mut remove = None;
    card(ui, |ui| {
        section_title(ui, &format!("Manual edits ({})", app.doc.edits.len()));
        ui.label(widgets::weak(ui, "Replayed after every split."));
        for (i, e) in app.doc.edits.iter().enumerate() {
            ui.horizontal(|ui| {
                let st = app.doc.edit_status.get(i);
                let (c, t) = match st {
                    Some(EditStatus::Applied) | None => (p.good, "✓"),
                    Some(EditStatus::Orphaned) => (p.warn, "?"),
                    Some(EditStatus::Failed(_)) => (p.bad, "✖"),
                };
                let r = ui.label(RichText::new(t).color(c));
                if let Some(EditStatus::Failed(m)) = st {
                    r.on_hover_text(m.clone());
                } else if let Some(EditStatus::Orphaned) = st {
                    r.on_hover_text("Its piece or cut no longer exists after re-splitting.");
                }
                ui.label(format!("{}. {}", i + 1, e.describe()));
                widgets::right(ui, |ui| {
                    if ui.small_button("✕").on_hover_text("Remove this edit (takes effect on the next split)").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
    });
    if let Some(i) = remove {
        app.doc.edits.remove(i);
        if i < app.doc.edit_status.len() {
            app.doc.edit_status.remove(i);
        }
    }
}

fn connectors(app: &mut S3dApp, ui: &mut Ui) {
    let busy = app.jobs.busy();
    if app.doc.split.is_none() {
        card(ui, |ui| {
            ui.label(widgets::weak(ui, "Split the model first. Connectors are added to the pieces after splitting."));
        });
    } else if app.status(Step::Connectors) == Status::Stale && stale_banner(app, ui, "Connector settings changed.") {
        app.start_connectors();
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
            app.start_connectors();
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
                setting(ui, "Clearance", "conn.clearance", |ui| num(ui, &mut c.clearance, "mm", 0.0..=2.0, 2));
                ui.end_row();
                setting(ui, "Pin length", "conn.length", |ui| num(ui, &mut c.length, "mm", 4.0..=200.0, 0));
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
            setting_grid(ui, "labels", 2, |ui| {
                setting(ui, "Size", "conn.label_size", |ui| num(ui, &mut c.label_size, "mm", 3.0..=50.0, 1));
                setting(ui, "Depth", "conn.label_depth", |ui| num(ui, &mut c.label_depth, "mm", 0.2..=5.0, 1));
                ui.end_row();
            });
        }
        ui.add_space(6.0);
        if primary_button(ui, "Update connectors", !busy && app.doc.split.is_some()).clicked() {
            app.start_connectors();
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
            setting(ui, "Assembly order", "export.keep_order", |ui| {
                ui.horizontal(|ui| {
                    toggle(ui, &mut e.keep_assembly_order);
                    ui.label(if e.keep_assembly_order { "Keep" } else { "Free" });
                })
            });
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
        ui.add_space(6.0);
        let ready = !busy && app.doc.split.is_some();
        if primary_button(ui, "Export files", ready).on_hover_text("Writes one 3MF per bed, the print list, guide and viewer").clicked() {
            let folder = app.doc.settings.export.folder.clone().map(std::path::PathBuf::from).or_else(|| rfd::FileDialog::new().set_title("Export folder").pick_folder());
            if let Some(f) = folder {
                app.doc.settings.export.folder = Some(f.display().to_string());
                app.start_export(f);
            }
        }
    });
    if !app.recent.export_history.is_empty() {
        card(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("EXPORT HISTORY").font(theme::semibold(12.5)).color(p.text_weak)).default_open(false).show(ui, |ui| {
                for h in &app.recent.export_history {
                    ui.label(RichText::new(h).size(12.0).color(p.text_weak));
                }
            });
        });
    }
}

/// Printer drop-down in the top bar.
pub fn printer_picker(app: &mut S3dApp, ui: &mut Ui) {
    let cur = app.doc.settings.printer.printer.clone();
    let mut list = s3d_core::printers::builtin();
    list.extend(app.recent.custom_printers.iter().cloned());
    let r = egui::ComboBox::from_id_salt("printer").selected_text(format!("🖨 {}", cur.name)).width(230.0).show_ui(ui, |ui| {
        for pr in list {
            let label = format!("{}   {:.0}×{:.0}×{:.0}", pr.name, pr.volume[0], pr.volume[1], pr.volume[2]);
            if ui.selectable_label(pr == cur, label).clicked() {
                app.doc.settings.printer.printer = pr;
            }
        }
        ui.separator();
        if ui.selectable_label(false, "Custom printer…").clicked() {
            app.show_settings = true;
        }
    });
    r.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Printer"));
    r.response.on_hover_ui(|ui| crate::tooltips::show(ui, "printer.model"));
}

/// The Settings window: printer size, margin, custom printers.
pub fn app_settings(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    ui.set_width(340.0);
    section_title(ui, "Printer");
    let pr = &mut app.doc.settings.printer;
    ui.label(RichText::new(&pr.printer.name).color(p.text_strong));
    setting_grid(ui, "printer", 3, |ui| {
        for (i, ax) in ["X", "Y", "Z"].iter().enumerate() {
            setting(ui, &format!("Build {ax}"), "printer.model", |ui| {
                if num(ui, &mut pr.printer.volume[i], "mm", 50.0..=2000.0, 0).changed() && !pr.printer.name.starts_with("Custom") {
                    pr.printer.name = "Custom".into();
                }
            });
        }
        ui.end_row();
        setting(ui, "Bed margin", "printer.margin", |ui| num(ui, &mut pr.margin, "mm", 0.0..=50.0, 1));
        ui.end_row();
    });
    if pr.printer.name.starts_with("Custom") {
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut pr.printer.name);
            if ui.button("Save printer").clicked() {
                let c = pr.printer.clone();
                app.recent.custom_printers.retain(|x| x.name != c.name);
                app.recent.custom_printers.push(c);
                app.recent.save();
            }
        });
    } else if ui.button("Make custom copy").clicked() {
        pr.printer.name = format!("Custom ({})", pr.printer.name);
    }
    let u = pr.usable();
    ui.label(RichText::new(format!("Usable volume: {:.0} × {:.0} × {:.0} mm", u[0], u[1], u[2])).color(p.text_weak));
}
