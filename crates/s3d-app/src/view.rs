//! Right side: view tabs (3D view, Beds, Assembly), toolbar and viewport.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui;
use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2};
use glam::{Mat4, Vec3, Vec4};
use s3d_core::assembly::Assembly;
use s3d_core::bvh::Bvh;
use s3d_core::edits::EditOp;
use s3d_core::mesh::{Aabb, Mesh};
use s3d_core::pack::{Layout, PackItem, PackParams};
use s3d_core::partition::{Partition, PieceId};
use s3d_core::plane::Plane;
use s3d_core::settings::fingerprint;
use s3d_core::zones::{Dab, Zones};

use crate::app::S3dApp;
use crate::doc::{Document, Step};
use crate::parts::{self, piece_color};
use crate::theme::{self, Palette};
use crate::viewport::renderer::{FLAG_GHOST, FLAG_HOVER, FLAG_SELECTED, FLAG_SUPPORT};
use crate::viewport::{DrawItem, LineSeg, MeshRef, SceneFrame, TriVert, ViewPreset, Viewport};
use crate::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Model,
    Beds,
    Assembly,
}

/// A cut line being drawn across one piece (points and surface normals in
/// the piece's own coordinates).
#[derive(Clone, Debug, Default)]
pub struct CutDraw {
    pub piece: PieceId,
    pub pts: Vec<Vec3>,
    pub normals: Vec<Vec3>,
}

pub struct ViewState {
    pub viewport: Viewport,
    pub tab: Tab,
    /// The selected piece (the one shown in the properties card).
    pub selected: Option<PieceId>,
    /// Further pieces added to the selection with Shift-click.
    pub multi: Vec<PieceId>,
    pub hover: Option<PieceId>,
    pub hidden: HashSet<PieceId>,
    pub isolate: Option<Vec<PieceId>>,
    pub show_planes: bool,
    /// Draw the part numbers on the pieces.
    pub show_numbers: bool,
    /// The parts list shows only pieces that need attention, worst first.
    pub attention: bool,
    /// Grid cut (axis, index) under the pointer in the Split card.
    pub grid_hover: Option<(usize, usize)>,
    pub section: bool,
    pub section_axis: usize,
    pub section_pos: f32,
    pub measure: bool,
    measure_pts: Vec<Vec3>,
    pub explode: f32,
    pub assembly_step: usize,
    /// Assembly view also shows the rest of the model, faintly.
    pub assembly_context: bool,
    /// Step the assembly camera was last framed on.
    assembly_framed: Option<usize>,
    pub paint: bool,
    pub brush: f32,
    pub erase: bool,
    /// Cut line being drawn, if the tool is active.
    pub cut_draw: Option<CutDraw>,
    pub beds: Layout,
    pub beds_key: Option<u64>,
    /// Beds tab shows the plates in 3D (else a flat plan for dragging).
    pub beds_3d: bool,
    pub beds_viewport: Viewport,
    pub arrange_needed: bool,
    /// Set when arranging failed, so it is not retried every frame.
    pub arrange_failed: bool,
    bvh: HashMap<u64, Arc<std::sync::OnceLock<Arc<Bvh>>>>,
    assembly: Option<(u64, Arc<Assembly>)>,
    pub(crate) bed_drag: Option<(PieceId, egui::Vec2)>,
    pub(crate) footprints: HashMap<u64, Arc<Vec<glam::Vec2>>>,
    /// Part number outlines per piece mesh.
    label_lines: HashMap<u64, Arc<Vec<[Vec3; 2]>>>,
    /// Sharp edges per piece mesh, outlined on selected pieces.
    edge_lines: HashMap<u64, Arc<Vec<[Vec3; 2]>>>,
    /// Painted-zone overlay triangles and what they were built from.
    paint_tris: Option<(u64, Arc<Vec<TriVert>>)>,
    fitted_once: bool,
    /// Cut lines and summary of the split's current best layout.
    pub search_preview: Arc<std::sync::Mutex<Option<(Arc<Vec<[Vec3; 2]>>, String)>>>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            viewport: Viewport::default(),
            tab: Tab::Model,
            selected: None,
            multi: Vec::new(),
            hover: None,
            hidden: HashSet::new(),
            isolate: None,
            show_planes: false,
            show_numbers: true,
            attention: false,
            grid_hover: None,
            section: false,
            section_axis: 0,
            section_pos: 0.5,
            measure: false,
            measure_pts: Vec::new(),
            explode: 0.0,
            assembly_step: 0,
            assembly_context: true,
            assembly_framed: None,
            paint: false,
            brush: 20.0,
            erase: false,
            cut_draw: None,
            beds: Layout::default(),
            beds_key: None,
            beds_3d: true,
            beds_viewport: Viewport::default(),
            arrange_needed: false,
            arrange_failed: false,
            bvh: HashMap::new(),
            assembly: None,
            bed_drag: None,
            footprints: HashMap::new(),
            label_lines: HashMap::new(),
            edge_lines: HashMap::new(),
            paint_tris: None,
            fitted_once: false,
            search_preview: Default::default(),
        }
    }
}

/// Packs the current pieces (runs in a background job).
pub fn compute_arrangement(split: &crate::doc::SplitState, s: &s3d_core::settings::Settings) -> Layout {
    let u = s.printer.usable();
    let order: Vec<PieceId> = if s.export.keep_assembly_order {
        s3d_core::assembly::plan(&split.partition).order
    } else {
        split.partition.pieces.iter().map(|p| p.id).collect()
    };
    let items: Vec<PackItem> = order
        .iter()
        .filter_map(|id| split.partition.piece(*id))
        .map(|p| PackItem { id: p.id, mesh: p.display_mesh().clone(), print: p.orient.map(|o| o.matrix()).unwrap_or(Mat4::IDENTITY) })
        .collect();
    s3d_core::pack::pack(&items, &PackParams { bed: [u[0], u[1]], spacing: s.export.spacing, keep_order: s.export.keep_assembly_order, angle_step: 15.0 })
}

/// Fingerprint of what the bed layout depends on.
pub fn arrange_key(doc: &Document) -> u64 {
    let e = &doc.settings.export;
    fingerprint(&(doc.split.as_ref().map(|s| (s.generation, s.partition.pieces.iter().map(|p| p.rev).collect::<Vec<_>>())), e.spacing, e.keep_assembly_order, &doc.settings.printer))
}

impl ViewState {
    /// Selects one piece (or none), dropping any multi-selection.
    pub fn select(&mut self, id: Option<PieceId>) {
        self.selected = id;
        self.multi.clear();
        if self.cut_draw.as_ref().is_some_and(|c| Some(c.piece) != id) {
            self.cut_draw = None;
        }
    }

    /// Shift-click: adds a piece to the selection, or takes it out.
    pub fn toggle_select(&mut self, id: PieceId) {
        self.cut_draw = None;
        match self.selected {
            None => self.selected = Some(id),
            Some(s) if s == id => self.selected = self.multi.pop(),
            Some(_) => {
                if let Some(i) = self.multi.iter().position(|m| *m == id) {
                    self.multi.remove(i);
                } else {
                    self.multi.push(id);
                }
            }
        }
    }

    /// Every selected piece, the main one first.
    pub fn selection(&self) -> Vec<PieceId> {
        self.selected.into_iter().chain(self.multi.iter().copied()).collect()
    }

    pub fn is_selected(&self, id: PieceId) -> bool {
        self.selected == Some(id) || self.multi.contains(&id)
    }

    /// Drops selected pieces that no longer exist.
    pub fn prune_selection(&mut self, part: &Partition) {
        self.multi.retain(|id| part.piece(*id).is_some());
        if self.selected.is_some_and(|id| part.piece(id).is_none()) {
            self.selected = self.multi.pop();
        }
    }

    pub fn is_hidden(&self, id: PieceId) -> bool {
        self.hidden.contains(&id) || self.isolate.as_ref().is_some_and(|v| !v.contains(&id))
    }

    pub fn toggle_hidden(&mut self, id: PieceId) {
        if !self.hidden.remove(&id) {
            self.hidden.insert(id);
        }
    }

    pub fn show_all(&mut self) {
        self.hidden.clear();
        self.isolate = None;
    }

    pub fn on_new_split(&mut self) {
        self.footprints.clear();
        self.label_lines.clear();
        self.select(None);
        self.hidden.clear();
        self.isolate = None;
        self.cut_draw = None;
        self.assembly = None;
        self.measure_pts.clear();
    }

    pub fn on_pieces_changed(&mut self) {
        self.cut_draw = None;
        self.assembly = None;
    }

    /// Starts drawing a cut line across a piece.
    pub fn start_cut_draw(&mut self, id: PieceId) {
        self.select(Some(id));
        self.paint = false;
        self.measure = false;
        self.cut_draw = Some(CutDraw { piece: id, ..Default::default() });
    }

    /// True when the bed layout is missing or no longer matches the pieces.
    pub fn arrangement_stale(&self, doc: &Document) -> bool {
        self.beds.beds.is_empty() || self.arrange_needed || self.beds_key != Some(arrange_key(doc))
    }

    pub fn set_arrangement(&mut self, layout: Layout, key: u64) {
        self.beds = layout;
        self.beds_key = Some(key);
        self.arrange_needed = false;
    }

    pub fn to_json(&self, doc: &Document) -> serde_json::Value {
        serde_json::json!({ "beds": self.beds, "beds_current": !self.arrangement_stale(doc) })
    }

    pub fn restore(&mut self, doc: &Document) {
        if let Some(v) = &doc.extra_view {
            if let Ok(l) = serde_json::from_value::<Layout>(v["beds"].clone()) {
                self.beds = l;
            }
            // Keys refer to the previous session's generation numbers: re-key
            // only a layout that was current when saved.
            if !self.beds.beds.is_empty() && v["beds_current"].as_bool() == Some(true) {
                self.beds_key = Some(arrange_key(doc));
            } else {
                self.arrange_needed = true;
            }
        }
    }

    /// Picking structure for a mesh; built on a worker thread the first time
    /// it is needed (None until ready) so large models never stall the UI.
    fn bvh_for(&mut self, id: u64, mesh: &Arc<Mesh>, ctx: &egui::Context) -> Option<Arc<Bvh>> {
        let slot = self.bvh.entry(id).or_insert_with(|| {
            let slot: Arc<std::sync::OnceLock<Arc<Bvh>>> = Arc::default();
            let (s2, m, c) = (slot.clone(), mesh.clone(), ctx.clone());
            rayon::spawn(move || {
                let _ = s2.set(Arc::new(Bvh::new(&m)));
                c.request_repaint();
            });
            slot
        });
        slot.get().cloned()
    }

    fn assembly(&mut self, doc: &Document) -> Option<Arc<Assembly>> {
        let s = doc.split.as_ref()?;
        let key = fingerprint(&(s.generation, s.partition.pieces.iter().map(|p| p.rev).collect::<Vec<_>>()));
        if self.assembly.as_ref().is_none_or(|a| a.0 != key) {
            self.assembly = Some((key, Arc::new(s3d_core::assembly::plan(&s.partition))));
        }
        self.assembly.as_ref().map(|a| a.1.clone())
    }

    /// Outline segments of a piece's part number (cached per mesh).
    fn label_outline(&mut self, mesh_id: u64, place: &s3d_core::labels::LabelPlace) -> Arc<Vec<[Vec3; 2]>> {
        self.label_lines.entry(mesh_id).or_insert_with(|| Arc::new(s3d_core::labels::outline(place))).clone()
    }

    /// Edges of a selected piece, drawn over it in the accent colour.
    pub(crate) fn outline_edges(&mut self, mesh_id: u64, mesh: &Mesh, model: Mat4, color: [f32; 4], width: f32, out: &mut Vec<LineSeg>) {
        let e = self.edge_lines.entry(mesh_id).or_insert_with(|| Arc::new(mesh.feature_edges(30.0)));
        out.extend(e.iter().map(|s| LineSeg::new(model.transform_point3(s[0]), model.transform_point3(s[1]), color, width)));
    }
}

/// Painted zones as surface triangles of the drawn meshes, slightly lifted
/// and shaded, so the paint looks like marker on the model. `to_local` maps
/// a dab (source coordinates) into the drawn meshes' own coordinates.
fn paint_overlay(zones: &Zones, drawn: &[(Arc<Bvh>, Mat4)], to_local: Mat4, color: [f32; 3]) -> Vec<TriVert> {
    let scale = to_local.to_scale_rotation_translation().0.abs().max_element().max(1e-6);
    let light = Vec3::new(0.35, 0.25, 0.9).normalize();
    let mut out = Vec::new();
    let dabs: Vec<Dab> = zones.dabs.iter().map(|d| Dab { p: to_local.transform_point3(d.p), radius: d.radius * scale, n: to_local.transform_vector3(d.n).normalize_or_zero() }).collect();
    for (bvh, model) in drawn {
        let mut seen: HashSet<u32> = HashSet::new();
        let mut push = |t: &[Vec3; 3]| {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            let lift = n * 0.12;
            let k = 0.6 + 0.4 * model.transform_vector3(n).normalize_or_zero().dot(light).abs();
            let c = [color[0] * k, color[1] * k, color[2] * k, 1.0];
            for v in t {
                out.push(TriVert { pos: model.transform_point3(*v + lift).into(), color: c });
            }
        };
        for d in &dabs {
            bvh.for_each_near(d.p, d.radius, |u| {
                let t = bvh.triangle(u);
                if !d.covers(&t) {
                    return;
                }
                let long = (0..3).map(|i| t[i].distance(t[(i + 1) % 3])).fold(0.0f32, f32::max);
                if long <= d.radius * 0.35 {
                    // Small triangle: whole, once, if its middle is painted.
                    if (t[0] + t[1] + t[2]).distance(d.p * 3.0) <= d.radius * 3.0 && seen.insert(u) {
                        push(&t);
                    }
                    return;
                }
                // Big triangle: only the painted part of a subdivision.
                let mut stack = vec![(t, 0)];
                while let Some((s, depth)) = stack.pop() {
                    let long = (0..3).map(|i| s[i].distance(s[(i + 1) % 3])).fold(0.0f32, f32::max);
                    let mid = (s[0] + s[1] + s[2]) / 3.0;
                    if long <= d.radius * 0.35 || depth >= 5 {
                        if mid.distance(d.p) <= d.radius {
                            push(&s);
                        }
                        continue;
                    }
                    if s3d_core::bvh::closest_on_tri(d.p, &s).distance(d.p) > d.radius {
                        continue;
                    }
                    let (a, b, c) = ((s[0] + s[1]) * 0.5, (s[1] + s[2]) * 0.5, (s[2] + s[0]) * 0.5);
                    for q in [[s[0], a, c], [a, s[1], b], [c, b, s[2]], [a, b, c]] {
                        stack.push((q, depth + 1));
                    }
                }
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------

pub fn show(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    // One projection choice for every 3D view.
    app.view.viewport.camera.ortho = app.recent.ortho;
    app.view.beds_viewport.camera.ortho = app.recent.ortho;
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.view.cut_draw = None;
    }
    egui::Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.card_stroke)).corner_radius(CornerRadius::same(8)).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        tabs_row(app, ui, &p);
        ui.add_space(2.0);
        // A running job gets a progress strip under the view.
        let busy = app.jobs.current.is_some();
        let full = ui.available_rect_before_wrap();
        let main = if busy { full.with_max_y(full.max.y - 40.0) } else { full };
        ui.scope_builder(egui::UiBuilder::new().max_rect(main), |ui| match app.view.tab {
            Tab::Model | Tab::Assembly => {
                toolbar(app, ui, &p);
                viewport(app, ui, &p);
            }
            Tab::Beds => crate::beds::beds_view(app, ui, &p),
        });
        if busy {
            let strip = full.with_min_y(main.max.y);
            ui.scope_builder(egui::UiBuilder::new().max_rect(strip.shrink2(Vec2::new(12.0, 6.0))), |ui| progress_strip(app, ui, &p));
        }
    });
}

/// Stage, a clock counting up (with the split's time limit beside it) and
/// Cancel for the running job.
fn progress_strip(app: &mut S3dApp, ui: &mut Ui, p: &Palette) {
    let Some(r) = &app.jobs.current else { return };
    let (kind, stage, secs) = (r.kind, r.ctx.stage(), r.started.elapsed().as_secs_f32());
    // Only Shatter keeps improving pieces up to the time limit.
    let shatter = app.doc.settings.split.mode == s3d_core::settings::SplitMode::Shatter;
    let limit = (kind == crate::jobs::JobKind::Split && shatter).then_some(app.doc.settings.split.time_limit);
    let mut cancel = false;
    ui.horizontal_centered(|ui| {
        ui.add(egui::Spinner::new().size(14.0).color(p.accent));
        widgets::right(ui, |ui| {
            cancel = ui.button("Cancel").clicked();
            ui.add_space(6.0);
            if let Some(l) = limit {
                ui.label(RichText::new(format!("/ {} limit", clock(l))).color(p.text_weak).monospace());
            }
            ui.label(RichText::new(clock(secs)).font(egui::FontId::monospace(15.0)).color(p.text_strong));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(format!("{} — {stage}", kind.label())).color(p.text)).truncate());
            });
        });
    });
    if cancel {
        app.jobs.cancel();
        app.notice = Some(crate::app::Notice::Info("Cancelled.".into()));
    }
}

/// Minutes and seconds, as 0:42.
fn clock(secs: f32) -> String {
    let s = secs.floor() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

fn tabs_row(app: &mut S3dApp, ui: &mut Ui, p: &Palette) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        let has_split = app.doc.split.is_some();
        for (tab, label) in [(Tab::Model, "3D view"), (Tab::Beds, "Beds"), (Tab::Assembly, "Assembly")] {
            let enabled = tab == Tab::Model || has_split;
            let sel = app.view.tab == tab;
            let extra = match tab {
                Tab::Beds if !app.view.beds.beds.is_empty() => format!("  {}", app.view.beds.beds.len()),
                Tab::Model if has_split => format!("  {}", app.doc.split.as_ref().unwrap().partition.pieces.len()),
                _ => String::new(),
            };
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(110.0, 40.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
            if sel {
                ui.painter().rect_filled(rect.shrink2(Vec2::new(0.0, 4.0)), CornerRadius::same(6), p.field);
                ui.painter().hline(rect.x_range().shrink(8.0), rect.bottom() - 2.0, Stroke::new(2.5, p.accent));
            }
            let col = if !enabled { p.text_weak.gamma_multiply(0.5) } else if sel { p.text_strong } else { p.text_weak };
            let g = ui.painter().layout_no_wrap(label.into(), theme::semibold(14.0), col);
            let g2 = ui.painter().layout_no_wrap(extra, egui::FontId::proportional(12.5), p.text_weak);
            let total = g.size().x + g2.size().x;
            let start = rect.center() - Vec2::new(total / 2.0, g.size().y / 2.0);
            ui.painter().galley(start, g.clone(), col);
            ui.painter().galley(start + Vec2::new(g.size().x, 1.5), g2, p.text_weak);
            if resp.clicked() && enabled {
                app.view.tab = tab;
            }
        }
    });
    let r = ui.max_rect();
    ui.painter().hline(r.x_range(), ui.cursor().top(), Stroke::new(1.0, p.card_stroke));
}

/// Perspective / isometric switch (shared by all 3D views, remembered).
pub fn projection_switch(app: &mut S3dApp, ui: &mut Ui) {
    let mut ortho = app.recent.ortho;
    // Laid out left to right even inside right-aligned toolbars.
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        widgets::segmented(ui, &mut ortho, &[(false, "Perspective"), (true, "Isometric")]).on_hover_text("Perspective view, or a parallel (isometric) projection without perspective");
    });
    if ortho != app.recent.ortho {
        app.recent.ortho = ortho;
        app.recent.save();
    }
}

fn toolbar(app: &mut S3dApp, ui: &mut Ui, p: &Palette) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        let is_assembly = app.view.tab == Tab::Assembly;
        let has_split = app.doc.split.is_some();
        // Assembly: one position per joining step, plus the finished model.
        let n = if is_assembly { app.view.assembly(&app.doc).map(|a| a.steps.len() + 1).unwrap_or(1) } else { 0 };
        let v = &mut app.view;
        if is_assembly {
            ui.label(widgets::weak(ui, "Step"));
            let mut step = v.assembly_step.min(n.saturating_sub(1)) + 1;
            if ui.add(egui::Slider::new(&mut step, 1..=n.max(1)).show_value(true)).changed() {
                v.assembly_step = step - 1;
            }
            if ui.small_button("◀").clicked() {
                v.assembly_step = v.assembly_step.saturating_sub(1);
            }
            if ui.small_button("▶").clicked() {
                v.assembly_step = (v.assembly_step + 1).min(n.saturating_sub(1));
            }
            widgets::toggle_button(ui, &mut v.assembly_context, "Context").on_hover_text("Also show the rest of the model, faintly");
        } else {
            ui.add_enabled_ui(has_split, |ui| {
                widgets::toggle_button(ui, &mut v.show_planes, "Cut planes").on_hover_text("Show the cut planes faintly");
                widgets::toggle_button(ui, &mut v.show_numbers, "Numbers").on_hover_text("Show the embossed or debossed part numbers");
                ui.label(widgets::weak(ui, "Explode"));
                ui.add(egui::Slider::new(&mut v.explode, 0.0..=1.0).show_value(false));
            });
            widgets::toggle_button(ui, &mut v.section, "Section").on_hover_text("Cut the view open along an axis");
            if v.section {
                let r = egui::ComboBox::from_id_salt("secaxis").width(44.0).selected_text(["X", "Y", "Z"][v.section_axis]).show_ui(ui, |ui| {
                    for (i, a) in ["X", "Y", "Z"].iter().enumerate() {
                        ui.selectable_value(&mut v.section_axis, i, *a);
                    }
                });
                r.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Section axis"));
                ui.add(egui::Slider::new(&mut v.section_pos, 0.0..=1.0).show_value(false));
            }
            if widgets::toggle_button(ui, &mut v.measure, "Measure").on_hover_text("Click two points to measure").changed() {
                v.measure_pts.clear();
            }
        }
        widgets::right(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            egui::Frame::new().stroke(Stroke::new(1.0, p.field_stroke)).corner_radius(CornerRadius::same(7)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
                ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
                // Right-to-left: last listed appears first.
                if ui.button("Fit").clicked() {
                    app.view.viewport.pending_fit = true;
                }
                for (label, preset) in [("Side", ViewPreset::Side), ("Front", ViewPreset::Front), ("Top", ViewPreset::Top), ("Iso", ViewPreset::Iso)] {
                    if ui.button(label).clicked() {
                        app.view.viewport.camera.preset(preset);
                        app.view.viewport.pending_fit = true;
                    }
                }
            });
            ui.add_space(8.0);
            projection_switch(app, ui);
        });
    });
    ui.add_space(2.0);
}

/// A piece as drawn this frame.
struct Drawn {
    id: PieceId,
    mesh_id: u64,
    mesh: Arc<Mesh>,
    model: Mat4,
}

fn explode_offset(c: Vec3, center: Vec3, f: f32) -> Vec3 {
    (c - center) * f * 0.6
}

pub fn rgba(c: Color32, a: f32) -> [f32; 4] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, a]
}

/// The camera's screen-up direction in world space.
/// Slow pulse between 0 and 1 (period 1.8 s) for highlighting.
pub fn pulse(time: f64) -> f32 {
    (0.5 - 0.5 * (time * std::f64::consts::TAU / 1.8).cos()) as f32
}

fn cam_up(cam: &crate::viewport::Camera) -> Vec3 {
    let v = cam.view();
    Vec3::new(v.x_axis.y, v.y_axis.y, v.z_axis.y)
}

/// Colour of painted no-cut zones (like a marker on the surface).
const PAINT: [f32; 3] = [0.62, 0.28, 0.95];

fn viewport(app: &mut S3dApp, ui: &mut Ui, p: &Palette) {
    let mut frame = SceneFrame {
        overhang_sin: app.doc.settings.split.overhang.to_radians().sin(),
        cap_color: rgba(p.accent, 1.0),
        support_color: [0.95, 0.18, 0.15, 1.0],
        bg: p.viewport_bg,
        ..Default::default()
    };
    let show_pieces = app.doc.split.is_some() && (matches!(app.step, Step::Split | Step::Features | Step::Export) || app.view.tab == Tab::Assembly);
    let mut drawn: Vec<Drawn> = Vec::new();
    let mut scene_bb = Aabb::EMPTY;
    let prepared_xf = app.doc.placement().unwrap_or(Mat4::IDENTITY);
    // Pick hits are converted to source coordinates for no-cut zones: the
    // unsplit, unhollowed model is drawn as the source mesh itself.
    let mut local_is_source = false;
    if let Some(split) = app.doc.split.clone() {
        app.view.prune_selection(&split.partition);
    }

    if let Some(split) = app.doc.split.clone().filter(|_| show_pieces) {
        let part = &split.partition;
        let center = part.pieces.iter().fold(Aabb::EMPTY, |b, q| b.union(&q.bbox)).center();
        let assembly = if app.view.tab == Tab::Assembly { app.view.assembly(&app.doc) } else { None };
        // Current step: the two parts being joined, in place, the one being
        // added highlighted; everything else faint for context.
        let step = assembly.as_ref().and_then(|a| a.steps.get(app.view.assembly_step.min(a.steps.len())).cloned());
        let numbers = app.view.show_numbers && app.view.tab == Tab::Model;
        // The joining part's outline pulses slowly in the assembly view.
        let outline = if assembly.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            let k = pulse(ui.input(|i| i.time));
            (rgba(p.accent, 0.3 + 0.7 * k), 1.5 + 2.0 * k)
        } else {
            (rgba(p.accent, 1.0), 2.0)
        };
        let ink = if p.dark { [0.06, 0.07, 0.09, 0.95] } else { [0.10, 0.11, 0.14, 0.95] };
        for q in &part.pieces {
            frame.meshes.push(MeshRef { id: split.mesh_id(q.rev), rev: q.rev, mesh: q.display_mesh().clone() });
        }
        for q in &part.pieces {
            let mut model = Mat4::IDENTITY;
            let mut flags = FLAG_SUPPORT;
            let color = piece_color(q.label, p.dark);
            let mut alpha = 1.0;
            if assembly.is_some() {
                flags = 0;
                if let Some(st) = &step {
                    if st.b.contains(&q.id) {
                        flags |= FLAG_SELECTED;
                    } else if !st.a.contains(&q.id) {
                        if !app.view.assembly_context {
                            continue;
                        }
                        flags |= FLAG_GHOST;
                        alpha = 0.08;
                    }
                }
            } else {
                if app.view.is_hidden(q.id) {
                    continue;
                }
                if app.view.explode > 0.0 {
                    model = Mat4::from_translation(explode_offset(q.centroid, center, app.view.explode));
                }
                if app.view.is_selected(q.id) {
                    flags |= FLAG_SELECTED;
                } else if app.view.hover == Some(q.id) {
                    flags |= FLAG_HOVER;
                }
            }
            let planes: Vec<Vec4> = if app.view.selected == Some(q.id) && app.view.multi.is_empty() { part.seams_of(q.id).map(|s| s.plane.as_vec4()).collect() } else { Vec::new() };
            let mesh_id = split.mesh_id(q.rev);
            frame.items.push(DrawItem {
                mesh: mesh_id,
                model,
                print: q.orient.map(|o| o.matrix()).unwrap_or(Mat4::IDENTITY),
                color: [color[0], color[1], color[2], alpha],
                highlight: if flags & FLAG_SELECTED != 0 { rgba(p.accent, 1.0) } else { [0.0; 4] },
                flags,
                planes,
            });
            if flags & FLAG_SELECTED != 0 {
                app.view.outline_edges(mesh_id, q.display_mesh(), model, outline.0, outline.1, &mut frame.lines);
            }
            if numbers && q.finished.is_some() {
                // Only numbers big enough on screen to read (tiny ones would
                // just be dark specks).
                let cam = &app.view.viewport.camera;
                let (aspect, h) = (app.view.viewport.aspect(), app.view.viewport.size().y);
                let readable = q.label_place.as_ref().is_some_and(|l| {
                    let c = model.transform_point3(l.center);
                    match (cam.project(c, aspect), cam.project(c + cam_up(cam) * l.size, aspect)) {
                        (Some(a), Some(b)) => (a - b).length() * 0.5 * h >= 7.0,
                        _ => false,
                    }
                });
                if let Some(place) = q.label_place.as_ref().filter(|_| readable) {
                    let segs = app.view.label_outline(mesh_id, place);
                    frame.lines.extend(segs.iter().map(|s| LineSeg::new(model.transform_point3(s[0]), model.transform_point3(s[1]), ink, 1.6)));
                }
            }
            let mut bb = q.bbox;
            let t = model.w_axis.truncate();
            bb.min += t;
            bb.max += t;
            scene_bb = scene_bb.union(&bb);
            drawn.push(Drawn { id: q.id, mesh_id, mesh: q.display_mesh().clone(), model });
        }
        let live: HashSet<u64> = drawn.iter().map(|d| d.mesh_id).collect();
        app.view.label_lines.retain(|k, _| live.contains(k));
        app.view.edge_lines.retain(|k, _| live.contains(k));
        if app.view.show_planes && app.view.tab == Tab::Model {
            for s in &part.seams {
                plane_quad(&mut frame, s.plane, s.center, (s.area.sqrt() as f32).max(10.0), [p.accent.r() as f32 / 255.0, p.accent.g() as f32 / 255.0, p.accent.b() as f32 / 255.0, 0.10]);
            }
        }
    } else if let Some((mesh, id, model)) = app.doc.display_model() {
        local_is_source = app.doc.valid_hollow().is_none();
        frame.meshes.push(MeshRef { id, rev: id, mesh: mesh.clone() });
        let c = if p.dark { [0.62, 0.68, 0.76] } else { [0.62, 0.67, 0.74] };
        frame.items.push(DrawItem { mesh: id, model, print: Mat4::IDENTITY, color: [c[0], c[1], c[2], 1.0], highlight: [0.0; 4], flags: 0, planes: vec![] });
        scene_bb = mesh.transformed_bbox(&model);
        drawn.push(Drawn { id: u32::MAX, mesh_id: id, mesh, model });
    }

    // Grid mode, before (re)splitting: the cut planes on the model.
    let grid_preview = app.step == Step::Split && app.view.tab == Tab::Model && app.doc.settings.split.mode == s3d_core::settings::SplitMode::Grid && (app.doc.split.is_none() || app.status(Step::Split) == crate::doc::Status::Stale);
    if grid_preview && !app.jobs.busy() {
        if let Some((mesh, _)) = app.doc.prepared_mesh() {
            let bb = mesh.bbox();
            let cuts = s3d_core::split::grid::cuts(&bb, &app.doc.settings);
            let pad = bb.size() * 0.04;
            for (a, list) in cuts.iter().enumerate() {
                for (i, &v) in list.iter().enumerate() {
                    let hot = app.view.grid_hover == Some((a, i));
                    let (u, w) = ((a + 1) % 3, (a + 2) % 3);
                    let corner = |su: f32, sw: f32| {
                        let mut c = Vec3::ZERO;
                        c[a] = v;
                        c[u] = if su < 0.0 { bb.min[u] - pad[u] } else { bb.max[u] + pad[u] };
                        c[w] = if sw < 0.0 { bb.min[w] - pad[w] } else { bb.max[w] + pad[w] };
                        c
                    };
                    let q = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
                    let fill = rgba(p.accent, if hot { 0.32 } else { 0.10 });
                    for t in [[0, 1, 2], [0, 2, 3]] {
                        frame.tris.extend(t.map(|k| TriVert { pos: q[k].into(), color: fill }));
                    }
                    let edge = rgba(p.accent, if hot { 1.0 } else { 0.6 });
                    for k in 0..4 {
                        frame.overlay_lines.push(LineSeg::new(q[k], q[(k + 1) % 4], edge, if hot { 2.5 } else { 1.2 }));
                    }
                }
            }
        }
    }

    // Split in progress: draw the current best layout's cut lines.
    let searching = app.jobs.current.as_ref().is_some_and(|r| r.kind == crate::jobs::JobKind::Split);
    if searching && !show_pieces {
        if let Some((edges, _)) = app.view.search_preview.lock().unwrap().clone() {
            let c = rgba(p.accent, 1.0);
            frame.lines.extend(edges.iter().map(|e| LineSeg::new(e[0], e[1], c, 1.6)));
        }
    }
    if scene_bb.is_empty() {
        scene_bb = Aabb { min: Vec3::splat(-100.0), max: Vec3::splat(100.0) };
    }
    let printer_bb = ground_and_printer(&mut frame, &scene_bb, &app.doc, p);
    let mut fit_bb = scene_bb.union(&printer_bb);
    // Assembly: frame the parts of the current step whenever it changes.
    if app.view.tab == Tab::Assembly {
        if let (Some(a), Some(split)) = (app.view.assembly(&app.doc), app.doc.split.clone()) {
            let k = app.view.assembly_step.min(a.steps.len());
            if app.view.assembly_framed != Some(k) {
                app.view.assembly_framed = Some(k);
                if let Some(st) = a.steps.get(k) {
                    let bb = st.a.iter().chain(&st.b).filter_map(|id| split.partition.piece(*id)).fold(Aabb::EMPTY, |b, q| b.union(&q.bbox));
                    if !bb.is_empty() {
                        fit_bb = Aabb { min: bb.min, max: bb.max + Vec3::splat(bb.size().max_element() * 0.15) };
                    }
                } else {
                    fit_bb = scene_bb;
                }
                app.view.viewport.pending_fit = true;
            }
        }
    }

    // Section plane.
    if app.view.section && app.view.tab == Tab::Model {
        let a = app.view.section_axis;
        let at = scene_bb.min[a] + (scene_bb.max[a] - scene_bb.min[a]) * app.view.section_pos;
        let mut n = Vec3::ZERO;
        n[a] = 1.0;
        frame.clip = Some(n.extend(at));
    }

    // Interaction.
    let painting = app.view.paint && app.step == Step::Split && app.view.tab == Tab::Model;
    let drawing_cut = app.view.cut_draw.is_some() && app.view.tab == Tab::Model && show_pieces;
    let tool_drag = painting || drawing_cut;
    // Picking structures are only needed (and built) while the pointer is
    // over the viewport or paint must be shown; caches keep only meshes
    // drawn now.
    let ctx = ui.ctx().clone();
    let live: HashSet<u64> = drawn.iter().map(|d| d.mesh_id).collect();
    app.view.bvh.retain(|k, _| live.contains(k));
    let show_paint = !app.doc.zones.is_empty() && app.step == Step::Split && app.view.tab == Tab::Model;
    let over = ctx.pointer_hover_pos().is_some_and(|pos| ui.available_rect_before_wrap().contains(pos)) || tool_drag;
    let bvhs: Vec<(PieceId, Arc<Bvh>, Mat4)> = if over || show_paint { drawn.iter().filter_map(|d| Some((d.id, app.view.bvh_for(d.mesh_id, &d.mesh, &ctx)?, d.model))).collect() } else { Vec::new() };
    // Painted zones, drawn as marker on the surface.
    if show_paint {
        let to_local = if local_is_source { Mat4::IDENTITY } else { prepared_xf };
        let key = fingerprint(&(&app.doc.zones, drawn.iter().map(|d| (d.mesh_id, d.model.to_cols_array().map(|x| x.to_bits()))).collect::<Vec<_>>(), to_local.to_cols_array().map(|x| x.to_bits())));
        let ready = bvhs.len() == drawn.len();
        if ready && app.view.paint_tris.as_ref().is_none_or(|c| c.0 != key) {
            let list: Vec<(Arc<Bvh>, Mat4)> = bvhs.iter().map(|b| (b.1.clone(), b.2)).collect();
            app.view.paint_tris = Some((key, Arc::new(paint_overlay(&app.doc.zones, &list, to_local, PAINT))));
        }
        if let Some((_, t)) = &app.view.paint_tris {
            frame.tris.extend_from_slice(t);
        }
    }
    let clip_plane = frame.clip;
    // Returns (piece, world hit, normal, hit in the mesh's own coordinates).
    let pick_full = |ray: (Vec3, Vec3)| -> Option<(PieceId, Vec3, Vec3, Vec3)> {
        let mut best: Option<(f32, PieceId, Vec3, Vec3, Vec3)> = None;
        for (id, b, model) in &bvhs {
            let inv = model.inverse();
            let o = inv.transform_point3(ray.0);
            let d = inv.transform_vector3(ray.1);
            if let Some(h) = b.ray(o, d, f32::INFINITY) {
                let local = o + d * h.t;
                let hit = model.transform_point3(local);
                if let Some(clip) = clip_plane {
                    if hit.dot(clip.truncate()) > clip.w {
                        continue;
                    }
                }
                let wt = (hit - ray.0).length();
                if best.is_none_or(|bb| wt < bb.0) {
                    best = Some((wt, *id, hit, model.transform_vector3(h.normal).normalize_or_zero(), local));
                }
            }
        }
        best.map(|b| (b.1, b.2, b.3, b.4))
    };
    let pick = |ray: (Vec3, Vec3)| pick_full(ray).map(|h| (h.0, h.1, h.2));
    if app.view.viewport.pending_fit || !app.view.fitted_once {
        let aspect = app.view.viewport.aspect();
        app.view.viewport.camera.fit(&fit_bb, aspect);
        app.view.viewport.pending_fit = false;
        app.view.fitted_once = true;
    }
    let pick_depth = |ray: (Vec3, Vec3)| pick(ray).map(|h| h.1);
    let input = app.view.viewport.interact(ui, tool_drag, &pick_depth);
    let rect = input.rect;
    let resp = input.response.clone().unwrap();
    app.view.hover = None;
    let mut hover_hit = None;
    if let Some(ray) = input.hover {
        if let Some((id, pt, n)) = pick(ray) {
            hover_hit = Some((id, pt, n));
            if id != u32::MAX {
                app.view.hover = Some(id);
            }
        }
    }

    // No-cut zone painting (on the unsplit model or pieces).
    if painting {
        if let Some(td) = input.tool_drag {
            if let Some((_, _, n, local)) = pick_full(td.ray) {
                // Undo explode/lift offsets (local), then map prepared -> source.
                let inv = if local_is_source { Mat4::IDENTITY } else { prepared_xf.inverse() };
                let src = inv.transform_point3(local);
                let src_n = inv.transform_vector3(n).normalize_or_zero();
                let scale = if local_is_source { 1.0 } else { prepared_xf.to_scale_rotation_translation().0.abs().max_element().max(1e-6) };
                let r = app.view.brush / scale;
                if app.view.erase {
                    app.doc.zones.erase(src, r);
                } else if app.doc.zones.dabs.last().is_none_or(|d| d.p.distance(src) > r * 0.3) {
                    app.doc.zones.dabs.push(Dab { p: src, radius: r, n: src_n });
                }
            }
        }
        if let Some((_, pt, n)) = hover_hit {
            circle(&mut frame.overlay_lines, pt, n, app.view.brush, if app.view.erase { [1.0, 1.0, 1.0, 0.9] } else { [PAINT[0], PAINT[1], PAINT[2], 0.95] }, 2.0);
        }
    }

    // Drawing a cut line across the selected piece.
    if drawing_cut {
        let target = app.view.cut_draw.as_ref().map(|c| c.piece).unwrap();
        let model = drawn.iter().find(|d| d.id == target).map(|d| d.model).unwrap_or(Mat4::IDENTITY);
        if let Some(td) = input.tool_drag {
            if td.started {
                if let Some(c) = &mut app.view.cut_draw {
                    c.pts.clear();
                    c.normals.clear();
                }
            }
            if let Some((_, _, n, local)) = pick_full(td.ray).filter(|h| h.0 == target) {
                if let Some(c) = &mut app.view.cut_draw {
                    if c.pts.last().is_none_or(|l| l.distance(local) > 0.8) {
                        c.pts.push(local);
                        c.normals.push(n);
                    }
                }
            }
            if td.stopped {
                let done = app.view.cut_draw.clone().filter(|c| {
                    let (lo, hi) = c.pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
                    c.pts.len() >= 2 && (hi - lo).length() > 4.0
                });
                if let Some(c) = done {
                    app.view.cut_draw = None;
                    cut_along_line(app, c);
                }
            }
        }
        if let Some(c) = &app.view.cut_draw {
            let col = [1.0, 0.85, 0.2, 1.0];
            for w in c.pts.windows(2) {
                frame.overlay_lines.push(LineSeg::new(model.transform_point3(w[0]), model.transform_point3(w[1]), col, 3.0));
            }
            // The straight line it will be fitted to.
            if c.pts.len() >= 2 {
                let (a, b) = (c.pts[0], *c.pts.last().unwrap());
                frame.overlay_lines.push(LineSeg::new(model.transform_point3(a), model.transform_point3(b), [1.0, 1.0, 1.0, 0.5], 1.5));
            }
        }
        if let Some((_, pt, n)) = hover_hit.filter(|h| h.0 == target) {
            circle(&mut frame.overlay_lines, pt, n, 2.5, [1.0, 0.85, 0.2, 1.0], 2.0);
        }
    }

    // Clicks: select (Shift adds to the selection), measure, context menu.
    let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    if let Some(ray) = input.click {
        let hit = pick(ray);
        if app.view.measure {
            if let Some((_, pt, _)) = hit {
                if app.view.measure_pts.len() >= 2 {
                    app.view.measure_pts.clear();
                }
                app.view.measure_pts.push(pt);
            }
        } else if !painting && !drawing_cut && app.view.tab == Tab::Model {
            match hit.map(|h| h.0).filter(|id| *id != u32::MAX) {
                Some(id) if shift => app.view.toggle_select(id),
                Some(id) => app.view.select(Some(id)),
                None if !shift => app.view.select(None),
                None => {}
            }
        }
    }
    if let Some(ray) = input.right_click {
        if let Some((id, _, _)) = pick(ray) {
            if id != u32::MAX && !app.view.is_selected(id) {
                app.view.select(Some(id));
            }
        }
    }
    if app.view.selected.is_some() && app.doc.split.is_some() && app.view.tab == Tab::Model && show_pieces {
        resp.context_menu(|ui| parts::context_menu(app, ui));
    }
    // Measure line.
    if app.view.measure_pts.len() == 2 {
        let (a, b) = (app.view.measure_pts[0], app.view.measure_pts[1]);
        frame.overlay_lines.push(LineSeg::new(a, b, [1.0, 0.85, 0.2, 1.0], 2.5));
    }

    app.view.viewport.paint(ui, rect, frame);
    overlays(app, ui, rect, p);
}

/// Cuts a piece along a line the user drew: the stroke is fitted with a
/// straight line and the cut angle chosen for printing (in the background).
pub fn cut_along_line(app: &mut S3dApp, c: CutDraw) {
    let zones = app.doc.placement().map(|xf| app.doc.zones.transformed(&xf)).unwrap_or_default();
    let Some(split) = &app.doc.split else { return };
    let settings = app.doc.settings.clone();
    let part = split.partition.clone();
    app.start_edit_with(move |job| {
        job.set_stage("Fitting the cut to the line");
        let q = s3d_core::split::quality::QualityParams::from_settings(&settings).with_zones(&zones);
        let plane = s3d_core::split::refine::cut_along(&part, c.piece, &c.pts, &c.normals, &q, job).ok_or_else(|| "the line does not cross the piece".to_string())?;
        Ok(EditOp::Cut(c.piece, plane))
    });
}

fn circle(out: &mut Vec<LineSeg>, c: Vec3, n: Vec3, r: f32, color: [f32; 4], w: f32) {
    let n = n.normalize_or(Vec3::Z);
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    let k = 28;
    for i in 0..k {
        let a0 = i as f32 / k as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / k as f32 * std::f32::consts::TAU;
        out.push(LineSeg::new(c + (u * a0.cos() + v * a0.sin()) * r + n * 0.2, c + (u * a1.cos() + v * a1.sin()) * r + n * 0.2, color, w));
    }
}

fn plane_quad(frame: &mut SceneFrame, pl: Plane, c: Vec3, size: f32, color: [f32; 4]) {
    let (u, v) = pl.basis();
    let (u, v) = (u.as_vec3() * size * 0.5, v.as_vec3() * size * 0.5);
    let c = pl.project(c.as_dvec3()).as_vec3();
    let q = [c - u - v, c + u - v, c + u + v, c - u + v];
    for t in [[0, 1, 2], [0, 2, 3]] {
        for i in t {
            frame.tris.push(TriVert { pos: q[i].into(), color });
        }
    }
}
/// Ground grid under the model and a wireframe of the printer's build volume.
/// Returns the bounds of the build-volume box (included when fitting the view).
fn ground_and_printer(frame: &mut SceneFrame, bb: &Aabb, doc: &Document, p: &Palette) -> Aabb {
    let ext = bb.size().x.max(bb.size().y).max(200.0) * 0.75 + 50.0;
    let step = [10.0f32, 25.0, 50.0, 100.0, 250.0, 500.0].into_iter().find(|s| ext / s < 14.0).unwrap_or(1000.0);
    let n = (ext / step).ceil() as i32;
    let c = bb.center();
    let (cx, cy) = ((c.x / step).round() * step, (c.y / step).round() * step);
    let col = if p.dark { [1.0, 1.0, 1.0, 0.06] } else { [0.0, 0.0, 0.0, 0.08] };
    for i in -n..=n {
        let o = i as f32 * step;
        frame.lines.push(LineSeg::new(Vec3::new(cx + o, cy - n as f32 * step, 0.0), Vec3::new(cx + o, cy + n as f32 * step, 0.0), col, 1.0));
        frame.lines.push(LineSeg::new(Vec3::new(cx - n as f32 * step, cy + o, 0.0), Vec3::new(cx + n as f32 * step, cy + o, 0.0), col, 1.0));
    }
    // Build volume, placed beside the model for scale.
    let v = doc.settings.printer.volume;
    let o = Vec3::new(bb.max.x + v[0] * 0.25 + 20.0, bb.min.y, 0.0);
    let s = Vec3::new(v[0], v[1], v[2]);
    let corner = |i: usize| o + Vec3::new(if i & 1 != 0 { s.x } else { 0.0 }, if i & 2 != 0 { s.y } else { 0.0 }, if i & 4 != 0 { s.z } else { 0.0 });
    let pc = [p.accent.r() as f32 / 255.0, p.accent.g() as f32 / 255.0, p.accent.b() as f32 / 255.0, 0.55];
    for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
        frame.lines.push(LineSeg::new(corner(a), corner(b), pc, 1.5));
    }
    frame.tris.extend([0usize, 1, 3, 0, 3, 2].iter().map(|&i| TriVert { pos: (corner(i) + Vec3::Z * 0.1).into(), color: [pc[0], pc[1], pc[2], 0.10] }));
    Aabb { min: o, max: o + s }
}

/// 2D overlays drawn by egui on top of the 3D image: labels, measure text.
fn overlays(app: &mut S3dApp, ui: &mut Ui, rect: egui::Rect, p: &Palette) {
    let painter = ui.painter_at(rect);
    let font = egui::FontId::monospace(12.0);
    if app.view.measure_pts.len() == 2 {
        let (a, b) = (app.view.measure_pts[0], app.view.measure_pts[1]);
        if let Some(sp) = app.view.viewport.to_screen(rect, (a + b) * 0.5) {
            let d = a.distance(b);
            let txt = format!("{d:.1} mm  (Δx {:.1}, Δy {:.1}, Δz {:.1})", (b.x - a.x).abs(), (b.y - a.y).abs(), (b.z - a.z).abs());
            let g = painter.layout_no_wrap(txt, font.clone(), Color32::BLACK);
            let r = egui::Rect::from_center_size(sp + Vec2::new(0.0, -16.0), g.size() + Vec2::new(12.0, 6.0));
            painter.rect_filled(r, CornerRadius::same(4), Color32::from_rgb(0xFF, 0xD9, 0x33));
            painter.galley(r.min + Vec2::new(6.0, 3.0), g, Color32::BLACK);
        }
    } else if app.view.measure {
        painter.text(rect.left_top() + Vec2::new(12.0, 12.0), egui::Align2::LEFT_TOP, "Click two points to measure", font.clone(), p.text);
    }
    if app.view.paint && app.step == Step::Split {
        painter.text(rect.left_top() + Vec2::new(12.0, 12.0), egui::Align2::LEFT_TOP, "Painting no-cut zones · drag on the model · right-drag to pan", font.clone(), p.accent);
    }
    if let Some(c) = &app.view.cut_draw {
        let label = app.doc.split.as_ref().and_then(|s| s.partition.piece(c.piece)).map(|q| q.label).unwrap_or(0);
        painter.text(rect.left_top() + Vec2::new(12.0, 12.0), egui::Align2::LEFT_TOP, format!("Draw where piece #{label} should be cut · the line is straightened and the cut angled for printing · Esc to cancel"), font.clone(), p.accent);
    }
    if app.jobs.current.as_ref().is_some_and(|r| r.kind == crate::jobs::JobKind::Split) {
        if let Some((_, msg)) = app.view.search_preview.lock().unwrap().as_ref() {
            painter.text(rect.left_top() + Vec2::new(14.0, 12.0), egui::Align2::LEFT_TOP, format!("Best layout so far: {msg}"), theme::semibold(14.0), p.text_strong);
        }
    }
    // Labels on pieces for the assembly view and when exploded.
    if let Some(split) = &app.doc.split {
        if app.view.tab == Tab::Assembly {
            if let Some(a) = app.view.assembly(&app.doc) {
                let k = app.view.assembly_step.min(a.steps.len());
                let total = a.steps.len() + 1;
                let (msg, sub) = match a.steps.get(k) {
                    Some(st) => {
                        let what = |name: &str, ids: &[PieceId]| if ids.len() == 1 { name.to_string() } else { format!("{name} ({} pieces)", ids.len()) };
                        (
                            format!("Step {} of {total}: join {} and {} into section {}", k + 1, what(&st.a_name, &st.a), what(&st.b_name, &st.b), st.name),
                            format!("Seam about {:.0} mm · about {:.0} heat staples from the inside", st.length, (st.length / 40.0).ceil().max(1.0)),
                        )
                    }
                    None => (format!("Step {total} of {total}: assembled"), format!("{} pieces", split.partition.pieces.len())),
                };
                painter.text(rect.left_top() + Vec2::new(14.0, 12.0), egui::Align2::LEFT_TOP, msg, theme::semibold(15.0), p.text_strong);
                painter.text(rect.left_top() + Vec2::new(14.0, 34.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(13.0), p.text_weak);
            }
        } else if matches!(app.step, Step::Split | Step::Features | Step::Export) {
            // Label pieces near the cursor's selection, and all of them when exploded.
            let center = split.partition.pieces.iter().fold(Aabb::EMPTY, |b, x| b.union(&x.bbox)).center();
            for q in &split.partition.pieces {
                if app.view.is_hidden(q.id) {
                    continue;
                }
                let many = split.partition.pieces.len() > 80;
                let show = (app.view.explode > 0.05 && !many) || app.view.is_selected(q.id) || app.view.hover == Some(q.id);
                if !show {
                    continue;
                }
                let c = q.centroid + explode_offset(q.centroid, center, app.view.explode);
                if let Some(sp) = app.view.viewport.to_screen(rect, c) {
                    let txt = format!("#{}", q.label);
                    let g = painter.layout_no_wrap(txt, theme::semibold(12.0), p.text_strong);
                    let r = egui::Rect::from_center_size(sp, g.size() + Vec2::new(10.0, 4.0));
                    painter.rect(r, CornerRadius::same(4), p.card.gamma_multiply(0.85), Stroke::new(1.0, p.card_stroke), egui::StrokeKind::Inside);
                    painter.galley(r.min + Vec2::new(5.0, 2.0), g, p.text_strong);
                }
            }
        }
    }
    painter.text(rect.left_bottom() + Vec2::new(10.0, -8.0), egui::Align2::LEFT_BOTTOM, "mm · Z up · drag to orbit · right-drag to pan · scroll to zoom", egui::FontId::monospace(11.0), p.text_weak);
}

// ---------------------------------------------------------------------------
// Automatic cuts for "Split in two" / "Find a better cut".

/// Reworks a piece and its surroundings: its shared cuts moved, then turned,
/// then the piece split (see `refine::improve_piece`).
pub fn find_better_cut(app: &mut S3dApp, id: PieceId) {
    let zones = app.doc.placement().map(|xf| app.doc.zones.transformed(&xf)).unwrap_or_default();
    let Some(split) = &app.doc.split else { return };
    let settings = app.doc.settings.clone();
    let part = split.partition.clone();
    app.start_edit_with(move |job| {
        job.set_stage("Trying cuts nearby, then other angles, then extra cuts");
        let q = s3d_core::split::quality::QualityParams::from_settings(&settings).with_zones(&zones);
        let i = s3d_core::split::refine::improve_piece(&part, id, &q, job).ok_or("no better cut found: moving or turning its cuts and splitting it all made things worse")?;
        Ok(EditOp::Replace { old: i.old, new: i.new, planes: i.planes })
    });
}

/// Adds the single cut through a piece that prints best.
pub fn split_piece_auto(app: &mut S3dApp, id: PieceId) {
    let zones = app.doc.placement().map(|xf| app.doc.zones.transformed(&xf)).unwrap_or_default();
    let Some(split) = &app.doc.split else { return };
    let settings = app.doc.settings.clone();
    let part = split.partition.clone();
    app.start_edit_with(move |job| {
        job.set_stage("Searching for the best cut");
        let q = s3d_core::split::quality::QualityParams::from_settings(&settings).with_zones(&zones);
        let (plane, _) = s3d_core::split::refine::best_split(&part, id, &q, &zones, job).ok_or("this piece cannot be cut")?;
        Ok(EditOp::Cut(id, plane))
    });
}
