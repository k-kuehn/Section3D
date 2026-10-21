//! Small persistent app config: recent projects, theme and projection.
//!
//! Stored in `%APPDATA%\Section3D\config.json`, or next to the executable when
//! a file named `portable` sits beside it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Recent {
    pub projects: Vec<PathBuf>,
    pub dark: bool,
    /// 3D views use a parallel (isometric) projection.
    pub ortho: bool,
}

impl Default for Recent {
    fn default() -> Self {
        Recent { projects: Vec::new(), dark: true, ortho: false }
    }
}

pub fn data_dir() -> Option<PathBuf> {
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        if exe_dir.join("portable").exists() {
            return Some(exe_dir.join("Section3D-data"));
        }
    }
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("XDG_CONFIG_HOME")).or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(base).join("Section3D"))
}

impl Recent {
    fn path() -> Option<PathBuf> {
        if std::env::var_os("S3D_NO_CONFIG").is_some() {
            return None;
        }
        data_dir().map(|d| d.join("config.json"))
    }

    pub fn load() -> Recent {
        Self::path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(p, serde_json::to_vec_pretty(self).unwrap_or_default());
        }
    }

    pub fn add(&mut self, p: &Path) {
        self.projects.retain(|q| q != p);
        self.projects.insert(0, p.to_path_buf());
        self.projects.truncate(12);
        self.save();
    }
}
