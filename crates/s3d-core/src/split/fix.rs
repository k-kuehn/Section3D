//! "Find a better cut": searches single planes through one piece and keeps
//! the one whose two halves fit, need the least support and are not tall.

use glam::{DMat3, DVec3};
use rayon::prelude::*;

use crate::job::JobCtx;
use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::orient::{self, OrientParams, SUPPORT_NOISE};
use crate::partition::{Flag, Partition, PieceId};
use crate::plane::Plane;

/// Lower is better: (too big, support area bucket, tall count, imbalance).
type Score = (u32, i64, u32, i64);

fn score_flags(flags: &[Flag]) -> (u32, f32, u32) {
    let mut big = 0;
    let mut sup = 0.0;
    let mut tall = 0;
    for f in flags {
        match f {
            Flag::TooBig => big += 1,
            Flag::NeedsSupport(a) => sup += a,
            Flag::Tall => tall += 1,
            _ => {}
        }
    }
    (big, sup, tall)
}

/// Principal axes of a point cloud.
pub fn pca_axes(mesh: &Mesh) -> [DVec3; 3] {
    let n = mesh.verts.len().max(1) as f64;
    let c = mesh.verts.iter().fold(DVec3::ZERO, |a, v| a + v.as_dvec3()) / n;
    let mut m = DMat3::ZERO;
    for v in &mesh.verts {
        let d = v.as_dvec3() - c;
        m += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    // Power iteration with deflation for the 3 axes.
    let mut axes = [DVec3::X, DVec3::Y, DVec3::Z];
    let mut a = m / n;
    for (k, ax) in axes.iter_mut().enumerate() {
        let mut v = [DVec3::new(1.0, 0.3, 0.1), DVec3::new(0.2, 1.0, 0.4), DVec3::new(0.3, 0.1, 1.0)][k];
        for _ in 0..60 {
            let w = a * v;
            if w.length_squared() < 1e-24 {
                break;
            }
            v = w.normalize();
        }
        let lambda = v.dot(a * v);
        a -= DMat3::from_cols(v * v.x, v * v.y, v * v.z) * lambda;
        *ax = v;
    }
    axes
}

pub fn candidate_normals(mesh: &Mesh, cut_normals: &[DVec3]) -> Vec<DVec3> {
    let mut v: Vec<DVec3> = vec![DVec3::X, DVec3::Y, DVec3::Z];
    v.extend(pca_axes(mesh));
    v.extend_from_slice(cut_normals);
    // 45° blends of the axis pairs (diagonal cuts print without support).
    let base: Vec<DVec3> = v.clone();
    for i in 0..base.len().min(6) {
        for j in i + 1..base.len().min(6) {
            for s in [1.0, -1.0] {
                let b = base[i] + base[j] * s;
                if b.length() > 0.3 {
                    v.push(b.normalize());
                }
            }
        }
    }
    let mut out: Vec<DVec3> = Vec::new();
    for n in v {
        let n = if n.z < 0.0 || (n.z == 0.0 && n.x < 0.0) { -n } else { n };
        if !out.iter().any(|o| o.dot(n).abs() > 0.995) {
            out.push(n);
        }
    }
    out
}

/// Best single cut through a piece, or None if no cut improves it.
pub fn best_cut(p: &Partition, id: PieceId, params: &OrientParams, job: &JobCtx) -> Option<Plane> {
    let q = p.piece(id)?;
    let normals_out = super::cut_normals(p, id);
    let mesh = &q.mesh;
    let (_, base_flags) = orient::best(mesh, &normals_out, params);
    let (b0, s0, t0) = score_flags(&base_flags);
    let base: Score = (b0, (s0 / SUPPORT_NOISE) as i64, t0, 0);
    let normals = candidate_normals(mesh, &normals_out);
    let mut cands: Vec<Plane> = Vec::new();
    for n in normals {
        let (lo, hi) = mesh.verts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            let d = n.dot(v.as_dvec3());
            (lo.min(d), hi.max(d))
        });
        for f in [0.35, 0.5, 0.65] {
            cands.push(Plane { normal: n, offset: lo + (hi - lo) * f });
        }
    }
    let solid: &Solid = &q.solid;
    let results: Vec<(Score, Plane)> = cands
        .par_iter()
        .filter_map(|pl| {
            if job.is_cancelled() {
                return None;
            }
            let (a, b) = solid.split_by_plane(pl.normal, pl.offset).ok()?;
            if a.is_empty() || b.is_empty() {
                return None;
            }
            let (va, vb) = (a.volume(), b.volume());
            let mut total = (0u32, 0f32, 0u32);
            for (part, side) in [(a, -1.0), (b, 1.0)] {
                let mut ns = normals_out.clone();
                ns.push(pl.normal * side);
                for body in part.bodies() {
                    let m = body.to_mesh();
                    let (_, f) = orient::best(&m, &ns, params);
                    let (x, y, z) = score_flags(&f);
                    total = (total.0 + x, total.1 + y, total.2 + z);
                }
            }
            let imbalance = ((va - vb).abs() / (va + vb).max(1e-9) * 100.0) as i64;
            Some(((total.0, (total.1 / SUPPORT_NOISE) as i64, total.2, imbalance), *pl))
        })
        .collect();
    let best = results.into_iter().min_by(|a, b| a.0.cmp(&b.0))?;
    // Require an improvement in the hard criteria (or a fix for a too-big piece).
    if (best.0.0, best.0.1, best.0.2) < (base.0, base.1, base.2) || base == (0, 0, 0, 0) { Some(best.1) } else { None }
}
