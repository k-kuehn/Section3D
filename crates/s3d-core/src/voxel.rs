//! Inside/outside occupancy grids by ray parity along all three axes
//! (majority vote), which tolerates small holes and flipped faces.

use glam::{Vec2, Vec3};
use rayon::prelude::*;

use crate::job::{Cancelled, JobCtx};
use crate::mesh::Mesh;

pub struct Occupancy {
    pub origin: Vec3,
    /// Voxel size.
    pub h: f32,
    pub dims: [usize; 3],
    /// Votes per voxel (0..=3); inside when >= 2.
    pub votes: Vec<u8>,
}

impl Occupancy {
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.dims[1] + y) * self.dims[0] + x
    }

    pub fn inside_voxel(&self, x: isize, y: isize, z: isize) -> bool {
        if x < 0 || y < 0 || z < 0 || x as usize >= self.dims[0] || y as usize >= self.dims[1] || z as usize >= self.dims[2] {
            return false;
        }
        self.votes[self.idx(x as usize, y as usize, z as usize)] >= 2
    }

    /// Is the point inside the solid (nearest voxel)?
    pub fn inside(&self, p: Vec3) -> bool {
        let g = ((p - self.origin) / self.h).floor();
        self.inside_voxel(g.x as isize, g.y as isize, g.z as isize)
    }
}

/// Builds an occupancy grid with about `res` voxels along the longest side.
pub fn occupancy(m: &Mesh, res: usize, job: &JobCtx) -> Result<Occupancy, Cancelled> {
    let bb = m.bbox();
    let longest = bb.size().max_element();
    let h = longest / (res as f32 - 8.0);
    let origin = bb.min - Vec3::splat(h * 3.0);
    let dims = ((bb.size() / h).ceil().as_uvec3() + 6).as_usizevec3();
    let (nx, ny, nz) = (dims.x, dims.y, dims.z);
    let mut votes = vec![0u8; nx * ny * nz];
    let idx = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    for axis in 0..3 {
        job.check()?;
        // Column axes (a, b) and ray axis r.
        let (a, b, r) = match axis {
            0 => (0, 1, 2),
            1 => (1, 2, 0),
            _ => (2, 0, 1),
        };
        let d = [nx, ny, nz];
        // Bucket triangles into columns by their (a,b) footprint.
        let mut cols: Vec<Vec<u32>> = vec![Vec::new(); d[a] * d[b]];
        for (t, _) in m.tris.iter().enumerate() {
            let p = m.tri(t);
            let lo = p.iter().fold(Vec3::INFINITY, |acc, v| acc.min(*v));
            let hi = p.iter().fold(Vec3::NEG_INFINITY, |acc, v| acc.max(*v));
            let ca0 = (((lo[a] - origin[a]) / h - 0.5).ceil().max(0.0)) as usize;
            let ca1 = (((hi[a] - origin[a]) / h - 0.5).floor()) as isize;
            let cb0 = (((lo[b] - origin[b]) / h - 0.5).ceil().max(0.0)) as usize;
            let cb1 = (((hi[b] - origin[b]) / h - 0.5).floor()) as isize;
            for i in ca0..=(ca1.max(-1) as usize).min(d[a] - 1) {
                if ca1 < 0 {
                    break;
                }
                for j in cb0..=(cb1.max(-1) as usize).min(d[b] - 1) {
                    if cb1 < 0 {
                        break;
                    }
                    cols[j * d[a] + i].push(t as u32);
                }
            }
        }
        let filled: Vec<Vec<(usize, usize, usize)>> = cols
            .par_iter()
            .enumerate()
            .map(|(ci, tris)| {
                let (i, j) = (ci % d[a], ci / d[a]);
                let (pa, pb) = (origin[a] + (i as f32 + 0.5) * h, origin[b] + (j as f32 + 0.5) * h);
                let mut hits: Vec<f32> = tris
                    .iter()
                    .filter_map(|&t| {
                        let p = m.tri(t as usize);
                        let q = |v: Vec3| Vec2::new(v[a] - pa, v[b] - pb);
                        let (p0, p1, p2) = (q(p[0]), q(p[1]), q(p[2]));
                        let w0 = p1.perp_dot(p2);
                        let w1 = p2.perp_dot(p0);
                        let w2 = p0.perp_dot(p1);
                        let all_pos = w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0;
                        let all_neg = w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0;
                        let s = w0 + w1 + w2;
                        if (all_pos || all_neg) && s.abs() > 1e-12 {
                            Some((w0 * p[0][r] + w1 * p[1][r] + w2 * p[2][r]) / s)
                        } else {
                            None
                        }
                    })
                    .collect();
                hits.sort_by(|x, y| x.total_cmp(y));
                hits.dedup_by(|x, y| (*x - *y).abs() < h * 1e-3);
                let mut out = Vec::new();
                for pair in hits.chunks_exact(2) {
                    let k0 = (((pair[0] - origin[r]) / h - 0.5).ceil().max(0.0)) as usize;
                    let k1 = (((pair[1] - origin[r]) / h - 0.5).floor()) as isize;
                    if k1 < 0 {
                        continue;
                    }
                    for k in k0..=(k1 as usize).min(d[r] - 1) {
                        let mut c = [0usize; 3];
                        c[a] = i;
                        c[b] = j;
                        c[r] = k;
                        out.push((c[0], c[1], c[2]));
                    }
                }
                out
            })
            .collect();
        for col in filled {
            for (x, y, z) in col {
                votes[idx(x, y, z)] += 1;
            }
        }
    }
    job.check()?;
    Ok(Occupancy { origin, h, dims: [nx, ny, nz], votes })
}
