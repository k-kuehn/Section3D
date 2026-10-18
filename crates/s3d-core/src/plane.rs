//! Oriented cut planes: `normal · p = offset`, normal pointing to the "positive" side.

use glam::{DVec3, Vec3, Vec4};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plane {
    pub normal: DVec3,
    pub offset: f64,
}

impl Plane {
    pub fn new(normal: DVec3, offset: f64) -> Plane {
        let l = normal.length();
        Plane { normal: normal / l, offset: offset / l }
    }

    pub fn through(point: DVec3, normal: DVec3) -> Plane {
        let n = normal.normalize();
        Plane { normal: n, offset: n.dot(point) }
    }

    pub fn axis(axis: usize, at: f64) -> Plane {
        let mut n = DVec3::ZERO;
        n[axis] = 1.0;
        Plane { normal: n, offset: at }
    }

    #[inline]
    pub fn dist(&self, p: Vec3) -> f64 {
        self.normal.dot(p.as_dvec3()) - self.offset
    }

    pub fn flipped(&self) -> Plane {
        Plane { normal: -self.normal, offset: -self.offset }
    }

    pub fn as_vec4(&self) -> Vec4 {
        Vec4::new(self.normal.x as f32, self.normal.y as f32, self.normal.z as f32, self.offset as f32)
    }

    /// True if both describe the same geometric plane (either orientation).
    pub fn coincident(&self, o: &Plane, ang_tol: f64, dist_tol: f64) -> bool {
        let d = self.normal.dot(o.normal);
        if d > 1.0 - ang_tol {
            (self.offset - o.offset).abs() < dist_tol
        } else if d < -(1.0 - ang_tol) {
            (self.offset + o.offset).abs() < dist_tol
        } else {
            false
        }
    }

    /// Orthonormal basis (u, v) spanning the plane.
    pub fn basis(&self) -> (DVec3, DVec3) {
        let u = self.normal.any_orthonormal_vector();
        (u, self.normal.cross(u))
    }

    pub fn project(&self, p: DVec3) -> DVec3 {
        p - self.normal * (self.normal.dot(p) - self.offset)
    }

    /// Rotates the plane about an axis lying in it through `pivot`.
    pub fn tilted(&self, pivot: DVec3, axis: DVec3, angle: f64) -> Plane {
        let q = glam::DQuat::from_axis_angle(axis.normalize(), angle);
        Plane::through(pivot, q * self.normal)
    }

    pub fn moved(&self, delta: f64) -> Plane {
        Plane { normal: self.normal, offset: self.offset + delta }
    }
}
