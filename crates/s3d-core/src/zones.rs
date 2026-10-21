//! Painted no-cut zones: brush dabs on the model surface that cuts avoid.

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};


#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dab {
    /// Position in source-model coordinates.
    pub p: Vec3,
    pub radius: f32,
    /// Surface normal where it was painted (zero for old projects); the
    /// paint covers only surface facing the same way, not the far side of a
    /// thin wall.
    #[serde(default)]
    pub n: Vec3,
}

impl Dab {
    /// True when surface triangle `t` (same coordinates) is painted by this dab.
    pub fn covers(&self, t: &[Vec3; 3]) -> bool {
        let n = (t[1] - t[0]).cross(t[2] - t[0]);
        if self.n != Vec3::ZERO && n.dot(self.n) <= 0.0 {
            return false;
        }
        self.touches(t)
    }

    /// True when triangle `t` comes within the dab's radius (either side).
    pub fn touches(&self, t: &[Vec3; 3]) -> bool {
        crate::bvh::closest_on_tri(self.p, t).distance_squared(self.p) <= self.radius * self.radius
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Zones {
    pub dabs: Vec<Dab>,
}

impl Zones {
    pub fn is_empty(&self) -> bool {
        self.dabs.is_empty()
    }

    /// Dabs mapped into prepared-model coordinates.
    pub fn transformed(&self, m: &Mat4) -> Zones {
        let s = m.to_scale_rotation_translation().0.abs().max_element();
        Zones { dabs: self.dabs.iter().map(|d| Dab { p: m.transform_point3(d.p), radius: d.radius * s, n: m.transform_vector3(d.n).normalize_or_zero() }).collect() }
    }

    /// Removes dabs within `radius` of `p` (eraser).
    pub fn erase(&mut self, p: Vec3, radius: f32) {
        self.dabs.retain(|d| d.p.distance(p) > radius);
    }
}
