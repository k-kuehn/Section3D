//! Fast vertex-clustering simplification for search proxies. Quality is
//! modest but it is linear time and never fails, which is what a proxy needs.

use glam::{IVec3, Vec3};
use rayon::prelude::*;

use crate::mesh::Mesh;

/// Simplifies to roughly `target` triangles (never more than the input).
pub fn cluster(mesh: &Mesh, target: usize) -> Mesh {
    if mesh.tris.len() <= target {
        return mesh.clone();
    }
    let bb = mesh.bbox();
    let area = mesh.area() as f32;
    // A surface of area A covered by cells of size h has ~2A/h² triangles.
    let mut h = (2.0 * area / target as f32).sqrt().max(bb.diagonal() * 1e-4);
    for _ in 0..6 {
        let m = cluster_at(mesh, h);
        if m.tris.len() <= target * 13 / 10 {
            return m;
        }
        h *= 1.25;
    }
    cluster_at(mesh, h)
}

pub fn cluster_at(mesh: &Mesh, h: f32) -> Mesh {
    let origin = mesh.bbox().min;
    let key = |v: Vec3| {
        let c = ((v - origin) / h).floor().as_ivec3();
        (c.x, c.y, c.z)
    };
    let mut order: Vec<u32> = (0..mesh.verts.len() as u32).collect();
    order.par_sort_unstable_by_key(|&i| key(mesh.verts[i as usize]));
    let mut remap = vec![0u32; mesh.verts.len()];
    let mut verts: Vec<Vec3> = Vec::new();
    let mut i = 0;
    while i < order.len() {
        let k = key(mesh.verts[order[i] as usize]);
        let mut j = i;
        let mut sum = Vec3::ZERO;
        while j < order.len() && key(mesh.verts[order[j] as usize]) == k {
            sum += mesh.verts[order[j] as usize];
            remap[order[j] as usize] = verts.len() as u32;
            j += 1;
        }
        verts.push(sum / (j - i) as f32);
        i = j;
    }
    let mut tris: Vec<[u32; 3]> = mesh
        .tris
        .par_iter()
        .filter_map(|t| {
            let r = t.map(|v| remap[v as usize]);
            (r[0] != r[1] && r[1] != r[2] && r[0] != r[2]).then_some(r)
        })
        .collect();
    // Drop duplicate faces created by collapsing.
    tris.par_sort_unstable_by_key(|t| {
        let mut s = *t;
        s.sort_unstable();
        s
    });
    tris.dedup_by_key(|t| {
        let mut s = *t;
        s.sort_unstable();
        s
    });
    let _ = IVec3::ZERO;
    let mut m = Mesh::new(verts, tris);
    m.compact();
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;

    #[test]
    fn sphere_reduces_and_keeps_volume() {
        let m = shapes::uv_sphere(100.0, 256, 128);
        let s = cluster(&m, 5000);
        assert!(s.tris.len() < 8000, "{}", s.tris.len());
        let (v0, v1) = (m.volume(), s.volume());
        assert!((v1 - v0).abs() / v0 < 0.05, "{v0} {v1}");
    }
}
