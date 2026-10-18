//! Painted no-cut zones: brush dabs on the model surface that cuts avoid.

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::plane::Plane;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dab {
    /// Position in source-model coordinates.
    pub p: Vec3,
    pub radius: f32,
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
        Zones { dabs: self.dabs.iter().map(|d| Dab { p: m.transform_point3(d.p), radius: d.radius * s }).collect() }
    }

    /// How badly a plane crosses painted areas (sum of overlap depth).
    pub fn plane_cost(&self, pl: &Plane) -> f32 {
        self.dabs
            .iter()
            .map(|d| {
                let dist = pl.dist(d.p).abs() as f32;
                (d.radius - dist).max(0.0) / d.radius.max(1e-3)
            })
            .sum()
    }

    /// Removes dabs within `radius` of `p` (eraser).
    pub fn erase(&mut self, p: Vec3, radius: f32) {
        self.dabs.retain(|d| d.p.distance(p) > radius);
    }
}
