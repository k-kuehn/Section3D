//! Application shell: top bar, step panel, view area, status bar, jobs.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align, Color32, CornerRadius, Layout, RichText, Stroke, Ui, Vec2};
use s3d_core::edits::{EditOp, EditStatus};
use s3d_core::job::JobCtx;
use s3d_core::partition::{Partition, PieceId};

use crate::doc::{Document, Hollow, Snapshot, Source, SplitState, Status, Step, Undo};
use crate::jobs::{JobKind, Jobs};
use crate::theme::{self, Palette};
use crate::view::ViewState;
use crate::{logo, panels, project, recent, view, widgets};

pub enum JobOut {
    Imported(Source),
    Hollowed(Hollow),
    Split(SplitState, Vec<EditStatus>),
    /// Result of an incremental edit or connector update.
    Updated { partition: Partition, edits: Vec<EditOp>, status: Vec<EditStatus>, conn_key: Option<u64>, changed: Vec<PieceId> },
    /// A seam adjustment preview (not yet applied).
    Preview(view::Preview),
    Exported(String),
    Saved(PathBuf),
    Opened(Box<project::Loaded>),
}

#[derive(Clone, Debug)]
pub enum Notice {
    Info(String),
    Warn(String),
    Error(String),
}

pub struct S3dApp {
    pub doc: Document,
    pub undo: Undo,
    pub jobs: Jobs<JobOut>,
    pub step: Step,
    pub view: ViewState,
    pub dark: bool,
    pub notice: Option<Notice>,
    pub recent: recent::Recent,
    pub confirm_discard: Option<PendingAction>,
    pub show_settings: bool,
    pub custom_printer: bool,
    last_autosave: std::time::Instant,
    autosave_rev: u64,
    pub close_confirmed: bool,
}

#[derive(Clone, Debug)]
pub enum PendingAction {
    New,
    OpenModel(Option<PathBuf>),
    OpenProject(Option<PathBuf>),
    Quit,
}

impl S3dApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let recent = recent::Recent::load();
        let dark = recent.dark;
        theme::apply(&cc.egui_ctx, dark);
        if let Some(rs) = &cc.wgpu_render_state {
            let r = crate::viewport::renderer::Renderer3d::new(&rs.device, rs.target_format);
            rs.renderer.write().callback_resources.insert(r);
        }
        let mut app = S3dApp::headless(dark, recent);
        app.check_recovery();
        app
    }

    /// App state without a window (tests).
    pub fn headless(dark: bool, recent: recent::Recent) -> Self {
        let doc = Document::default();
        let mut undo = Undo::default();
        undo.reset(&doc);
        S3dApp {
            doc,
            undo,
            jobs: Jobs::default(),
            step: Step::Import,
            view: ViewState::default(),
            dark,
            notice: None,
            recent,
            confirm_discard: None,
            show_settings: false,
            custom_printer: false,
            last_autosave: std::time::Instant::now(),
            autosave_rev: 0,
            close_confirmed: false,
        }
    }

    pub fn palette(&self) -> Palette {
        if self.dark { theme::DARK } else { theme::LIGHT }
    }

    // ------------------------------------------------------------------
    // Actions

    pub fn request(&mut self, action: PendingAction) {
        if self.doc.dirty && self.doc.source.is_some() && !matches!(action, PendingAction::OpenModel(_) if self.doc.source.is_none()) {
            self.confirm_discard = Some(action);
        } else {
            self.perform(action);
        }
    }

    pub fn perform(&mut self, action: PendingAction) {
        match action {
            PendingAction::New => {
                self.jobs.cancel();
                self.doc = Document::default();
                self.undo.reset(&self.doc);
                self.view = ViewState::default();
                self.step = Step::Import;
                project::clear_autosave();
            }
            PendingAction::OpenModel(path) => {
                let path = path.or_else(|| rfd::FileDialog::new().add_filter("3D models", s3d_core::io::MESH_EXTENSIONS).set_title("Open model").pick_file());
                if let Some(p) = path {
                    self.start_import(p);
                }
            }
            PendingAction::OpenProject(path) => {
                let path = path.or_else(|| rfd::FileDialog::new().add_filter("Section3D project", &["s3d"]).set_title("Open project").pick_file());
                if let Some(p) = path {
                    self.start_open(p);
                }
            }
            PendingAction::Quit => {
                self.close_confirmed = true;
            }
        }
    }

    pub fn start_import(&mut self, path: PathBuf) {
        let ctx = self.ctx_handle();
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "model".into());
        self.jobs.start(JobKind::Import, &ctx, move |job| {
            job.set_stage("Reading file");
            let mesh = s3d_core::io::load(&path).map_err(|e| e.to_string())?;
            job.set_progress(0.4);
            let r = s3d_core::repair::repair(mesh, job).map_err(|e| e.to_string())?;
            Ok(JobOut::Imported(Source { name, path: Some(path), mesh: Arc::new(r.mesh), health: r.health, mesh_id: 0 }))
        });
    }

    pub fn start_open(&mut self, path: PathBuf) {
        let ctx = self.ctx_handle();
        self.jobs.start(JobKind::Open, &ctx, move |job| project::load(&path, job).map(|l| JobOut::Opened(Box::new(l))));
    }

    pub fn start_hollow(&mut self) {
        let Some((mesh, _)) = self.doc.prepared_mesh_unhollowed() else { return };
        let wall = self.doc.settings.prepare.wall;
        let key = self.doc.hollow_key();
        let ctx = self.ctx_handle();
        self.jobs.start(JobKind::Hollow, &ctx, move |job| {
            let m = s3d_core::hollow::hollow(&mesh, wall, job).map_err(|e| e.to_string())?;
            Ok(JobOut::Hollowed(Hollow { key, mesh: Arc::new(m), mesh_id: 0 }))
        });
    }

    pub fn start_split(&mut self) {
        let Some((mesh, xf)) = self.doc.prepared_mesh() else { return };
        let settings = self.doc.settings.clone();
        let zones = self.doc.zones.transformed(&xf);
        let edits = self.doc.edits.clone();
        let keep: Vec<_> = self.doc.split.as_ref().map(|s| s.partition.pieces.iter().filter(|p| p.locked).map(|p| p.solid.clone()).collect()).unwrap_or_default();
        let key = self.doc.split_key();
        let conn_key = self.doc.conn_key();
        let ctx = self.ctx_handle();
        self.jobs.start(JobKind::Split, &ctx, move |job| {
            let out = s3d_core::split::run(s3d_core::split::SplitInput { mesh, settings: settings.clone(), zones, edits, keep }, job)?;
            let mut partition = out.partition;
            let all: Vec<PieceId> = partition.pieces.iter().map(|p| p.id).collect();
            s3d_core::connectors::finish(&mut partition, &all, &settings, job)?;
            Ok(JobOut::Split(SplitState { partition, key, conn_key: Some(conn_key), generation: 0 }, out.edit_status))
        });
    }

    /// Re-runs connectors and part numbers on the existing pieces.
    pub fn start_connectors(&mut self) {
        let Some(split) = self.doc.split.clone() else { return };
        let settings = self.doc.settings.clone();
        let conn_key = self.doc.conn_key();
        let edits = self.doc.edits.clone();
        let status = self.doc.edit_status.clone();
        let ctx = self.ctx_handle();
        self.jobs.start(JobKind::Connectors, &ctx, move |job| {
            let mut partition = split.partition.clone();
            let all: Vec<PieceId> = partition.pieces.iter().map(|p| p.id).collect();
            s3d_core::connectors::finish(&mut partition, &all, &settings, job)?;
            Ok(JobOut::Updated { partition, edits, status, conn_key: Some(conn_key), changed: all })
        });
    }

    /// Applies a manual edit to the current pieces in the background.
    pub fn start_edit(&mut self, op: EditOp) {
        self.start_edit_with(move |_| Ok(op));
    }

    /// Like `start_edit`, but the edit is computed in the background first.
    pub fn start_edit_with(&mut self, make: impl FnOnce(&JobCtx) -> Result<EditOp, String> + Send + 'static) {
        let Some(split) = self.doc.split.clone() else { return };
        let settings = self.doc.settings.clone();
        let mut edits = self.doc.edits.clone();
        let mut status = self.doc.edit_status.clone();
        let conn = split.conn_key;
        let ctx = self.ctx_handle();
        self.jobs.start(JobKind::Edit, &ctx, move |job| {
            let op = make(job)?;
            let mut partition = split.partition.clone();
            let (st, changed) = s3d_core::edits::apply(&mut partition, &op);
            if let EditStatus::Failed(e) = &st {
                return Err(e.clone());
            }
            if st == EditStatus::Orphaned {
                return Err("the piece or cut no longer exists".into());
            }
            // Neighbours of changed pieces may have gained or lost cut faces.
            let mut touched = changed.clone();
            for id in &changed {
                touched.extend(partition.neighbours(*id));
            }
            touched.sort_unstable();
            touched.dedup();
            let params = s3d_core::split::orient_params(&settings);
            s3d_core::split::orient_pieces(&mut partition, &touched, &params);
            if conn.is_some() {
                s3d_core::connectors::finish(&mut partition, &touched, &settings, job)?;
            }
            edits.push(op);
            status.push(st);
            Ok(JobOut::Updated { partition, edits, status, conn_key: conn, changed: touched })
        });
    }

    pub fn start_export(&mut self, folder: PathBuf) {
        let Some(split) = self.doc.split.clone() else { return };
        let settings = self.doc.settings.clone();
        let layout = self.view.beds.clone();
        let title = self.doc.title();
        let ctx = self.ctx_handle();
        self.jobs.start(JobKind::Export, &ctx, move |job| crate::export::run(&folder, &title, &split.partition, &layout, &settings, job).map(JobOut::Exported));
    }

    pub fn save(&mut self, save_as: bool) {
        let path = match (&self.doc.path, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new()
                .add_filter("Section3D project", &["s3d"])
                .set_file_name(format!("{}.s3d", self.doc.title()))
                .set_title("Save project")
                .save_file(),
        };
        let Some(path) = path else { return };
        let data = project::Saved::from_doc(&self.doc, &self.view);
        let ctx = self.ctx_handle();
        self.jobs.start(JobKind::Save, &ctx, move |job| project::save(&path, &data, job).map(|_| JobOut::Saved(path)));
    }

    fn ctx_handle(&self) -> egui::Context {
        crate::CTX.get().cloned().unwrap_or_default()
    }

    fn handle_job_result(&mut self, kind: JobKind, res: Result<JobOut, String>) {
        let out = match res {
            Ok(o) => o,
            Err(e) if e == "cancelled" => return,
            Err(e) => {
                self.notice = Some(Notice::Error(format!("{} failed: {e}", kind.label())));
                return;
            }
        };
        match out {
            JobOut::Imported(mut src) => {
                src.mesh_id = self.doc.new_id();
                let health = src.health.clone();
                self.jobs.cancel();
                let settings = self.doc.settings.clone();
                self.doc = Document::default();
                // Keep printer and connector choices across models.
                self.doc.settings.printer = settings.printer;
                self.doc.settings.connectors = settings.connectors;
                self.doc.settings.export = settings.export;
                self.doc.source = Some(src);
                self.doc.dirty = true;
                self.undo.reset(&self.doc);
                self.view = ViewState::default();
                self.step = Step::Import;
                self.notice = Some(match &health {
                    s3d_core::repair::Health::Clean => Notice::Info("Model imported. The mesh is clean.".into()),
                    s3d_core::repair::Health::Repaired { notes } => Notice::Info(format!("Model imported and repaired: {}.", notes.join(", "))),
                    s3d_core::repair::Health::Rebuilt { .. } => Notice::Warn("The mesh was badly broken and was rebuilt from voxels. Check fine details.".into()),
                    s3d_core::repair::Health::Unresolved { reason } => Notice::Error(format!("The mesh could not be repaired: {reason}.")),
                });
            }
            JobOut::Hollowed(mut h) => {
                h.mesh_id = self.doc.new_id();
                self.doc.hollow = Some(h);
                self.doc.dirty = true;
                self.notice = Some(Notice::Info("Model hollowed.".into()));
            }
            JobOut::Split(mut st, status) => {
                st.generation = self.doc.new_id();
                st.key = self.doc.split_key();
                let n = st.partition.pieces.len();
                let flagged = st.partition.pieces.iter().filter(|p| !p.flags.is_empty()).count();
                self.doc.split = Some(Arc::new(st));
                let orphaned = status.iter().filter(|s| **s != EditStatus::Applied).count();
                self.doc.edit_status = status;
                self.doc.dirty = true;
                self.view.on_new_split();
                self.notice = Some(if orphaned > 0 {
                    Notice::Warn(format!("Split into {n} pieces. {orphaned} manual edit(s) could not be re-applied."))
                } else if flagged > 0 {
                    Notice::Warn(format!("Split into {n} pieces. {flagged} piece(s) need attention."))
                } else {
                    Notice::Info(format!("Split into {n} pieces. All pieces fit and print without supports."))
                });
                self.view.arrange_needed = true;
            }
            JobOut::Updated { partition, edits, status, conn_key, changed } => {
                let Some(old) = &self.doc.split else { return };
                let generation = old.generation;
                self.doc.edits = edits;
                self.doc.edit_status = status;
                let key = self.doc.split_key();
                self.doc.split = Some(Arc::new(SplitState { partition, key, conn_key, generation }));
                self.doc.dirty = true;
                self.view.on_pieces_changed(&changed);
                self.view.arrange_needed = true;
            }
            JobOut::Preview(p) => self.view.preview = Some(p),
            JobOut::Exported(msg) => {
                self.notice = Some(Notice::Info(msg));
            }
            JobOut::Saved(path) => {
                self.doc.path = Some(path.clone());
                self.doc.dirty = false;
                self.recent.add(&path);
                project::clear_autosave();
                self.notice = Some(Notice::Info(format!("Saved {}", path.display())));
            }
            JobOut::Opened(l) => {
                self.jobs.cancel();
                let path = l.path.clone();
                self.doc = l.into_doc();
                self.undo.reset(&self.doc);
                self.view = ViewState::default();
                self.view.restore(&self.doc);
                self.step = if self.doc.split.is_some() { Step::Split } else { Step::Import };
                if let Some(p) = path {
                    self.recent.add(&p);
                }
                self.notice = Some(Notice::Info("Project opened.".into()));
            }
        }
    }

    fn check_recovery(&mut self) {
        if let Some(p) = project::autosave_path().filter(|p| p.exists()) {
            if project::crashed_last_time() {
                self.confirm_discard = None;
                self.view.recovery = Some(p);
            }
        }
        project::mark_running();
    }

    fn autosave(&mut self) {
        if self.doc.source.is_none() || !self.doc.dirty || self.jobs.busy() {
            return;
        }
        let rev = s3d_core::settings::fingerprint(&(self.doc.split_key(), self.doc.conn_key(), self.doc.split.as_ref().map(|s| s.generation)));
        if self.last_autosave.elapsed().as_secs() < 120 || rev == self.autosave_rev {
            return;
        }
        self.last_autosave = std::time::Instant::now();
        self.autosave_rev = rev;
        let data = project::Saved::from_doc(&self.doc, &self.view);
        std::thread::spawn(move || {
            if let Some(p) = project::autosave_path() {
                let _ = project::save(&p, &data, &JobCtx::new());
            }
        });
    }

    // ------------------------------------------------------------------
    // UI

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let _ = crate::CTX.set(ctx.clone());
        if let Some((kind, res)) = self.jobs.poll() {
            self.handle_job_result(kind, res);
        }
        if self.jobs.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.shortcuts(&ctx);
        self.handle_drop(&ctx);
        self.handle_close(&ctx);

        let p = self.palette();
        egui::Panel::top("topbar")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(14, 8)).stroke(Stroke::new(1.0, p.card_stroke)))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("statusbar")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(14, 5)))
            .show(ui, |ui| self.status_bar(ui));
        if self.doc.source.is_none() && !self.jobs.busy() {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(p.panel)).show(ui, |ui| self.start_screen(ui));
        } else {
            egui::Panel::left("steps")
                .resizable(true)
                .default_size(400.0)
                .size_range(330.0..=560.0)
                .frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::same(12)))
                .show(ui, |ui| panels::show(self, ui));
            egui::CentralPanel::default().frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin { left: 0, right: 12, top: 12, bottom: 12 })).show(ui, |ui| view::show(self, ui));
        }
        self.dialogs(&ctx);
        if !ctx.input(|i| i.pointer.any_down()) && !self.jobs.busy() && self.undo.checkpoint(&self.doc) {
            self.doc.dirty = true;
        }
        self.autosave();
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let sc = |m, k| KeyboardShortcut::new(m, k);
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))) || ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Y))) {
            self.redo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Z))) {
            self.undo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::S))) {
            self.save(true);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::S))) {
            self.save(false);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::O))) {
            self.request(PendingAction::OpenModel(None));
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::N))) {
            self.request(PendingAction::New);
        }
    }

    pub fn undo(&mut self) {
        if !self.jobs.busy() {
            self.undo.undo(&mut self.doc);
            self.view.on_new_split();
        }
    }

    pub fn redo(&mut self) {
        if !self.jobs.busy() {
            self.undo.redo(&mut self.doc);
            self.view.on_new_split();
        }
    }

    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(p) = dropped.into_iter().next() {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext == "s3d" {
                self.request(PendingAction::OpenProject(Some(p)));
            } else {
                self.request(PendingAction::OpenModel(Some(p)));
            }
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.close_confirmed || !self.doc.dirty || self.doc.source.is_none() {
                project::mark_clean_exit();
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_discard = Some(PendingAction::Quit);
            }
        }
        if self.close_confirmed {
            project::mark_clean_exit();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        let p = self.palette();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            logo::wordmark(ui, 30.0, self.dark);
            ui.add_space(22.0);
            ui.label(RichText::new(self.doc.title()).font(theme::semibold(16.0)).color(p.text_strong));
            if self.doc.dirty && self.doc.source.is_some() {
                ui.label(RichText::new(" •").color(p.accent));
            }
            ui.add_space(18.0);
            let busy = self.jobs.busy();
            button_group(ui, &p, |ui| {
                if ui.button("New").on_hover_text("Start over (Ctrl+N)").clicked() {
                    self.request(PendingAction::New);
                }
                sep(ui, &p);
                let open = ui.button("Open").on_hover_text("Open a model or project (Ctrl+O)");
                if open.clicked() {
                    self.request(PendingAction::OpenModel(None));
                }
                sep(ui, &p);
                egui::containers::menu::MenuButton::new("▾").ui(ui, |ui| {
                    if ui.button("Open model…").clicked() {
                        self.request(PendingAction::OpenModel(None));
                    }
                    if ui.button("Open project…").clicked() {
                        self.request(PendingAction::OpenProject(None));
                    }
                    let recents = self.recent.projects.clone();
                    if !recents.is_empty() {
                        ui.separator();
                        for r in recents {
                            if ui.button(r.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()).on_hover_text(r.display().to_string()).clicked() {
                                self.request(PendingAction::OpenProject(Some(r)));
                            }
                        }
                    }
                });
                sep(ui, &p);
                if ui.add_enabled(self.doc.source.is_some() && !busy, egui::Button::new("Save")).on_hover_text("Save project (Ctrl+S)").clicked() {
                    self.save(false);
                }
                sep(ui, &p);
                if ui.add_enabled(self.doc.source.is_some() && !busy, egui::Button::new("Save As")).on_hover_text("Save project as (Ctrl+Shift+S)").clicked() {
                    self.save(true);
                }
            });
            ui.add_space(10.0);
            button_group(ui, &p, |ui| {
                if ui.add_enabled(self.undo.can_undo() && !busy, egui::Button::new("↶")).on_hover_text("Undo (Ctrl+Z)").clicked() {
                    self.undo();
                }
                sep(ui, &p);
                if ui.add_enabled(self.undo.can_redo() && !busy, egui::Button::new("↷")).on_hover_text("Redo (Ctrl+Y)").clicked() {
                    self.redo();
                }
            });
            ui.add_space(10.0);
            let can_export = self.doc.split.is_some() && !busy;
            if ui.add_enabled(can_export, egui::Button::new(RichText::new("Export").font(theme::semibold(14.0)))).on_hover_text("Go to Export").clicked() {
                self.step = Step::Export;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let icon = if self.dark { "☀" } else { "☾" };
                if ui.button(icon).on_hover_text("Switch light / dark theme").clicked() {
                    self.dark = !self.dark;
                    theme::apply(ui.ctx(), self.dark);
                    self.recent.dark = self.dark;
                    self.recent.save();
                }
                if ui.button("Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
                panels::printer_picker(self, ui);
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let p = self.palette();
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            if let Some(r) = &self.jobs.current {
                let frac = r.ctx.progress();
                let stage = r.ctx.stage();
                ui.add(egui::Spinner::new().size(14.0).color(p.accent));
                ui.label(RichText::new(format!("{} — {}", r.kind.label(), stage)).color(p.text));
                ui.add(egui::ProgressBar::new(frac).desired_width(220.0).desired_height(8.0).fill(p.accent).corner_radius(CornerRadius::same(4)));
                ui.label(RichText::new(format!("{:.0}%  ·  {:.1} s", frac * 100.0, r.started.elapsed().as_secs_f32())).color(p.text_weak));
                if ui.button("Cancel").clicked() {
                    self.jobs.cancel();
                    self.notice = Some(Notice::Info("Cancelled.".into()));
                }
                return;
            }
            let (icon, color, text) = match &self.notice {
                None => ("✓", p.good, self.default_status()),
                Some(Notice::Info(s)) => ("✓", p.good, s.clone()),
                Some(Notice::Warn(s)) => ("⚠", p.warn, s.clone()),
                Some(Notice::Error(s)) => ("✖", p.bad, s.clone()),
            };
            ui.label(RichText::new(icon).color(color));
            ui.label(RichText::new(text).color(color));
            if self.notice.is_some() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Dismiss").clicked() {
                        self.notice = None;
                    }
                });
            }
        });
    }

    fn default_status(&self) -> String {
        match &self.doc.split {
            Some(s) => {
                let flagged = s.partition.pieces.iter().filter(|p| !p.flags.is_empty()).count();
                if flagged == 0 { format!("{} pieces · no errors or warnings", s.partition.pieces.len()) } else { format!("{} pieces · {flagged} need attention", s.partition.pieces.len()) }
            }
            None => "No errors or warnings".into(),
        }
    }

    fn start_screen(&mut self, ui: &mut Ui) {
        let p = self.palette();
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.16).max(20.0));
            logo::full(ui, 360.0, self.dark);
            ui.add_space(26.0);
            let w = 340.0;
            ui.allocate_ui(Vec2::new(w, 0.0), |ui| {
                ui.set_width(w);
                if widgets::primary_button(ui, "Open model…", true).clicked() {
                    self.request(PendingAction::OpenModel(None));
                }
                ui.add_space(6.0);
                if ui.add_sized([w, 34.0], egui::Button::new("Open project…")).clicked() {
                    self.request(PendingAction::OpenProject(None));
                }
                ui.add_space(4.0);
                ui.label(widgets::weak(ui, "or drop an STL, OBJ or 3MF file anywhere"));
            });
            ui.add_space(26.0);
            if let Some(rec) = self.view.recovery.clone() {
                ui.allocate_ui(Vec2::new(420.0, 0.0), |ui| {
                    widgets::card(ui, |ui| {
                        ui.label(RichText::new("Section3D closed unexpectedly").font(theme::semibold(14.0)).color(p.warn));
                        ui.label(widgets::weak(ui, "An autosaved copy of your last session is available."));
                        ui.horizontal(|ui| {
                            if ui.button("Recover").clicked() {
                                self.view.recovery = None;
                                self.start_open(rec.clone());
                            }
                            if ui.button("Discard").clicked() {
                                self.view.recovery = None;
                                project::clear_autosave();
                            }
                        });
                    });
                });
                ui.add_space(14.0);
            }
            let recents = self.recent.projects.clone();
            if !recents.is_empty() {
                ui.allocate_ui(Vec2::new(420.0, 0.0), |ui| {
                    widgets::card(ui, |ui| {
                        widgets::section_title(ui, "Recent projects");
                        ui.add_space(4.0);
                        for r in recents.iter().take(8) {
                            let name = r.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            let resp = ui
                                .add(egui::Button::new(RichText::new(&name).color(p.text_strong)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(Vec2::new(ui.available_width(), 26.0)))
                                .on_hover_text(r.display().to_string());
                            if resp.clicked() {
                                self.request(PendingAction::OpenProject(Some(r.clone())));
                            }
                        }
                    });
                });
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(action) = self.confirm_discard.clone() {
            let p = self.palette();
            let modal = egui::Modal::new(egui::Id::unique("discard")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("Unsaved changes").font(theme::semibold(16.0)).color(p.text_strong));
                ui.add_space(4.0);
                ui.label("Your changes to this project have not been saved. Save them first?");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        self.confirm_discard = None;
                        self.save(false);
                    }
                    if ui.button("Don't save").clicked() {
                        self.confirm_discard = None;
                        self.doc.dirty = false;
                        self.perform(action.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_discard = None;
                    }
                });
            });
            if modal.should_close() {
                self.confirm_discard = None;
            }
        }
        if self.show_settings {
            let mut open = true;
            egui::Window::new("Settings").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| panels::app_settings(self, ui));
            self.show_settings = open;
        }
    }

    pub fn status(&self, step: Step) -> Status {
        self.doc.status(step)
    }

    pub fn restore_snapshot(&mut self, s: Snapshot) {
        self.doc.restore(s);
    }
}

/// Joined button group.
fn button_group(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().stroke(Stroke::new(1.0, p.field_stroke)).corner_radius(CornerRadius::same(7)).fill(if p.dark { Color32::from_rgb(0x1C, 0x21, 0x2A) } else { Color32::WHITE }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding = egui::vec2(14.0, 6.0);
        ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
        ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
        ui.visuals_mut().widgets.active.bg_stroke = Stroke::NONE;
        ui.horizontal(add);
    });
}

fn sep(ui: &mut Ui, p: &Palette) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 30.0), egui::Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, p.field_stroke));
}

impl eframe::App for S3dApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        S3dApp::ui(self, ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.palette().bg.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        project::mark_clean_exit();
    }
}
