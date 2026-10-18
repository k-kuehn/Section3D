//! Pieces, seams and the operations that edit them.
//!
//! A seam is the contact between exactly two pieces on one cut plane, so
//! adjusting it re-cuts only those two pieces (their union) and leaves every
//! other piece untouched.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DVec2, Vec3};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::manifold::{GResult, Solid};
use crate::mesh::{Aabb, Mesh};
use crate::plane::Plane;

pub type PieceId = u32;
pub type SeamId = u32;

/// Distance within which a vertex counts as lying on a cut plane (mm).
pub const ON_PLANE: f64 = 1e-3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Flag {
    /// Piece does not fit the printer in any orientation.
    TooBig,
    /// No orientation avoids support; area in mm².
    NeedsSupport(f32),
    /// Taller than wide in print orientation.
    Tall,
    /// Smaller than the minimum piece size but could not be merged.
    Small,
    /// A dowel hole was skipped because the wall was too thin.
    SkippedDowel,
    /// No clear path to insert during assembly.
    NoAssemblyPath,
}

impl Flag {
    pub fn short(&self) -> &'static str {
        match self {
            Flag::TooBig => "TOO BIG",
            Flag::NeedsSupport(_) => "SUPPORT",
            Flag::Tall => "TALL",
            Flag::Small => "SMALL",
            Flag::SkippedDowel => "PIN SKIPPED",
            Flag::NoAssemblyPath => "NO PATH",
        }
    }
    pub fn explain(&self) -> String {
        match self {
            Flag::TooBig => "Does not fit the printer. Split this piece or lower the piece size.".into(),
            Flag::NeedsSupport(a) => format!("Needs support ({a:.0} mm²) in every orientation. Use Fix support to split it or adjust a cut."),
            Flag::Tall => "Taller than it is wide when printed. Move a cut or split it for a steadier print.".into(),
            Flag::Small => "Smaller than the minimum piece size and could not be merged.".into(),
            Flag::SkippedDowel => "A dowel hole was skipped where the wall is too thin.".into(),
            Flag::NoAssemblyPath => "Cannot be slid into place after its neighbours; assemble it earlier or glue without pins.".into(),
        }
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Flag::TooBig)
    }
}

/// Print orientation: rotation taking the assembled piece to its print pose,
/// then translation placing it on the bed (min z = 0, centred in XY).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Orientation {
    pub rotation: glam::Quat,
    pub translation: Vec3,
    /// Size in print pose.
    pub size: Vec3,
    pub support_area: f32,
}

impl Orientation {
    pub fn matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_translation(self.translation) * glam::Mat4::from_quat(self.rotation)
    }
}

#[derive(Clone)]
pub struct Piece {
    pub id: PieceId,
    pub solid: Arc<Solid>,
    /// Cut solid without connectors or labels.
    pub mesh: Arc<Mesh>,
    /// With dowel holes and part number, once connectors ran.
    pub finished: Option<Arc<Mesh>>,
    /// Bumped whenever the geometry changes (GPU cache key).
    pub rev: u64,
    pub locked: bool,
    pub label: u32,
    pub orient: Option<Orientation>,
    pub flags: Vec<Flag>,
    pub volume: f64,
    pub centroid: Vec3,
    pub bbox: Aabb,
}

impl Piece {
    pub fn new(id: PieceId, solid: Solid, rev: u64) -> Piece {
        let mesh = solid.to_mesh();
        Piece::from_parts(id, Arc::new(solid), Arc::new(mesh), rev)
    }

    pub fn from_parts(id: PieceId, solid: Arc<Solid>, mesh: Arc<Mesh>, rev: u64) -> Piece {
        let volume = solid.volume();
        let bbox = mesh.bbox();
        let centroid = mesh.centroid();
        Piece { id, solid, mesh, finished: None, rev, locked: false, label: id + 1, orient: None, flags: Vec::new(), volume, centroid, bbox }
    }

    /// Mesh to export/show: finished if available.
    pub fn display_mesh(&self) -> &Arc<Mesh> {
        self.finished.as_ref().unwrap_or(&self.mesh)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Seam {
    pub id: SeamId,
    /// Oriented so that `a` lies on the positive side.
    pub plane: Plane,
    pub a: PieceId,
    pub b: PieceId,
    /// Contact area (mm²).
    pub area: f64,
    /// Centroid of the contact region.
    pub center: Vec3,
    pub locked: bool,
}

impl Seam {
    pub fn other(&self, p: PieceId) -> PieceId {
        if self.a == p { self.b } else { self.a }
    }
    pub fn touches(&self, p: PieceId) -> bool {
        self.a == p || self.b == p
    }
}

#[derive(Clone, Default)]
pub struct Partition {
    pub pieces: Vec<Piece>,
    pub seams: Vec<Seam>,
    pub next_piece: PieceId,
    pub next_seam: SeamId,
    pub next_rev: u64,
}

/// Cap triangles of a mesh lying on a plane, in the plane's 2D frame.
/// `outward_sign` is +1 for faces whose normal equals the plane normal.
fn caps_on(mesh: &Mesh, plane: &Plane, outward_sign: f64) -> Vec<[DVec2; 3]> {
    let (u, v) = plane.basis();
    let tol = ON_PLANE.max(mesh.bbox().diagonal() as f64 * 1e-7);
    (0..mesh.tris.len())
        .filter_map(|t| {
            let p = mesh.tri(t);
            if p.iter().any(|q| plane.dist(*q).abs() > tol) {
                return None;
            }
            let n = mesh.tri_cross(t).as_dvec3();
            let l = n.length();
            if l < 1e-12 || n.dot(plane.normal) * outward_sign < 0.99 * l {
                return None;
            }
            Some(p.map(|q| {
                let q = q.as_dvec3();
                DVec2::new(q.dot(u), q.dot(v))
            }))
        })
        .collect()
}

fn tri_area(t: &[DVec2; 3]) -> f64 {
    ((t[1] - t[0]).perp_dot(t[2] - t[0])).abs() * 0.5
}

fn point_in_tri(p: DVec2, t: &[DVec2; 3]) -> bool {
    let d1 = (t[1] - t[0]).perp_dot(p - t[0]);
    let d2 = (t[2] - t[1]).perp_dot(p - t[1]);
    let d3 = (t[0] - t[2]).perp_dot(p - t[2]);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Uniform-grid lookup over a set of 2D triangles.
struct TriGrid {
    tris: Vec<[DVec2; 3]>,
    min: DVec2,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl TriGrid {
    fn new(tris: Vec<[DVec2; 3]>) -> TriGrid {
        let mut min = DVec2::splat(f64::INFINITY);
        let mut max = DVec2::splat(f64::NEG_INFINITY);
        for t in &tris {
            for p in t {
                min = min.min(*p);
                max = max.max(*p);
            }
        }
        let ext = (max - min).max(DVec2::splat(1e-6));
        let n = (tris.len() as f64).sqrt().ceil().clamp(1.0, 256.0);
        let cell = (ext.x.max(ext.y) / n).max(1e-6);
        let nx = (ext.x / cell).ceil() as usize + 1;
        let ny = (ext.y / cell).ceil() as usize + 1;
        let mut cells = vec![Vec::new(); nx * ny];
        for (i, t) in tris.iter().enumerate() {
            let lo = t.iter().fold(DVec2::splat(f64::INFINITY), |a, p| a.min(*p));
            let hi = t.iter().fold(DVec2::splat(f64::NEG_INFINITY), |a, p| a.max(*p));
            let (x0, y0) = (((lo.x - min.x) / cell) as usize, ((lo.y - min.y) / cell) as usize);
            let (x1, y1) = ((((hi.x - min.x) / cell) as usize).min(nx - 1), (((hi.y - min.y) / cell) as usize).min(ny - 1));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    cells[y * nx + x].push(i as u32);
                }
            }
        }
        TriGrid { tris, min, cell, nx, ny, cells }
    }

    fn contains(&self, p: DVec2) -> bool {
        let q = (p - self.min) / self.cell;
        if q.x < 0.0 || q.y < 0.0 {
            return false;
        }
        let (x, y) = (q.x as usize, q.y as usize);
        if x >= self.nx || y >= self.ny {
            return false;
        }
        self.cells[y * self.nx + x].iter().any(|&i| point_in_tri(p, &self.tris[i as usize]))
    }
}

/// Contact area and centre between caps `a` (on one side) and `b` (other side).
fn contact(a: &[[DVec2; 3]], b: &TriGrid) -> Option<(f64, DVec2)> {
    // Sample each triangle at its centroid and three inset points.
    const W: [[f64; 3]; 4] = [[1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0], [0.7, 0.15, 0.15], [0.15, 0.7, 0.15], [0.15, 0.15, 0.7]];
    let mut area = 0.0;
    let mut c = DVec2::ZERO;
    for t in a {
        let ta = tri_area(t);
        if ta < 1e-9 {
            continue;
        }
        let hits = W.iter().filter(|w| b.contains(t[0] * w[0] + t[1] * w[1] + t[2] * w[2])).count();
        if hits > 0 {
            let frac = hits as f64 / W.len() as f64;
            area += ta * frac;
            c += (t[0] + t[1] + t[2]) / 3.0 * ta * frac;
        }
    }
    if area > 1e-3 { Some((area, c / area)) } else { None }
}

impl Partition {
    pub fn from_solids(solids: Vec<Solid>) -> Partition {
        let mut p = Partition::default();
        let pieces: Vec<Piece> = solids.into_par_iter().enumerate().map(|(i, s)| Piece::new(i as u32, s, i as u64)).collect();
        p.next_piece = pieces.len() as u32;
        p.next_rev = pieces.len() as u64;
        p.pieces = pieces;
        p
    }

    pub fn piece(&self, id: PieceId) -> Option<&Piece> {
        self.pieces.iter().find(|p| p.id == id)
    }

    pub fn piece_mut(&mut self, id: PieceId) -> Option<&mut Piece> {
        self.pieces.iter_mut().find(|p| p.id == id)
    }

    pub fn seam(&self, id: SeamId) -> Option<&Seam> {
        self.seams.iter().find(|s| s.id == id)
    }

    pub fn seams_of(&self, p: PieceId) -> impl Iterator<Item = &Seam> {
        self.seams.iter().filter(move |s| s.touches(p))
    }

    pub fn neighbours(&self, p: PieceId) -> Vec<PieceId> {
        let mut n: Vec<PieceId> = self.seams_of(p).map(|s| s.other(p)).collect();
        n.sort_unstable();
        n.dedup();
        n
    }

    /// Distinct planes of all current seams.
    pub fn seam_planes(&self) -> Vec<Plane> {
        dedup_planes(self.seams.iter().map(|s| s.plane))
    }

    fn take_rev(&mut self) -> u64 {
        self.next_rev += 1;
        self.next_rev
    }

    /// Recomputes every seam from scratch for the given cut planes.
    pub fn rebuild_seams(&mut self, planes: &[Plane]) {
        let all: Vec<PieceId> = self.pieces.iter().map(|p| p.id).collect();
        self.seams.clear();
        self.compute_seams(&all, &all, planes);
    }

    /// Adds seams on `planes` between pieces where at least one is in `affected`.
    fn compute_seams(&mut self, affected: &[PieceId], candidates: &[PieceId], planes: &[Plane]) {
        let planes = dedup_planes(planes.iter().copied());
        let idx: HashMap<PieceId, usize> = self.pieces.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        let cand: Vec<usize> = candidates.iter().filter_map(|id| idx.get(id).copied()).collect();
        let found: Vec<(Plane, PieceId, PieceId, f64, Vec3)> = planes
            .par_iter()
            .flat_map_iter(|pl| {
                let tol = 1.0;
                // Pieces that touch this plane at all (cheap bbox test first).
                let near: Vec<usize> = cand
                    .iter()
                    .copied()
                    .filter(|&i| {
                        let b = &self.pieces[i].bbox;
                        let corners = (0..8).map(|k| Vec3::new(if k & 1 == 0 { b.min.x } else { b.max.x }, if k & 2 == 0 { b.min.y } else { b.max.y }, if k & 4 == 0 { b.min.z } else { b.max.z }));
                        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
                        for c in corners {
                            let d = pl.dist(c);
                            lo = lo.min(d);
                            hi = hi.max(d);
                        }
                        lo <= tol && hi >= -tol
                    })
                    .collect();
                // Positive-side pieces have caps facing -n; negative-side ones +n.
                let pos: Vec<(usize, Vec<[DVec2; 3]>)> = near.iter().map(|&i| (i, caps_on(&self.pieces[i].mesh, pl, -1.0))).filter(|c| !c.1.is_empty()).collect();
                let neg: Vec<(usize, TriGrid)> = near.iter().map(|&i| (i, caps_on(&self.pieces[i].mesh, pl, 1.0))).filter(|c| !c.1.is_empty()).map(|(i, c)| (i, TriGrid::new(c))).collect();
                let (u, v) = pl.basis();
                let mut out = Vec::new();
                for (pi, pc) in &pos {
                    for (ni, ng) in &neg {
                        if pi == ni {
                            continue;
                        }
                        let (a, b) = (self.pieces[*pi].id, self.pieces[*ni].id);
                        if !affected.contains(&a) && !affected.contains(&b) {
                            continue;
                        }
                        if let Some((area, c2)) = contact(pc, ng) {
                            let c3 = u * c2.x + v * c2.y + pl.normal * pl.offset;
                            out.push((*pl, a, b, area, c3.as_vec3()));
                        }
                    }
                }
                out
            })
            .collect();
        for (plane, a, b, area, center) in found {
            let id = self.next_seam;
            self.next_seam += 1;
            self.seams.push(Seam { id, plane, a, b, area, center, locked: false });
        }
    }

    /// Replaces `old` pieces by `new` solids and refreshes seams around them.
    /// `extra_planes` are planes the new pieces were just cut with.
    fn replace(&mut self, old: &[PieceId], new: Vec<Solid>, extra_planes: &[Plane]) -> Vec<PieceId> {
        let mut planes: Vec<Plane> = self.seams.iter().filter(|s| old.iter().any(|o| s.touches(*o))).map(|s| s.plane).collect();
        planes.extend_from_slice(extra_planes);
        let mut neighbours: Vec<PieceId> = old.iter().flat_map(|o| self.neighbours(*o)).filter(|n| !old.contains(n)).collect();
        neighbours.sort_unstable();
        neighbours.dedup();
        let locked_seams: Vec<Seam> = self.seams.iter().filter(|s| s.locked).cloned().collect();
        self.seams.retain(|s| !old.iter().any(|o| s.touches(*o)));
        self.pieces.retain(|p| !old.contains(&p.id));
        let base_id = self.next_piece;
        self.next_piece += new.len() as u32;
        let base_rev = self.next_rev;
        self.next_rev += new.len() as u64;
        let pieces: Vec<Piece> = new.into_par_iter().enumerate().map(|(i, s)| Piece::new(base_id + i as u32, s, base_rev + i as u64 + 1)).collect();
        let ids: Vec<PieceId> = pieces.iter().map(|p| p.id).collect();
        self.pieces.extend(pieces);
        let mut cands = ids.clone();
        cands.extend(neighbours);
        self.compute_seams(&ids, &cands, &planes);
        // Keep lock state for seams that survived between the same planes.
        for s in &mut self.seams {
            if locked_seams.iter().any(|l| l.plane.coincident(&s.plane, 1e-9, 1e-6) && (s.center - l.center).length() < 1.0) {
                s.locked = true;
            }
        }
        ids
    }

    /// Splits one piece by a plane; each resulting body becomes a piece.
    pub fn split_piece(&mut self, id: PieceId, plane: Plane) -> GResult<Vec<PieceId>> {
        let p = self.piece(id).ok_or(crate::manifold::GeomError::Failed("no such piece"))?;
        let (a, b) = p.solid.split_by_plane(plane.normal, plane.offset)?;
        let mut parts = a.bodies();
        parts.extend(b.bodies());
        parts.retain(|s| !s.is_empty());
        if parts.len() <= 1 {
            return Ok(vec![id]);
        }
        Ok(self.replace(&[id], parts, &[plane]))
    }

    /// Merges pieces into one.
    pub fn merge(&mut self, ids: &[PieceId]) -> GResult<PieceId> {
        let solids: Vec<&Solid> = ids.iter().filter_map(|i| self.piece(*i)).map(|p| p.solid.as_ref()).collect();
        if solids.len() < 2 {
            return Ok(ids.first().copied().unwrap_or(0));
        }
        let u = Solid::batch(&solids, crate::manifold::Op::Union)?;
        Ok(self.replace(ids, vec![u], &[])[0])
    }

    /// Re-cuts the union of the two pieces of a seam with a new plane. Only
    /// these two pieces change.
    pub fn recut_seam(&mut self, seam: SeamId, plane: Plane) -> GResult<Vec<PieceId>> {
        let s = self.seam(seam).ok_or(crate::manifold::GeomError::Failed("no such seam"))?.clone();
        let (pa, pb) = (self.piece(s.a).unwrap().solid.clone(), self.piece(s.b).unwrap().solid.clone());
        let u = pa.union(&pb)?;
        let (x, y) = u.split_by_plane(plane.normal, plane.offset)?;
        let mut parts = x.bodies();
        parts.extend(y.bodies());
        parts.retain(|s| !s.is_empty());
        Ok(self.replace(&[s.a, s.b], parts, &[plane]))
    }

    /// Bumps a piece's revision after its display geometry changed.
    pub fn touch(&mut self, id: PieceId) {
        let r = self.take_rev();
        if let Some(p) = self.piece_mut(id) {
            p.rev = r;
        }
    }

    pub fn total_volume(&self) -> f64 {
        self.pieces.iter().map(|p| p.volume).sum()
    }
}

pub fn dedup_planes(it: impl Iterator<Item = Plane>) -> Vec<Plane> {
    let mut out: Vec<Plane> = Vec::new();
    for p in it {
        if !out.iter().any(|q| q.coincident(&p, 1e-9, 1e-6)) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(s: f64) -> Solid {
        Solid::cube(DVec3::splat(s), false).unwrap()
    }

    #[test]
    fn split_creates_seam() {
        let mut p = Partition::from_solids(vec![cube(100.0)]);
        let ids = p.split_piece(0, Plane::axis(0, 40.0)).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(p.seams.len(), 1);
        let s = &p.seams[0];
        assert!((s.area - 10000.0).abs() < 1.0, "{}", s.area);
        // `a` is on the positive side.
        assert!(p.piece(s.a).unwrap().centroid.x > 40.0);
    }

    #[test]
    fn seams_are_per_pair() {
        // Cut in X, then cut the right half in Y: the X plane now has two seams.
        let mut p = Partition::from_solids(vec![cube(100.0)]);
        let ids = p.split_piece(0, Plane::axis(0, 50.0)).unwrap();
        let right = ids.iter().copied().find(|i| p.piece(*i).unwrap().centroid.x > 50.0).unwrap();
        p.split_piece(right, Plane::axis(1, 50.0)).unwrap();
        assert_eq!(p.pieces.len(), 3);
        let x_seams: Vec<&Seam> = p.seams.iter().filter(|s| s.plane.normal.x.abs() > 0.9).collect();
        assert_eq!(x_seams.len(), 2);
        for s in &x_seams {
            assert!((s.area - 5000.0).abs() < 1.0, "{}", s.area);
        }
        assert_eq!(p.seams.len(), 3);
        assert!((p.total_volume() - 1e6).abs() < 1.0);
    }

    #[test]
    fn recut_seam_only_changes_pair() {
        let mut p = Partition::from_solids(vec![cube(100.0)]);
        p.split_piece(0, Plane::axis(0, 50.0)).unwrap();
        let right = p.pieces.iter().find(|q| q.centroid.x > 50.0).unwrap().id;
        p.split_piece(right, Plane::axis(1, 50.0)).unwrap();
        let left = p.pieces.iter().find(|q| q.centroid.x < 50.0).unwrap().id;
        let left_rev = p.piece(left).unwrap().rev;
        // Move the Y seam between the two right pieces.
        let sy = p.seams.iter().find(|s| s.plane.normal.y.abs() > 0.9).unwrap().id;
        p.recut_seam(sy, Plane::axis(1, 70.0)).unwrap();
        assert_eq!(p.pieces.len(), 3);
        assert_eq!(p.piece(left).unwrap().rev, left_rev, "left piece untouched");
        let vols: Vec<f64> = p.pieces.iter().filter(|q| q.id != left).map(|q| q.volume).collect();
        assert!(vols.iter().any(|v| (v - 350000.0).abs() < 1.0), "{vols:?}");
        assert_eq!(p.seams.len(), 3);
    }

    #[test]
    fn merge_pieces() {
        let mut p = Partition::from_solids(vec![cube(100.0)]);
        let ids = p.split_piece(0, Plane::axis(2, 30.0)).unwrap();
        let m = p.merge(&ids).unwrap();
        assert_eq!(p.pieces.len(), 1);
        assert!((p.piece(m).unwrap().volume - 1e6).abs() < 1.0);
        assert!(p.seams.is_empty());
    }
}
