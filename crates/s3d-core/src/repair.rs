//! Automatic mesh repair: degenerate/duplicate removal, tolerance weld,
//! consistent winding, hole filling and, as a last resort, voxel rebuild.

use glam::{DVec3, IVec3, Vec2, Vec3};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::job::{Cancelled, JobCtx};
use crate::manifold::Solid;
use crate::march;
use crate::mesh::Mesh;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Health {
    /// Imported mesh was already a closed, valid solid.
    Clean,
    /// Problems were fixed; `notes` lists what was done.
    Repaired { notes: Vec<String> },
    /// Mesh was rebuilt from voxels: fine detail may be lost.
    Rebuilt { notes: Vec<String> },
    /// The mesh could not be made into a valid solid.
    Unresolved { reason: String },
}

impl Health {
    pub fn label(&self) -> &'static str {
        match self {
            Health::Clean => "Clean",
            Health::Repaired { .. } => "Repaired",
            Health::Rebuilt { .. } => "Rebuilt from voxels",
            Health::Unresolved { .. } => "Unresolved",
        }
    }
    pub fn is_usable(&self) -> bool {
        !matches!(self, Health::Unresolved { .. })
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct EdgeStats {
    pub boundary: usize,
    pub non_manifold: usize,
    pub inconsistent: usize,
}

impl EdgeStats {
    pub fn is_closed_manifold(&self) -> bool {
        self.boundary == 0 && self.non_manifold == 0 && self.inconsistent == 0
    }
}

/// Directed edge record: (undirected key, face, from, to).
fn edge_records(m: &Mesh) -> Vec<(u64, u32, u32, u32)> {
    let mut e: Vec<(u64, u32, u32, u32)> = m
        .tris
        .par_iter()
        .enumerate()
        .flat_map_iter(|(f, t)| {
            (0..3).map(move |k| {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let key = ((a.min(b) as u64) << 32) | a.max(b) as u64;
                (key, f as u32, a, b)
            })
        })
        .collect();
    e.par_sort_unstable_by_key(|r| (r.0, r.1));
    e
}

/// Groups of records sharing one undirected edge.
fn edge_groups(e: &[(u64, u32, u32, u32)]) -> impl Iterator<Item = &[(u64, u32, u32, u32)]> {
    e.chunk_by(|a, b| a.0 == b.0)
}

pub fn edge_stats(m: &Mesh) -> EdgeStats {
    let e = edge_records(m);
    let mut s = EdgeStats::default();
    for g in edge_groups(&e) {
        match g.len() {
            1 => s.boundary += 1,
            2 => {
                if g[0].2 == g[1].2 {
                    s.inconsistent += 1
                }
            }
            _ => s.non_manifold += 1,
        }
    }
    s
}

fn remove_degenerate(m: &mut Mesh) -> usize {
    let before = m.tris.len();
    let eps = (m.bbox().diagonal() as f64 * 1e-7).powi(2);
    let verts = &m.verts;
    m.tris.retain(|t| {
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            return false;
        }
        let (a, b, c) = (verts[t[0] as usize].as_dvec3(), verts[t[1] as usize].as_dvec3(), verts[t[2] as usize].as_dvec3());
        (b - a).cross(c - a).length_squared() > eps * eps
    });
    before - m.tris.len()
}

fn remove_duplicate_tris(m: &mut Mesh) -> usize {
    let before = m.tris.len();
    let mut keyed: Vec<([u32; 3], u32)> = m
        .tris
        .par_iter()
        .enumerate()
        .map(|(i, t)| {
            let mut k = *t;
            k.sort_unstable();
            (k, i as u32)
        })
        .collect();
    keyed.par_sort_unstable();
    let mut keep = vec![true; m.tris.len()];
    for w in keyed.windows(2) {
        if w[0].0 == w[1].0 {
            keep[w[1].1 as usize] = false;
        }
    }
    let mut i = 0;
    m.tris.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    before - m.tris.len()
}

/// Merges vertices closer than `tol` (grid hashing + union-find).
pub fn weld_tolerance(m: &mut Mesh, tol: f32) -> usize {
    let n = m.verts.len();
    let cell = |v: Vec3| ((v / tol).floor()).as_ivec3();
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.par_sort_unstable_by_key(|&i| {
        let c = cell(m.verts[i as usize]);
        (c.x, c.y, c.z)
    });
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    // Cell -> range in `order`.
    let mut ranges = std::collections::HashMap::with_capacity(n / 2);
    let mut s = 0;
    while s < n {
        let c = cell(m.verts[order[s] as usize]);
        let mut e = s + 1;
        while e < n && cell(m.verts[order[e] as usize]) == c {
            e += 1;
        }
        ranges.insert((c.x, c.y, c.z), (s, e));
        s = e;
    }
    let tol2 = tol * tol;
    for (&(cx, cy, cz), &(s, e)) in &ranges {
        for dx in 0..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && (dy < 0 || (dy == 0 && dz < 0)) {
                        continue;
                    }
                    let Some(&(s2, e2)) = ranges.get(&(cx + dx, cy + dy, cz + dz)) else { continue };
                    let same = dx == 0 && dy == 0 && dz == 0;
                    for i in s..e {
                        let start = if same { i + 1 } else { s2 };
                        for j in start..e2 {
                            let (a, b) = (order[i], order[j]);
                            if m.verts[a as usize].distance_squared(m.verts[b as usize]) <= tol2 {
                                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                                if ra != rb {
                                    parent[ra.max(rb) as usize] = ra.min(rb);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut merged = 0;
    for i in 0..n as u32 {
        if find(&mut parent, i) != i {
            merged += 1;
        }
    }
    if merged > 0 {
        for t in &mut m.tris {
            for v in t.iter_mut() {
                *v = find(&mut parent, *v);
            }
        }
        m.compact();
    }
    merged
}

/// Makes winding consistent per connected component and outward-facing.
/// Returns the number of flipped triangles.
fn orient(m: &mut Mesh) -> usize {
    let e = edge_records(m);
    let nf = m.tris.len();
    // Adjacency through 2-manifold edges: (neighbour, same_direction).
    let mut adj: Vec<Vec<(u32, bool)>> = vec![Vec::new(); nf];
    for g in edge_groups(&e) {
        if g.len() == 2 && g[0].1 != g[1].1 {
            let same = g[0].2 == g[1].2;
            adj[g[0].1 as usize].push((g[1].1, same));
            adj[g[1].1 as usize].push((g[0].1, same));
        }
    }
    let mut flip = vec![false; nf];
    let mut comp = vec![u32::MAX; nf];
    let mut comps = 0u32;
    let mut stack = Vec::new();
    for seed in 0..nf {
        if comp[seed] != u32::MAX {
            continue;
        }
        comp[seed] = comps;
        stack.push(seed as u32);
        while let Some(f) = stack.pop() {
            for &(g, same) in &adj[f as usize] {
                if comp[g as usize] == u32::MAX {
                    comp[g as usize] = comps;
                    flip[g as usize] = flip[f as usize] ^ same;
                    stack.push(g);
                }
            }
        }
        comps += 1;
    }
    // Outward: make each component's signed volume positive.
    let mut vol = vec![0f64; comps as usize];
    for f in 0..nf {
        let [a, b, c] = m.tri(f);
        let v = a.as_dvec3().dot(b.as_dvec3().cross(c.as_dvec3()));
        vol[comp[f] as usize] += if flip[f] { -v } else { v };
    }
    let mut flipped = 0;
    for f in 0..nf {
        if flip[f] ^ (vol[comp[f] as usize] < 0.0) {
            m.tris[f].swap(1, 2);
            flipped += 1;
        }
    }
    flipped
}

/// Gives each fan of faces around a non-manifold vertex/edge its own vertex copy,
/// so every edge joins at most two consistently-oriented faces.
fn split_non_manifold(m: &mut Mesh) -> usize {
    let e = edge_records(m);
    let nc = m.tris.len() * 3;
    let corner = |f: u32, v: u32, tris: &[[u32; 3]]| f * 3 + tris[f as usize].iter().position(|&x| x == v).unwrap() as u32;
    let mut parent: Vec<u32> = (0..nc as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let mut bad = 0;
    for g in edge_groups(&e) {
        if g.len() == 2 && g[0].1 != g[1].1 {
            for (u, _) in [(g[0].2, 0), (g[0].3, 1)] {
                let (a, b) = (find(&mut parent, corner(g[0].1, u, &m.tris)), find(&mut parent, corner(g[1].1, u, &m.tris)));
                if a != b {
                    parent[a.max(b) as usize] = a.min(b);
                }
            }
        } else if g.len() > 2 {
            bad += 1;
        }
    }
    if bad == 0 {
        return 0;
    }
    let mut new_id = vec![u32::MAX; nc];
    let mut verts = Vec::with_capacity(m.verts.len());
    let mut tris = m.tris.clone();
    for f in 0..m.tris.len() {
        for k in 0..3 {
            let r = find(&mut parent, (f * 3 + k) as u32) as usize;
            if new_id[r] == u32::MAX {
                new_id[r] = verts.len() as u32;
                verts.push(m.verts[m.tris[f][k] as usize]);
            }
            tris[f][k] = new_id[r];
        }
    }
    m.verts = verts;
    m.tris = tris;
    bad
}

/// Triangulates a simple 3D polygon loop by ear clipping in its best-fit plane.
pub fn triangulate_loop(verts: &[Vec3], lp: &[u32]) -> Vec<[u32; 3]> {
    let n = lp.len();
    if n < 3 {
        return Vec::new();
    }
    // Newell normal.
    let mut nrm = DVec3::ZERO;
    for i in 0..n {
        let (a, b) = (verts[lp[i] as usize].as_dvec3(), verts[lp[(i + 1) % n] as usize].as_dvec3());
        nrm += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    let nrm = nrm.normalize_or(DVec3::Z);
    let u = nrm.any_orthonormal_vector();
    let v = nrm.cross(u);
    let p2: Vec<Vec2> = lp.iter().map(|&i| {
        let p = verts[i as usize].as_dvec3();
        Vec2::new(p.dot(u) as f32, p.dot(v) as f32)
    }).collect();
    ear_clip(&p2).into_iter().map(|[a, b, c]| [lp[a], lp[b], lp[c]]).collect()
}

/// Ear clipping of a CCW-or-CW simple polygon; returns index triples with the
/// polygon's own orientation. Falls back to a fan if no ear is found.
pub fn ear_clip(p: &[Vec2]) -> Vec<[usize; 3]> {
    let n = p.len();
    let area: f32 = (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum();
    let s = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n.saturating_sub(2));
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let cr = (p[b] - p[a]).perp_dot(p[c] - p[b]) * s;
            if cr <= 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&q| {
                if q == a || q == b || q == c {
                    return false;
                }
                let d1 = (p[b] - p[a]).perp_dot(p[q] - p[a]) * s;
                let d2 = (p[c] - p[b]).perp_dot(p[q] - p[b]) * s;
                let d3 = (p[a] - p[c]).perp_dot(p[q] - p[c]) * s;
                d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
            });
            if !inside {
                out.push([a, b, c]);
                idx.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    for k in 1..idx.len().saturating_sub(1) {
        out.push([idx[0], idx[k], idx[k + 1]]);
    }
    out
}

/// Closes boundary loops. Returns the number of holes filled.
fn fill_holes(m: &mut Mesh) -> usize {
    let e = edge_records(m);
    // Hole edges run opposite to the boundary edge of the existing face.
    let mut next: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for g in edge_groups(&e) {
        if g.len() == 1 {
            next.entry(g[0].3).or_default().push(g[0].2);
        }
    }
    let mut filled = 0;
    let starts: Vec<u32> = next.keys().copied().collect();
    for s in starts {
        loop {
            let Some(first) = next.get_mut(&s).and_then(|v| v.pop()) else { break };
            let mut lp = vec![s];
            let mut cur = first;
            let mut ok = false;
            for _ in 0..1_000_000 {
                if cur == s {
                    ok = true;
                    break;
                }
                lp.push(cur);
                match next.get_mut(&cur).and_then(|v| v.pop()) {
                    Some(nx) => cur = nx,
                    None => break,
                }
            }
            if ok && lp.len() >= 3 {
                if lp.len() > 3000 {
                    // Large or non-planar: fan from the centroid.
                    let c = lp.iter().map(|&i| m.verts[i as usize]).sum::<Vec3>() / lp.len() as f32;
                    let ci = m.verts.len() as u32;
                    m.verts.push(c);
                    for i in 0..lp.len() {
                        m.tris.push([lp[i], lp[(i + 1) % lp.len()], ci]);
                    }
                } else {
                    let tris = triangulate_loop(&m.verts, &lp);
                    m.tris.extend(tris);
                }
                filled += 1;
            }
        }
    }
    filled
}

pub struct RepairResult {
    pub mesh: Mesh,
    pub health: Health,
}

/// Runs the repair pipeline. The returned mesh is a valid closed solid unless
/// health is `Unresolved`.
pub fn repair(mut m: Mesh, job: &JobCtx) -> Result<RepairResult, Cancelled> {
    let mut notes = Vec::new();
    job.set_stage("Checking mesh");
    let stats = edge_stats(&m);
    // A closed, consistently oriented 2-manifold with positive volume is all
    // Manifold requires, so the (slow) solid construction is skipped here.
    if stats.is_closed_manifold() && m.volume() > 0.0 {
        return Ok(RepairResult { mesh: m, health: Health::Clean });
    }
    job.check()?;
    job.set_stage("Repairing mesh");
    let n = remove_degenerate(&mut m);
    if n > 0 {
        notes.push(format!("removed {n} degenerate triangles"));
    }
    let n = remove_duplicate_tris(&mut m);
    if n > 0 {
        notes.push(format!("removed {n} duplicate triangles"));
    }
    if edge_stats(&m).boundary > 0 {
        let tol = m.bbox().diagonal() * 1e-5;
        let n = weld_tolerance(&mut m, tol);
        if n > 0 {
            notes.push(format!("merged {n} nearly-coincident vertices"));
            remove_degenerate(&mut m);
            remove_duplicate_tris(&mut m);
        }
    }
    job.check()?;
    let n = split_non_manifold(&mut m);
    if n > 0 {
        notes.push(format!("separated {n} non-manifold edges"));
    }
    let n = orient(&mut m);
    if n > 0 {
        notes.push(format!("fixed winding of {n} triangles"));
    }
    job.check()?;
    let n = fill_holes(&mut m);
    if n > 0 {
        notes.push(format!("closed {n} holes"));
        orient(&mut m);
    }
    m.compact();
    remove_degenerate(&mut m);
    m.compact();
    if edge_stats(&m).is_closed_manifold() && m.volume() > 0.0 {
        return Ok(RepairResult { mesh: m, health: Health::Repaired { notes } });
    }
    job.check()?;
    job.set_stage("Rebuilding mesh from voxels");
    match voxel_rebuild(&m, job)? {
        Some(rebuilt) => {
            notes.push("mesh could not be repaired directly and was rebuilt from voxels; check fine details".into());
            Ok(RepairResult { mesh: rebuilt, health: Health::Rebuilt { notes } })
        }
        None => Ok(RepairResult {
            mesh: m,
            health: Health::Unresolved { reason: "the mesh has too many defects to repair or rebuild".into() },
        }),
    }
}

/// Rebuilds a closed solid from an occupancy grid built by ray parity along
/// all three axes (majority vote), which tolerates holes and flipped faces.
pub fn voxel_rebuild(m: &Mesh, job: &JobCtx) -> Result<Option<Mesh>, Cancelled> {
    if m.bbox().is_empty() {
        return Ok(None);
    }
    let occ_grid = crate::voxel::occupancy(m, 256, job)?;
    let (origin, h) = (occ_grid.origin, occ_grid.h);
    let [nx, ny, nz] = occ_grid.dims;
    let dims = glam::USizeVec3::new(nx, ny, nz);
    let idx = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    let votes = occ_grid.votes;
    let occ: Vec<f32> = votes.iter().map(|&v| if v >= 2 { 1.0 } else { -1.0 }).collect();
    if !occ.iter().any(|&v| v > 0.0) {
        return Ok(None);
    }
    let sample = |p: Vec3| -> f32 {
        // Trilinear interpolation of the +-1 occupancy field (voxel centres).
        let g = (p - origin) / h - Vec3::splat(0.5);
        let i0 = g.floor();
        let f = g - i0;
        let mut acc = 0.0f32;
        for dz in 0..2 {
            for dy in 0..2 {
                for dx in 0..2 {
                    let (x, y, z) = (i0.x as isize + dx, i0.y as isize + dy, i0.z as isize + dz);
                    let v = if x < 0 || y < 0 || z < 0 || x >= nx as isize || y >= ny as isize || z >= nz as isize {
                        -1.0
                    } else {
                        occ[idx(x as usize, y as usize, z as usize)]
                    };
                    let w = (if dx == 1 { f.x } else { 1.0 - f.x }) * (if dy == 1 { f.y } else { 1.0 - f.y }) * (if dz == 1 { f.z } else { 1.0 - f.z });
                    acc += v * w;
                }
            }
        }
        acc
    };
    let blocks = march::blocks_in(IVec3::ZERO, dims.as_ivec3());
    let surf = march::extract(origin, h, &blocks, sample);
    match Solid::from_mesh(&surf) {
        Ok(s) if !s.is_empty() => Ok(Some(s.to_mesh())),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;

    #[test]
    fn clean_cube_is_clean() {
        let r = repair(shapes::cube(Vec3::splat(10.0)), &JobCtx::new()).unwrap();
        assert_eq!(r.health, Health::Clean);
    }

    #[test]
    fn flipped_faces_and_hole() {
        let mut m = shapes::uv_sphere(10.0, 24, 12);
        for t in m.tris.iter_mut().step_by(7) {
            t.swap(1, 2);
        }
        m.tris.remove(40);
        m.tris.remove(41);
        assert!(!edge_stats(&m).is_closed_manifold());
        let r = repair(m, &JobCtx::new()).unwrap();
        assert!(matches!(r.health, Health::Repaired { .. }), "{:?}", r.health);
        assert!(edge_stats(&r.mesh).is_closed_manifold());
        assert!(r.mesh.volume() > 3500.0);
    }

    #[test]
    fn inside_out_mesh_is_flipped() {
        let mut m = shapes::cube(Vec3::splat(2.0));
        m.flip();
        let r = repair(m, &JobCtx::new()).unwrap();
        assert!((r.mesh.volume() - 8.0).abs() < 1e-4);
    }

    #[test]
    fn soup_with_gaps_rebuilds() {
        // Unwelded soup with a slightly shifted vertex: repaired or rebuilt, but valid.
        let m = shapes::uv_sphere(10.0, 24, 12);
        let mut soup = Mesh::default();
        for t in 0..m.tris.len() {
            let p = m.tri(t);
            let b = soup.verts.len() as u32;
            soup.verts.extend(p);
            soup.tris.push([b, b + 1, b + 2]);
        }
        soup.verts[5] += Vec3::splat(1e-6);
        let r = repair(soup, &JobCtx::new()).unwrap();
        assert!(r.health.is_usable(), "{:?}", r.health);
        assert!(Solid::from_mesh(&r.mesh).is_ok());
    }

    #[test]
    fn voxel_rebuild_box() {
        let m = shapes::cube(Vec3::new(10.0, 20.0, 5.0));
        let r = voxel_rebuild(&m, &JobCtx::new()).unwrap().unwrap();
        let v = r.volume();
        assert!((v - 1000.0).abs() / 1000.0 < 0.1, "{v}");
    }
}
