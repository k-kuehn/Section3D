//! Fast scoring of a layout on a simplified proxy of the model.
//!
//! Each proxy triangle is clipped into the cells it crosses. Within a cell,
//! connected triangles form islands (separate pieces after cutting). Each
//! island is tried resting on each of its cut faces and scored by the
//! priorities: fits > no supports > not tall > size > seams.

use std::collections::HashMap;

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use super::layout::{Frame, Layout, Line, Row};
use crate::job::JobCtx;
use crate::mesh::Mesh;
use crate::orient::SUPPORT_NOISE;
use crate::voxel::Occupancy;
use crate::zones::Zones;

/// Simplified model with per-triangle data used by the search.
pub struct Proxy {
    pub mesh: Mesh,
    pub normals: Vec<Vec3>,
    pub area: Vec<f32>,
    /// Neighbour across each edge (u32::MAX = none).
    pub adj: Vec<[u32; 3]>,
    /// 0..1, how sharp the creases at this triangle are (cuts avoid them).
    pub detail: Vec<f32>,
    /// 1 inside a painted no-cut zone.
    pub zone: Vec<f32>,
    pub occ: Option<Occupancy>,
    /// Polygons closer than this belong to the same island (mm). Covers the
    /// two faces of a thin wall and gaps left by simplification.
    pub island_gap: f64,
}

impl Proxy {
    pub fn new(full: &Mesh, target_tris: usize, zones: &Zones, job: &JobCtx) -> Proxy {
        let mesh = crate::simplify::cluster(full, target_tris);
        let normals: Vec<Vec3> = mesh.face_normals();
        let area: Vec<f32> = (0..mesh.tris.len()).map(|t| mesh.tri_cross(t).length() * 0.5).collect();
        // Edge adjacency.
        let mut edges: Vec<(u64, u32, u8)> = Vec::with_capacity(mesh.tris.len() * 3);
        for (t, tri) in mesh.tris.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                edges.push((((a.min(b) as u64) << 32) | a.max(b) as u64, t as u32, k as u8));
            }
        }
        edges.par_sort_unstable();
        let mut adj = vec![[u32::MAX; 3]; mesh.tris.len()];
        for g in edges.chunk_by(|a, b| a.0 == b.0) {
            if g.len() == 2 {
                adj[g[0].1 as usize][g[0].2 as usize] = g[1].1;
                adj[g[1].1 as usize][g[1].2 as usize] = g[0].1;
            }
        }
        let detail: Vec<f32> = (0..mesh.tris.len())
            .map(|t| {
                let n = normals[t];
                adj[t].iter().filter(|&&o| o != u32::MAX).map(|&o| (1.0 - n.dot(normals[o as usize])).clamp(0.0, 2.0) * 0.5).fold(0.0f32, f32::max)
            })
            .collect();
        let zone: Vec<f32> = (0..mesh.tris.len())
            .map(|t| {
                let c = (mesh.tri(t)[0] + mesh.tri(t)[1] + mesh.tri(t)[2]) / 3.0;
                if zones.dabs.iter().any(|d| d.p.distance(c) < d.radius) { 1.0 } else { 0.0 }
            })
            .collect();
        let occ = crate::voxel::occupancy(&mesh, 160, job).ok();
        let total: f64 = area.iter().map(|a| *a as f64).sum();
        let spacing = (2.0 * total / mesh.tris.len().max(1) as f64).sqrt();
        let island_gap = (spacing * 1.5).max(6.0);
        Proxy { mesh, normals, area, adj, detail, zone, occ, island_gap }
    }
}

/// Proxy coordinates in one frame.
pub struct FrameData {
    pub frame: Frame,
    pub verts: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    pub lo: DVec3,
    pub hi: DVec3,
    /// Per triangle: (min v, max v).
    pub vrange: Vec<(f64, f64)>,
}

impl FrameData {
    pub fn new(p: &Proxy, frame: Frame) -> FrameData {
        let verts: Vec<DVec3> = p.mesh.verts.par_iter().map(|v| frame.local(*v)).collect();
        let normals: Vec<DVec3> = p.normals.iter().map(|n| frame.local_dir(*n)).collect();
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for v in &verts {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        let vrange = p
            .mesh
            .tris
            .iter()
            .map(|t| {
                let ys = t.map(|i| verts[i as usize].y);
                (ys[0].min(ys[1]).min(ys[2]), ys[0].max(ys[1]).max(ys[2]))
            })
            .collect();
        FrameData { frame, verts, normals, lo, hi, vrange }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EvalParams {
    /// Usable build volume.
    pub bed: [f64; 3],
    pub sin_overhang: f64,
    pub min_piece: f64,
}

/// Lexicographic layout score; lower is better.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    pub unfit: u32,
    /// Pieces that need support, and their total support area (mm²).
    pub support_pieces: u32,
    pub support: f64,
    pub tall: u32,
    pub pieces: f64,
    pub seam: f64,
}

impl Score {
    /// Expected piece count after flagged pieces get a fourth cut: a piece
    /// needing support or standing tall usually becomes two.
    pub fn effective_pieces(&self) -> f64 {
        self.pieces + 1.5 * self.support_pieces as f64 + self.tall as f64
    }

    fn key(&self) -> (u32, i64, u32, u32, i64, i64) {
        (self.unfit, (self.effective_pieces() * 2.0).round() as i64, self.support_pieces, self.tall, (self.support / 1000.0).round() as i64, self.seam.round() as i64)
    }
    pub fn better_than(&self, o: &Score) -> bool {
        self.key() < o.key()
    }
    pub fn printable(&self) -> bool {
        self.unfit == 0 && self.support_pieces == 0 && self.tall == 0
    }
    pub fn add(&mut self, o: &Score) {
        self.unfit += o.unfit;
        self.support_pieces += o.support_pieces;
        self.support += o.support;
        self.tall += o.tall;
        self.pieces += o.pieces;
        self.seam += o.seam;
    }
}

#[derive(Clone, Debug, Default)]
pub struct RowEval {
    pub score: Score,
    /// Cut edges (world coordinates) for previews, when requested.
    pub edges: Vec<[Vec3; 2]>,
}

const BOTTOM: u8 = 1;
const TOP: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;
const FRONT: u8 = 16;
const BACK: u8 = 32;

#[derive(Clone, Copy)]
struct V {
    p: DVec3,
    m: u8,
}

/// Keeps the part of `poly` where `f >= 0`; new vertices get `bit`.
fn clip(poly: &[V], f: impl Fn(DVec3) -> f64, bit: u8, out: &mut Vec<V>) {
    out.clear();
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let (da, db) = (f(a.p), f(b.p));
        if da >= 0.0 {
            out.push(if da == 0.0 { V { p: a.p, m: a.m | bit } } else { a });
        }
        if (da >= 0.0) != (db >= 0.0) && da != 0.0 && db != 0.0 {
            let t = da / (da - db);
            out.push(V { p: a.p + (b.p - a.p) * t, m: (a.m & b.m) | bit });
        }
    }
}

#[inline]
fn line_dist(l: &Line, row: &Row, p: DVec3) -> f64 {
    let t = (p.y - row.v0) / (row.v1 - row.v0);
    p.x - l.at(t)
}

fn poly_area(poly: &[V]) -> f64 {
    let mut c = DVec3::ZERO;
    for i in 1..poly.len().saturating_sub(1) {
        c += (poly[i].p - poly[0].p).cross(poly[i + 1].p - poly[0].p);
    }
    c.length() * 0.5
}

struct Piece {
    pts_start: usize,
    pts_end: usize,
    polys: Vec<(u32, usize, usize, f64)>, // (tri, start, end, area) into the row arena
}

/// Evaluates one row of a layout.
pub fn eval_row(p: &Proxy, fd: &FrameData, lay: &Layout, ri: usize, tris: &[u32], ep: &EvalParams, want_edges: bool) -> RowEval {
    let row = &lay.rows[ri];
    let mut arena: Vec<DVec3> = Vec::with_capacity(tris.len() * 8);
    // (cell key, tri, start, end, area)
    let mut polys: Vec<(u32, u32, usize, usize, f64)> = Vec::with_capacity(tris.len() * 2);
    let mut seam = 0.0;
    let mut edges = Vec::new();
    let (mut a, mut b) = (Vec::with_capacity(12), Vec::with_capacity(12));
    let nd = row.depth_cells() as u32;
    // Cell lookup helpers on the ordered lines.
    let lines = &row.lines;
    for &t in tris {
        let tri = p.mesh.tris[t as usize];
        a.clear();
        for i in tri {
            a.push(V { p: fd.verts[i as usize], m: 0 });
        }
        clip(&a, |q| q.y - row.v0, BOTTOM, &mut b);
        if b.len() < 3 {
            continue;
        }
        clip(&b, |q| row.v1 - q.y, TOP, &mut a);
        if a.len() < 3 {
            continue;
        }
        let slab = a.clone();
        let (umin, umax) = slab.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.p.x), hi.max(v.p.x)));
        let first = lines.partition_point(|l| l.max() < umin);
        let last = lines.partition_point(|l| l.min() <= umax);
        for cell in first..=last.min(lines.len()) {
            let mut poly = slab.clone();
            if cell > 0 {
                let l = &lines[cell - 1];
                clip(&poly, |q| line_dist(l, row, q), LEFT, &mut b);
                std::mem::swap(&mut poly, &mut b);
                if poly.len() < 3 {
                    continue;
                }
            }
            if cell < lines.len() {
                let r = &lines[cell];
                clip(&poly, |q| -line_dist(r, row, q), RIGHT, &mut b);
                std::mem::swap(&mut poly, &mut b);
                if poly.len() < 3 {
                    continue;
                }
            }
            for d in 0..nd as usize {
                let (w0, w1) = lay.depth_bounds(row, d);
                let mut q = poly.clone();
                if d > 0 {
                    clip(&q, |x| x.z - w0, FRONT, &mut b);
                    std::mem::swap(&mut q, &mut b);
                    if q.len() < 3 {
                        continue;
                    }
                }
                if d + 1 < nd as usize {
                    clip(&q, |x| w1 - x.z, BACK, &mut b);
                    std::mem::swap(&mut q, &mut b);
                    if q.len() < 3 {
                        continue;
                    }
                }
                let area = poly_area(&q);
                if area < 1e-9 {
                    continue;
                }
                // Cut edges: consecutive vertices on the same internal cut.
                let internal = |bit: u8| match bit {
                    BOTTOM => ri > 0,
                    TOP => ri + 1 < lay.rows.len(),
                    LEFT => cell > 0,
                    RIGHT => cell < lines.len(),
                    FRONT => d > 0,
                    _ => d + 1 < nd as usize,
                };
                for i in 0..q.len() {
                    let (x, y) = (q[i], q[(i + 1) % q.len()]);
                    let common = x.m & y.m;
                    if common != 0 && [BOTTOM, TOP, LEFT, RIGHT, FRONT, BACK].iter().any(|&bit| common & bit != 0 && internal(bit)) {
                        let len = (y.p - x.p).length();
                        let w = 1.0 + 4.0 * p.detail[t as usize] as f64 + 50.0 * p.zone[t as usize] as f64;
                        seam += 0.5 * len * w;
                        if want_edges {
                            edges.push([fd.frame.world(x.p).as_vec3(), fd.frame.world(y.p).as_vec3()]);
                        }
                    }
                }
                let start = arena.len();
                arena.extend(q.iter().map(|v| v.p));
                polys.push(((cell as u32) * nd + d as u32, t, start, arena.len(), area));
            }
        }
    }
    polys.sort_unstable_by_key(|x| (x.0, x.1));
    let mut score = Score { seam, ..Default::default() };
    // Group by cell, split into islands, evaluate each.
    let mut i = 0;
    let mut extra: Vec<DVec3> = Vec::new();
    let mut seen_cells: Vec<u32> = Vec::new();
    while i < polys.len() {
        let key = polys[i].0;
        let mut j = i;
        while j < polys.len() && polys[j].0 == key {
            j += 1;
        }
        seen_cells.push(key);
        let group = &polys[i..j];
        let cell = (key / nd) as usize;
        let d = (key % nd) as usize;
        // Islands by proximity: polygons with vertices in the same or
        // neighbouring grid buckets join (robust to proxy topology).
        let mut parent: Vec<usize> = (0..group.len()).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        fn union(p: &mut [usize], a: usize, b: usize) {
            let (x, y) = (find(p, a), find(p, b));
            if x != y {
                p[x.max(y)] = x.min(y);
            }
        }
        let gap = p.island_gap;
        let mut buckets: HashMap<(i32, i32, i32), usize> = HashMap::with_capacity(group.len() * 2);
        for (k, g) in group.iter().enumerate() {
            for q in &arena[g.2..g.3] {
                let key = ((q.x / gap).floor() as i32, (q.y / gap).floor() as i32, (q.z / gap).floor() as i32);
                match buckets.get(&key) {
                    Some(&j) => union(&mut parent, k, j),
                    None => {
                        buckets.insert(key, k);
                    }
                }
            }
        }
        const FWD: [(i32, i32, i32); 13] = [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 0), (1, -1, 0), (1, 0, 1), (1, 0, -1), (0, 1, 1), (0, 1, -1), (1, 1, 1), (1, 1, -1), (1, -1, 1), (1, -1, -1)];
        let keys: Vec<((i32, i32, i32), usize)> = buckets.iter().map(|(k, v)| (*k, *v)).collect();
        for (key, k) in keys {
            for o in FWD {
                if let Some(&j) = buckets.get(&(key.0 + o.0, key.1 + o.1, key.2 + o.2)) {
                    union(&mut parent, k, j);
                }
            }
        }
        // Solid interior: corners of the cell inside the material join all islands.
        extra.clear();
        corners_inside(p, fd, lay, row, cell, d, &mut extra);
        let mut comps: HashMap<usize, Vec<usize>> = HashMap::new();
        for k in 0..group.len() {
            let r = if extra.is_empty() { find(&mut parent, k) } else { 0 };
            comps.entry(r).or_default().push(k);
        }
        for (_, members) in comps {
            let area: f64 = members.iter().map(|&k| group[k].4).sum();
            if extra.is_empty() && area < (ep.min_piece * ep.min_piece).max(100.0) {
                continue;
            }
            let pc = Piece { pts_start: 0, pts_end: 0, polys: members.iter().map(|&k| (group[k].1, group[k].2, group[k].3, group[k].4)).collect() };
            let s = eval_piece(p, fd, lay, row, cell, d, &pc, &arena, &extra, ep);
            score.add(&s);
        }
        i = j;
    }
    // Cells fully inside the solid have no surface polygons.
    if let Some(_occ) = &p.occ {
        for cell in 0..row.cells() {
            for d in 0..row.depth_cells() {
                let key = cell as u32 * nd + d as u32;
                if seen_cells.binary_search(&key).is_ok() {
                    continue;
                }
                extra.clear();
                corners_inside(p, fd, lay, row, cell, d, &mut extra);
                if extra.len() >= 8 {
                    let pc = Piece { pts_start: 0, pts_end: 0, polys: Vec::new() };
                    score.add(&eval_piece(p, fd, lay, row, cell, d, &pc, &arena, &extra, ep));
                }
            }
        }
    }
    RowEval { score, edges }
}

/// Cell corners (local coords) that lie inside the solid.
fn corners_inside(p: &Proxy, fd: &FrameData, lay: &Layout, row: &Row, cell: usize, d: usize, out: &mut Vec<DVec3>) {
    let Some(occ) = &p.occ else { return };
    let (l, r) = lay.cell_lines(row, cell);
    let (w0, w1) = lay.depth_bounds(row, d);
    let clamp_u = |x: f64| x.clamp(fd.lo.x - 1.0, fd.hi.x + 1.0);
    for (u, v) in [(l.xb, row.v0), (r.xb, row.v0), (l.xt, row.v1), (r.xt, row.v1)] {
        for w in [w0, w1] {
            let q = DVec3::new(clamp_u(u), v, w.clamp(fd.lo.z - 1.0, fd.hi.z + 1.0));
            if occ.inside(fd.frame.world(q).as_vec3()) {
                out.push(q);
            }
        }
    }
}

/// Scores one island: best of resting on each of its cut faces.
#[allow(clippy::too_many_arguments)]
fn eval_piece(p: &Proxy, fd: &FrameData, lay: &Layout, row: &Row, cell: usize, d: usize, pc: &Piece, arena: &[DVec3], extra: &[DVec3], ep: &EvalParams) -> Score {
    let _ = (pc.pts_start, pc.pts_end);
    let (l, r) = lay.cell_lines(row, cell);
    let up_cell = lay.cell_points_up(row, cell);
    // Outward normals of the cell faces, in local coords, as rest candidates.
    let dv = row.v1 - row.v0;
    let side_n = |ln: &Line, sign: f64| DVec3::new(sign, -sign * (ln.xt - ln.xb) / dv, 0.0).normalize();
    let mut downs: Vec<DVec3> = vec![if up_cell { -DVec3::Y } else { DVec3::Y }];
    if cell > 0 {
        downs.push(side_n(&l, -1.0));
    }
    if cell < row.lines.len() {
        downs.push(side_n(&r, 1.0));
    }
    if d > 0 {
        downs.push(-DVec3::Z);
    }
    if d + 1 < row.depth_cells() {
        downs.push(DVec3::Z);
    }
    let pts = || pc.polys.iter().flat_map(|&(_, s, e, _)| arena[s..e].iter()).chain(extra.iter());
    let mut best: Option<((u8, u8, u8, i64, i64), Score)> = None;
    for down in downs {
        let up = -down;
        let a1 = {
            let x = DVec3::Z.cross(up);
            if x.length() > 0.3 { x.normalize() } else { DVec3::X.cross(up).normalize() }
        };
        let a2 = up.cross(a1);
        let (mut hl, mut hh) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut ex = [(f64::INFINITY, f64::NEG_INFINITY); 4];
        let dirs = [a1, a2, (a1 + a2) * std::f64::consts::FRAC_1_SQRT_2, (a1 - a2) * std::f64::consts::FRAC_1_SQRT_2];
        let mut any = false;
        for q in pts() {
            any = true;
            let h = q.dot(up);
            hl = hl.min(h);
            hh = hh.max(h);
            for k in 0..4 {
                let s = q.dot(dirs[k]);
                ex[k] = (ex[k].0.min(s), ex[k].1.max(s));
            }
        }
        if !any {
            return Score::default();
        }
        let e: [f64; 4] = ex.map(|x| x.1 - x.0);
        let height = hh - hl;
        let (bx, by, bz) = (ep.bed[0].max(ep.bed[1]), ep.bed[0].min(ep.bed[1]), ep.bed[2]);
        let fits_box = |a: f64, b: f64| a.max(b) <= bx + 1e-6 && a.min(b) <= by + 1e-6;
        let fits = height <= bz + 1e-6 && (fits_box(e[0], e[1]) || fits_box(e[2], e[3]));
        let mut support = 0.0;
        for &(t, s, e2, area) in &pc.polys {
            let n = fd.normals[t as usize];
            if n.dot(down) > ep.sin_overhang {
                let low = arena[s..e2].iter().map(|q| q.dot(up)).fold(f64::INFINITY, f64::min);
                if low > hl + 0.3 {
                    support += area;
                }
            }
        }
        let foot = e[0].max(e[1]);
        let tall = height > foot * 1.05;
        let mut dims = [e[0], e[1], height];
        dims.sort_by(|x, y| x.total_cmp(y));
        let small = dims[1] < ep.min_piece || dims[2] < ep.min_piece * 2.0;
        let needs = support >= SUPPORT_NOISE as f64;
        let s = Score {
            unfit: (!fits) as u32,
            support_pieces: needs as u32,
            support: if needs { support } else { 0.0 },
            tall: tall as u32,
            pieces: if small { 1.5 } else { 1.0 },
            seam: 0.0,
        };
        let key = (s.unfit as u8, s.support_pieces as u8, s.tall as u8, (s.support / 100.0) as i64, (height * 10.0) as i64);
        if best.as_ref().is_none_or(|b| key < b.0) {
            best = Some((key, s));
        }
    }
    let _ = p;
    best.map(|b| b.1).unwrap_or_default()
}

/// Triangles per row (by v range).
pub fn bucket_rows(fd: &FrameData, lay: &Layout) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); lay.rows.len()];
    for (t, &(v0, v1)) in fd.vrange.iter().enumerate() {
        let a = lay.rows.partition_point(|r| r.v1 <= v0);
        let b = lay.rows.partition_point(|r| r.v0 < v1);
        for k in a..b.min(lay.rows.len()) {
            out[k].push(t as u32);
        }
    }
    out
}

/// Evaluates all rows (in parallel).
pub fn eval_layout(p: &Proxy, fd: &FrameData, lay: &Layout, ep: &EvalParams, want_edges: bool) -> (Score, Vec<RowEval>) {
    let buckets = bucket_rows(fd, lay);
    let rows: Vec<RowEval> = (0..lay.rows.len()).into_par_iter().map(|ri| eval_row(p, fd, lay, ri, &buckets[ri], ep, want_edges)).collect();
    let mut s = Score::default();
    for r in &rows {
        s.add(&r.score);
    }
    (s, rows)
}

#[cfg(test)]
mod tests {
    use super::super::layout::LatticeParams;
    use super::*;
    use crate::manifold::Solid;

    #[test]
    fn block_scores_fit_and_counts_pieces() {
        let m = Solid::cube(DVec3::new(600.0, 100.0, 300.0), false).unwrap().to_mesh();
        let p = Proxy::new(&m, 20000, &Zones::default(), &JobCtx::new());
        let fd = FrameData::new(&p, Frame::new(DVec3::Z, DVec3::Y));
        let ep = EvalParams { bed: [250.0, 250.0, 250.0], sin_overhang: 45f64.to_radians().sin(), min_piece: 15.0 };
        let lay = Layout::lattice(fd.frame, fd.lo, fd.hi, &LatticeParams { l: 240.0, theta: 45f64.to_radians(), phase_v: 0.0, phase_u: 0.0, depth: None });
        let (s, _) = eval_layout(&p, &fd, &lay, &ep, false);
        assert_eq!(s.unfit, 0, "{s:?}");
        assert!(s.support < 1.0, "{s:?}");
        assert!(s.pieces >= 8.0 && s.pieces < 30.0, "{s:?}");
        // A huge lattice cannot fit the bed.
        let big = Layout::lattice(fd.frame, fd.lo, fd.hi, &LatticeParams { l: 900.0, theta: 45f64.to_radians(), phase_v: 0.0, phase_u: 0.0, depth: None });
        let (s2, _) = eval_layout(&p, &fd, &big, &ep, false);
        assert!(s2.unfit > 0, "{s2:?}");
    }
}
