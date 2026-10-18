//! Sparse, parallel marching tetrahedra. Extracts the closed surface `f = 0`
//! (positive inside) from a field sampled on a grid, visiting only the given
//! blocks. Kuhn subdivision makes the output watertight and manifold as long
//! as every sign change lies inside the visited blocks.

use glam::{IVec3, Vec3};
use rayon::prelude::*;

use crate::mesh::Mesh;

/// Cells per block edge.
pub const BLOCK: i32 = 4;

/// The six Kuhn tetrahedra of a unit cube, as corner indices (bit0=x, bit1=y, bit2=z).
const TETS: [[usize; 4]; 6] = [
    [0, 1, 3, 7],
    [0, 1, 5, 7],
    [0, 2, 3, 7],
    [0, 2, 6, 7],
    [0, 4, 5, 7],
    [0, 4, 6, 7],
];

type Key = (i32, i32, i32, u8);

fn corner(i: usize) -> IVec3 {
    IVec3::new((i & 1) as i32, ((i >> 1) & 1) as i32, ((i >> 2) & 1) as i32)
}

/// `f` maps a world position to a field value (positive inside).
pub fn extract<F>(origin: Vec3, h: f32, blocks: &[IVec3], f: F) -> Mesh
where
    F: Fn(Vec3) -> f32 + Sync,
{
    let n = (BLOCK + 1) as usize;
    let parts: Vec<(Vec<(Key, Vec3)>, Vec<[u32; 3]>)> = blocks
        .par_iter()
        .map(|&b| {
            let base = b * BLOCK;
            let mut vals = vec![0f32; n * n * n];
            let mut any_pos = false;
            let mut any_neg = false;
            for z in 0..n {
                for y in 0..n {
                    for x in 0..n {
                        let g = base + IVec3::new(x as i32, y as i32, z as i32);
                        let mut v = f(origin + g.as_vec3() * h);
                        if v == 0.0 {
                            v = 1e-7;
                        }
                        any_pos |= v > 0.0;
                        any_neg |= v < 0.0;
                        vals[(z * n + y) * n + x] = v;
                    }
                }
            }
            let mut verts: Vec<(Key, Vec3)> = Vec::new();
            let mut tris: Vec<[u32; 3]> = Vec::new();
            if !(any_pos && any_neg) {
                return (verts, tris);
            }
            let val = |g: IVec3| vals[((g.z as usize) * n + g.y as usize) * n + g.x as usize];
            for cz in 0..BLOCK {
                for cy in 0..BLOCK {
                    for cx in 0..BLOCK {
                        let c = IVec3::new(cx, cy, cz);
                        for tet in TETS {
                            let g: [IVec3; 4] = tet.map(|k| c + corner(k));
                            let v: [f32; 4] = g.map(val);
                            let inside: Vec<usize> = (0..4).filter(|&i| v[i] > 0.0).collect();
                            if inside.is_empty() || inside.len() == 4 {
                                continue;
                            }
                            let outside: Vec<usize> = (0..4).filter(|&i| v[i] <= 0.0).collect();
                            // Returns the vertex index and the doubled edge midpoint in
                            // grid units; orientation is decided on midpoints so that
                            // near-degenerate triangles still get a consistent winding.
                            let mut edge_vert = |i: usize, o: usize| -> (u32, Vec3) {
                                // Kuhn edges always go from the componentwise-lower corner.
                                let (lo, hi) = if g[i].cmple(g[o]).all() { (i, o) } else { (o, i) };
                                let d = g[hi] - g[lo];
                                let gl = base + g[lo];
                                let key = (gl.x, gl.y, gl.z, (d.x | (d.y << 1) | (d.z << 2)) as u8);
                                let t = v[lo] / (v[lo] - v[hi]);
                                let p = origin + (gl.as_vec3() + d.as_vec3() * t) * h;
                                verts.push((key, p));
                                (verts.len() as u32 - 1, (g[i] + g[o]).as_vec3())
                            };
                            let pin2 = g[inside[0]].as_vec3() * 2.0;
                            let mut emit = |a: (u32, Vec3), b: (u32, Vec3), c: (u32, Vec3)| {
                                let nrm = (b.1 - a.1).cross(c.1 - a.1);
                                if nrm.dot(pin2 - a.1) < 0.0 { tris.push([a.0, b.0, c.0]) } else { tris.push([a.0, c.0, b.0]) }
                            };
                            match inside.len() {
                                1 | 3 => {
                                    let (lone, others) = if inside.len() == 1 { (inside[0], &outside) } else { (outside[0], &inside) };
                                    let a = edge_vert(lone, others[0]);
                                    let b = edge_vert(lone, others[1]);
                                    let c = edge_vert(lone, others[2]);
                                    emit(a, b, c);
                                }
                                _ => {
                                    let (i0, i1, o0, o1) = (inside[0], inside[1], outside[0], outside[1]);
                                    let a = edge_vert(i0, o0);
                                    let b = edge_vert(i0, o1);
                                    let c = edge_vert(i1, o1);
                                    let d = edge_vert(i1, o0);
                                    emit(a, b, c);
                                    emit(a, c, d);
                                }
                            }
                        }
                    }
                }
            }
            (verts, tris)
        })
        .collect();

    // Global vertex dedup by edge key.
    let total: usize = parts.iter().map(|p| p.0.len()).sum();
    let mut all: Vec<(Key, u32)> = Vec::with_capacity(total);
    let mut pos: Vec<Vec3> = Vec::with_capacity(total);
    for (vs, _) in &parts {
        for (k, p) in vs {
            all.push((*k, pos.len() as u32));
            pos.push(*p);
        }
    }
    all.par_sort_unstable_by_key(|e| e.0);
    let mut remap = vec![0u32; total];
    let mut verts = Vec::with_capacity(total / 4);
    let mut prev: Option<Key> = None;
    for (k, i) in all {
        if prev != Some(k) {
            verts.push(pos[i as usize]);
            prev = Some(k);
        }
        remap[i as usize] = verts.len() as u32 - 1;
    }
    let mut tris = Vec::new();
    let mut off = 0u32;
    for (vs, ts) in &parts {
        for t in ts {
            let r = t.map(|i| remap[(i + off) as usize]);
            if r[0] != r[1] && r[1] != r[2] && r[0] != r[2] {
                tris.push(r);
            }
        }
        off += vs.len() as u32;
    }
    let mut m = Mesh::new(verts, tris);
    m.compact();
    m
}

/// All blocks covering the box `[lo, hi]` (in grid cells).
pub fn blocks_in(lo: IVec3, hi: IVec3) -> Vec<IVec3> {
    let (b0, b1) = (lo.div_euclid(IVec3::splat(BLOCK)), hi.div_euclid(IVec3::splat(BLOCK)));
    let mut out = Vec::new();
    for z in b0.z..=b1.z {
        for y in b0.y..=b1.y {
            for x in b0.x..=b1.x {
                out.push(IVec3::new(x, y, z));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::Solid;

    #[test]
    fn sphere_is_closed_solid() {
        let h = 0.5;
        let blocks = blocks_in(IVec3::splat(-14), IVec3::splat(14));
        let m = extract(Vec3::ZERO, h, &blocks, |p| 5.0 - p.length());
        let s = Solid::from_mesh(&m).expect("manifold");
        let v = 4.0 / 3.0 * std::f64::consts::PI * 125.0;
        assert!((s.volume() - v).abs() / v < 0.03, "{}", s.volume());
    }
}
