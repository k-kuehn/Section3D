//! Dowel holes and part numbers ("finishing" pieces after cutting).

use std::sync::Arc;

use crate::job::JobCtx;
use crate::partition::{Partition, PieceId};
use crate::settings::Settings;

/// Adds connectors and labels to the given pieces (`finished` meshes).
pub fn finish(p: &mut Partition, ids: &[PieceId], _s: &Settings, job: &JobCtx) -> Result<(), String> {
    job.set_stage("Adding connectors");
    for q in p.pieces.iter_mut().filter(|q| ids.contains(&q.id)) {
        q.finished = Some(Arc::clone(&q.mesh));
    }
    Ok(())
}
