//! Indexed triangle mesh: the common currency between I/O, geometry and rendering.

use glam::{Mat4, Vec3};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) };

    pub fn from_points<'a>(pts: impl IntoIterator<Item = &'a Vec3>) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in pts {
            b.grow(*p);
        }
        b
    }
    pub fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    pub fn size(&self) -> Vec3 {
        (self.max - self.min).max(Vec3::ZERO)
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
    pub fn diagonal(&self) -> f32 {
        self.size().length()
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Mesh {
    pub verts: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
}

impl Mesh {
    pub fn new(verts: Vec<Vec3>, tris: Vec<[u32; 3]>) -> Self {
        Mesh { verts, tris }
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    #[inline]
    pub fn tri(&self, t: usize) -> [Vec3; 3] {
        let [a, b, c] = self.tris[t];
        [self.verts[a as usize], self.verts[b as usize], self.verts[c as usize]]
    }

    /// Unnormalised face normal (length = 2 * area).
    #[inline]
    pub fn tri_cross(&self, t: usize) -> Vec3 {
        let [a, b, c] = self.tri(t);
        (b - a).cross(c - a)
    }

    pub fn face_normals(&self) -> Vec<Vec3> {
        (0..self.tris.len()).into_par_iter().map(|t| self.tri_cross(t).normalize_or_zero()).collect()
    }

    pub fn bbox(&self) -> Aabb {
        self.verts
            .par_chunks(65536)
            .map(|c| Aabb::from_points(c))
            .reduce(|| Aabb::EMPTY, |a, b| a.union(&b))
    }

    pub fn area(&self) -> f64 {
        (0..self.tris.len()).into_par_iter().map(|t| self.tri_cross(t).length() as f64 * 0.5).sum()
    }

    /// Signed volume (positive for outward-facing CCW winding).
    pub fn volume(&self) -> f64 {
        (0..self.tris.len())
            .into_par_iter()
            .map(|t| {
                let [a, b, c] = self.tri(t);
                a.as_dvec3().dot(b.as_dvec3().cross(c.as_dvec3())) / 6.0
            })
            .sum()
    }

    pub fn centroid(&self) -> Vec3 {
        // Volume-weighted centroid; falls back to the bbox centre for open meshes.
        let (v, c) = (0..self.tris.len())
            .into_par_iter()
            .map(|t| {
                let [a, b, c] = self.tri(t);
                let (a, b, c) = (a.as_dvec3(), b.as_dvec3(), c.as_dvec3());
                let v = a.dot(b.cross(c)) / 6.0;
                (v, (a + b + c) * (v / 4.0))
            })
            .reduce(|| (0.0, glam::DVec3::ZERO), |x, y| (x.0 + y.0, x.1 + y.1));
        if v.abs() < 1e-9 { self.bbox().center() } else { (c / v).as_vec3() }
    }

    pub fn transform(&mut self, m: &Mat4) {
        let flip = m.determinant() < 0.0;
        self.verts.par_iter_mut().for_each(|v| *v = m.transform_point3(*v));
        if flip {
            self.tris.par_iter_mut().for_each(|t| t.swap(1, 2));
        }
    }

    pub fn transformed(&self, m: &Mat4) -> Mesh {
        let mut out = self.clone();
        out.transform(m);
        out
    }

    pub fn translate(&mut self, d: Vec3) {
        self.verts.par_iter_mut().for_each(|v| *v += d);
    }

    pub fn append(&mut self, other: &Mesh) {
        let off = self.verts.len() as u32;
        self.verts.extend_from_slice(&other.verts);
        self.tris.extend(other.tris.iter().map(|t| [t[0] + off, t[1] + off, t[2] + off]));
    }

    pub fn flip(&mut self) {
        self.tris.par_iter_mut().for_each(|t| t.swap(1, 2));
    }

    /// Drops vertices not referenced by any triangle.
    pub fn compact(&mut self) {
        let mut map = vec![u32::MAX; self.verts.len()];
        let mut verts = Vec::with_capacity(self.verts.len());
        for t in &mut self.tris {
            for i in t.iter_mut() {
                let m = &mut map[*i as usize];
                if *m == u32::MAX {
                    *m = verts.len() as u32;
                    verts.push(self.verts[*i as usize]);
                }
                *i = *m;
            }
        }
        self.verts = verts;
    }

    /// Flat-shaded vertex stream (3 verts per triangle) for rendering.
    pub fn unindexed_positions(&self) -> Vec<Vec3> {
        self.tris.par_iter().flat_map_iter(|t| t.map(|i| self.verts[i as usize])).collect()
    }
}

#[cfg(test)]
pub mod shapes {
    //! Procedural test shapes.
    use super::*;

    pub fn cube(size: Vec3) -> Mesh {
        let s = size;
        let verts = (0..8)
            .map(|i| Vec3::new(if i & 1 != 0 { s.x } else { 0.0 }, if i & 2 != 0 { s.y } else { 0.0 }, if i & 4 != 0 { s.z } else { 0.0 }))
            .collect();
        let tris = vec![
            [0, 2, 1], [1, 2, 3], // z=0
            [4, 5, 6], [5, 7, 6], // z=max
            [0, 1, 4], [1, 5, 4], // y=0
            [2, 6, 3], [3, 6, 7], // y=max
            [0, 4, 2], [2, 4, 6], // x=0
            [1, 3, 5], [3, 7, 5], // x=max
        ];
        Mesh::new(verts, tris)
    }

    pub fn uv_sphere(r: f32, seg: u32, rings: u32) -> Mesh {
        let mut verts = vec![Vec3::new(0.0, 0.0, -r)];
        for j in 1..rings {
            let phi = std::f32::consts::PI * j as f32 / rings as f32 - std::f32::consts::FRAC_PI_2;
            for i in 0..seg {
                let th = std::f32::consts::TAU * i as f32 / seg as f32;
                verts.push(Vec3::new(r * phi.cos() * th.cos(), r * phi.cos() * th.sin(), r * phi.sin()));
            }
        }
        verts.push(Vec3::new(0.0, 0.0, r));
        let top = verts.len() as u32 - 1;
        let ring = |j: u32, i: u32| 1 + (j - 1) * seg + (i % seg);
        let mut tris = Vec::new();
        for i in 0..seg {
            tris.push([0, ring(1, i + 1), ring(1, i)]);
            tris.push([top, ring(rings - 1, i), ring(rings - 1, i + 1)]);
        }
        for j in 1..rings - 1 {
            for i in 0..seg {
                let (a, b, c, d) = (ring(j, i), ring(j, i + 1), ring(j + 1, i), ring(j + 1, i + 1));
                tris.push([a, b, d]);
                tris.push([a, d, c]);
            }
        }
        Mesh::new(verts, tris)
    }
}

#[cfg(test)]
mod tests {
    use super::shapes::*;
    use super::*;

    #[test]
    fn cube_volume_area() {
        let m = cube(Vec3::new(10.0, 20.0, 30.0));
        assert!((m.volume() - 6000.0).abs() < 1e-6);
        assert!((m.area() - 2200.0).abs() < 1e-4);
        assert!((m.centroid() - Vec3::new(5.0, 10.0, 15.0)).length() < 1e-4);
    }

    #[test]
    fn sphere_is_outward() {
        let m = uv_sphere(10.0, 32, 16);
        assert!(m.volume() > 3900.0 && m.volume() < 4189.0);
    }

    #[test]
    fn mirror_keeps_orientation() {
        let mut m = cube(Vec3::ONE);
        m.transform(&Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        assert!(m.volume() > 0.0);
    }
}
