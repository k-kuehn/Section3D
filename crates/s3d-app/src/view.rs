//! Right side: view tabs (3D view, Beds, Assembly), toolbar and viewport.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2, pos2};
use glam::{DVec3, Mat4, Vec3, Vec4};
use s3d_core::assembly::Assembly;
use s3d_core::bvh::Bvh;
use s3d_core::edits::{EditOp, PieceRef, SeamRef};
use s3d_core::job::JobCtx;
use s3d_core::mesh::{Aabb, Mesh};
use s3d_core::pack::{Layout, PackItem, PackParams};
use s3d_core::partition::{Orientation, Partition, PieceId, SeamId};
use s3d_core::plane::Plane;
use s3d_core::settings::fingerprint;
use s3d_core::zones::Dab;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingSeam {
    pub seam: SeamId,
    pub offset: f32,
    pub tilt: f32,
}

pub struct Preview {
    pub pending: PendingSeam,
    pub pieces: Vec<(Arc<Mesh>, Option<Orientation>, u64)>,
    pub replaced: Vec<PieceId>,
    pub support: f32,
}

#[derive(Clone, Copy)]
enum HandleKind {
    Move,
    Tilt,
}

#[derive(Clone, Copy)]
struct HandleDrag {
    seam: SeamId,
    kind: HandleKind,
    start: PendingSeam,
    accum: Vec2,
}

pub struct ViewState {
    pub viewport: Viewport,
    pub tab: Tab,
    pub selected: Option<PieceId>,
    pub hover: Option<PieceId>,
    pub hover_seam: Option<SeamId>,
    pub hidden: HashSet<PieceId>,
    pub isolate: Option<Vec<PieceId>>,
    pub show_planes: bool,
    pub section: bool,
    pub section_axis: usize,
    pub section_pos: f32,
    pub measure: bool,
    measure_pts: Vec<Vec3>,
    pub explode: f32,
    pub assembly_step: usize,
    pub paint: bool,
    pub brush: f32,
    pub erase: bool,
    pub pending: Option<PendingSeam>,
    pub preview: Option<Preview>,
    preview_rx: Option<(PendingSeam, JobCtx, Receiver<Preview>)>,
    preview_due: Option<(PendingSeam, Instant)>,
    pub beds: Layout,
    pub beds_key: Option<u64>,
    pub arrange_needed: bool,
    pub recovery: Option<std::path::PathBuf>,
    bvh: HashMap<u64, Arc<Bvh>>,
    assembly: Option<(u64, Arc<Assembly>)>,
    drag: Option<HandleDrag>,
    bed_drag: Option<(PieceId, egui::Vec2)>,
    footprints: HashMap<u64, Arc<Vec<glam::Vec2>>>,
    fitted_once: bool,
    /// Handle screen positions from the last frame, for hit testing.
    handle_screen_positions: std::cell::RefCell<Vec<(SeamId, egui::Pos2, HandleKindTag, Vec3)>>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            viewport: Viewport::default(),
            tab: Tab::Model,
            selected: None,
            hover: None,
            hover_seam: None,
            hidden: HashSet::new(),
            isolate: None,
            show_planes: false,
            section: false,
            section_axis: 0,
            section_pos: 0.5,
            measure: false,
            measure_pts: Vec::new(),
            explode: 0.0,
            assembly_step: 0,
            paint: false,
            brush: 20.0,
            erase: false,
            pending: None,
            preview: None,
            preview_rx: None,
            preview_due: None,
            beds: Layout::default(),
            beds_key: None,
            arrange_needed: false,
            recovery: None,
            bvh: HashMap::new(),
            assembly: None,
            drag: None,
            bed_drag: None,
            footprints: HashMap::new(),
            fitted_once: false,
            handle_screen_positions: Default::default(),
        }
    }
}

/// Fingerprint of what the bed layout depends on.
pub fn arrange_key(doc: &Document) -> u64 {
    let e = &doc.settings.export;
    fingerprint(&(doc.split.as_ref().map(|s| (s.generation, s.partition.pieces.iter().map(|p| p.rev).collect::<Vec<_>>())), e.spacing, e.keep_assembly_order, &doc.settings.printer))
}

impl ViewState {
    pub fn select(&mut self, id: Option<PieceId>) {
        if self.selected != id {
            self.clear_pending();
        }
        self.selected = id;
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
        self.selected = None;
        self.hidden.clear();
        self.isolate = None;
        self.clear_pending();
        self.assembly = None;
        self.measure_pts.clear();
    }

    pub fn on_pieces_changed(&mut self, _changed: &[PieceId]) {
        self.clear_pending();
        self.assembly = None;
    }

    pub fn set_pending(&mut self, pd: PendingSeam) {
        if pd.offset.abs() < 1e-4 && pd.tilt.abs() < 1e-4 {
            self.clear_pending();
            return;
        }
        self.pending = Some(pd);
        self.preview_due = Some((pd, Instant::now()));
    }

    pub fn clear_pending(&mut self) {
        self.pending = None;
        self.preview = None;
        self.preview_due = None;
        if let Some((_, ctx, _)) = self.preview_rx.take() {
            ctx.cancel();
        }
    }

    /// The adjusted plane for the pending seam edit.
    pub fn pending_plane(&self, part: &Partition) -> Option<Plane> {
        let pd = self.pending?;
        let s = part.seam(pd.seam)?;
        Some(adjusted_plane(&s.plane, s.center, pd.offset, pd.tilt))
    }

    pub fn arrange(&mut self, doc: &Document) {
        let Some(split) = &doc.split else { return };
        let u = doc.settings.printer.usable();
        let assembly = s3d_core::assembly::plan(&split.partition);
        let order: Vec<PieceId> = if doc.settings.export.keep_assembly_order { assembly.order.clone() } else { split.partition.pieces.iter().map(|p| p.id).collect() };
        let items: Vec<PackItem> = order
            .iter()
            .filter_map(|id| split.partition.piece(*id))
            .map(|p| PackItem { id: p.id, mesh: p.display_mesh().clone(), print: p.orient.map(|o| o.matrix()).unwrap_or(Mat4::IDENTITY) })
            .collect();
        self.beds = s3d_core::pack::pack(&items, &PackParams { bed: [u[0], u[1]], spacing: doc.settings.export.spacing, keep_order: doc.settings.export.keep_assembly_order, angle_step: 15.0 });
        self.beds_key = Some(arrange_key(doc));
        self.arrange_needed = false;
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "beds": self.beds, "beds_key": self.beds_key })
    }

    pub fn restore(&mut self, doc: &Document) {
        if let Some(v) = &doc.extra_view {
            if let Ok(l) = serde_json::from_value::<Layout>(v["beds"].clone()) {
                self.beds = l;
            }
            // Keys refer to the previous session's generation numbers; re-key.
            if !self.beds.beds.is_empty() {
                self.beds_key = Some(arrange_key(doc));
            }
        }
    }

    fn bvh_for(&mut self, id: u64, mesh: &Arc<Mesh>) -> Arc<Bvh> {
        self.bvh.entry(id).or_insert_with(|| Arc::new(Bvh::new(mesh))).clone()
    }

    fn assembly(&mut self, doc: &Document) -> Option<Arc<Assembly>> {
        let s = doc.split.as_ref()?;
        let key = fingerprint(&(s.generation, s.partition.pieces.iter().map(|p| p.rev).collect::<Vec<_>>()));
        if self.assembly.as_ref().is_none_or(|a| a.0 != key) {
            self.assembly = Some((key, Arc::new(s3d_core::assembly::plan(&s.partition))));
        }
        self.assembly.as_ref().map(|a| a.1.clone())
    }

    fn poll_preview(&mut self, doc: &Document, ctx: &egui::Context) {
        if let Some((pd, _, rx)) = &self.preview_rx {
            if let Ok(p) = rx.try_recv() {
                if Some(*pd) == self.pending {
                    self.preview = Some(p);
                }
                self.preview_rx = None;
            }
        }
        if let Some((pd, at)) = self.preview_due {
            if at.elapsed().as_millis() < 250 || self.drag.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(60));
                return;
            }
            self.preview_due = None;
            let (Some(split), Some(plane)) = (doc.split.clone(), doc.split.as_ref().and_then(|s| self.pending_plane(&s.partition))) else { return };
            let Some(seam) = SeamRef::of(&split.partition, pd.seam) else { return };
            let settings = doc.settings.clone();
            let job = JobCtx::new();
            let (tx, rx) = channel();
            let jc = job.clone();
            let repaint = ctx.clone();
            let generation = split.generation;
            std::thread::spawn(move || {
                let mut part = split.partition.clone();
                let s = part.seam(pd.seam).cloned();
                let (st, changed) = s3d_core::edits::apply(&mut part, &EditOp::MoveSeam { seam, plane });
                if jc.is_cancelled() || st != s3d_core::edits::EditStatus::Applied {
                    return;
                }
                let params = s3d_core::split::orient_params(&settings);
                s3d_core::split::orient_pieces(&mut part, &changed, &params);
                let pieces: Vec<(Arc<Mesh>, Option<Orientation>, u64)> =
                    changed.iter().filter_map(|id| part.piece(*id)).map(|p| (p.mesh.clone(), p.orient, (generation << 32) ^ 0x8000_0000 ^ p.rev)).collect();
                let support = pieces.iter().filter_map(|p| p.1.map(|o| o.support_area)).fold(0.0, f32::max);
                let replaced = s.map(|s| vec![s.a, s.b]).unwrap_or_default();
                let _ = tx.send(Preview { pending: pd, pieces, replaced, support });
                repaint.request_repaint();
            });
            if let Some((_, old, _)) = self.preview_rx.replace((pd, job, rx)) {
                old.cancel();
            }
        }
    }
}

/// Moves a plane along its normal and tilts it about its most horizontal
/// in-plane axis through the seam centre.
pub fn adjusted_plane(base: &Plane, center: Vec3, offset: f32, tilt_deg: f32) -> Plane {
    let moved = base.moved(offset as f64);
    if tilt_deg.abs() < 1e-6 {
        return moved;
    }
    let pivot = moved.project(center.as_dvec3());
    moved.tilted(pivot, tilt_axis(&moved), (tilt_deg as f64).to_radians())
}

pub fn tilt_axis(pl: &Plane) -> DVec3 {
    let a = pl.normal.cross(DVec3::Z);
    if a.length() > 0.2 { a.normalize() } else { pl.normal.cross(DVec3::X).normalize() }
}

// ---------------------------------------------------------------------------

pub fn show(app: &mut S3dApp, ui: &mut Ui) {
    let p = app.palette();
    app.view.poll_preview(&app.doc, ui.ctx());
    if app.view.arrange_needed && app.step == Step::Export && !app.jobs.busy() && app.view.tab == Tab::Beds {
        app.view.arrange(&app.doc);
    }
    egui::Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.card_stroke)).corner_radius(CornerRadius::same(8)).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        tabs_row(app, ui, &p);
        ui.add_space(2.0);
        match app.view.tab {
            Tab::Model | Tab::Assembly => {
                toolbar(app, ui, &p);
                viewport(app, ui, &p);
            }
            Tab::Beds => beds_view(app, ui, &p),
        }
    });
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
                if tab == Tab::Beds && (app.view.beds.beds.is_empty() || app.view.beds_key != Some(arrange_key(&app.doc))) {
                    app.view.arrange(&app.doc);
                }
            }
        }
    });
    let r = ui.max_rect();
    ui.painter().hline(r.x_range(), ui.cursor().top(), Stroke::new(1.0, p.card_stroke));
}

fn toolbar(app: &mut S3dApp, ui: &mut Ui, p: &Palette) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        let is_assembly = app.view.tab == Tab::Assembly;
        let has_split = app.doc.split.is_some();
        let n = app.doc.split.as_ref().map(|s| s.partition.pieces.len()).unwrap_or(0);
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
        } else {
            ui.add_enabled_ui(has_split, |ui| {
                ui.toggle_value(&mut v.show_planes, "Cut planes").on_hover_text("Show the cut planes faintly");
                ui.label(widgets::weak(ui, "Explode"));
                ui.add(egui::Slider::new(&mut v.explode, 0.0..=1.0).show_value(false));
            });
            ui.toggle_value(&mut v.section, "Section").on_hover_text("Cut the view open along an axis");
            if v.section {
                let r = egui::ComboBox::from_id_salt("secaxis").width(44.0).selected_text(["X", "Y", "Z"][v.section_axis]).show_ui(ui, |ui| {
                    for (i, a) in ["X", "Y", "Z"].iter().enumerate() {
                        ui.selectable_value(&mut v.section_axis, i, *a);
                    }
                });
                r.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Section axis"));
                ui.add(egui::Slider::new(&mut v.section_pos, 0.0..=1.0).show_value(false));
            }
            if ui.toggle_value(&mut v.measure, "Measure").on_hover_text("Click two points to measure").changed() {
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

fn rgba(c: Color32, a: f32) -> [f32; 4] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, a]
}

fn viewport(app: &mut S3dApp, ui: &mut Ui, p: &Palette) {
    let mut frame = SceneFrame {
        overhang_sin: app.doc.settings.split.overhang.to_radians().sin(),
        cap_color: rgba(p.accent, 1.0),
        support_color: [0.95, 0.18, 0.15, 1.0],
        bg: p.viewport_bg,
        ..Default::default()
    };
    let show_pieces = app.doc.split.is_some() && matches!(app.step, Step::Split | Step::Connectors | Step::Export) || app.view.tab == Tab::Assembly;
    let mut drawn: Vec<Drawn> = Vec::new();
    let mut scene_bb = Aabb::EMPTY;
    let prepared = app.doc.prepared_mesh();
    let prepared_id = app.doc.prepared_id();
    let prepared_xf = prepared.as_ref().map(|x| x.1).unwrap_or(Mat4::IDENTITY);

    if show_pieces {
        let split = app.doc.split.clone().unwrap();
        let part = &split.partition;
        let center = part.pieces.iter().fold(Aabb::EMPTY, |b, q| b.union(&q.bbox)).center();
        let assembly = if app.view.tab == Tab::Assembly { app.view.assembly(&app.doc) } else { None };
        let preview_replaced: Vec<PieceId> = app.view.preview.as_ref().filter(|pv| Some(pv.pending) == app.view.pending).map(|pv| pv.replaced.clone()).unwrap_or_default();
        for q in &part.pieces {
            frame.meshes.push(MeshRef { id: split.mesh_id(q.rev), rev: q.rev, mesh: q.display_mesh().clone() });
        }
        for q in &part.pieces {
            let mut model = Mat4::IDENTITY;
            let mut flags = FLAG_SUPPORT;
            let mut color = piece_color(q.label, p.dark);
            let mut alpha = 1.0;
            if let Some(a) = &assembly {
                let idx = a.order.iter().position(|x| *x == q.id).unwrap_or(0);
                let step = app.view.assembly_step.min(a.order.len().saturating_sub(1));
                if idx > step {
                    continue;
                }
                flags = 0;
                if idx == step {
                    // Current piece: lifted back along its insertion path.
                    let dir = a.insert_dir.get(&q.id).copied().unwrap_or(-Vec3::Z);
                    model = Mat4::from_translation(-dir * q.bbox.size().max_element() * 0.35);
                    flags |= FLAG_SELECTED;
                } else {
                    color = [color[0] * 0.75, color[1] * 0.75, color[2] * 0.75];
                }
            } else {
                if app.view.is_hidden(q.id) {
                    continue;
                }
                if app.view.explode > 0.0 {
                    model = Mat4::from_translation(explode_offset(q.centroid, center, app.view.explode));
                }
                if app.view.selected == Some(q.id) {
                    flags |= FLAG_SELECTED;
                } else if app.view.hover == Some(q.id) {
                    flags |= FLAG_HOVER;
                }
                if preview_replaced.contains(&q.id) {
                    flags |= FLAG_GHOST;
                    alpha = 0.25;
                }
            }
            let planes: Vec<Vec4> = if app.view.selected == Some(q.id) { part.seams_of(q.id).map(|s| s.plane.as_vec4()).collect() } else { Vec::new() };
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
            let mut bb = q.bbox;
            let t = model.w_axis.truncate();
            bb.min += t;
            bb.max += t;
            scene_bb = scene_bb.union(&bb);
            drawn.push(Drawn { id: q.id, mesh_id, mesh: q.display_mesh().clone(), model });
        }
        // Seam adjustment preview.
        if let Some(pv) = app.view.preview.as_ref().filter(|pv| Some(pv.pending) == app.view.pending) {
            for (i, (m, o, id)) in pv.pieces.iter().enumerate() {
                frame.meshes.push(MeshRef { id: *id, rev: *id, mesh: m.clone() });
                let c = if i % 2 == 0 { [0.95, 0.75, 0.55] } else { [0.6, 0.78, 0.95] };
                frame.items.push(DrawItem { mesh: *id, model: Mat4::IDENTITY, print: o.map(|o| o.matrix()).unwrap_or(Mat4::IDENTITY), color: [c[0], c[1], c[2], 1.0], highlight: [0.0; 4], flags: FLAG_SUPPORT, planes: vec![] });
            }
        }
        if app.view.show_planes && app.view.tab == Tab::Model {
            for s in &part.seams {
                plane_quad(&mut frame, s.plane, s.center, (s.area.sqrt() as f32).max(10.0), [p.accent.r() as f32 / 255.0, p.accent.g() as f32 / 255.0, p.accent.b() as f32 / 255.0, 0.10]);
            }
        }
    } else if let Some((mesh, _)) = prepared.clone() {
        frame.meshes.push(MeshRef { id: prepared_id, rev: prepared_id, mesh: mesh.clone() });
        let c = if p.dark { [0.62, 0.68, 0.76] } else { [0.62, 0.67, 0.74] };
        frame.items.push(DrawItem { mesh: prepared_id, model: Mat4::IDENTITY, print: Mat4::IDENTITY, color: [c[0], c[1], c[2], 1.0], highlight: [0.0; 4], flags: 0, planes: vec![] });
        scene_bb = mesh.bbox();
        drawn.push(Drawn { id: u32::MAX, mesh_id: prepared_id, mesh, model: Mat4::IDENTITY });
    }

    if scene_bb.is_empty() {
        scene_bb = Aabb { min: Vec3::splat(-100.0), max: Vec3::splat(100.0) };
    }
    ground_and_printer(&mut frame, &scene_bb, &app.doc, p);

    // Section plane.
    if app.view.section && app.view.tab == Tab::Model {
        let a = app.view.section_axis;
        let at = scene_bb.min[a] + (scene_bb.max[a] - scene_bb.min[a]) * app.view.section_pos;
        let mut n = Vec3::ZERO;
        n[a] = 1.0;
        frame.clip = Some(n.extend(at));
    }

    // Interaction.
    let tool_drag = app.view.paint || app.view.drag.is_some() || handle_under_pointer(app, ui);
    let pick_drawn: Vec<(PieceId, u64, Arc<Mesh>, Mat4)> = drawn.iter().map(|d| (d.id, d.mesh_id, d.mesh.clone(), d.model)).collect();
    let bvhs: Vec<(PieceId, Arc<Bvh>, Mat4)> = pick_drawn.iter().map(|(id, mid, m, model)| (*id, app.view.bvh_for(*mid, m), *model)).collect();
    let clip_plane = frame.clip;
    let pick = |ray: (Vec3, Vec3)| -> Option<(PieceId, Vec3, Vec3)> {
        let mut best: Option<(f32, PieceId, Vec3, Vec3)> = None;
        for (id, b, model) in &bvhs {
            let inv = model.inverse();
            let o = inv.transform_point3(ray.0);
            let d = inv.transform_vector3(ray.1);
            if let Some(h) = b.ray(o, d, f32::INFINITY) {
                let hit = model.transform_point3(o + d * h.t);
                if let Some(clip) = clip_plane {
                    if hit.dot(clip.truncate()) > clip.w {
                        continue;
                    }
                }
                if best.is_none_or(|bb| h.t < bb.0) {
                    best = Some((h.t, *id, hit, h.normal.normalize_or_zero()));
                }
            }
        }
        best.map(|b| (b.1, b.2, b.3))
    };
    if app.view.viewport.pending_fit || !app.view.fitted_once {
        let aspect = app.view.viewport.aspect();
        app.view.viewport.camera.fit(&scene_bb, aspect);
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
            hover_hit = Some((pt, n));
            if id != u32::MAX {
                app.view.hover = Some(id);
            }
        }
    }

    // No-cut zone painting (on the unsplit model or pieces).
    if app.view.paint && app.step == Step::Split {
        if let Some(td) = input.tool_drag {
            if let Some((_, pt, _)) = pick(td.ray) {
                let inv = prepared_xf.inverse();
                let src = inv.transform_point3(pt);
                let scale = prepared_xf.to_scale_rotation_translation().0.abs().max_element().max(1e-6);
                let r = app.view.brush / scale;
                if app.view.erase {
                    app.doc.zones.erase(src, r);
                } else if app.doc.zones.dabs.last().is_none_or(|d| d.p.distance(src) > r * 0.4) {
                    app.doc.zones.dabs.push(Dab { p: src, radius: r });
                }
            }
        }
        if let Some((pt, n)) = hover_hit {
            circle(&mut frame.overlay_lines, pt, n, app.view.brush, if app.view.erase { [1.0, 1.0, 1.0, 0.9] } else { rgba(p.accent, 0.95) }, 2.0);
        }
    }
    if !app.doc.zones.is_empty() && app.step == Step::Split {
        for d in &app.doc.zones.dabs {
            let c = prepared_xf.transform_point3(d.p);
            let scale = prepared_xf.to_scale_rotation_translation().0.abs().max_element();
            let n = (c - scene_bb.center()).normalize_or(Vec3::Z);
            circle(&mut frame.lines, c, n, d.radius * scale, [0.25, 0.55, 1.0, 0.75], 2.0);
        }
    }

    // Seam handles for the selected piece.
    if app.view.tab == Tab::Model {
        seam_handles(app, &mut frame, &input, rect, p);
    }

    // Clicks: select, measure, context menu.
    if let Some(ray) = input.click {
        let hit = pick(ray);
        if app.view.measure {
            if let Some((_, pt, _)) = hit {
                if app.view.measure_pts.len() >= 2 {
                    app.view.measure_pts.clear();
                }
                app.view.measure_pts.push(pt);
            }
        } else if !app.view.paint && app.view.tab == Tab::Model {
            app.view.select(hit.map(|h| h.0).filter(|id| *id != u32::MAX));
        }
    }
    if let Some(ray) = input.right_click {
        if let Some((id, _, _)) = pick(ray) {
            if id != u32::MAX {
                app.view.select(Some(id));
            }
        }
    }
    if let Some(id) = app.view.selected.filter(|_| app.doc.split.is_some() && app.view.tab == Tab::Model) {
        resp.context_menu(|ui| parts::context_menu(app, ui, id));
    }
    // Measure line.
    if app.view.measure_pts.len() == 2 {
        let (a, b) = (app.view.measure_pts[0], app.view.measure_pts[1]);
        frame.overlay_lines.push(LineSeg::new(a, b, [1.0, 0.85, 0.2, 1.0], 2.5));
    }

    app.view.viewport.paint(ui, rect, frame);
    overlays(app, ui, rect, p);
}

fn handle_under_pointer(app: &S3dApp, ui: &Ui) -> bool {
    let Some(pos) = ui.ctx().pointer_hover_pos() else { return false };
    let handles = app.view.handle_screen_positions.borrow();
    handles.iter().any(|h| h.1.distance(pos) < 14.0)
}

fn seam_handles(app: &mut S3dApp, frame: &mut SceneFrame, input: &crate::viewport::ViewportInput, rect: egui::Rect, p: &Palette) {
    app.view.handle_screen_positions.borrow_mut().clear();
    let (Some(id), Some(split)) = (app.view.selected, app.doc.split.clone()) else { return };
    let part = &split.partition;
    let Some(q) = part.piece(id) else { return };
    if q.locked {
        return;
    }
    let center = part.pieces.iter().fold(Aabb::EMPTY, |b, x| b.union(&x.bbox)).center();
    let off = if app.view.explode > 0.0 { explode_offset(q.centroid, center, app.view.explode) } else { Vec3::ZERO };
    let accent = rgba(p.accent, 1.0);
    let seams: Vec<_> = part.seams_of(id).cloned().collect();
    for s in &seams {
        // Point the arrow out of the selected piece.
        let out = if s.a == id { -s.plane.normal } else { s.plane.normal }.as_vec3();
        let pd = app.view.pending.filter(|pd| pd.seam == s.id).unwrap_or(PendingSeam { seam: s.id, offset: 0.0, tilt: 0.0 });
        let pl = adjusted_plane(&s.plane, s.center, pd.offset, pd.tilt);
        let c = pl.project(s.center.as_dvec3()).as_vec3() + off;
        let len = (s.area.sqrt() as f32 * 0.3).clamp(15.0, 150.0);
        let n = pl.normal.as_vec3() * out.dot(s.plane.normal.as_vec3()).signum();
        let tip = c + n * len;
        let hovered_seam = app.view.hover_seam == Some(s.id);
        let col = if hovered_seam { [1.0, 1.0, 1.0, 1.0] } else { accent };
        frame.overlay_lines.push(LineSeg::new(c, tip, col, 3.0));
        // Arrow head.
        let side = tilt_axis(&pl).as_vec3();
        frame.overlay_lines.push(LineSeg::new(tip, tip - n * len * 0.18 + side * len * 0.1, col, 3.0));
        frame.overlay_lines.push(LineSeg::new(tip, tip - n * len * 0.18 - side * len * 0.1, col, 3.0));
        // Tilt knob: an arc in the plane containing the normal, around the tilt axis.
        let u = side.cross(n).normalize_or_zero();
        let knob = c + u * len * 0.8;
        let mut prev = None;
        for i in 0..=16 {
            let a = (i as f32 / 16.0 - 0.5) * 1.2;
            let pt = c + (u * a.cos() + n * a.sin()) * len * 0.8;
            if let Some(pp) = prev {
                frame.overlay_lines.push(LineSeg::new(pp, pt, [0.4, 0.75, 1.0, 1.0], 2.5));
            }
            prev = Some(pt);
        }
        // Outline of the pending plane.
        if app.view.pending.is_some_and(|x| x.seam == s.id) {
            plane_outline(frame, pl, c, (s.area.sqrt() as f32).max(20.0), [1.0, 0.85, 0.2, 1.0]);
        }
        if let (Some(sc), Some(sk)) = (app.view.viewport.to_screen(rect, tip), app.view.viewport.to_screen(rect, knob)) {
            let mut hs = app.view.handle_screen_positions.borrow_mut();
            hs.push((s.id, sc, HandleKindTag::Move, n));
            hs.push((s.id, sk, HandleKindTag::Tilt, n));
        }
    }
    // Drag handling.
    let pos = input.response.as_ref().and_then(|r| r.interact_pointer_pos().or(r.hover_pos()));
    if let Some(td) = input.tool_drag {
        if app.view.drag.is_none() && td.started {
            if let Some(pos) = pos {
                let hs = app.view.handle_screen_positions.borrow().clone();
                if let Some(h) = hs.iter().min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos))).filter(|h| h.1.distance(pos) < 16.0) {
                    let start = app.view.pending.filter(|pd| pd.seam == h.0).unwrap_or(PendingSeam { seam: h.0, offset: 0.0, tilt: 0.0 });
                    app.view.drag = Some(HandleDrag { seam: h.0, kind: if matches!(h.2, HandleKindTag::Move) { HandleKind::Move } else { HandleKind::Tilt }, start, accum: Vec2::ZERO });
                }
            }
        }
        if let Some(mut d) = app.view.drag {
            d.accum += Vec2::new(td.delta_px.x, td.delta_px.y);
            app.view.drag = Some(d);
            if let Some(s) = part.seam(d.seam) {
                // Screen direction of the normal decides drag sign and scale.
                let c = s.center + off;
                let n = s.plane.normal.as_vec3() * if s.a == id { -1.0 } else { 1.0 };
                let len = (s.area.sqrt() as f32 * 0.3).clamp(15.0, 150.0);
                if let (Some(a), Some(b)) = (app.view.viewport.to_screen(rect, c), app.view.viewport.to_screen(rect, c + n * len)) {
                    let sd = b - a;
                    let px_per_mm = sd.length() / len;
                    let along = d.accum.dot(sd.normalized());
                    let mut pd = d.start;
                    let sign = if s.a == id { -1.0 } else { 1.0 };
                    match d.kind {
                        HandleKind::Move => pd.offset = (d.start.offset + sign * along / px_per_mm.max(1e-3)).round(),
                        HandleKind::Tilt => pd.tilt = (d.start.tilt + along * 0.25).clamp(-60.0, 60.0).round(),
                    }
                    app.view.set_pending(pd);
                }
            }
        }
        if td.stopped {
            app.view.drag = None;
        }
    }
}

#[derive(Clone, Copy)]
pub enum HandleKindTag {
    Move,
    Tilt,
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

fn plane_outline(frame: &mut SceneFrame, pl: Plane, c: Vec3, size: f32, color: [f32; 4]) {
    let (u, v) = pl.basis();
    let (u, v) = (u.as_vec3() * size * 0.6, v.as_vec3() * size * 0.6);
    let q = [c - u - v, c + u - v, c + u + v, c - u + v];
    for i in 0..4 {
        frame.overlay_lines.push(LineSeg::new(q[i], q[(i + 1) % 4], color, 1.5));
    }
    plane_quad(frame, pl, c, size * 1.2, [color[0], color[1], color[2], 0.12]);
}

/// Ground grid under the model and a wireframe of the printer's build volume.
fn ground_and_printer(frame: &mut SceneFrame, bb: &Aabb, doc: &Document, p: &Palette) {
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
    let v = doc.settings.printer.printer.volume;
    let o = Vec3::new(bb.max.x + v[0] * 0.25 + 20.0, bb.min.y, 0.0);
    let s = Vec3::new(v[0], v[1], v[2]);
    let corner = |i: usize| o + Vec3::new(if i & 1 != 0 { s.x } else { 0.0 }, if i & 2 != 0 { s.y } else { 0.0 }, if i & 4 != 0 { s.z } else { 0.0 });
    let pc = [p.accent.r() as f32 / 255.0, p.accent.g() as f32 / 255.0, p.accent.b() as f32 / 255.0, 0.55];
    for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
        frame.lines.push(LineSeg::new(corner(a), corner(b), pc, 1.5));
    }
    frame.tris.extend([0usize, 1, 3, 0, 3, 2].iter().map(|&i| TriVert { pos: (corner(i) + Vec3::Z * 0.1).into(), color: [pc[0], pc[1], pc[2], 0.10] }));
}

/// 2D overlays drawn by egui on top of the 3D image: labels, measure text.
fn overlays(app: &mut S3dApp, ui: &mut Ui, rect: egui::Rect, p: &Palette) {
    let painter = ui.painter_at(rect);
    // Printer label next to its build volume.
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
    // Labels on pieces for the assembly view and when exploded.
    if let Some(split) = &app.doc.split {
        if app.view.tab == Tab::Assembly {
            if let Some(a) = app.view.assembly(&app.doc) {
                let step = app.view.assembly_step.min(a.order.len().saturating_sub(1));
                if let Some(q) = a.order.get(step).and_then(|id| split.partition.piece(*id)) {
                    let msg = format!("Step {} of {}: add piece #{}", step + 1, a.order.len(), q.label);
                    painter.text(rect.left_top() + Vec2::new(14.0, 12.0), egui::Align2::LEFT_TOP, msg, theme::semibold(15.0), p.text_strong);
                    if a.blocked.contains(&q.id) {
                        painter.text(rect.left_top() + Vec2::new(14.0, 34.0), egui::Align2::LEFT_TOP, "No straight path: insert before its neighbours or glue without pins", egui::FontId::proportional(13.0), p.warn);
                    }
                }
            }
        } else if matches!(app.step, Step::Split | Step::Connectors | Step::Export) {
            // Label pieces near the cursor's selection, and all of them when exploded.
            let center = split.partition.pieces.iter().fold(Aabb::EMPTY, |b, x| b.union(&x.bbox)).center();
            for q in &split.partition.pieces {
                if app.view.is_hidden(q.id) {
                    continue;
                }
                let show = app.view.explode > 0.05 || app.view.selected == Some(q.id) || app.view.hover == Some(q.id);
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
// Beds view

fn footprint(view: &mut ViewState, key: u64, mesh: &Mesh, print: Mat4) -> Arc<Vec<glam::Vec2>> {
    view.footprints
        .entry(key)
        .or_insert_with(|| {
            let pts: Vec<glam::DVec2> = mesh.verts.iter().map(|v| {
                let q = print.transform_point3(*v);
                glam::DVec2::new(q.x as f64, q.y as f64)
            }).collect();
            Arc::new(hull(pts).into_iter().map(|p| p.as_vec2()).collect())
        })
        .clone()
}

fn hull(mut pts: Vec<glam::DVec2>) -> Vec<glam::DVec2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut h: Vec<glam::DVec2> = Vec::new();
    for pass in 0..2 {
        let start = h.len();
        let seq: Vec<glam::DVec2> = if pass == 0 { pts.clone() } else { pts.iter().rev().cloned().collect() };
        for p in seq {
            while h.len() >= start + 2 && (h[h.len() - 1] - h[h.len() - 2]).perp_dot(p - h[h.len() - 2]) <= 0.0 {
                h.pop();
            }
            h.push(p);
        }
        h.pop();
    }
    h
}

fn beds_view(app: &mut S3dApp, ui: &mut Ui, p: &Palette) {
    let Some(split) = app.doc.split.clone() else { return };
    if app.view.beds.beds.is_empty() {
        ui.centered_and_justified(|ui| ui.label(widgets::weak(ui, "Press Arrange beds in the Export step.")));
        return;
    }
    let stale = app.view.beds_key != Some(arrange_key(&app.doc));
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(format!("{} beds", app.view.beds.beds.len())).font(theme::semibold(14.0)).color(p.text_strong));
        ui.label(widgets::weak(ui, "· drag pieces to move them, right-click to rotate 90°"));
        if stale {
            ui.label(RichText::new("· out of date").color(p.warn));
            if ui.small_button("Arrange again").clicked() {
                app.view.arrange(&app.doc);
            }
        }
    });
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

// ---------------------------------------------------------------------------
// Automatic cuts for "Split in two" / "Find a better cut".

pub fn find_better_cut(app: &mut S3dApp, id: PieceId) {
    split_with_best_cut(app, id);
}

pub fn split_piece_auto(app: &mut S3dApp, id: PieceId) {
    split_with_best_cut(app, id);
}

fn split_with_best_cut(app: &mut S3dApp, id: PieceId) {
    let Some(split) = &app.doc.split else { return };
    let Some(r) = PieceRef::of(&split.partition, id) else { return };
    let settings = app.doc.settings.clone();
    let part = split.partition.clone();
    app.start_edit_with(move |job| {
        job.set_stage("Searching for a better cut");
        let params = s3d_core::split::orient_params(&settings);
        let plane = s3d_core::split::fix::best_cut(&part, id, &params, job).ok_or_else(|| "no cut improves this piece".to_string())?;
        Ok(EditOp::Cut { piece: r, plane })
    });
}
