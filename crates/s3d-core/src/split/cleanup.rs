//! Removes slivers: pieces thinner than the minimum size are merged into the
//! neighbour they share the most cut area with, if the result still fits.

use glam::DVec3;

use crate::job::{Cancelled, JobCtx};
use crate::manifold::Solid;
use crate::orient::{self, OrientParams};
use crate::partition::{Flag, Partition, PieceId};

/// Smallest width of a piece: min over hull face normals of its extent.
pub fn min_width(solid: &Solid) -> f64 {
    let Ok(h) = solid.hull() else { return 0.0 };
    let m = h.to_mesh();
    let mut best = f64::INFINITY;
    let mut seen: Vec<DVec3> = Vec::new();
    for t in 0..m.tris.len() {
        let n = m.tri_cross(t).as_dvec3();
        if n.length_squared() < 1e-18 {
            continue;
        }
        let n = n.normalize();
        if seen.iter().any(|s| s.dot(n).abs() > 0.9999) {
            continue;
        }
        seen.push(n);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for v in &m.verts {
            let d = n.dot(v.as_dvec3());
            lo = lo.min(d);
            hi = hi.max(d);
        }
        best = best.min(hi - lo);
    }
    if best.is_finite() { best } else { 0.0 }
}

fn is_small(p: &Partition, id: PieceId, min_piece: f64) -> bool {
    let Some(pc) = p.piece(id) else { return false };
    if pc.locked {
        return false;
    }
    // Cheap rejection first: a big bounding box can still be a thin plate.
    let s = pc.bbox.size();
    if s.min_element() as f64 >= min_piece {
        return min_width(&pc.solid) < min_piece;
    }
    true
}

/// Merges small pieces away. Returns ids that stayed small (flagged).
pub fn merge_slivers(p: &mut Partition, min_piece: f64, params: &OrientParams, job: &JobCtx) -> Result<Vec<PieceId>, Cancelled> {
    use rayon::prelude::*;
    let ids: Vec<PieceId> = p.pieces.iter().map(|q| q.id).collect();
    let flags: Vec<bool> = ids.par_iter().map(|&id| is_small(p, id, min_piece)).collect();
    // Candidates, re-checked lazily as merges create new pieces.
    let mut small: Vec<PieceId> = ids.iter().zip(flags).filter(|(_, s)| *s).map(|(id, _)| *id).collect();
    let mut stuck: Vec<PieceId> = Vec::new();
    while !small.is_empty() {
        job.check()?;
        small.retain(|id| p.piece(*id).is_some());
        // Smallest volume first so slivers fold into substantial neighbours.
        let Some(pos) = (0..small.len()).min_by(|&a, &b| p.piece(small[a]).unwrap().volume.total_cmp(&p.piece(small[b]).unwrap().volume)) else { break };
        let id = small.swap_remove(pos);
        // Neighbours ranked by shared area.
        let mut nbrs: Vec<(f64, PieceId)> = Vec::new();
        for s in p.seams_of(id) {
            if s.locked {
                continue;
            }
            let o = s.other(id);
            if p.piece(o).is_some_and(|q| q.locked) {
                continue;
            }
            match nbrs.iter_mut().find(|n| n.1 == o) {
                Some(n) => n.0 += s.area,
                None => nbrs.push((s.area, o)),
            }
        }
        nbrs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut merged = None;
        for (_, o) in nbrs {
            let (a, b) = (p.piece(id).unwrap(), p.piece(o).unwrap());
            let Ok(u) = a.solid.union(&b.solid) else { continue };
            let um = u.to_mesh();
            let normals: Vec<DVec3> = p.seams_of(id).chain(p.seams_of(o)).filter(|s| s.other(id) != o).map(|s| if s.a == id || s.a == o { -s.plane.normal } else { s.plane.normal }).collect();
            let (_, flags) = orient::best(&um, &normals, params);
            if flags.contains(&Flag::TooBig) {
                continue;
            }
            if let Ok(m) = p.merge(&[id, o]) {
                small.retain(|x| *x != o);
                merged = Some(m);
                break;
            }
        }
        match merged {
            Some(m) => {
                if is_small(p, m, min_piece) {
                    small.push(m);
                }
            }
            None => stuck.push(id),
        }
    }
    Ok(stuck)
}
