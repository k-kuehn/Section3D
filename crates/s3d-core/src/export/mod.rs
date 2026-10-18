//! Export: 3MF per bed, single STLs, print list CSV, assembly guide PDF and
//! an offline HTML viewer.

use std::path::{Path, PathBuf};

use crate::job::JobCtx;
use crate::pack::Layout;
use crate::partition::Partition;
use crate::settings::Settings;

pub struct ExportResult {
    pub files: Vec<PathBuf>,
    pub folder: PathBuf,
}

impl ExportResult {
    pub fn summary(&self) -> String {
        format!("Exported {} files to {}", self.files.len(), self.folder.display())
    }
}

pub fn run(folder: &Path, _title: &str, _part: &Partition, _layout: &Layout, _s: &Settings, _job: &JobCtx) -> Result<ExportResult, String> {
    Err(format!("export to {} is not implemented yet", folder.display()))
}
