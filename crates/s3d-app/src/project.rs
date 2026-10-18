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
            extra: view.to_json(doc),
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
            let content = s.mesh.content_hash();
            doc.source = Some(Source { name: s.name, path: s.path.map(PathBuf::from), mesh: s.mesh, health: s.health, mesh_id: id, content });
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
        let is_autosave = self.path.as_ref().is_some_and(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("autosave-")));
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

fn session_dir() -> Option<PathBuf> {
    if std::env::var_os("S3D_NO_CONFIG").is_some() {
        return None;
    }
    data_dir().map(|d| d.join("sessions"))
}

/// This session's autosave file (one per running instance).
pub fn autosave_path() -> Option<PathBuf> {
    session_dir().map(|d| d.join(format!("autosave-{}.s3d", std::process::id())))
}

fn lock_path(pid: u32) -> Option<PathBuf> {
    session_dir().map(|d| d.join(format!("running-{pid}.lock")))
}

/// Handle on this session's lock file, held open (unshared) while running.
static LOCK: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

pub fn clear_autosave() {
    if let Some(p) = autosave_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// Autosaves left by sessions that ended without a clean exit. A running
/// instance holds its lock file open, so only dead sessions' locks can be
/// removed.
pub fn crashed_sessions() -> Vec<PathBuf> {
    let Some(dir) = session_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let me = std::process::id();
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("running-").and_then(|r| r.strip_suffix(".lock")).and_then(|r| r.parse::<u32>().ok()) else { continue };
        if pid == me || std::fs::remove_file(e.path()).is_err() {
            continue;
        }
        let save = dir.join(format!("autosave-{pid}.s3d"));
        if save.exists() {
            out.push(save);
        }
    }
    // Autosaves whose lock was already cleaned up but never recovered.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(pid) = name.strip_prefix("autosave-").and_then(|r| r.strip_suffix(".s3d")).and_then(|r| r.parse::<u32>().ok()) else { continue };
            if pid != me && !dir.join(format!("running-{pid}.lock")).exists() && !out.contains(&e.path()) {
                out.push(e.path());
            }
        }
    }
    out
}

pub fn mark_running() {
    let Some(p) = lock_path(std::process::id()) else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(0);
    }
    if let Ok(f) = opts.open(&p) {
        *LOCK.lock().unwrap() = Some(f);
    }
}

/// Called on a normal exit: this session's autosave is only for crashes.
pub fn mark_clean_exit() {
    LOCK.lock().unwrap().take();
    if let Some(p) = lock_path(std::process::id()) {
        let _ = std::fs::remove_file(p);
    }
    clear_autosave();
}
