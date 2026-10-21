//! Improves a partition until every piece prints well: rounds of local fixes
//! on the real geometry, each accepted only if it lowers the cost.
//!
//! Problems, most severe first: too big, needs support, tall, sharp corner or
//! feather edge, thin strip, seams lining up into a long straight line, tiny
//! fragment, cuts meeting the surface far from square. Fixes are: an extra
//! cut through a piece; merging two pieces and dividing them again along a
//! different plane (this also breaks up aligned seams); or merging a piece
//! into a neighbour. Rounds stop when nothing is left to fix, when a round
//! makes no progress, or at the deadline.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use glam::{DMat3, DVec3, Vec3};
use rayon::prelude::*;

use super::quality::{self, Chain, PieceReport, QualityParams, SeamCache, analyze_piece};
use crate::job::JobCtx;
use crate::manifold::{Op, Solid};
use crate::mesh::{Aabb, Mesh};
use crate::orient::TALL_RATIO;
use crate::partition::{Flag, Partition, PieceId, SeamId};
use crate::plane::Plane;
use crate::zones::Zones;

#[derive(Clone, Debug, Default)]
pub struct RefineStats {
    pub rounds: usize,
    pub actions: usize,
    pub cost_before: f64,
    pub cost_after: f64,
    /// Problems left at the end, by kind.
    pub left: Vec<(&'static str, usize)>,
}

/// Extra cost of one new seam continuing an existing seam.
const ALIGN_COST: f64 = 500.0;
/// Cost per piece: prefer fewer pieces when everything else is equal.
const PIECE_COST: f64 = 25.0;
/// Candidates fully evaluated per fix.
const FULL_EVALS: usize = 10;

fn piece_size(m: &Mesh) -> f32 {
    m.bbox().size().max_element()
}

fn is_small(m: &Mesh, q: &QualityParams) -> bool {
    (piece_size(m) as f64) < q.min_piece * 2.5
}

/// Scalar cost of one piece (lower is better).
pub fn piece_cost(r: &PieceReport, small: bool) -> f64 {
    let mut c = PIECE_COST;
    for f in &r.flags {
        c += match f {
            Flag::TooBig => 10_000.0,
            Flag::NeedsSupport(a) => 2_000.0 + (*a as f64 / 10.0).min(3_000.0),
            Flag::Tall => 600.0 + 200.0 * (r.ratio - TALL_RATIO).clamp(0.0, 3.0) as f64,
            Flag::Sharp => 700.0 + (r.sharp_len * 2.0 + r.corners as f64 * 40.0).min(400.0),
            Flag::Thin => 700.0 + 300.0 * r.thin,
            Flag::InZone => 1_500.0 + 50.0 * r.zone.min(40) as f64,
            _ => 0.0,
        };
    }
    if small {
        c += 300.0;
    }
    c + 60.0 * r.skive + (r.skive_len * 0.3).min(300.0) + 15.0 * (r.ratio as f64).min(2.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Kind {
    TooBig,
    Zone,
    Support,
    Tall,
    Shape,
    Aligned,
    Small,
    Skive,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::TooBig => "too big",
            Kind::Zone => "in no-cut zone",
            Kind::Support => "support",
            Kind::Tall => "tall",
            Kind::Shape => "sharp or thin",
            Kind::Aligned => "aligned seams",
            Kind::Small => "small",
            Kind::Skive => "skived cut",
        }
    }
}

#[derive(Clone, Debug)]
enum Problem {
    Piece(Kind, PieceId),
    /// A violating chain: seams of the run (any may be re-cut), and the cuts
    /// meeting it at a crossing (sliding one turns the X into two T's).
    Chain(Vec<SeamId>, Vec<SeamId>),
}

/// A fix ready to apply: replace `old` pieces by `new` solids.
struct Action {
    old: Vec<PieceId>,
    new: Vec<Solid>,
    reports: Vec<PieceReport>,
    planes: Vec<Plane>,
    gain: f64,
}

/// Symmetric 3x3 eigen decomposition (Jacobi); values descending.
pub fn sym_eigen(m: DMat3) -> ([f64; 3], [DVec3; 3]) {
    let mut a = m.to_cols_array_2d();
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..32 {
        let (mut p, mut q, mut big) = (0, 1, 0.0);
        for i in 0..3 {
            for j in i + 1..3 {
                if a[i][j].abs() > big {
                    big = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if big < 1e-15 {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let t = if theta == 0.0 { 1.0 } else { t };
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        for k in 0..3 {
            let (akp, akq) = (a[k][p], a[k][q]);
            a[k][p] = c * akp - s * akq;
            a[k][q] = s * akp + c * akq;
        }
        for k in 0..3 {
            let (apk, aqk) = (a[p][k], a[q][k]);
            a[p][k] = c * apk - s * aqk;
            a[q][k] = s * apk + c * aqk;
        }
        for k in 0..3 {
            let (vkp, vkq) = (v[k][p], v[k][q]);
            v[k][p] = c * vkp - s * vkq;
            v[k][q] = s * vkp + c * vkq;
        }
    }
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&x, &y| a[y][y].total_cmp(&a[x][x]));
    let vals = idx.map(|i| a[i][i]);
    let vecs = idx.map(|i| DVec3::new(v[0][i], v[1][i], v[2][i]).normalize_or(DVec3::X));
    (vals, vecs)
}

/// Cut directions besides those square to the surface: the axes, the
/// piece's principal axes, its cut normals and 45° blends of the first six
/// (diagonal cuts print without support).
fn candidate_normals(mesh: &Mesh, cut_normals: &[DVec3]) -> Vec<DVec3> {
    let n = mesh.verts.len().max(1) as f64;
    let c = mesh.verts.iter().fold(DVec3::ZERO, |a, v| a + v.as_dvec3()) / n;
    let mut m = DMat3::ZERO;
    for v in &mesh.verts {
        let d = v.as_dvec3() - c;
        m += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    let mut v: Vec<DVec3> = vec![DVec3::X, DVec3::Y, DVec3::Z];
    v.extend(sym_eigen(m).1);
    v.extend_from_slice(cut_normals);
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

/// Candidate cut planes through a piece, best first by a quick estimate.
/// Directions favour cuts square to the surface (in the plane of the two
/// weakest surface-normal axes); offsets spread across the piece.
fn candidates(mesh: &Mesh, cuts: &[Plane], extra: &[Plane], q: &QualityParams, zones: &Zones, avoid: &(dyn Fn(&Plane) -> bool + Sync)) -> Vec<Plane> {
    let cap = quality::cap_index(mesh, cuts);
    let mut t = DMat3::ZERO;
    for (i, c) in cap.iter().enumerate() {
        if *c != u32::MAX {
            continue;
        }
        let n = mesh.tri_cross(i).as_dvec3();
        let a = n.length();
        if a < 1e-12 {
            continue;
        }
        let u = n / a;
        t += DMat3::from_cols(u * u.x, u * u.y, u * u.z) * (a * 0.5);
    }
    let (_, e) = sym_eigen(t);
    let mut dirs: Vec<DVec3> = Vec::new();
    for k in 0..12 {
        let a = (k as f64 * 15.0).to_radians();
        dirs.push(e[1] * a.cos() + e[2] * a.sin());
    }
    dirs.extend(candidate_normals(mesh, &cuts.iter().map(|c| c.normal).collect::<Vec<_>>()));
    let mut uniq: Vec<DVec3> = Vec::new();
    for d in dirs {
        let d = d.normalize_or(DVec3::Z);
        if !uniq.iter().any(|u| u.dot(d).abs() > 0.995) {
            uniq.push(d);
        }
    }
    let mut cands: Vec<Plane> = extra.to_vec();
    for d in uniq {
        let (lo, hi) = mesh.verts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            let x = d.dot(v.as_dvec3());
            (lo.min(x), hi.max(x))
        });
        if hi - lo < q.min_piece * 2.0 {
            continue;
        }
        for f in [0.3, 0.4, 0.5, 0.6, 0.7] {
            cands.push(Plane { normal: d, offset: lo + (hi - lo) * f });
        }
    }
    let n_extra = extra.len();
    let mut scored: Vec<(f64, Plane)> = cands
        .par_iter()
        .enumerate()
        .filter_map(|(i, pl)| quick_score(mesh, &cap, cuts, pl, q, zones, avoid).map(|s| (if i < n_extra { s - 20.0 } else { s }, *pl)))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.into_iter().map(|s| s.1).collect()
}

/// Quick estimate for a plane through a piece (lower is better), or None if
/// it would make a corner sharper than allowed.
fn quick_score(mesh: &Mesh, cap: &[u32], cuts: &[Plane], pl: &Plane, q: &QualityParams, zones: &Zones, avoid: &(dyn Fn(&Plane) -> bool + Sync)) -> Option<f64> {
    let (mut len, mut graze, mut bad) = (0.0, 0.0, 0.0);
    let (mut pos, mut neg) = (0usize, 0usize);
    let sharp_cos = q.min_angle.cos();
    let mut crossed: Vec<u32> = Vec::new();
    // An estimate: on big pieces every few surface triangles will do (cut
    // faces are always checked, they decide the corner angles).
    let stride = (mesh.tris.len() / 5000).max(1);
    for (t, tri) in mesh.tris.iter().enumerate() {
        if stride > 1 && t % stride != 0 && cap[t] == u32::MAX {
            continue;
        }
        let p = tri.map(|i| mesh.verts[i as usize]);
        let d = p.map(|v| pl.dist(v));
        let above = d.iter().filter(|x| **x > 0.0).count();
        pos += above;
        neg += 3 - above;
        if above == 0 || above == 3 {
            continue;
        }
        let c = cap[t];
        if c != u32::MAX {
            if !crossed.contains(&c) {
                crossed.push(c);
            }
            continue;
        }
        // Length of the cut across this triangle.
        let mut pts = Vec::with_capacity(2);
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            if (d[a] > 0.0) != (d[b] > 0.0) {
                let s = d[a] / (d[a] - d[b]);
                pts.push(p[a] + (p[b] - p[a]) * s as f32);
            }
        }
        if pts.len() < 2 {
            continue;
        }
        let l = (pts[1] - pts[0]).length() as f64;
        let n = mesh.tri_cross(t).as_dvec3().normalize_or_zero();
        let g = n.dot(pl.normal).abs();
        len += l;
        graze += l * g * g;
        if g > sharp_cos {
            bad += l;
        }
    }
    if len <= 0.0 || pos == 0 || neg == 0 {
        return None;
    }
    // Corners with the piece's other cuts.
    for c in crossed {
        let a = cuts[c as usize].normal.dot(pl.normal).abs();
        if a > sharp_cos {
            return None;
        }
    }
    let balance = (pos as f64 - neg as f64).abs() / (pos + neg) as f64;
    let bb = mesh.bbox();
    let inside = |p: Vec3| p.cmpge(bb.min).all() && p.cmple(bb.max).all();
    let zone = zones.dabs.iter().filter(|d| pl.dist(d.p).abs() < d.radius as f64 && inside(d.p)).count() as f64;
    let align = if avoid(pl) { 1.0 } else { 0.0 };
    Some(100.0 * graze / len + 300.0 * (bad / len).min(1.0) + 15.0 * balance + 200.0 * zone + 300.0 * align)
}

/// Splits a solid by a plane and scores the bodies.
fn eval_cut(solid: &Solid, cuts: &[Plane], pl: &Plane, q: &QualityParams, align_cost: f64) -> Option<(f64, Vec<Solid>, Vec<PieceReport>)> {
    let (a, b) = solid.split_by_plane(pl.normal, pl.offset).ok()?;
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let mut bodies: Vec<(Solid, Plane)> = a.bodies().into_iter().map(|s| (s, pl.flipped())).collect();
    bodies.extend(b.bodies().into_iter().map(|s| (s, *pl)));
    bodies.retain(|b| b.0.volume() > 1e-3);
    if bodies.len() < 2 {
        return None;
    }
    let scored: Vec<(f64, PieceReport)> = bodies
        .iter()
        .map(|(s, out)| {
            let m = s.to_mesh();
            let mut c = cuts.to_vec();
            c.push(*out);
            let r = analyze_piece(&m, &c, q);
            (piece_cost(&r, is_small(&m, q)), r)
        })
        .collect();
    let cost = scored.iter().map(|x| x.0).sum::<f64>() + align_cost;
    let (solids, reports) = (bodies.into_iter().map(|b| b.0).collect(), scored.into_iter().map(|x| x.1).collect());
    Some((cost, solids, reports))
}

struct Ctx<'a> {
    deadline: Instant,
    q: &'a QualityParams,
    zones: &'a Zones,
    job: &'a JobCtx,
    align: bool,
    soft: bool,
}

impl Ctx<'_> {
    fn stop(&self) -> bool {
        self.job.is_cancelled() || Instant::now() >= self.deadline
    }
}

/// Outward cuts of a group of pieces merged into one.
fn group_cuts(p: &Partition, ids: &[PieceId]) -> Vec<Plane> {
    let mut out: Vec<Plane> = Vec::new();
    for id in ids {
        for c in quality::piece_cuts(p, *id) {
            if !out.iter().any(|o| o.normal.dot(c.normal) > 0.9999 && (o.offset - c.offset).abs() < 1e-6) {
                out.push(c);
            }
        }
    }
    out
}

/// Would a new seam on `pl` inside `region` continue an existing seam not
/// bordering `group`?
fn continues(p: &Partition, cache: &SeamCache, pl: &Plane, region: &Aabb, group: &[PieceId]) -> bool {
    continues_any(p, cache, pl, region, group, None)
}

/// Like `continues`, optionally only against the given seams.
fn continues_any(p: &Partition, cache: &SeamCache, pl: &Plane, region: &Aabb, group: &[PieceId], only: Option<&HashSet<SeamId>>) -> bool {
    let cos = 15f64.to_radians().cos();
    let r = Aabb { min: region.min - Vec3::splat(3.0), max: region.max + Vec3::splat(3.0) };
    p.seams.iter().any(|s| {
        if only.is_some_and(|o| !o.contains(&s.id)) || group.iter().any(|g| s.touches(*g)) || s.plane.normal.dot(pl.normal).abs() < cos {
            return false;
        }
        cache.get(s.id).is_some_and(|pts| {
            pts.iter().any(|v| {
                v.x >= r.min.x && v.y >= r.min.y && v.z >= r.min.z && v.x <= r.max.x && v.y <= r.max.y && v.z <= r.max.z && pl.dist(*v).abs() < 2.0
            })
        })
    })
}

/// Best way to fix a problem, if any lowers the cost.
fn plan_fix(p: &Partition, cache: &SeamCache, reports: &HashMap<PieceId, PieceReport>, prob: &Problem, cx: &Ctx, level: u8) -> Option<Action> {
    let cost_of = |id: PieceId| -> f64 {
        let q = p.piece(id).unwrap();
        reports.get(&id).map(|r| piece_cost(r, is_small(&q.mesh, cx.q))).unwrap_or(PIECE_COST)
    };
    let usable = |id: &PieceId| p.piece(*id).is_some_and(|q| !q.locked);
    let free_seam = |a: PieceId, b: PieceId| !p.seams.iter().any(|s| s.locked && s.touches(a) && s.touches(b));
    let mut groups: Vec<Group> = Vec::new();
    match prob {
        Problem::Piece(kind, id) => {
            if !usable(id) {
                return None;
            }
            // First try the cheap fix (a cut through the piece, or merging
            // a fragment away); neighbours are reworked only after that failed.
            let split_first = matches!(kind, Kind::TooBig | Kind::Support | Kind::Tall);
            if split_first {
                groups.push((vec![*id], Vec::new(), false, 0.0));
                if level == 0 {
                    return best_of(p, groups, cache, &cost_of, cx, false);
                }
            }
            // Re-divide with (or merge into) each neighbour.
            let mut nb: Vec<(f64, PieceId)> = Vec::new();
            for s in p.seams_of(*id) {
                let o = s.other(*id);
                if !usable(&o) || !free_seam(*id, o) {
                    continue;
                }
                // Seams through painted spots are reworked first.
                let reach = s.area.sqrt() as f32;
                let painted = *kind == Kind::Zone && cx.q.zones.dabs.iter().any(|d| s.plane.dist(d.p).abs() < d.radius as f64 && d.p.distance(s.center) < reach + d.radius);
                let w = s.area + if painted { 1e9 } else { 0.0 };
                match nb.iter_mut().find(|x| x.1 == o) {
                    Some(x) => x.0 += w,
                    None => nb.push((w, o)),
                }
            }
            nb.sort_by(|a, b| b.0.total_cmp(&a.0));
            let take = if level == 0 { 2 } else { 5 };
            let near: Vec<PieceId> = nb.iter().map(|x| x.1).collect();
            for (_, o) in nb.into_iter().take(take) {
                // Small moves of the seam between them come first: they keep
                // the layout and usually just step past a lip or flange.
                let extra: Vec<Plane> = p.seams_of(*id).filter(|s| s.touches(o)).flat_map(|s| nudges(s)).collect();
                groups.push((vec![*id, o], extra, matches!(kind, Kind::Small | Kind::Shape | Kind::Skive | Kind::Zone), 0.0));
            }
            // Last resort for shape problems: rework the piece with two
            // neighbours at once (the deep search may cut the union twice).
            if level >= 2 && !split_first && near.len() >= 2 {
                for pair in [(0, 1), (0, 2), (1, 2)] {
                    if let (Some(&a), Some(&b)) = (near.get(pair.0), near.get(pair.1)) {
                        groups.push((vec![*id, a, b], Vec::new(), false, 0.0));
                    }
                }
            }
            if !split_first && level > 0 {
                groups.push((vec![*id], Vec::new(), false, 0.0));
            }
        }
        Problem::Chain(seams, stems) => {
            for sid in stems {
                let Some(s) = p.seam(*sid) else { continue };
                if s.locked || !usable(&s.a) || !usable(&s.b) {
                    continue;
                }
                groups.push((vec![s.a, s.b], nudges(s), false, ALIGN_COST));
            }
            for sid in seams {
                let Some(s) = p.seam(*sid) else { continue };
                if s.locked || !usable(&s.a) || !usable(&s.b) {
                    continue;
                }
                // Rotations of the seam about the surface normal (square to
                // the surface still) and about its run, plus shifts.
                let pts = cache.get(*sid).map(|v| v.as_slice().to_vec()).unwrap_or_default();
                let trace = principal_dir(&pts, &s.plane);
                let normal_axis = s.plane.normal.cross(trace).normalize_or(s.plane.normal.any_orthonormal_vector());
                let c = s.center.as_dvec3();
                let mut extra = Vec::new();
                for deg in [22.0f64, -22.0, 35.0, -35.0, 50.0, -50.0] {
                    let q = glam::DQuat::from_axis_angle(normal_axis, deg.to_radians());
                    extra.push(Plane::through(c, q * s.plane.normal));
                }
                // A parallel shift also puts a jog in the run.
                for d in [20.0, -20.0, 35.0, -35.0] {
                    extra.push(s.plane.moved(d));
                }
                // The existing run stays a reference: re-dividing the union
                // pays the alignment cost again only if it continues it.
                groups.push((vec![s.a, s.b], extra, true, ALIGN_COST));
            }
        }
    }
    best_of(p, groups, cache, &cost_of, cx, level >= 2)
}

type Group = (Vec<PieceId>, Vec<Plane>, bool, f64);

/// Best action over groups of pieces to rework: (pieces, extra planes to
/// try, may merge them without a cut, extra cost of the current state).
/// `deep` tries more planes and, if no single cut helps, a second cut through
/// the worst resulting piece.
fn best_of(p: &Partition, groups: Vec<Group>, cache: &SeamCache, cost_of: &dyn Fn(PieceId) -> f64, cx: &Ctx, deep: bool) -> Option<Action> {
    let mut best: Option<Action> = None;
    for (ids, extra, may_merge, before_extra) in groups {
        if cx.stop() {
            break;
        }
        let before: f64 = ids.iter().map(|i| cost_of(*i)).sum::<f64>() + before_extra;
        let solids: Vec<&Solid> = ids.iter().map(|i| p.piece(*i).unwrap().solid.as_ref()).collect();
        let union = if solids.len() == 1 { solids[0].clone() } else {
            match Solid::batch(&solids, Op::Union) {
                Ok(u) => u,
                Err(_) => continue,
            }
        };
        let mesh = union.to_mesh();
        let cuts = group_cuts(p, &ids);
        let region = mesh.bbox();
        let avoid = |pl: &Plane| cx.align && continues(p, cache, pl, &region, &ids);
        let mut options: Vec<Opt> = Vec::new();
        if may_merge && ids.len() > 1 && union.bodies().len() == 1 {
            let r = analyze_piece(&mesh, &cuts, cx.q);
            let c = piece_cost(&r, is_small(&mesh, cx.q));
            options.push((c, vec![union.clone()], vec![r], Vec::new()));
        }
        let take = if deep { FULL_EVALS * 3 } else { FULL_EVALS };
        options.extend(cut_options(&union, &mesh, &cuts, &extra, take, &avoid, cx));
        options.sort_by(|a, b| a.0.total_cmp(&b.0));
        if deep && options.first().is_none_or(|o| o.0 >= before - 1.0) {
            // Two cuts: re-cut the worst piece of the best few single cuts.
            let mut two: Vec<Opt> = Vec::new();
            for o in options.iter().filter(|o| o.1.len() >= 2).take(4) {
                let costs: Vec<f64> = o.1.iter().zip(&o.2).map(|(s, r)| piece_cost(r, is_small(&s.to_mesh(), cx.q))).collect();
                let Some(w) = (0..costs.len()).max_by(|a, b| costs[*a].total_cmp(&costs[*b])) else { continue };
                let wm = o.1[w].to_mesh();
                let mut wc = cuts.clone();
                wc.extend(o.3.iter().map(|pl| if pl.dist(wm.centroid()) > 0.0 { pl.flipped() } else { *pl }));
                let wb = wm.bbox();
                let wavoid = |pl: &Plane| cx.align && continues(p, cache, pl, &wb, &ids);
                if let Some(sub) = cut_options(&o.1[w], &wm, &wc, &[], FULL_EVALS, &wavoid, cx).into_iter().min_by(|a, b| a.0.total_cmp(&b.0)) {
                    let mut solids: Vec<Solid> = o.1.iter().enumerate().filter(|(i, _)| *i != w).map(|(_, s)| s.clone()).collect();
                    let mut reports: Vec<PieceReport> = o.2.iter().enumerate().filter(|(i, _)| *i != w).map(|(_, r)| r.clone()).collect();
                    solids.extend(sub.1);
                    reports.extend(sub.2);
                    let mut planes = o.3.clone();
                    planes.extend(sub.3);
                    two.push((o.0 - costs[w] + sub.0, solids, reports, planes));
                }
            }
            options.extend(two);
            options.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        if let Some(o) = options.into_iter().next() {
            let gain = before - o.0;
            if gain > 1.0 && best.as_ref().is_none_or(|b| gain > b.gain) {
                best = Some(Action { old: ids.clone(), new: o.1, reports: o.2, planes: o.3, gain });
            }
        }
    }
    best
}

/// (cost, bodies, their reports, planes cut).
type Opt = (f64, Vec<Solid>, Vec<PieceReport>, Vec<Plane>);

/// The best `take` candidate cuts through a solid, fully evaluated.
fn cut_options(solid: &Solid, mesh: &Mesh, cuts: &[Plane], extra: &[Plane], take: usize, avoid: &(dyn Fn(&Plane) -> bool + Sync), cx: &Ctx) -> Vec<Opt> {
    let cands = candidates(mesh, cuts, extra, cx.q, cx.zones, avoid);
    cands
        .into_iter()
        .take(take)
        .collect::<Vec<_>>()
        .par_iter()
        .filter_map(|pl| {
            if cx.stop() {
                return None;
            }
            let ac = if avoid(pl) { ALIGN_COST } else { 0.0 };
            eval_cut(solid, cuts, pl, cx.q, ac).map(|(c, s, r)| (c, s, r, vec![*pl]))
        })
        .collect()
}

/// Small variations of a seam's plane: shifts, and tilts about two axes in
/// the plane through the seam centre.
fn nudges(s: &crate::partition::Seam) -> Vec<Plane> {
    let mut out = Vec::new();
    for d in [6.0, -6.0, 14.0, -14.0, 28.0, -28.0] {
        out.push(s.plane.moved(d));
    }
    let (u, v) = s.plane.basis();
    let c = s.plane.project(s.center.as_dvec3());
    for axis in [u, v] {
        for deg in [9.0f64, -9.0, 18.0, -18.0] {
            out.push(s.plane.tilted(c, axis, deg.to_radians()));
        }
    }
    out
}

fn principal_dir(pts: &[Vec3], plane: &Plane) -> DVec3 {
    let (u, v) = plane.basis();
    if pts.len() < 2 {
        return u;
    }
    let m = pts.iter().fold(DVec3::ZERO, |a, p| a + p.as_dvec3()) / pts.len() as f64;
    let mut c = DMat3::ZERO;
    for p in pts {
        let d = p.as_dvec3() - m;
        c += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    let (_, e) = sym_eigen(c);
    let d = e[0] - plane.normal * e[0].dot(plane.normal);
    let _ = v;
    if d.length() < 1e-6 { u } else { d.normalize() }
}

fn problems(p: &Partition, reports: &HashMap<PieceId, PieceReport>, chains: &[Chain], cx: &Ctx) -> Vec<(Kind, Problem)> {
    let mut out: Vec<(Kind, Problem)> = Vec::new();
    for q in &p.pieces {
        if q.locked {
            continue;
        }
        let Some(r) = reports.get(&q.id) else { continue };
        let kind = if r.flags.contains(&Flag::TooBig) {
            Some(Kind::TooBig)
        } else if r.flags.contains(&Flag::InZone) {
            Some(Kind::Zone)
        } else if r.flags.iter().any(|f| matches!(f, Flag::NeedsSupport(_))) {
            Some(Kind::Support)
        } else if r.flags.contains(&Flag::Tall) {
            Some(Kind::Tall)
        } else if r.flags.iter().any(|f| matches!(f, Flag::Sharp | Flag::Thin)) {
            Some(Kind::Shape)
        } else if is_small(&q.mesh, cx.q) {
            Some(Kind::Small)
        } else if cx.soft && r.cut_len > 0.0 && r.skive_len > 0.2 * r.cut_len {
            Some(Kind::Skive)
        } else {
            None
        };
        if let Some(k) = kind {
            out.push((k, Problem::Piece(k, q.id)));
        }
    }
    if cx.align {
        for c in chains.iter().filter(|c| c.is_violation(cx.q)) {
            // Re-cut the crossing seams, or the middle of a long run.
            let mut seams: Vec<SeamId> = c.crossings.iter().flat_map(|x| [x.0, x.1]).collect();
            if seams.is_empty() {
                let mut by: Vec<(f32, SeamId)> = c.seams.iter().filter_map(|id| p.seam(*id)).map(|s| (s.center.length(), s.id)).collect();
                by.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mid = by.len() / 2;
                seams = by.iter().skip(mid.saturating_sub(1)).take(3).map(|x| x.1).collect();
            }
            seams.sort_unstable();
            seams.dedup();
            seams.truncate(4);
            // Cuts between the pieces on either side of each crossing.
            let mut stems: Vec<SeamId> = Vec::new();
            for (x, y) in &c.crossings {
                let (Some(a), Some(b)) = (p.seam(*x), p.seam(*y)) else { continue };
                for s in &p.seams {
                    let joins = |u: PieceId, v: PieceId| s.touches(u) && s.touches(v) && u != v;
                    if [a.a, a.b].iter().any(|u| [b.a, b.b].iter().any(|v| joins(*u, *v))) {
                        stems.push(s.id);
                    }
                }
            }
            stems.sort_unstable();
            stems.dedup();
            stems.truncate(4);
            out.push((Kind::Aligned, Problem::Chain(seams, stems)));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Pieces a problem is about. With `context`, also their neighbours (a fix
/// tried on unchanged surroundings fails the same way again).
fn problem_pieces(p: &Partition, prob: &Problem, context: bool) -> Vec<PieceId> {
    match prob {
        Problem::Piece(_, id) => {
            let mut v = vec![*id];
            if context {
                v.extend(p.neighbours(*id));
            }
            v
        }
        Problem::Chain(seams, stems) => seams.iter().chain(stems).filter_map(|s| p.seam(*s)).flat_map(|s| [s.a, s.b]).collect(),
    }
}

/// Runs the improvement rounds. `report` receives the partition after each
/// round with a status line (for previews).
pub fn refine(p: &mut Partition, q: &QualityParams, zones: &Zones, deadline: Instant, job: &JobCtx, report: &dyn Fn(&Partition, &str)) -> RefineStats {
    let cx = Ctx { deadline, q, zones, job, align: true, soft: true };
    let all: Vec<PieceId> = p.pieces.iter().map(|x| x.id).collect();
    let mut reports = quality::analyze(p, &all, q);
    let mut cache = SeamCache::default();
    let mut stats = RefineStats::default();
    let total = |p: &Partition, reports: &HashMap<PieceId, PieceReport>| -> f64 { p.pieces.iter().map(|x| reports.get(&x.id).map(|r| piece_cost(r, is_small(&x.mesh, q))).unwrap_or(0.0)).sum() };
    stats.cost_before = total(p, &reports);
    // How far each problem got on unchanged pieces: 1 = the cheap fix
    // failed, 2 = reworking neighbours failed too, 3 = the deep search
    // failed (not retried).
    let mut failed: HashMap<Vec<PieceId>, u8> = HashMap::new();
    loop {
        if job.is_cancelled() || Instant::now() >= deadline {
            break;
        }
        stats.rounds += 1;
        cache.update(p);
        let chains = quality::chains(p, &cache);
        let probs = problems(p, &reports, &chains, &cx);
        let open: Vec<&(Kind, Problem)> = probs.iter().filter(|(_, pr)| {
            let mut key = problem_pieces(p, pr, true);
            key.sort_unstable();
            failed.get(&key).is_none_or(|l| *l < 3)
        }).collect();
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for (k, _) in &probs {
            match counts.iter_mut().find(|c| c.0 == k.name()) {
                Some(c) => c.1 += 1,
                None => counts.push((k.name(), 1)),
            }
        }
        stats.left = counts.clone();
        let msg = if counts.is_empty() { format!("{} pieces, all checks pass", p.pieces.len()) } else { format!("{} pieces · fixing {}", p.pieces.len(), counts.iter().map(|c| format!("{} {}", c.1, c.0)).collect::<Vec<_>>().join(", ")) };
        job.set_stage(format!("Improving pieces: {msg}"));
        report(p, &msg);
        if open.is_empty() {
            break;
        }
        // Independent problems (no shared pieces) are planned in parallel.
        let mut claimed: HashSet<PieceId> = HashSet::new();
        let mut batch: Vec<&Problem> = Vec::new();
        for (_, pr) in &open {
            let ps = problem_pieces(p, pr, false);
            if ps.iter().any(|x| claimed.contains(x)) {
                continue;
            }
            claimed.extend(ps);
            batch.push(pr);
            if batch.len() >= 256 {
                break;
            }
        }
        let planned: Vec<(Vec<PieceId>, Option<Action>)> = batch
            .par_iter()
            .map(|pr| {
                let mut key = problem_pieces(p, pr, true);
                key.sort_unstable();
                if job.is_cancelled() || Instant::now() >= deadline {
                    return (Vec::new(), None);
                }
                let level = failed.get(&key).copied().unwrap_or(0);
                (key, plan_fix(p, &cache, &reports, pr, &cx, level))
            })
            .collect();
        let mut applied = 0;
        // A failed cheap fix still moves on (to the thorough one next round).
        let mut progressed = false;
        let mut fresh: HashSet<SeamId> = HashSet::new();
        for (key, act) in planned {
            let Some(act) = act else {
                if !key.is_empty() {
                    *failed.entry(key).or_insert(0) += 1;
                    progressed = true;
                }
                continue;
            };
            if act.old.iter().any(|id| p.piece(*id).is_none()) {
                continue;
            }
            // Fixes were planned side by side: a cut lining up with a seam
            // made earlier this round waits and is planned again next round.
            if !fresh.is_empty() {
                let region = act.new.iter().fold(Aabb::EMPTY, |b, s| b.union(&s.bbox()));
                if act.planes.iter().any(|pl| continues_any(p, &cache, pl, &region, &act.old, Some(&fresh))) {
                    progressed = true;
                    continue;
                }
            }
            let ids = p.replace(&act.old, act.new, &act.planes);
            fresh.extend(p.seams.iter().filter(|s| ids.iter().any(|i| s.touches(*i))).map(|s| s.id));
            cache.update(p);
            for id in &act.old {
                reports.remove(id);
            }
            if ids.len() == act.reports.len() {
                for (id, r) in ids.iter().zip(act.reports) {
                    reports.insert(*id, r);
                }
            } else {
                reports.extend(quality::analyze(p, &ids, q));
            }
            applied += 1;
            stats.actions += 1;
        }
        if applied == 0 && !progressed {
            break;
        }
    }
    stats.cost_after = total(p, &reports);
    quality::apply_reports(p, &reports);
    flag_small(p, q);
    cache.update(p);
    let chains = quality::chains(p, &cache);
    let bad = quality::aligned_pieces(p, &chains, q);
    for x in p.pieces.iter_mut() {
        x.flags.retain(|f| *f != Flag::Aligned);
        if bad.contains(&x.id) {
            x.flags.push(Flag::Aligned);
        }
    }
    stats
}

/// Flags fragments smaller than the smallest piece size.
pub fn flag_small(p: &mut Partition, q: &QualityParams) {
    for x in p.pieces.iter_mut() {
        x.flags.retain(|f| *f != Flag::Small);
        if !x.locked && is_small(&x.mesh, q) {
            x.flags.push(Flag::Small);
        }
    }
}

/// Best single cut through one piece and how much it lowers the cost
/// (negative if it makes things worse).
pub fn best_split(p: &Partition, id: PieceId, q: &QualityParams, zones: &Zones, job: &JobCtx) -> Option<(Plane, f64)> {
    let piece = p.piece(id)?;
    let cuts = quality::piece_cuts(p, id);
    let before = piece_cost(&analyze_piece(&piece.mesh, &cuts, q), is_small(&piece.mesh, q));
    let cands = candidates(&piece.mesh, &cuts, &[], q, zones, &|_| false);
    let best = cands
        .into_iter()
        .take(FULL_EVALS * 2)
        .collect::<Vec<_>>()
        .par_iter()
        .filter_map(|pl| {
            if job.is_cancelled() {
                return None;
            }
            eval_cut(&piece.solid, &cuts, pl, q, 0.0).map(|(c, _, _)| (c, *pl))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    Some((best.1, before - best.0))
}

/// A better layout for one piece and its surroundings ("Find a better cut").
pub struct Improvement {
    /// Pieces to replace.
    pub old: Vec<PieceId>,
    pub new: Vec<Solid>,
    /// The new cuts.
    pub planes: Vec<Plane>,
}

/// "Find a better cut" for one piece. Tries, in order, and keeps the first
/// kind of change that lowers the cost of the pieces involved:
/// 1. merging it with a neighbour and cutting again a little to either side
///    of their shared cut;
/// 2. the same with the cut's angle changed;
/// 3. splitting the piece into two pieces, or three if one cut is not enough.
pub fn improve_piece(p: &Partition, id: PieceId, q: &QualityParams, job: &JobCtx) -> Option<Improvement> {
    let piece = p.piece(id).filter(|x| !x.locked)?;
    let cost = |ids: &[PieceId]| -> f64 {
        ids.iter()
            .filter_map(|i| p.piece(*i))
            .map(|x| piece_cost(&analyze_piece(&x.mesh, &quality::piece_cuts(p, x.id), q), is_small(&x.mesh, q)))
            .sum()
    };
    // Neighbours across free seams, the longest contact first.
    let mut seams: Vec<&crate::partition::Seam> = p.seams_of(id).filter(|s| !s.locked && p.piece(s.other(id)).is_some_and(|o| !o.locked)).collect();
    seams.sort_by(|a, b| b.area.total_cmp(&a.area));
    seams.truncate(6);
    let pairs: Vec<(Vec<PieceId>, Solid, Vec<Plane>, f64, &crate::partition::Seam)> = seams
        .par_iter()
        .filter_map(|s| {
            let ids = vec![id, s.other(id)];
            let solids: Vec<&Solid> = ids.iter().map(|i| p.piece(*i).unwrap().solid.as_ref()).collect();
            let u = Solid::batch(&solids, Op::Union).ok()?;
            Some((ids.clone(), u, group_cuts(p, &ids), cost(&ids), *s))
        })
        .collect();
    let try_planes = |variants: &dyn Fn(&crate::partition::Seam) -> Vec<Plane>| -> Option<Improvement> {
        let jobs: Vec<(usize, Plane)> = pairs.iter().enumerate().flat_map(|(k, pr)| variants(pr.4).into_iter().map(move |pl| (k, pl))).collect();
        let best = jobs
            .par_iter()
            .filter_map(|(k, pl)| {
                if job.is_cancelled() {
                    return None;
                }
                let (u, cuts, before) = (&pairs[*k].1, &pairs[*k].2, pairs[*k].3);
                let (c, bodies, _) = eval_cut(u, cuts, pl, q, 0.0)?;
                (c < before - 1.0).then_some((before - c, *k, bodies, *pl))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))?;
        Some(Improvement { old: pairs[best.1].0.clone(), new: best.2, planes: vec![best.3] })
    };
    // 1. The shared cut moved a little.
    let shifts = |s: &crate::partition::Seam| [3.0, -3.0, 6.0, -6.0, 10.0, -10.0, 15.0, -15.0, 22.0, -22.0, 30.0, -30.0].map(|d| s.plane.moved(d)).to_vec();
    if let Some(i) = try_planes(&shifts) {
        return Some(i);
    }
    // 2. The shared cut turned (about two axes in the cut, through its centre).
    let turns = |s: &crate::partition::Seam| {
        let (u, v) = s.plane.basis();
        let c = s.plane.project(s.center.as_dvec3());
        let mut out = Vec::new();
        for axis in [u, v] {
            for deg in [5.0f64, -5.0, 10.0, -10.0, 15.0, -15.0, 22.0, -22.0, 30.0, -30.0] {
                out.push(s.plane.tilted(c, axis, deg.to_radians()));
            }
        }
        out
    };
    if let Some(i) = try_planes(&turns) {
        return Some(i);
    }
    // 3. Split the piece itself, with a second cut if one is not enough.
    let cx = Ctx { deadline: Instant::now() + Duration::from_secs(120), q, zones: &q.zones, job, align: false, soft: false };
    let cache = SeamCache::default();
    let before = cost(&[id]);
    let act = best_of(p, vec![(vec![id], Vec::new(), false, 0.0)], &cache, &|_| before, &cx, true)?;
    let _ = piece;
    Some(Improvement { old: act.old, new: act.new, planes: act.planes })
}

/// The cut a user drew across a piece: `pts` are points of the stroke on the
/// surface and `normals` the surface normals there. The stroke is fitted
/// with a straight line; the plane through it starts square to the surface
/// and is then tilted, turned and shifted a little where that makes the two
/// halves print better.
pub fn cut_along(p: &Partition, id: PieceId, pts: &[Vec3], normals: &[Vec3], q: &QualityParams, job: &JobCtx) -> Option<Plane> {
    let piece = p.piece(id)?;
    if pts.len() < 2 {
        return None;
    }
    let c = pts.iter().fold(DVec3::ZERO, |a, v| a + v.as_dvec3()) / pts.len() as f64;
    let mut cov = DMat3::ZERO;
    for v in pts {
        let d = v.as_dvec3() - c;
        cov += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    let (_, e) = sym_eigen(cov);
    let dir = e[0];
    let half = pts.iter().map(|v| (v.as_dvec3() - c).dot(dir).abs()).fold(0.0, f64::max).max(1.0);
    let avg = normals.iter().fold(DVec3::ZERO, |a, n| a + n.as_dvec3());
    let up = (avg - dir * avg.dot(dir)).normalize_or(dir.any_orthonormal_vector());
    let base = dir.cross(up).normalize();
    let cuts = quality::piece_cuts(p, id);
    // Turning the line is limited so its ends stay within ~8 mm of the stroke.
    let yaw_max = (8.0 / half).atan().min(10f64.to_radians());
    let mut cands: Vec<(f64, Plane)> = Vec::new();
    for tilt in [0.0f64, 8.0, -8.0, 16.0, -16.0, 25.0, -25.0] {
        let qt = glam::DQuat::from_axis_angle(dir, tilt.to_radians());
        for yaw in [0.0f64, 0.5, -0.5, 1.0, -1.0] {
            let qy = glam::DQuat::from_axis_angle(up, yaw * yaw_max);
            let n = (qy * qt * base).normalize();
            for shift in [0.0f64, 3.0, -3.0, 6.0, -6.0] {
                // Deviation from what was drawn, in cost units.
                let dev = tilt.abs() * 3.0 + yaw.abs() * yaw_max.to_degrees() * 3.0 + shift.abs() * 4.0;
                cands.push((dev, Plane::through(c + base * shift, n)));
            }
        }
    }
    let cap = quality::cap_index(&piece.mesh, &cuts);
    let mut scored: Vec<(f64, f64, Plane)> = cands
        .par_iter()
        .filter_map(|(dev, pl)| quick_score(&piece.mesh, &cap, &cuts, pl, q, &q.zones, &|_| false).map(|s| (s + dev, *dev, *pl)))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    let best = scored
        .iter()
        .take(FULL_EVALS + 4)
        .collect::<Vec<_>>()
        .par_iter()
        .filter_map(|(_, dev, pl)| {
            if job.is_cancelled() {
                return None;
            }
            eval_cut(&piece.solid, &cuts, pl, q, 0.0).map(|(cost, _, _)| (cost + dev, *pl))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match best {
        Some(b) => Some(b.1),
        // Nothing scored (every variant makes a sharp corner): cut as drawn.
        None => {
            let pl = Plane::through(c, base);
            let (a, b) = piece.solid.split_by_plane(pl.normal, pl.offset).ok()?;
            (!a.is_empty() && !b.is_empty()).then_some(pl)
        }
    }
}
