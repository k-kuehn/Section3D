//! Print orientation: which face goes on the bed.
//!
//! Candidates are the piece's cut faces plus its large flat convex-hull faces.
//! Ranking, in order: fits the printer > least support area > not tall >
//! least total overhang > largest bed contact.

use glam::{DVec2, DVec3, Quat, Vec2, Vec3};
use rayon::prelude::*;

use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::partition::{Flag, Orientation};

/// Support areas below this are ignored as scan noise (mm²).
pub const SUPPORT_NOISE: f32 = 25.0;
/// Faces this close to the bed rest on it (mm).
const BED_TOL: f32 = 0.3;

#[derive(Clone, Copy, Debug)]
pub struct OrientParams {
    /// Usable build volume X, Y, Z.
    pub bed: [f32; 3],
    /// Overhang limit from vertical (degrees).
    pub overhang_deg: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Eval {
    pub fits: bool,
    pub support: f32,
    pub tall: bool,
    pub overhang: f32,
    pub contact: f32,
    pub size: Vec3,
}

impl Eval {
    /// Lexicographic ranking key (smaller is better).
    fn key(&self) -> (u8, f32, u8, f32, f32) {
        let support = if self.support < SUPPORT_NOISE { 0.0 } else { (self.support / 10.0).round() };
        (!self.fits as u8, support, self.tall as u8, self.overhang, -self.contact)
    }
}

/// Rotation that puts a face with outward normal `down` on the bed.
pub fn rest_on(down: DVec3) -> Quat {
    Quat::from_rotation_arc(down.as_vec3().normalize(), -Vec3::Z)
}

/// Area-weighted downward normals of the hull's large flat faces.
fn hull_faces(mesh: &Mesh, limit: usize) -> Vec<DVec3> {
    let hull = match Solid::from_mesh(mesh).and_then(|s| s.hull()) {
        Ok(h) => h.to_mesh(),
        Err(_) => return Vec::new(),
    };
    // Group hull triangles by normal.
    let mut groups: Vec<(DVec3, f64)> = Vec::new();
    for t in 0..hull.tris.len() {
        let c = hull.tri_cross(t).as_dvec3();
        let a = c.length() * 0.5;
        if a < 1e-9 {
            continue;
        }
        let n = c.normalize();
        match groups.iter_mut().find(|g| g.0.dot(n) > 0.9995) {
            Some(g) => g.1 += a,
            None => groups.push((n, a)),
        }
    }
    groups.sort_by(|a, b| b.1.total_cmp(&a.1));
    groups.into_iter().take(limit).map(|g| g.0).collect()
}

/// 2D convex hull (Andrew's monotone chain).
fn hull2d(mut pts: Vec<DVec2>) -> Vec<DVec2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut h: Vec<DVec2> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = h.len();
        let it: Box<dyn Iterator<Item = &DVec2>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in it {
            while h.len() >= start + 2 && (h[h.len() - 1] - h[h.len() - 2]).perp_dot(p - h[h.len() - 2]) <= 0.0 {
                h.pop();
            }
            h.push(p);
        }
        h.pop();
    }
    h
}

/// Best yaw (rotation about Z) for a footprint: the angle (from hull edges)
/// that fits the bed, preferring the smallest bounding rectangle.
/// Returns (yaw, width, depth).
pub fn fit_yaw(foot: &[DVec2], bed: [f32; 3]) -> (f64, f64, f64) {
    let h = hull2d(foot.to_vec());
    if h.len() < 3 {
        return (0.0, 0.0, 0.0);
    }
    let mut best: Option<(bool, f64, f64, f64, f64)> = None; // (fits, area, yaw, w, d)
    // Hull edge directions give the minimal rectangles; a sweep of angles
    // finds diagonal placements on long pieces.
    let mut yaws: Vec<f64> = Vec::with_capacity(h.len() * 2 + 90);
    for i in 0..h.len() {
        let e = h[(i + 1) % h.len()] - h[i];
        if e.length_squared() < 1e-12 {
            continue;
        }
        let ang = e.y.atan2(e.x);
        yaws.push(-ang);
        yaws.push(-ang + std::f64::consts::FRAC_PI_2);
    }
    yaws.extend((0..90).map(|i| (i as f64 * 2.0).to_radians()));
    {
        for yaw in yaws {
            let (s, c) = yaw.sin_cos();
            let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
            for p in &h {
                let q = DVec2::new(c * p.x - s * p.y, s * p.x + c * p.y);
                lo = lo.min(q);
                hi = hi.max(q);
            }
            let (w, d) = (hi.x - lo.x, hi.y - lo.y);
            let fits = w <= bed[0] as f64 + 1e-6 && d <= bed[1] as f64 + 1e-6;
            let area = w * d;
            let better = match best {
                None => true,
                Some((bf, ba, ..)) => (fits && !bf) || (fits == bf && area < ba - 1e-9),
            };
            if better {
                best = Some((fits, area, yaw, w, d));
            }
        }
    }
    let b = best.unwrap();
    (b.2, b.3, b.4)
}

/// Evaluates the piece resting with outward normal `down` on the bed.
pub fn evaluate(mesh: &Mesh, down: DVec3, p: &OrientParams) -> (Eval, Orientation) {
    let q0 = rest_on(down);
    let rotated: Vec<Vec3> = mesh.verts.iter().map(|v| q0 * *v).collect();
    let zmin = rotated.iter().map(|v| v.z).fold(f32::INFINITY, f32::min);
    let foot: Vec<DVec2> = rotated.iter().map(|v| DVec2::new(v.x as f64, v.y as f64)).collect();
    let (yaw, w, d) = fit_yaw(&foot, p.bed);
    let q = Quat::from_rotation_z(yaw as f32) * q0;
    let zmax = rotated.iter().map(|v| v.z).fold(f32::NEG_INFINITY, f32::max);
    let height = zmax - zmin;
    let sin_lim = p.overhang_deg.to_radians().sin();
    let (mut support, mut overhang, mut contact) = (0f32, 0f32, 0f32);
    for t in 0..mesh.tris.len() {
        let [a, b, c] = mesh.tris[t].map(|i| rotated[i as usize]);
        let cr = (b - a).cross(c - a);
        let area = cr.length() * 0.5;
        if area < 1e-12 {
            continue;
        }
        let nz = cr.z / (2.0 * area);
        let low = a.z.min(b.z).min(c.z) - zmin;
        if nz < -0.999 && low < BED_TOL && a.z.max(b.z).max(c.z) - zmin < BED_TOL {
            contact += area;
            continue;
        }
        if nz < 0.0 && low > BED_TOL * 0.5 {
            overhang += area * (-nz);
            if nz < -sin_lim {
                support += area;
            }
        }
    }
    let size = Vec3::new(w as f32, d as f32, height);
    let fits = size.x <= p.bed[0] + 1e-3 && size.y <= p.bed[1] + 1e-3 && size.z <= p.bed[2] + 1e-3;
    let tall = height > size.x.max(size.y) * 1.05;
    // Centre footprint on the bed origin with min z = 0.
    let rv: Vec<Vec3> = mesh.verts.iter().map(|v| q * *v).collect();
    let lo = rv.iter().fold(Vec3::INFINITY, |a, v| a.min(*v));
    let hi = rv.iter().fold(Vec3::NEG_INFINITY, |a, v| a.max(*v));
    let c = (lo + hi) * 0.5;
    let translation = Vec3::new(-c.x, -c.y, -lo.z);
    (
        Eval { fits, support, tall, overhang, contact, size },
        Orientation { rotation: q, translation, size, support_area: support },
    )
}

/// Chooses the best orientation. `cut_normals` are outward normals of the
/// piece's cut faces. Returns the orientation and any flags it implies.
pub fn best(mesh: &Mesh, cut_normals: &[DVec3], p: &OrientParams) -> (Orientation, Vec<Flag>) {
    let mut cands: Vec<DVec3> = cut_normals.to_vec();
    cands.extend(hull_faces(mesh, 10));
    let mut uniq: Vec<DVec3> = Vec::new();
    for c in cands {
        if !uniq.iter().any(|u| u.dot(c) > 0.9999) {
            uniq.push(c);
        }
    }
    if uniq.is_empty() {
        uniq.push(-DVec3::Z);
    }
    let evals: Vec<(Eval, Orientation)> = uniq.par_iter().map(|d| evaluate(mesh, *d, p)).collect();
    let (e, o) = evals.into_iter().min_by(|a, b| a.0.key().partial_cmp(&b.0.key()).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
    let mut flags = Vec::new();
    if !e.fits {
        flags.push(Flag::TooBig);
    }
    if e.support >= SUPPORT_NOISE {
        flags.push(Flag::NeedsSupport(e.support));
    }
    if e.tall {
        flags.push(Flag::Tall);
    }
    (o, flags)
}

/// Faces of `mesh` needing support in orientation `o` (for display).
pub fn support_mask(mesh: &Mesh, o: &Orientation, overhang_deg: f32) -> Vec<bool> {
    let sin_lim = overhang_deg.to_radians().sin();
    let m = o.matrix();
    let v: Vec<Vec3> = mesh.verts.iter().map(|p| m.transform_point3(*p)).collect();
    mesh.tris
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| v[i as usize]);
            let n = (b - a).cross(c - a).normalize_or_zero();
            n.z < -sin_lim && a.z.min(b.z).min(c.z) > BED_TOL * 0.5
        })
        .collect()
}

pub fn footprint_vec2(pts: &[Vec3]) -> Vec<Vec2> {
    pts.iter().map(|p| Vec2::new(p.x, p.y)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;

    fn params() -> OrientParams {
        OrientParams { bed: [200.0, 200.0, 200.0], overhang_deg: 45.0 }
    }

    #[test]
    fn long_bar_lies_flat() {
        let m = shapes::cube(Vec3::new(20.0, 20.0, 150.0));
        let (o, flags) = best(&m, &[], &params());
        assert!(o.size.z < 25.0, "{:?}", o.size);
        assert!(flags.is_empty(), "{flags:?}");
    }

    #[test]
    fn diagonal_fit() {
        // 250 long: only fits diagonally on a 200x200 bed.
        let m = shapes::cube(Vec3::new(250.0, 20.0, 10.0));
        let (o, flags) = best(&m, &[], &params());
        assert!(!flags.contains(&Flag::TooBig), "{:?} {flags:?}", o.size);
    }

    #[test]
    fn upside_down_cone_rests_on_its_base() {
        // A cone with its point down needs support unless flipped.
        let s = Solid::cylinder(80.0, 0.0, 60.0, 64, false).unwrap();
        let (o, flags) = best(&s.to_mesh(), &[], &params());
        assert!(flags.is_empty(), "{flags:?}");
        assert!(o.size.z > 70.0);
    }
}
