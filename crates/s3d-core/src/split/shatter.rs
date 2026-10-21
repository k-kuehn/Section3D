//! Shatter layout: a centroidal Voronoi tessellation of the model surface.
//!
//! Seeds are spread evenly over the surface (Lloyd relaxation of area
//! samples), and each piece is the model inside one Voronoi cell. Every cut
//! is then the plane halfway between two neighbouring seeds, which meets the
//! surface roughly square; three pieces meet at each corner at about 120°,
//! so no seam runs straight on past a corner, like cracks in broken glass.
//! Pieces that do not print well yet are refined afterwards (`refine`).

use std::collections::HashMap;

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use crate::job::JobCtx;
use crate::manifold::Solid;
use crate::mesh::{Aabb, Mesh};
use crate::plane::Plane;
use crate::zones::Zones;

use super::rand01;

/// Area-weighted points on the surface.
pub fn surface_samples(mesh: &Mesh, count: usize, seed: u64) -> Vec<DVec3> {
    let area: Vec<f64> = (0..mesh.tris.len()).map(|t| mesh.tri_cross(t).length() as f64 * 0.5).collect();
    let total: f64 = area.iter().sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let rate = count as f64 / total;
    let mut rng = seed | 1;
    let mut out = Vec::with_capacity(count + mesh.tris.len() / 4);
    for t in 0..mesh.tris.len() {
        let n = area[t] * rate;
        let k = n.floor() as usize + usize::from(rand01(&mut rng) < n.fract());
        let [a, b, c] = mesh.tri(t).map(|v| v.as_dvec3());
        for _ in 0..k {
            let (mut u, mut v) = (rand01(&mut rng), rand01(&mut rng));
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            out.push(a + (b - a) * u + (c - a) * v);
        }
    }
    out
}

/// Uniform hash grid over points for nearest-neighbour queries.
struct Grid {
    cell: f64,
    map: HashMap<(i32, i32, i32), Vec<u32>>,
}

impl Grid {
    fn new(pts: &[DVec3], cell: f64) -> Grid {
        let mut map: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (i, p) in pts.iter().enumerate() {
            map.entry(Self::key_of(*p, cell)).or_default().push(i as u32);
        }
        Grid { cell, map }
    }
    fn key_of(p: DVec3, cell: f64) -> (i32, i32, i32) {
        ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32, (p.z / cell).floor() as i32)
    }
    /// Nearest point index, searching outward ring by ring.
    fn nearest(&self, pts: &[DVec3], q: DVec3) -> Option<u32> {
        let k = Self::key_of(q, self.cell);
        let mut best: Option<(f64, u32)> = None;
        for r in 0..64i32 {
            for dx in -r..=r {
                for dy in -r..=r {
                    for dz in -r..=r {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                            continue;
                        }
                        if let Some(list) = self.map.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                            for &i in list {
                                let d = pts[i as usize].distance_squared(q);
                                if best.is_none_or(|b| d < b.0) {
                                    best = Some((d, i));
                                }
                            }
                        }
                    }
                }
            }
            // Anything in ring r+1 is at least r*cell away.
            if let Some((d, i)) = best {
                if d.sqrt() <= r as f64 * self.cell {
                    return Some(i);
                }
            }
        }
        best.map(|b| b.1)
    }
}

/// Evenly spread seeds with roughly `spacing` between neighbours: a Poisson
/// disk start refined by Lloyd iterations over the surface samples.
pub fn seeds(samples: &[DVec3], spacing: f64, iterations: usize, job: &JobCtx) -> Vec<DVec3> {
    if samples.is_empty() {
        return Vec::new();
    }
    // Poisson disk: after relaxation neighbours end up ~1.28 r apart.
    let r = spacing / 1.28;
    let mut order: Vec<usize> = (0..samples.len()).collect();
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    for i in (1..order.len()).rev() {
        let j = (rand01(&mut rng) * (i + 1) as f64) as usize;
        order.swap(i, j.min(i));
    }
    let mut seeds: Vec<DVec3> = Vec::new();
    let mut map: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for &i in &order {
        let p = samples[i];
        let k = Grid::key_of(p, r);
        let mut free = true;
        'outer: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = map.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                        if list.iter().any(|&s| seeds[s as usize].distance_squared(p) < r * r) {
                            free = false;
                            break 'outer;
                        }
                    }
                }
            }
        }
        if free {
            map.entry(k).or_default().push(seeds.len() as u32);
            seeds.push(p);
        }
    }
    lloyd(samples, &mut seeds, spacing, iterations, job);
    seeds
}

/// Moves each seed to the centroid of the samples nearest to it.
fn lloyd(samples: &[DVec3], seeds: &mut Vec<DVec3>, spacing: f64, iterations: usize, job: &JobCtx) {
    for _ in 0..iterations {
        if job.is_cancelled() || seeds.len() < 2 {
            return;
        }
        let grid = Grid::new(seeds, spacing.max(1.0));
        let owner: Vec<u32> = samples.par_iter().map(|p| grid.nearest(seeds, *p).unwrap_or(0)).collect();
        let mut sum = vec![(DVec3::ZERO, 0usize); seeds.len()];
        for (p, &o) in samples.iter().zip(&owner) {
            sum[o as usize].0 += *p;
            sum[o as usize].1 += 1;
        }
        let mut moved = 0.0f64;
        let mut keep = Vec::with_capacity(seeds.len());
        for (s, (acc, n)) in seeds.iter().zip(&sum) {
            if *n == 0 {
                continue;
            }
            let c = *acc / *n as f64;
            moved = moved.max(c.distance(*s));
            keep.push(c);
        }
        *seeds = keep;
        if moved < spacing * 0.002 {
            break;
        }
    }
    // Drop near-duplicate seeds (degenerate bisectors).
    let mut out: Vec<DVec3> = Vec::with_capacity(seeds.len());
    for s in seeds.iter() {
        if !out.iter().rev().take(64).any(|o| o.distance(*s) < 1.0) {
            out.push(*s);
        }
    }
    *seeds = out;
}

/// Moves seeds so no-cut spots sit inside a cell rather than on a border.
pub fn respect_zones(seeds: &mut [DVec3], zones: &Zones) {
    for _ in 0..4 {
        for d in &zones.dabs {
            let c = d.p.as_dvec3();
            let r = d.radius as f64;
            let mut idx: Vec<(f64, usize)> = seeds.iter().enumerate().map(|(i, s)| (s.distance(c), i)).collect();
            if idx.len() < 2 {
                return;
            }
            idx.select_nth_unstable_by(1, |a, b| a.0.total_cmp(&b.0));
            let (mut a, mut b) = (idx[0], idx[1]);
            if b.0 < a.0 {
                std::mem::swap(&mut a, &mut b);
            }
            let (s1, s2) = (seeds[a.1], seeds[b.1]);
            let gap = (b.0 * b.0 - a.0 * a.0) / (2.0 * s1.distance(s2).max(1e-9));
            if gap < r {
                // Pull the nearest seed onto the spot (but not past the other).
                let t = ((r - gap) / s1.distance(c).max(1e-9)).min(1.0);
                seeds[a.1] = s1 + (c - s1) * t;
            }
        }
    }
}

/// One Voronoi cell clipped to the model's box.
#[derive(Clone, Debug)]
pub struct Cell {
    pub seed: DVec3,
    /// Bounding planes, normals pointing out of the cell.
    pub planes: Vec<Plane>,
    /// Neighbouring seed per plane.
    pub neighbours: Vec<usize>,
    pub bbox: Aabb,
}

/// Convex polytope as a list of planar faces.
struct Poly {
    faces: Vec<Vec<DVec3>>,
    /// Neighbour index per face; usize::MAX for the bounding box.
    tags: Vec<usize>,
    planes: Vec<Plane>,
}

impl Poly {
    fn boxed(lo: DVec3, hi: DVec3) -> Poly {
        let c = |i: usize| DVec3::new(if i & 1 != 0 { hi.x } else { lo.x }, if i & 2 != 0 { hi.y } else { lo.y }, if i & 4 != 0 { hi.z } else { lo.z });
        // Faces wound outward.
        let quads = [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]];
        let normals = [-DVec3::X, DVec3::X, -DVec3::Y, DVec3::Y, -DVec3::Z, DVec3::Z];
        let faces: Vec<Vec<DVec3>> = quads.iter().map(|q| q.iter().map(|&i| c(i)).collect()).collect();
        let planes = faces.iter().zip(normals).map(|(f, n)| Plane::through(f[0], n)).collect();
        Poly { faces, tags: vec![usize::MAX; 6], planes }
    }

    fn max_dist(&self, p: DVec3) -> f64 {
        self.faces.iter().flatten().map(|v| v.distance(p)).fold(0.0, f64::max)
    }

    /// Keeps the part where `plane.dist <= 0`.
    fn clip(&mut self, plane: Plane, tag: usize) {
        let eps = 1e-9;
        let mut faces = Vec::with_capacity(self.faces.len() + 1);
        let mut tags = Vec::with_capacity(self.faces.len() + 1);
        let mut planes = Vec::with_capacity(self.faces.len() + 1);
        let mut cut: Vec<DVec3> = Vec::new();
        let d = |p: DVec3| plane.normal.dot(p) - plane.offset;
        for ((f, &t), pl) in self.faces.iter().zip(&self.tags).zip(&self.planes) {
            let mut out = Vec::with_capacity(f.len() + 2);
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                let (da, db) = (d(a), d(b));
                if da <= eps {
                    out.push(a);
                }
                if (da < -eps && db > eps) || (da > eps && db < -eps) {
                    let x = a + (b - a) * (da / (da - db));
                    out.push(x);
                    cut.push(x);
                } else if da.abs() <= eps {
                    cut.push(a);
                }
            }
            if out.len() >= 3 {
                faces.push(out);
                tags.push(t);
                planes.push(*pl);
            }
        }
        if cut.len() >= 3 {
            // Order the cap polygon around its centre.
            let c = cut.iter().fold(DVec3::ZERO, |a, p| a + *p) / cut.len() as f64;
            let (u, v) = plane.basis();
            cut.sort_by(|a, b| {
                let (pa, pb) = (*a - c, *b - c);
                pa.dot(v).atan2(pa.dot(u)).total_cmp(&pb.dot(v).atan2(pb.dot(u)))
            });
            cut.dedup_by(|a, b| a.distance(*b) < 1e-7);
            if cut.len() >= 3 {
                faces.push(cut);
                tags.push(tag);
                planes.push(plane);
            }
        }
        self.faces = faces;
        self.tags = tags;
        self.planes = planes;
    }
}

/// Bisector between seeds `i < j`, normal from i towards j. Both cells use
/// this exact plane (one flipped), so their cuts match bit for bit.
fn bisector(seeds: &[DVec3], i: usize, j: usize) -> Plane {
    let (a, b) = if i < j { (i, j) } else { (j, i) };
    let n = (seeds[b] - seeds[a]).normalize();
    Plane { normal: n, offset: n.dot((seeds[a] + seeds[b]) * 0.5) }
}

/// Voronoi cells of the seeds, clipped to `bb`.
pub fn cells(seeds: &[DVec3], bb: &Aabb) -> Vec<Cell> {
    let lo = bb.min.as_dvec3() - DVec3::splat(2.0);
    let hi = bb.max.as_dvec3() + DVec3::splat(2.0);
    (0..seeds.len())
        .into_par_iter()
        .map(|i| {
            let s = seeds[i];
            let mut order: Vec<(f64, usize)> = seeds.iter().enumerate().filter(|(j, _)| *j != i).map(|(j, p)| (p.distance(s), j)).collect();
            order.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut poly = Poly::boxed(lo, hi);
            let mut reach = poly.max_dist(s);
            for (d, j) in order {
                // A seed further than twice the farthest vertex cannot clip.
                if d * 0.5 > reach {
                    break;
                }
                let b = bisector(seeds, i, j);
                let out = if i < j { b } else { b.flipped() };
                poly.clip(out, j);
                reach = poly.max_dist(s);
            }
            let mut bbox = Aabb::EMPTY;
            for v in poly.faces.iter().flatten() {
                bbox.grow(v.as_vec3());
            }
            let mut planes = Vec::new();
            let mut neighbours = Vec::new();
            for (t, pl) in poly.tags.iter().zip(&poly.planes) {
                if *t != usize::MAX {
                    planes.push(*pl);
                    neighbours.push(*t);
                }
            }
            Cell { seed: s, planes, neighbours, bbox }
        })
        .collect()
}

/// Trims `solid` to the positive side of each plane (skipping planes that
/// do not cross it). Returns None if nothing is left.
fn trim_all(mut solid: Solid, keep_inside: &[Plane]) -> Result<Option<Solid>, crate::manifold::GeomError> {
    for pl in keep_inside {
        if solid.is_empty() {
            return Ok(None);
        }
        let b = solid.bbox();
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for k in 0..8 {
            let c = Vec3::new(if k & 1 == 0 { b.min.x } else { b.max.x }, if k & 2 == 0 { b.min.y } else { b.max.y }, if k & 4 == 0 { b.min.z } else { b.max.z });
            let d = pl.dist(c);
            lo = lo.min(d);
            hi = hi.max(d);
        }
        if lo >= 0.0 {
            continue;
        }
        if hi <= 0.0 {
            return Ok(None);
        }
        solid = solid.trim_by_plane(pl.normal, pl.offset)?;
    }
    Ok(if solid.is_empty() { None } else { Some(solid) })
}

fn box_planes(b: &Aabb) -> Vec<Plane> {
    let (lo, hi) = (b.min.as_dvec3() - DVec3::splat(0.5), b.max.as_dvec3() + DVec3::splat(0.5));
    vec![Plane::axis(0, lo.x), Plane::axis(1, lo.y), Plane::axis(2, lo.z), Plane::axis(0, hi.x).flipped(), Plane::axis(1, hi.y).flipped(), Plane::axis(2, hi.z).flipped()]
}

/// Cuts the solid into one region per cell. A k-d tree over the seeds crops
/// the solid to each subtree's cells first, so every leaf trims only a small
/// solid. Returns (cell index, region) for non-empty regions.
pub fn apply(solid: Solid, cells: &[Cell], job: &JobCtx) -> Result<Vec<(usize, Solid)>, String> {
    let mut ids: Vec<usize> = (0..cells.len()).collect();
    let mut out = Vec::new();
    rec(solid, cells, &mut ids, job, &mut out)?;
    Ok(out)
}

fn rec(solid: Solid, cells: &[Cell], ids: &mut [usize], job: &JobCtx, out: &mut Vec<(usize, Solid)>) -> Result<(), String> {
    job.check().map_err(|e| e.to_string())?;
    if ids.len() == 1 {
        let c = &cells[ids[0]];
        let keep: Vec<Plane> = c.planes.iter().map(|p| p.flipped()).collect();
        if let Some(s) = trim_all(solid, &keep).map_err(|e| e.to_string())? {
            out.push((ids[0], s));
        }
        return Ok(());
    }
    let bb = ids.iter().fold(Aabb::EMPTY, |b, &i| b.union(&cells[i].bbox));
    let Some(solid) = trim_all(solid, &box_planes(&bb)).map_err(|e| e.to_string())? else { return Ok(()) };
    // Split the seeds at the median of their widest axis.
    let sb = ids.iter().fold(Aabb::EMPTY, |mut b, &i| {
        b.grow(cells[i].seed.as_vec3());
        b
    });
    let axis = sb.size().max_position();
    let mid = ids.len() / 2;
    ids.select_nth_unstable_by(mid, |&a, &b| cells[a].seed[axis].total_cmp(&cells[b].seed[axis]));
    let (left, right) = ids.split_at_mut(mid);
    let s2 = solid.clone();
    let (mut oa, mut ob) = (Vec::new(), Vec::new());
    let (ra, rb) = rayon::join(|| rec(solid, cells, left, job, &mut oa), || rec(s2, cells, right, job, &mut ob));
    ra?;
    rb?;
    out.extend(oa);
    out.extend(ob);
    Ok(())
}

/// Cut lines of the cells on a coarse copy of the surface, for previews.
pub fn preview_lines(proxy: &Mesh, seeds: &[DVec3], spacing: f64) -> Vec<[Vec3; 2]> {
    let grid = Grid::new(seeds, spacing.max(1.0));
    let owner: Vec<u32> = proxy.verts.par_iter().map(|v| grid.nearest(seeds, v.as_dvec3()).unwrap_or(0)).collect();
    let mut out = Vec::new();
    for t in &proxy.tris {
        let o = t.map(|i| owner[i as usize]);
        if o[0] == o[1] && o[1] == o[2] {
            continue;
        }
        let mid = |a: usize, b: usize| (proxy.verts[t[a] as usize] + proxy.verts[t[b] as usize]) * 0.5;
        let mut m = Vec::new();
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            if o[a] != o[b] {
                m.push(mid(a, b));
            }
        }
        if m.len() >= 2 {
            out.push([m[0], m[1]]);
        }
    }
    out
}

/// Seed spacing for a target piece size. Shell-like models (thin walls)
/// print standing on a cut, so their cells may be larger.
pub fn spacing_for(mesh: &Mesh, s: &crate::settings::Settings) -> f64 {
    let bed = s.printer.usable();
    let short = bed[0].min(bed[1]) as f64;
    if let Some(t) = s.split.target_size {
        return (t as f64).min(short * 1.2).max(20.0);
    }
    let thickness = 2.0 * mesh.volume() / mesh.area().max(1e-9);
    // Cells are later halved for steadier printing where needed.
    if thickness < short * 0.15 { short * 1.05 } else { short * 0.8 }
}

/// Builds the cells for a model.
pub fn layout(mesh: &Mesh, s: &crate::settings::Settings, zones: &Zones, job: &JobCtx) -> (Vec<Cell>, Vec<DVec3>, f64) {
    let spacing = spacing_for(mesh, s);
    let proxy = crate::simplify::cluster(mesh, 120_000);
    // About 60 samples per cell area keeps the centroids stable.
    let one_side = proxy.area();
    let n_cells = (one_side / (0.866 * spacing * spacing)).max(1.0);
    let samples = surface_samples(&proxy, ((n_cells * 120.0) as usize).clamp(20_000, 400_000), 0x51ED_270B);
    let mut seeds = seeds(&samples, spacing, 40, job);
    respect_zones(&mut seeds, zones);
    let cells = cells(&seeds, &mesh.bbox());
    (cells, seeds, spacing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_partition_a_block() {
        let m = Solid::cube(DVec3::new(400.0, 300.0, 40.0), false).unwrap();
        let mesh = m.to_mesh();
        let samples = surface_samples(&mesh, 20_000, 7);
        let seeds = seeds(&samples, 120.0, 30, &JobCtx::new());
        assert!(seeds.len() >= 6 && seeds.len() <= 30, "{}", seeds.len());
        let cells = cells(&seeds, &mesh.bbox());
        let parts = apply(m.clone(), &cells, &JobCtx::new()).unwrap();
        let v: f64 = parts.iter().map(|p| p.1.volume()).sum();
        assert!((v - m.volume()).abs() / m.volume() < 1e-4, "{v}");
        // Thin plate: cells run through the thickness (no cut parallel to the faces).
        for c in &cells {
            for p in &c.planes {
                assert!(p.normal.z.abs() < 0.5, "{:?}", p.normal);
            }
        }
    }
}
