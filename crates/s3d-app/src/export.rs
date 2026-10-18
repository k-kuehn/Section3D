//! Export step: print files, print list, guide and viewer.

use std::path::Path;

use s3d_core::job::JobCtx;
use s3d_core::pack::Layout;
use s3d_core::partition::Partition;
use s3d_core::settings::Settings;

pub fn run(folder: &Path, title: &str, part: &Partition, layout: &Layout, s: &Settings, job: &JobCtx) -> Result<String, String> {
    if layout.beds.is_empty() && !part.pieces.is_empty() {
        return Err("no pieces fit on a bed".into());
    }
    s3d_core::export::run(folder, title, part, layout, s, job).map(|r| r.summary())
}
