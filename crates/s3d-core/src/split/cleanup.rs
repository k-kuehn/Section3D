//! Merging pieces: slivers and undersized fragments are merged into a
//! neighbour when the union still fits, needs no new support and is not tall.
//!
//! Candidate merges are evaluated cheaply on the two meshes joined without
//! their shared cut faces; only accepted merges run the real union.

use std::collections::HashSet;

use glam::DVec3;
use rayon::prelude::*;

use crate::job::{Cancelled, JobCtx};
use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::orient::{self, OrientParams, SUPPORT_NOISE};
use crate::partition::{Flag, ON_PLANE, Partition, Piece, PieceId};
use crate::plane::Plane;

/// Smallest width of a piece: min over hull face normals of its extent.
pub fn min_width(solid: &Solid) -> f64 {
    widths(solid)[0]
}

/// Approximate extents of a piece: thinnest width over hull face normals,
/// then the smallest and largest widths perpendicular to it.
pub fn widths(solid: &Solid) -> [f64; 3] {
    let Ok(h) = solid.hull() else { return [0.0; 3] };
    let m = h.to_mesh();
    let extent = |n: DVec3| {
        let (lo, hi) = m.verts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            let d = n.dot(v.as_dvec3());
            (lo.min(d), hi.max(d))
        });
        hi - lo
    };
    let mut best = (f64::INFINITY, DVec3::Z);
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
        let e = extent(n);
        if e < best.0 {
            best = (e, n);
        }
    }
    if !best.0.is_finite() {
        return [0.0; 3];
    }
    let a = best.1.any_orthonormal_vector();
    let b = best.1.cross(a);
    let (mut mid, mut max) = (f64::INFINITY, 0.0f64);
    for k in 0..18 {
        let ang = std::f64::consts::PI * k as f64 / 18.0;
        let e = extent(a * ang.cos() + b * ang.sin());
        mid = mid.min(e);
        max = max.max(e);
    }
    [best.0, mid, max]
}

/// A sliver is a strip (thin in two directions) or tiny. Thin plates, like
/// shell panels, are not slivers.
pub fn is_sliver(q: &Piece, min_piece: f64) -> bool {
    if q.locked {
        return false;
    }
    let s = q.bbox.size();
    let mut d = [s.x as f64, s.y as f64, s.z as f64];
    d.sort_by(|a, b| a.total_cmp(b));
    if d[1] >= min_piece * 3.0 && d[0] >= min_piece {
        return false;
    }
    let w = widths(&q.solid);
    w[1] < min_piece || w[2] < min_piece * 2.0
}

/// The two pieces' meshes joined without their shared cut faces.
fn joined(a: &Piece, b: &Piece, shared: &[Plane]) -> Mesh {
    let mut out = Mesh::default();
    for m in [&a.mesh, &b.mesh] {
        let base = out.verts.len() as u32;
        out.verts.extend_from_slice(&m.verts);
        for (t, tri) in m.tris.iter().enumerate() {
            let p = m.tri(t);
            if shared.iter().any(|pl| p.iter().all(|v| pl.dist(*v).abs() < ON_PLANE * 10.0)) {
                continue;
            }
            out.tris.push(tri.map(|i| i + base));
        }
    }
    out
}

fn support_of(flags: &[Flag]) -> f32 {
    flags.iter().map(|f| if let Flag::NeedsSupport(a) = f { *a } else { 0.0 }).sum()
}

/// Would merging `a` and `b` keep the result printable?
pub fn merge_ok(p: &Partition, a: PieceId, b: PieceId, params: &OrientParams) -> bool {
    let (Some(pa), Some(pb)) = (p.piece(a), p.piece(b)) else { return false };
    let shared: Vec<Plane> = p.seams_of(a).filter(|s| s.other(a) == b).map(|s| s.plane).collect();
    let mesh = joined(pa, pb, &shared);
    let normals: Vec<DVec3> = p
        .seams_of(a)
        .chain(p.seams_of(b))
        .filter(|s| !(s.touches(a) && s.touches(b)))
        .map(|s| if s.a == a || s.a == b { -s.plane.normal } else { s.plane.normal })
        .collect();
    let (_, flags) = orient::best(&mesh, &normals, params);
    if flags.contains(&Flag::TooBig) {
        return false;
    }
    if support_of(&flags) > support_of(&pa.flags) + support_of(&pb.flags) + SUPPORT_NOISE {
        return false;
    }
    let was_tall = pa.flags.contains(&Flag::Tall) || pb.flags.contains(&Flag::Tall);
    !(flags.contains(&Flag::Tall) && !was_tall)
}

/// Merges pieces selected by `want` into neighbours, in parallel rounds.
/// Returns the wanted pieces that could not be merged.
pub fn consolidate(p: &mut Partition, params: &OrientParams, job: &JobCtx, want: &(dyn Fn(&Piece) -> bool + Sync)) -> Result<Vec<PieceId>, Cancelled> {
    let missing: Vec<PieceId> = p.pieces.iter().filter(|q| q.orient.is_none()).map(|q| q.id).collect();
    crate::split::orient_pieces(p, &missing, params);
    let mut stuck: HashSet<PieceId> = HashSet::new();
    for _round in 0..64 {
        job.check()?;
        let cands: Vec<PieceId> = p.pieces.iter().filter(|q| !q.locked && !stuck.contains(&q.id) && want(q)).map(|q| q.id).collect();
        if cands.is_empty() {
            break;
        }
        // Best acceptable partner for each candidate (largest shared area).
        let picks: Vec<(PieceId, Option<PieceId>)> = cands
            .par_iter()
            .map(|&id| {
                let mut nbrs: Vec<(f64, PieceId)> = Vec::new();
                for s in p.seams_of(id) {
                    let o = s.other(id);
                    if s.locked || p.piece(o).is_some_and(|q| q.locked) {
                        continue;
                    }
                    match nbrs.iter_mut().find(|n| n.1 == o) {
                        Some(n) => n.0 += s.area,
                        None => nbrs.push((s.area, o)),
                    }
                }
                nbrs.sort_by(|a, b| b.0.total_cmp(&a.0));
                for (_, o) in nbrs.into_iter().take(4) {
                    if job.is_cancelled() {
                        break;
                    }
                    if merge_ok(p, id, o, params) {
                        return (id, Some(o));
                    }
                }
                (id, None)
            })
            .collect();
        let mut order: Vec<(f64, PieceId, PieceId)> = Vec::new();
        for (id, pick) in picks {
            match pick {
                Some(o) => order.push((p.piece(id).map(|q| q.volume).unwrap_or(0.0), id, o)),
                None => {
                    stuck.insert(id);
                }
            }
        }
        // Smallest first; each piece takes part in at most one merge per round.
        order.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut used: HashSet<PieceId> = HashSet::new();
        let mut merged = Vec::new();
        for (_, a, b) in order {
            if used.contains(&a) || used.contains(&b) {
                continue;
            }
            if let Ok(m) = p.merge(&[a, b]) {
                used.extend([a, b]);
                merged.push(m);
            }
        }
        if merged.is_empty() {
            break;
        }
        crate::split::orient_pieces(p, &merged, params);
    }
    Ok(stuck.into_iter().filter(|id| p.piece(*id).is_some()).collect())
}

/// Merges slivers away. Returns ids that stayed slivers (to be flagged).
pub fn merge_slivers(p: &mut Partition, min_piece: f64, params: &OrientParams, job: &JobCtx) -> Result<Vec<PieceId>, Cancelled> {
    let first_new = p.next_piece;
    let slivers: HashSet<PieceId> = p.pieces.par_iter().filter(|q| is_sliver(q, min_piece)).map(|q| q.id).collect::<Vec<_>>().into_iter().collect();
    // Pieces created by merges are re-checked.
    let stuck = consolidate(p, params, job, &|q: &Piece| slivers.contains(&q.id) || (q.id >= first_new && is_sliver(q, min_piece)))?;
    Ok(stuck.into_iter().filter(|id| p.piece(*id).is_some_and(|q| is_sliver(q, min_piece))).collect())
}

/// Merges undersized pieces (largest print dimension below `fraction` of the
/// target size) into neighbours, keeping everything printable.
pub fn merge_undersized(p: &mut Partition, target: f64, fraction: f64, params: &OrientParams, job: &JobCtx) -> Result<usize, Cancelled> {
    let before = p.pieces.len();
    let limit = (target * fraction) as f32;
    consolidate(p, params, job, &|q: &Piece| q.orient.map(|o| o.size.max_element()).unwrap_or(q.bbox.size().max_element()) < limit)?;
    Ok(before - p.pieces.len())
}
