//! Safe wrapper over the Manifold C shim (csrc/shim.cpp).
//!
//! `Solid` is an immutable, guaranteed-manifold solid. Operations return new
//! solids; failures surface as `Err(GeomError)` instead of null handles.

use std::ffi::c_void;

use glam::{DMat4, DVec3, Vec3};

use crate::mesh::{Aabb, Mesh};

#[repr(C)]
struct RawMf {
    _p: [u8; 0],
}
#[repr(C)]
struct RawMesh {
    _p: [u8; 0],
}

type SdfFn = extern "C" fn(f64, f64, f64, *mut c_void) -> f64;

unsafe extern "C" {
    fn s3d_mf_free(h: *mut RawMf);
    fn s3d_mesh_free(h: *mut RawMesh);
    fn s3d_mf_from_mesh(verts: *const f32, nverts: u32, merge: i32, tris: *const u32, ntris: u32) -> *mut RawMf;
    fn s3d_mf_clone(h: *const RawMf) -> *mut RawMf;
    fn s3d_mf_status(h: *const RawMf) -> i32;
    fn s3d_mf_is_empty(h: *const RawMf) -> i32;
    fn s3d_mf_num_tri(h: *const RawMf) -> u64;
    fn s3d_mf_volume(h: *const RawMf) -> f64;
    fn s3d_mf_genus(h: *const RawMf) -> i32;
    fn s3d_mf_bbox(h: *const RawMf, out6: *mut f64);
    fn s3d_mf_get_mesh(h: *const RawMf) -> *mut RawMesh;
    fn s3d_mesh_num_vert(m: *const RawMesh) -> u32;
    fn s3d_mesh_num_tri(m: *const RawMesh) -> u32;
    fn s3d_mesh_copy(m: *const RawMesh, verts: *mut f32, tris: *mut u32);
    fn s3d_mf_boolean(a: *const RawMf, b: *const RawMf, op: i32) -> *mut RawMf;
    fn s3d_mf_batch_boolean(hs: *const *const RawMf, n: u32, op: i32) -> *mut RawMf;
    fn s3d_mf_compose(hs: *const *const RawMf, n: u32) -> *mut RawMf;
    fn s3d_mf_split_by_plane(h: *const RawMf, nx: f64, ny: f64, nz: f64, off: f64, pos: *mut *mut RawMf, neg: *mut *mut RawMf) -> i32;
    fn s3d_mf_trim_by_plane(h: *const RawMf, nx: f64, ny: f64, nz: f64, off: f64) -> *mut RawMf;
    fn s3d_mf_decompose(h: *const RawMf, out: *mut *mut *mut RawMf) -> u32;
    fn s3d_mf_free_array(arr: *mut *mut RawMf);
    fn s3d_mf_transform(h: *const RawMf, m: *const f64) -> *mut RawMf;
    fn s3d_mf_cube(x: f64, y: f64, z: f64, center: i32) -> *mut RawMf;
    fn s3d_mf_cylinder(h: f64, r_low: f64, r_high: f64, segments: i32, center: i32) -> *mut RawMf;
    fn s3d_mf_sphere(r: f64, segments: i32) -> *mut RawMf;
    fn s3d_mf_extrude(xy: *const f64, sizes: *const u32, n: u32, height: f64, scale_top: f64) -> *mut RawMf;
    fn s3d_mf_hull(h: *const RawMf) -> *mut RawMf;
    fn s3d_mf_hull_points(pts: *const f64, n: u32) -> *mut RawMf;
    fn s3d_mf_simplify(h: *const RawMf, tol: f64) -> *mut RawMf;
    fn s3d_mf_level_set(sdf: SdfFn, ctx: *mut c_void, bounds6: *const f64, edge: f64, level: f64) -> *mut RawMf;
}

#[derive(Debug, Clone, PartialEq)]
pub enum GeomError {
    /// Manifold threw or returned null.
    Failed(&'static str),
    /// Manifold status code (see Manifold::Error).
    Status(i32),
}

impl std::fmt::Display for GeomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeomError::Failed(op) => write!(f, "geometry operation '{op}' failed"),
            GeomError::Status(1) => write!(f, "mesh has non-finite vertices"),
            GeomError::Status(2) => write!(f, "mesh is not a closed manifold"),
            GeomError::Status(s) => write!(f, "geometry error (code {s})"),
        }
    }
}
impl std::error::Error for GeomError {}

pub type GResult<T> = Result<T, GeomError>;

#[derive(Clone, Copy)]
pub enum Op {
    Union = 0,
    Subtract = 1,
}

pub struct Solid {
    h: *mut RawMf,
}

// Manifold objects are immutable value types with internal shared_ptrs that are
// safe to use from multiple threads.
unsafe impl Send for Solid {}
unsafe impl Sync for Solid {}

impl Drop for Solid {
    fn drop(&mut self) {
        unsafe { s3d_mf_free(self.h) }
    }
}

impl Clone for Solid {
    fn clone(&self) -> Self {
        Solid::wrap(unsafe { s3d_mf_clone(self.h) }, "clone").expect("clone")
    }
}

impl std::fmt::Debug for Solid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Solid({} tris)", self.num_tri())
    }
}

impl Solid {
    fn wrap(h: *mut RawMf, op: &'static str) -> GResult<Solid> {
        if h.is_null() {
            return Err(GeomError::Failed(op));
        }
        let s = Solid { h };
        match unsafe { s3d_mf_status(h) } {
            0 => Ok(s),
            code => Err(GeomError::Status(code)),
        }
    }

    pub fn from_mesh(m: &Mesh) -> GResult<Solid> {
        let h = unsafe {
            s3d_mf_from_mesh(m.verts.as_ptr() as *const f32, m.verts.len() as u32, 0, m.tris.as_ptr() as *const u32, m.tris.len() as u32)
        };
        Solid::wrap(h, "from_mesh")
    }

    pub fn to_mesh(&self) -> Mesh {
        unsafe {
            let raw = s3d_mf_get_mesh(self.h);
            if raw.is_null() {
                return Mesh::default();
            }
            let nv = s3d_mesh_num_vert(raw) as usize;
            let nt = s3d_mesh_num_tri(raw) as usize;
            let mut verts = vec![Vec3::ZERO; nv];
            let mut tris = vec![[0u32; 3]; nt];
            s3d_mesh_copy(raw, verts.as_mut_ptr() as *mut f32, tris.as_mut_ptr() as *mut u32);
            s3d_mesh_free(raw);
            Mesh::new(verts, tris)
        }
    }

    pub fn empty() -> Solid {
        // A zero-size cube is Manifold's canonical empty solid.
        Solid::wrap(unsafe { s3d_mf_cube(0.0, 0.0, 0.0, 0) }, "empty").expect("empty")
    }

    pub fn is_empty(&self) -> bool {
        unsafe { s3d_mf_is_empty(self.h) != 0 }
    }
    pub fn num_tri(&self) -> usize {
        unsafe { s3d_mf_num_tri(self.h) as usize }
    }
    pub fn volume(&self) -> f64 {
        unsafe { s3d_mf_volume(self.h) }
    }
    pub fn genus(&self) -> i32 {
        unsafe { s3d_mf_genus(self.h) }
    }
    pub fn bbox(&self) -> Aabb {
        let mut b = [0f64; 6];
        unsafe { s3d_mf_bbox(self.h, b.as_mut_ptr()) };
        Aabb {
            min: Vec3::new(b[0] as f32, b[1] as f32, b[2] as f32),
            max: Vec3::new(b[3] as f32, b[4] as f32, b[5] as f32),
        }
    }

    pub fn boolean(&self, other: &Solid, op: Op) -> GResult<Solid> {
        Solid::wrap(unsafe { s3d_mf_boolean(self.h, other.h, op as i32) }, "boolean")
    }
    pub fn union(&self, other: &Solid) -> GResult<Solid> {
        self.boolean(other, Op::Union)
    }
    pub fn subtract(&self, other: &Solid) -> GResult<Solid> {
        self.boolean(other, Op::Subtract)
    }

    pub fn batch(solids: &[&Solid], op: Op) -> GResult<Solid> {
        if solids.is_empty() {
            return Ok(Solid::empty());
        }
        let hs: Vec<*const RawMf> = solids.iter().map(|s| s.h as *const RawMf).collect();
        Solid::wrap(unsafe { s3d_mf_batch_boolean(hs.as_ptr(), hs.len() as u32, op as i32) }, "batch_boolean")
    }

    /// Combines solids known not to overlap, without a boolean.
    pub fn compose(solids: &[&Solid]) -> GResult<Solid> {
        let hs: Vec<*const RawMf> = solids.iter().map(|s| s.h as *const RawMf).collect();
        Solid::wrap(unsafe { s3d_mf_compose(hs.as_ptr(), hs.len() as u32) }, "compose")
    }

    /// Splits by the plane `normal · p = offset`. Returns (positive side, negative side).
    pub fn split_by_plane(&self, normal: DVec3, offset: f64) -> GResult<(Solid, Solid)> {
        let (mut a, mut b) = (std::ptr::null_mut(), std::ptr::null_mut());
        let ok = unsafe { s3d_mf_split_by_plane(self.h, normal.x, normal.y, normal.z, offset, &mut a, &mut b) };
        if ok == 0 {
            return Err(GeomError::Failed("split_by_plane"));
        }
        Ok((Solid::wrap(a, "split_by_plane")?, Solid::wrap(b, "split_by_plane")?))
    }

    /// Keeps the part on the positive side of `normal · p = offset`.
    pub fn trim_by_plane(&self, normal: DVec3, offset: f64) -> GResult<Solid> {
        Solid::wrap(unsafe { s3d_mf_trim_by_plane(self.h, normal.x, normal.y, normal.z, offset) }, "trim_by_plane")
    }

    /// Splits into connected components (each a separate solid body).
    pub fn decompose(&self) -> Vec<Solid> {
        unsafe {
            let mut arr = std::ptr::null_mut();
            let n = s3d_mf_decompose(self.h, &mut arr) as usize;
            if arr.is_null() {
                return Vec::new();
            }
            let out = std::slice::from_raw_parts(arr, n).iter().map(|&h| Solid { h }).collect();
            s3d_mf_free_array(arr);
            out
        }
    }

    /// Separate solid bodies: like `decompose`, but internal voids stay with
    /// the shell that encloses them instead of becoming negative solids.
    pub fn bodies(&self) -> Vec<Solid> {
        let comps = self.decompose();
        if comps.len() <= 1 {
            return comps;
        }
        let (mut pos, neg): (Vec<Solid>, Vec<Solid>) = comps.into_iter().partition(|c| c.volume() >= 0.0);
        if neg.is_empty() {
            return pos;
        }
        let meshes: Vec<Mesh> = pos.iter().map(|s| s.to_mesh()).collect();
        let bvhs: Vec<crate::bvh::Bvh> = meshes.iter().map(crate::bvh::Bvh::new).collect();
        let mut groups: Vec<Vec<Solid>> = pos.drain(..).map(|s| vec![s]).collect();
        for v in neg {
            let vm = v.to_mesh();
            let probe = vm.verts.first().copied().unwrap_or_default();
            let vb = v.bbox();
            // Smallest enclosing shell wins.
            let host = (0..bvhs.len())
                .filter(|&i| {
                    let b = bvhs[i].bbox();
                    b.min.cmple(vb.min).all() && b.max.cmpge(vb.max).all() && bvhs[i].contains(probe)
                })
                .min_by(|&a, &b| groups[a][0].volume().total_cmp(&groups[b][0].volume()));
            match host {
                Some(i) => groups[i].push(v),
                None => groups.push(vec![v]), // orphan void: keep as-is
            }
        }
        groups
            .into_iter()
            .map(|g| {
                let refs: Vec<&Solid> = g.iter().collect();
                if refs.len() == 1 { g.into_iter().next().unwrap() } else { Solid::compose(&refs).expect("compose") }
            })
            .collect()
    }

    pub fn transform(&self, m: &DMat4) -> GResult<Solid> {
        let c = m.to_cols_array();
        let m34 = [c[0], c[1], c[2], c[4], c[5], c[6], c[8], c[9], c[10], c[12], c[13], c[14]];
        Solid::wrap(unsafe { s3d_mf_transform(self.h, m34.as_ptr()) }, "transform")
    }

    pub fn translate(&self, d: DVec3) -> GResult<Solid> {
        self.transform(&DMat4::from_translation(d))
    }

    pub fn cube(size: DVec3, center: bool) -> GResult<Solid> {
        Solid::wrap(unsafe { s3d_mf_cube(size.x, size.y, size.z, center as i32) }, "cube")
    }

    /// Cylinder along +Z from z=0 (or centred), radius may taper to `r_high`.
    pub fn cylinder(height: f64, r_low: f64, r_high: f64, segments: i32, center: bool) -> GResult<Solid> {
        Solid::wrap(unsafe { s3d_mf_cylinder(height, r_low, r_high, segments, center as i32) }, "cylinder")
    }

    pub fn sphere(r: f64, segments: i32) -> GResult<Solid> {
        Solid::wrap(unsafe { s3d_mf_sphere(r, segments) }, "sphere")
    }

    /// Extrudes 2D contours (outer CCW, holes CW) along +Z.
    pub fn extrude(contours: &[Vec<glam::DVec2>], height: f64) -> GResult<Solid> {
        Self::extrude_scaled(contours, height, 1.0)
    }

    /// Extrudes with the top scaled by `scale_top` (0 makes a pyramid/cone).
    pub fn extrude_scaled(contours: &[Vec<glam::DVec2>], height: f64, scale_top: f64) -> GResult<Solid> {
        let sizes: Vec<u32> = contours.iter().map(|c| c.len() as u32).collect();
        let xy: Vec<f64> = contours.iter().flatten().flat_map(|p| [p.x, p.y]).collect();
        Solid::wrap(unsafe { s3d_mf_extrude(xy.as_ptr(), sizes.as_ptr(), sizes.len() as u32, height, scale_top) }, "extrude")
    }

    pub fn hull(&self) -> GResult<Solid> {
        Solid::wrap(unsafe { s3d_mf_hull(self.h) }, "hull")
    }

    pub fn hull_points(pts: &[DVec3]) -> GResult<Solid> {
        let flat: Vec<f64> = pts.iter().flat_map(|p| [p.x, p.y, p.z]).collect();
        Solid::wrap(unsafe { s3d_mf_hull_points(flat.as_ptr(), pts.len() as u32) }, "hull_points")
    }

    pub fn simplify(&self, tolerance: f64) -> GResult<Solid> {
        Solid::wrap(unsafe { s3d_mf_simplify(self.h, tolerance) }, "simplify")
    }

    /// Builds the surface where `sdf(p) == level` inside `bounds`; `sdf` is
    /// positive inside the solid. Evaluated on one thread.
    pub fn level_set(sdf: &dyn Fn(DVec3) -> f64, bounds: Aabb, edge_length: f64, level: f64) -> GResult<Solid> {
        extern "C" fn tramp(x: f64, y: f64, z: f64, ctx: *mut c_void) -> f64 {
            let f = unsafe { &*(ctx as *const &dyn Fn(DVec3) -> f64) };
            f(DVec3::new(x, y, z))
        }
        let b = [bounds.min.x as f64, bounds.min.y as f64, bounds.min.z as f64, bounds.max.x as f64, bounds.max.y as f64, bounds.max.z as f64];
        let ctx: *const &dyn Fn(DVec3) -> f64 = &sdf;
        Solid::wrap(unsafe { s3d_mf_level_set(tramp, ctx as *mut c_void, b.as_ptr(), edge_length, level) }, "level_set")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;

    #[test]
    fn roundtrip_and_split() {
        let m = shapes::cube(Vec3::splat(10.0));
        let s = Solid::from_mesh(&m).unwrap();
        assert!((s.volume() - 1000.0).abs() < 1e-3);
        let (pos, neg) = s.split_by_plane(DVec3::X, 4.0).unwrap();
        assert!((pos.volume() - 600.0).abs() < 1e-3);
        assert!((neg.volume() - 400.0).abs() < 1e-3);
        let back = pos.to_mesh();
        assert!((back.volume() - 600.0).abs() < 1e-3);
    }

    #[test]
    fn booleans_and_decompose() {
        let a = Solid::cube(DVec3::splat(10.0), false).unwrap();
        let b = Solid::cube(DVec3::splat(10.0), false).unwrap().translate(DVec3::new(20.0, 0.0, 0.0)).unwrap();
        let both = a.union(&b).unwrap();
        assert_eq!(both.decompose().len(), 2);
        let hole = Solid::cylinder(20.0, 2.0, 2.0, 32, true).unwrap().translate(DVec3::splat(5.0)).unwrap();
        let drilled = a.subtract(&hole).unwrap();
        assert!(drilled.volume() < 1000.0 - 100.0);
        assert_eq!(drilled.genus(), 1);
    }

    #[test]
    fn level_set_sphere() {
        let f = |p: DVec3| 5.0 - p.length();
        let b = Aabb { min: Vec3::splat(-6.0), max: Vec3::splat(6.0) };
        let s = Solid::level_set(&f, b, 0.5, 0.0).unwrap();
        let v = 4.0 / 3.0 * std::f64::consts::PI * 125.0;
        assert!((s.volume() - v).abs() / v < 0.05, "{}", s.volume());
    }

    #[test]
    fn open_mesh_is_rejected() {
        let mut m = shapes::cube(Vec3::ONE);
        m.tris.pop();
        assert!(Solid::from_mesh(&m).is_err());
    }
}
