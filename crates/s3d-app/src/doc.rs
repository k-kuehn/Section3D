//! The open document: model, settings, results and step status.

use std::path::PathBuf;
use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3};
use s3d_core::edits::{EditOp, EditStatus};
use s3d_core::mesh::Mesh;
use s3d_core::partition::Partition;
use s3d_core::repair::Health;
use s3d_core::settings::{Settings, fingerprint};
use s3d_core::zones::Zones;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Import,
    Prepare,
    Split,
    Connectors,
    Export,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::Import, Step::Prepare, Step::Split, Step::Connectors, Step::Export];
    pub fn label(self) -> &'static str {
        match self {
            Step::Import => "Import",
            Step::Prepare => "Prepare",
            Step::Split => "Split",
            Step::Connectors => "Connectors",
            Step::Export => "Export",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    NotRun,
    Done,
    Stale,
}

#[derive(Clone)]
pub struct Source {
    pub name: String,
    pub path: Option<PathBuf>,
    pub mesh: Arc<Mesh>,
    pub health: Health,
    /// GPU/cache id, unique in this process.
    pub mesh_id: u64,
    /// Content hash of the mesh: keys results across sessions.
    pub content: u64,
}

#[derive(Clone)]
pub struct Hollow {
    /// Fingerprint of the transform + wall it was made with.
    pub key: u64,
    pub mesh: Arc<Mesh>,
    pub mesh_id: u64,
}

#[derive(Clone)]
pub struct SplitState {
    pub partition: Partition,
    /// Fingerprint of everything the split depended on.
    pub key: u64,
    /// Fingerprint of the connector settings the pieces were finished with.
    pub conn_key: Option<u64>,
    /// Distinguishes GPU mesh ids across results.
    pub generation: u64,
    /// Smaller pin size to suggest when pins had to be skipped.
    pub pin_advice: Option<s3d_core::connectors::PinAdvice>,
}

impl SplitState {
    pub fn mesh_id(&self, rev: u64) -> u64 {
        (self.generation << 32) | (rev & 0xFFFF_FFFF)
    }
}

/// Undoable part of the document.
#[derive(Clone)]
pub struct Snapshot {
    pub settings: Settings,
    pub edits: Vec<EditOp>,
    pub edit_status: Vec<EditStatus>,
    pub zones: Zones,
    pub split: Option<Arc<SplitState>>,
    pub hollow: Option<Hollow>,
}

pub struct Document {
    pub settings: Settings,
    pub source: Option<Source>,
    pub hollow: Option<Hollow>,
    pub zones: Zones,
    pub split: Option<Arc<SplitState>>,
    pub edits: Vec<EditOp>,
    pub edit_status: Vec<EditStatus>,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    /// View state loaded from a project, consumed by the view.
    pub extra_view: Option<serde_json::Value>,
    /// Fingerprint of what was last exported (for the Export step status).
    pub exported_key: Option<u64>,
    prepared_cache: Option<(u64, Arc<Mesh>, Mat4)>,
    placement_cache: Option<(u64, Mat4)>,
}

/// Inputs of the Split step except the edit list (see `Document::split_key`).
#[derive(Clone, serde::Serialize)]
pub struct SplitKeyParts {
    transform: u64,
    hollow: Option<u64>,
    printer: s3d_core::settings::PrinterSettings,
    split: s3d_core::settings::SplitSettings,
    zones: Zones,
}

impl SplitKeyParts {
    pub fn key(&self, edits: &[EditOp]) -> u64 {
        fingerprint(&(self.transform, self.hollow, &self.printer, &self.split, &self.zones, edits))
    }
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Process-wide unique id for GPU and picking caches.
pub fn new_id() -> u64 {
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Default for Document {
    fn default() -> Self {
        Document {
            settings: Settings::default(),
            source: None,
            hollow: None,
            zones: Zones::default(),
            split: None,
            edits: Vec::new(),
            edit_status: Vec::new(),
            path: None,
            dirty: false,
            extra_view: None,
            exported_key: None,
            prepared_cache: None,
            placement_cache: None,
        }
    }
}

impl Document {
    pub fn new_id(&mut self) -> u64 {
        new_id()
    }

    pub fn title(&self) -> String {
        if let Some(p) = &self.path {
            return p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        self.source.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| "Untitled".into())
    }

    /// Rotation, scale and mirror from the Prepare settings (no placement).
    pub fn shape_transform(&self) -> Mat4 {
        let p = &self.settings.prepare;
        let r = Quat::from_euler(EulerRot::ZYX, p.rotate[2].to_radians(), p.rotate[1].to_radians(), p.rotate[0].to_radians());
        let s = p.scale / 100.0;
        let m = Vec3::new(if p.mirror[0] { -s } else { s }, if p.mirror[1] { -s } else { s }, if p.mirror[2] { -s } else { s });
        Mat4::from_quat(r) * Mat4::from_scale(m)
    }

    fn transform_key(&self) -> u64 {
        let p = &self.settings.prepare;
        fingerprint(&(p.rotate, p.scale, p.mirror, self.source.as_ref().map(|s| s.content)))
    }

    /// Source -> prepared transform, computed from transformed bounds only
    /// (no mesh copy), so dragging Prepare values stays cheap.
    pub fn placement(&mut self) -> Option<Mat4> {
        let src = self.source.as_ref()?;
        let key = self.transform_key();
        if let Some((k, t)) = self.placement_cache {
            if k == key {
                return Some(t);
            }
        }
        let shape = self.shape_transform();
        let bb = src.mesh.transformed_bbox(&shape);
        let c = bb.center();
        let t = Mat4::from_translation(Vec3::new(-c.x, -c.y, -bb.min.z)) * shape;
        self.placement_cache = Some((key, t));
        Some(t)
    }

    /// What to draw before splitting: (mesh, cache id, model matrix).
    pub fn display_model(&mut self) -> Option<(Arc<Mesh>, u64, Mat4)> {
        if let Some(h) = self.valid_hollow() {
            return Some((h.mesh.clone(), h.mesh_id, Mat4::IDENTITY));
        }
        let t = self.placement()?;
        let s = self.source.as_ref()?;
        Some((s.mesh.clone(), s.mesh_id, t))
    }

    /// Full source → prepared transform: shape transform, then centred on
    /// the origin in XY and resting on z = 0.
    pub fn prepared_transform(&mut self) -> Mat4 {
        self.prepared_mesh().map(|(_, m)| m).unwrap_or(Mat4::IDENTITY)
    }

    /// The model as it will be split, and the source → prepared transform.
    pub fn prepared_mesh(&mut self) -> Option<(Arc<Mesh>, Mat4)> {
        let src = self.source.as_ref()?;
        let key = self.transform_key();
        if let Some((k, m, t)) = &self.prepared_cache {
            if *k == key {
                return Some((self.valid_hollow().map(|h| h.mesh.clone()).unwrap_or_else(|| m.clone()), *t));
            }
        }
        let _ = src;
        let t = self.placement()?;
        let m = Arc::new(self.source.as_ref()?.mesh.transformed(&t));
        self.prepared_cache = Some((key, m.clone(), t));
        Some((self.valid_hollow().map(|h| h.mesh.clone()).unwrap_or(m), t))
    }

    /// The transformed model ignoring any hollowing (input to Hollow).
    pub fn prepared_mesh_unhollowed(&mut self) -> Option<(Arc<Mesh>, Mat4)> {
        self.prepared_mesh()?;
        self.prepared_cache.as_ref().map(|(_, m, t)| (m.clone(), *t))
    }

    pub fn hollow_key(&self) -> u64 {
        fingerprint(&(self.transform_key(), self.settings.prepare.wall))
    }

    pub fn valid_hollow(&self) -> Option<&Hollow> {
        self.hollow.as_ref().filter(|h| h.key == self.hollow_key())
    }

    pub fn split_key_parts(&self) -> SplitKeyParts {
        let s = &self.settings;
        SplitKeyParts { transform: self.transform_key(), hollow: self.hollow.as_ref().map(|h| h.key), printer: s.printer.clone(), split: s.split.clone(), zones: self.zones.clone() }
    }

    /// Fingerprint of all inputs of the Split step.
    pub fn split_key(&self) -> u64 {
        self.split_key_parts().key(&self.edits)
    }

    pub fn conn_key(&self) -> u64 {
        fingerprint(&self.settings.connectors)
    }

    pub fn status(&self, step: Step) -> Status {
        match step {
            Step::Import => {
                if self.source.is_some() { Status::Done } else { Status::NotRun }
            }
            Step::Prepare => match (&self.source, &self.hollow) {
                (None, _) => Status::NotRun,
                (Some(_), Some(h)) if h.key != self.hollow_key() => Status::Stale,
                _ => Status::Done,
            },
            Step::Split => match &self.split {
                None => Status::NotRun,
                Some(s) if s.key != self.split_key() || self.status(Step::Prepare) == Status::Stale => Status::Stale,
                Some(_) => Status::Done,
            },
            Step::Connectors => match &self.split {
                None => Status::NotRun,
                Some(s) => {
                    if self.status(Step::Split) == Status::Stale || s.conn_key != Some(self.conn_key()) {
                        Status::Stale
                    } else {
                        Status::Done
                    }
                }
            },
            Step::Export => match (&self.split, self.exported_key) {
                (None, _) | (_, None) => Status::NotRun,
                (Some(_), Some(k)) if self.status(Step::Connectors) != Status::Done || k != self.export_key_basic() => Status::Stale,
                _ => Status::Done,
            },
        }
    }

    /// Fingerprint of what an export depends on (pieces and export settings).
    pub fn export_key_basic(&self) -> u64 {
        fingerprint(&(self.split.as_ref().map(|s| (s.generation, s.partition.pieces.iter().map(|p| p.rev).collect::<Vec<_>>())), &self.settings.export, &self.settings.printer))
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            settings: self.settings.clone(),
            edits: self.edits.clone(),
            edit_status: self.edit_status.clone(),
            zones: self.zones.clone(),
            split: self.split.clone(),
            hollow: self.hollow.clone(),
        }
    }

    pub fn restore(&mut self, s: Snapshot) {
        self.settings = s.settings;
        self.edits = s.edits;
        self.edit_status = s.edit_status;
        self.zones = s.zones;
        self.split = s.split;
        self.hollow = s.hollow;
        self.dirty = true;
    }
}

/// Undo/redo over document snapshots. Results are shared, so this is cheap.
#[derive(Default)]
pub struct Undo {
    past: Vec<Snapshot>,
    future: Vec<Snapshot>,
    last: Option<Snapshot>,
}

fn same(a: &Snapshot, b: &Snapshot) -> bool {
    a.settings == b.settings
        && a.edits == b.edits
        && a.zones == b.zones
        && match (&a.split, &b.split) {
            (Some(x), Some(y)) => Arc::ptr_eq(x, y),
            (None, None) => true,
            _ => false,
        }
        && a.hollow.as_ref().map(|h| h.mesh_id) == b.hollow.as_ref().map(|h| h.mesh_id)
}

impl Undo {
    /// Records the document state if it changed since the last checkpoint.
    pub fn checkpoint(&mut self, doc: &Document) -> bool {
        let now = doc.snapshot();
        match &self.last {
            Some(prev) if same(prev, &now) => false,
            Some(prev) => {
                self.past.push(prev.clone());
                if self.past.len() > 200 {
                    self.past.remove(0);
                }
                self.future.clear();
                self.last = Some(now);
                true
            }
            None => {
                self.last = Some(now);
                false
            }
        }
    }

    pub fn reset(&mut self, doc: &Document) {
        self.past.clear();
        self.future.clear();
        self.last = Some(doc.snapshot());
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn undo(&mut self, doc: &mut Document) {
        if let Some(prev) = self.past.pop() {
            if let Some(cur) = self.last.take() {
                self.future.push(cur);
            }
            doc.restore(prev.clone());
            self.last = Some(prev);
        }
    }

    pub fn redo(&mut self, doc: &mut Document) {
        if let Some(next) = self.future.pop() {
            if let Some(cur) = self.last.take() {
                self.past.push(cur);
            }
            doc.restore(next.clone());
            self.last = Some(next);
        }
    }
}
