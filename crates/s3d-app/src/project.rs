//! Saving/opening projects, autosave and crash recovery.

use std::path::PathBuf;
use std::sync::Arc;

use s3d_core::job::JobCtx;
use s3d_core::project::{ProjectData, SourceData, SplitData};

use crate::doc::{Document, Hollow, Source, SplitState};
use crate::recent::data_dir;
use crate::view::ViewState;

pub struct Saved(ProjectData);

impl Saved {
    pub fn from_doc(doc: &Document, view: &ViewState) -> Saved {
        Saved(ProjectData {
            settings: doc.settings.clone(),
            edits: doc.edits.clone(),
            edit_status: doc.edit_status.clone(),
            zones: doc.zones.clone(),
            source: doc.source.as_ref().map(|s| SourceData { name: s.name.clone(), path: s.path.as_ref().map(|p| p.display().to_string()), health: s.health.clone(), mesh: s.mesh.clone() }),
            hollow: doc.hollow.as_ref().map(|h| (h.key, h.mesh.clone())),
            split: doc.split.as_ref().map(|s| SplitData { key: s.key, conn_key: s.conn_key, partition: s.partition.clone() }),
            extra: view.to_json(),
        })
    }
}

pub struct Loaded {
    pub data: ProjectData,
    pub path: Option<PathBuf>,
}

impl Loaded {
    pub fn into_doc(self) -> Document {
        let mut doc = Document::default();
        let d = self.data;
        doc.settings = d.settings;
        doc.edits = d.edits;
        doc.edit_status = d.edit_status;
        doc.zones = d.zones;
        if let Some(s) = d.source {
            let id = doc.new_id();
            doc.source = Some(Source { name: s.name, path: s.path.map(PathBuf::from), mesh: s.mesh, health: s.health, mesh_id: id });
        }
        if let Some((key, mesh)) = d.hollow {
            let id = doc.new_id();
            doc.hollow = Some(Hollow { key, mesh, mesh_id: id });
        }
        if let Some(s) = d.split {
            let generation = doc.new_id();
            doc.split = Some(Arc::new(SplitState { partition: s.partition, key: s.key, conn_key: s.conn_key, generation, pin_advice: None }));
        }
        doc.extra_view = Some(d.extra);
        // Autosave copies are not the user's file.
        let is_autosave = self.path.as_ref().zip(autosave_path()).is_some_and(|(a, b)| *a == b);
        doc.path = if is_autosave { None } else { self.path };
        doc.dirty = is_autosave;
        doc
    }
}

pub fn save(path: &std::path::Path, data: &Saved, job: &JobCtx) -> Result<(), String> {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    s3d_core::project::save(path, &data.0, job)
}

pub fn load(path: &std::path::Path, job: &JobCtx) -> Result<Loaded, String> {
    Ok(Loaded { data: s3d_core::project::load(path, job)?, path: Some(path.to_path_buf()) })
}

pub fn autosave_path() -> Option<PathBuf> {
    if std::env::var_os("S3D_NO_CONFIG").is_some() {
        return None;
    }
    data_dir().map(|d| d.join("autosave.s3d"))
}

fn lock_path() -> Option<PathBuf> {
    autosave_path().map(|p| p.with_file_name("running.lock"))
}

pub fn clear_autosave() {
    if let Some(p) = autosave_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// True if the previous session did not exit cleanly.
pub fn crashed_last_time() -> bool {
    lock_path().is_some_and(|p| p.exists())
}

pub fn mark_running() {
    if let Some(p) = lock_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, std::process::id().to_string());
    }
}

/// Called on a normal exit: the autosave is only for crash recovery.
pub fn mark_clean_exit() {
    if let Some(p) = lock_path() {
        let _ = std::fs::remove_file(p);
    }
    clear_autosave();
}
