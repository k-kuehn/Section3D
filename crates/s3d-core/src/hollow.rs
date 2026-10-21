//! Hollowing to a shell of a given wall thickness.
//!
//! The cavity is the region deeper than `wall` below the surface. Its surface
//! is extracted with sparse marching tetrahedra in a narrow band around the
//! model, then subtracted from the solid.

use glam::{IVec3, Vec3};
use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::job::{Cancelled, JobCtx};
use crate::manifold::Solid;
use crate::march::{self, BLOCK};
use crate::mesh::Mesh;

#[derive(Debug)]
pub enum HollowError {
    Cancelled,
    /// Wall is so thick (or the model so thin) that nothing would be removed.
    NothingToRemove,
    Geometry(String),
}

impl From<Cancelled> for HollowError {
    fn from(_: Cancelled) -> Self {
        HollowError::Cancelled
    }
}

impl std::fmt::Display for HollowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HollowError::Cancelled => f.write_str("cancelled"),
            HollowError::NothingToRemove => f.write_str("the model is thinner than twice the wall thickness everywhere, so there is nothing to hollow"),
            HollowError::Geometry(s) => write!(f, "hollowing failed: {s}"),
        }
    }
}

/// Grid spacing used for the cavity surface.
pub fn resolution(mesh: &Mesh, wall: f32) -> f32 {
    (wall * 0.5).max(mesh.bbox().diagonal() / 500.0)
}

/// Returns the hollowed solid. Walls are at least `wall` thick (up to about
/// one grid step thicker, since the cavity is kept conservative).
pub fn hollow(mesh: &Mesh, wall: f32, job: &JobCtx) -> Result<Mesh, HollowError> {
    let outer = Solid::from_mesh(mesh).map_err(|e| HollowError::Geometry(e.to_string()))?;
    job.set_stage("Hollowing: measuring walls");
    let bvh = Bvh::new(mesh);
    let h = resolution(mesh, wall);
    let iso = wall + 0.5 * h;
    let bb = mesh.bbox();
    let origin = bb.min - Vec3::splat(h * BLOCK as f32);
    let dims = ((bb.size() / h).ceil().as_ivec3()) + IVec3::splat(2 * BLOCK);
    let half_diag = (BLOCK as f32 * h) * 0.5 * 3f32.sqrt();
    let all = march::blocks_in(IVec3::ZERO, dims);
    job.check()?;
    // Keep blocks whose centre lies inside the solid, or close enough to the
    // surface that the cavity boundary could pass through them.
    let active: Vec<IVec3> = all
        .par_iter()
        .copied()
        .filter(|b| {
            let c = origin + (b.as_vec3() + Vec3::splat(0.5)) * (BLOCK as f32 * h);
            bvh.closest(c, iso + half_diag + h).is_some()
        })
        .collect();
    job.check()?;
    job.set_stage("Hollowing: building inner surface");
    let reach = iso + 2.0 * h;
    let field = |p: Vec3| -> f32 {
        match bvh.closest(p, reach) {
            Some((d, q, t)) => {
                let tri = bvh.triangle(t);
                let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
                let off = p - q;
                // Trust the face normal only when the closest point is clearly
                // inside the face; otherwise do a full inside test.
                let inside = if off.length_squared() > 1e-12 && on_face_interior(q, &tri) { off.dot(n) < 0.0 } else { bvh.contains(p) };
                if inside { d - iso } else { -d - iso }
            }
            // Farther than `reach` from the surface: deep inside or outside.
            None => {
                if bvh.contains(p) { reach - iso } else { -reach - iso }
            }
        }
    };
    let cavity = march::extract(origin, h, &active, field);
    job.check()?;
    if cavity.tris.is_empty() {
        return Err(HollowError::NothingToRemove);
    }
    job.set_stage("Hollowing: subtracting cavity");
    // A stray inside/outside sign can leave a tiny hole; close it if so.
    let cavity = if crate::repair::edge_stats(&cavity).is_closed_manifold() { cavity } else { crate::repair::repair(cavity, job)?.mesh };
    let cav = Solid::from_mesh(&cavity).map_err(|e| HollowError::Geometry(e.to_string()))?;
    let shell = outer.subtract(&cav).map_err(|e| HollowError::Geometry(e.to_string()))?;
    Ok(shell.to_mesh())
}

fn on_face_interior(q: Vec3, t: &[Vec3; 3]) -> bool {
    let n = (t[1] - t[0]).cross(t[2] - t[0]);
    let a2 = n.length_squared();
    if a2 < 1e-20 {
        return false;
    }
    let b0 = (t[1] - q).cross(t[2] - q).dot(n) / a2;
    let b1 = (t[2] - q).cross(t[0] - q).dot(n) / a2;
    let b2 = 1.0 - b0 - b1;
    b0 > 1e-3 && b1 > 1e-3 && b2 > 1e-3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;

    #[test]
    fn hollow_cube_keeps_wall() {
        let m = shapes::cube(Vec3::splat(40.0));
        let out = hollow(&m, 3.0, &JobCtx::new()).unwrap();
        let s = Solid::from_mesh(&out).unwrap();
        let v = s.volume();
        // Cavity is ~ (40 - 2*3)^3 = 39304 minus the conservative margin.
        assert!(v > 64000.0 - 39304.0 && v < 64000.0 - 25000.0, "{v}");
        assert_eq!(s.decompose().len(), 2);
        assert_eq!(s.bodies().len(), 1);
    }

    #[test]
    fn thin_plate_has_nothing_to_remove() {
        let m = shapes::cube(Vec3::new(40.0, 40.0, 4.0));
        assert!(matches!(hollow(&m, 3.0, &JobCtx::new()), Err(HollowError::NothingToRemove)));
    }
}
