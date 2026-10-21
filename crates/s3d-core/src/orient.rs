//! Print orientation: which face goes on the bed.
//!
//! Candidates are the piece's cut faces plus its large flat convex-hull faces.
//! Ranking, in order: fits the printer > least support area > not tall >
//! steadiest (height against bed contact) > least total overhang > largest
//! bed contact.

use glam::{DVec2, DVec3, Quat, Vec3};
use rayon::prelude::*;

use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::partition::{Flag, Orientation};

/// Support areas below this are ignored (mm²): tiny overhangs from scan
/// noise or small bumps bridge fine.
pub const SUPPORT_NOISE: f32 = 100.0;
/// Faces this close to the bed rest on it (mm).
const BED_TOL: f32 = 0.3;
/// A piece is tall when it stands higher than this times the length of its
/// bed contact: thin standing panels tip over or peel off the bed.
pub const TALL_RATIO: f32 = 0.8;
/// Pieces lower than this never count as tall (mm).
pub const TALL_MIN_HEIGHT: f32 = 40.0;

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
    /// Height over the longest span of the bed contact.
    pub ratio: f32,
}

/// Bed contact below this is a piece balancing on points or a line (mm²).
const MIN_CONTACT: f32 = 20.0;

impl Eval {
    /// Lexicographic ranking key (smaller is better). Small overhangs count
    /// as none for the first comparison, but between otherwise equal poses
    /// the one resting flat with less overhang wins.
    fn key(&self) -> (u8, f32, u8, u8, f32, f32, f32, f32) {
        let support = if self.support < SUPPORT_NOISE { 0.0 } else { (self.support / 10.0).round() };
        let rocks = self.contact < MIN_CONTACT;
        (!self.fits as u8, support, self.tall as u8, rocks as u8, (self.support / 25.0).round(), (self.ratio.min(4.0) * 8.0).round(), self.overhang, -self.contact)
    }
}

/// Longest distance between two points of a small 2D point set (via its hull).
fn diameter(pts: Vec<DVec2>) -> f64 {
    let h = hull2d(pts);
    let mut d = 0.0f64;
    for i in 0..h.len() {
        for j in i + 1..h.len() {
            d = d.max(h[i].distance(h[j]));
        }
    }
    d
}

/// Rotation that puts a face with outward normal `down` on the bed.
pub fn rest_on(down: DVec3) -> Quat {
    Quat::from_rotation_arc(down.as_vec3().normalize(), -Vec3::Z)
}

/// Convex hull of a piece: its vertices (all extreme points, enough for
/// footprints, heights and bed contact) and its largest flat faces.
struct Hull {
    pts: Vec<Vec3>,
    faces: Vec<DVec3>,
}

fn hull_of(mesh: &Mesh, limit: usize) -> Hull {
    // Hull of the points: works for open meshes too (merge estimates).
    let step = (mesh.verts.len() / 20_000).max(1);
    let pts: Vec<DVec3> = mesh.verts.iter().step_by(step).map(|v| v.as_dvec3()).collect();
    let hull = match Solid::hull_points(&pts) {
        Ok(h) => h.to_mesh(),
        Err(_) => return Hull { pts: mesh.verts.clone(), faces: Vec::new() },
    };
    if hull.verts.len() < 4 {
        return Hull { pts: mesh.verts.clone(), faces: Vec::new() };
    }
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
    Hull { pts: hull.verts, faces: groups.into_iter().take(limit).map(|g| g.0).collect() }
}

/// 2D convex hull (Andrew's monotone chain), counter-clockwise.
pub fn hull2d(mut pts: Vec<DVec2>) -> Vec<DVec2> {
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
    evaluate_with(mesh, &hull_of(mesh, 0).pts, down, p)
}

fn evaluate_with(mesh: &Mesh, hull: &[Vec3], down: DVec3, p: &OrientParams) -> (Eval, Orientation) {
    let q0 = rest_on(down);
    let rotated: Vec<Vec3> = hull.iter().map(|v| q0 * *v).collect();
    let zmin = rotated.iter().map(|v| v.z).fold(f32::INFINITY, f32::min);
    let foot: Vec<DVec2> = rotated.iter().map(|v| DVec2::new(v.x as f64, v.y as f64)).collect();
    let (yaw, w, d) = fit_yaw(&foot, p.bed);
    let q = Quat::from_rotation_z(yaw as f32) * q0;
    let zmax = rotated.iter().map(|v| v.z).fold(f32::NEG_INFINITY, f32::max);
    let height = zmax - zmin;
    let sin_lim = p.overhang_deg.to_radians().sin();
    let (mut support, mut overhang, mut contact) = (0f32, 0f32, 0f32);
    let zs: Vec<f32> = mesh.verts.iter().map(|v| (q0 * *v).z).collect();
    for t in 0..mesh.tris.len() {
        let [a, b, c] = mesh.tris[t].map(|i| zs[i as usize]);
        let cr = q0 * mesh.tri_cross(t);
        let area = cr.length() * 0.5;
        if area < 1e-12 {
            continue;
        }
        let nz = cr.z / (2.0 * area);
        let low = a.min(b).min(c) - zmin;
        if nz < -0.999 && low < BED_TOL && a.max(b).max(c) - zmin < BED_TOL {
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
    // Bed contact: points resting on the bed (works for faces, edges and
    // the rims of standing shells alike); they are all hull points.
    let base: Vec<DVec2> = rotated.iter().filter(|v| v.z - zmin < BED_TOL).map(|v| DVec2::new(v.x as f64, v.y as f64)).collect();
    let span = diameter(base) as f32;
    let ratio = height / span.max(1.0);
    let tall = height > TALL_MIN_HEIGHT && ratio > TALL_RATIO;
    // Centre footprint on the bed origin with min z = 0.
    let rv: Vec<Vec3> = hull.iter().map(|v| q * *v).collect();
    let lo = rv.iter().fold(Vec3::INFINITY, |a, v| a.min(*v));
    let hi = rv.iter().fold(Vec3::NEG_INFINITY, |a, v| a.max(*v));
    let c = (lo + hi) * 0.5;
    let translation = Vec3::new(-c.x, -c.y, -lo.z);
    (
        Eval { fits, support, tall, overhang, contact, size, ratio },
        Orientation { rotation: q, translation, size, support_area: support },
    )
}

/// Chooses the best orientation. `cut_normals` are outward normals of the
/// piece's cut faces. Returns the orientation and any flags it implies.
pub fn best(mesh: &Mesh, cut_normals: &[DVec3], p: &OrientParams) -> (Orientation, Vec<Flag>) {
    let (_, o, f) = best_eval(mesh, cut_normals, p);
    (o, f)
}

/// Like `best`, also returning the evaluation of the chosen pose.
pub fn best_eval(mesh: &Mesh, cut_normals: &[DVec3], p: &OrientParams) -> (Eval, Orientation, Vec<Flag>) {
    let hull = hull_of(mesh, 10);
    let mut cands: Vec<DVec3> = cut_normals.to_vec();
    cands.extend(hull.faces.iter().copied());
    let mut uniq: Vec<DVec3> = Vec::new();
    for c in cands {
        if !uniq.iter().any(|u| u.dot(c) > 0.9999) {
            uniq.push(c);
        }
    }
    if uniq.is_empty() {
        uniq.push(-DVec3::Z);
    }
    let evals: Vec<(Eval, Orientation)> = uniq.par_iter().map(|d| evaluate_with(mesh, &hull.pts, *d, p)).collect();
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
    (e, o, flags)
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
    fn flat_face_beats_a_slightly_overhanging_pose() {
        // A wedge: on its slanted face it stands lower and wider, but a
        // little overhangs; on its square end it rests flat with none.
        let s = Solid::cube(DVec3::new(120.0, 30.0, 40.0), false).unwrap();
        let cut = s.split_by_plane(DVec3::new(0.25, 0.0, 1.0).normalize(), 40.0).unwrap().1;
        let m = cut.to_mesh();
        let (o, _) = best(&m, &[], &params());
        let (e, _) = evaluate(&m, (o.rotation.inverse() * -Vec3::Z).as_dvec3(), &params());
        assert!(e.contact >= MIN_CONTACT, "{e:?}");
        assert!(e.support < 1.0, "{e:?}");
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
