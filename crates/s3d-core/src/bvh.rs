//! Triangle bounding-volume hierarchy: ray casts, closest-point queries and
//! inside tests. Used for picking, distance fields and wall-thickness checks.

use glam::Vec3;

use crate::mesh::{Aabb, Mesh};

#[derive(Clone, Copy, Debug)]
struct Node {
    bb: Aabb,
    /// Leaf: first triangle index into `order`; inner: index of the left child
    /// (right child is `left + 1`).
    start: u32,
    /// Leaf triangle count; 0 for inner nodes.
    count: u32,
}

pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
    tris: Vec<[Vec3; 3]>,
}

#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub t: f32,
    pub tri: u32,
    /// Geometric normal of the hit triangle (unnormalised winding direction).
    pub normal: Vec3,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn new(mesh: &Mesh) -> Bvh {
        let tris: Vec<[Vec3; 3]> = (0..mesh.tris.len()).map(|t| mesh.tri(t)).collect();
        Self::from_triangles(tris)
    }

    pub fn from_triangles(tris: Vec<[Vec3; 3]>) -> Bvh {
        let n = tris.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let cent: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let mut nodes = Vec::with_capacity(2 * n / LEAF + 1);
        nodes.push(Node { bb: Aabb::EMPTY, start: 0, count: 0 });
        if n > 0 {
            // Iterative build: (node index, range start, range end).
            let mut stack = vec![(0usize, 0usize, n)];
            while let Some((ni, s, e)) = stack.pop() {
                let mut bb = Aabb::EMPTY;
                let mut cb = Aabb::EMPTY;
                for &t in &order[s..e] {
                    for p in tris[t as usize] {
                        bb.grow(p);
                    }
                    cb.grow(cent[t as usize]);
                }
                if e - s <= LEAF {
                    nodes[ni] = Node { bb, start: s as u32, count: (e - s) as u32 };
                    continue;
                }
                let axis = cb.size().max_position();
                let mid = (s + e) / 2;
                order[s..e].select_nth_unstable_by(mid - s, |&a, &b| cent[a as usize][axis].total_cmp(&cent[b as usize][axis]));
                let left = nodes.len();
                nodes.push(Node { bb: Aabb::EMPTY, start: 0, count: 0 });
                nodes.push(Node { bb: Aabb::EMPTY, start: 0, count: 0 });
                nodes[ni] = Node { bb, start: left as u32, count: 0 };
                stack.push((left, s, mid));
                stack.push((left + 1, mid, e));
            }
        }
        Bvh { nodes, order, tris }
    }

    pub fn bbox(&self) -> Aabb {
        self.nodes[0].bb
    }

    pub fn triangle(&self, t: u32) -> [Vec3; 3] {
        self.tris[t as usize]
    }

    /// Nearest hit along `origin + t*dir` for t in (0, max_t].
    pub fn ray(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<RayHit> {
        if self.tris.is_empty() {
            return None;
        }
        let inv = dir.recip();
        let mut best: Option<RayHit> = None;
        let mut limit = max_t;
        let mut stack = Vec::with_capacity(64);
        stack.push(0usize);
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            if slab(&n.bb, origin, inv).is_none_or(|t| t > limit) {
                continue;
            }
            if n.count > 0 {
                for &t in &self.order[n.start as usize..(n.start + n.count) as usize] {
                    if let Some(d) = ray_tri(origin, dir, &self.tris[t as usize]) {
                        if d > 0.0 && d < limit {
                            limit = d;
                            let tr = &self.tris[t as usize];
                            best = Some(RayHit { t: d, tri: t, normal: (tr[1] - tr[0]).cross(tr[2] - tr[0]) });
                        }
                    }
                }
            } else {
                let (l, r) = (n.start as usize, n.start as usize + 1);
                // Visit the nearer child first.
                let tl = slab(&self.nodes[l].bb, origin, inv).unwrap_or(f32::INFINITY);
                let tr = slab(&self.nodes[r].bb, origin, inv).unwrap_or(f32::INFINITY);
                if tl < tr {
                    stack.push(r);
                    stack.push(l);
                } else {
                    stack.push(l);
                    stack.push(r);
                }
            }
        }
        best
    }

    /// Counts all crossings along a ray (for parity inside tests).
    pub fn ray_crossings(&self, origin: Vec3, dir: Vec3) -> u32 {
        let inv = dir.recip();
        let mut count = 0;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            if slab(&n.bb, origin, inv).is_none() {
                continue;
            }
            if n.count > 0 {
                for &t in &self.order[n.start as usize..(n.start + n.count) as usize] {
                    if ray_tri(origin, dir, &self.tris[t as usize]).is_some_and(|d| d > 0.0) {
                        count += 1;
                    }
                }
            } else {
                stack.push(n.start as usize);
                stack.push(n.start as usize + 1);
            }
        }
        count
    }

    /// Inside test for closed meshes: majority vote of crossing parity along
    /// five rays in "irrational" directions, so rays from grid points do not
    /// run exactly through the edges of axis-aligned faces.
    pub fn contains(&self, p: Vec3) -> bool {
        const DIRS: [Vec3; 5] = [
            Vec3::new(0.412_31, 0.537_93, 0.735_36),
            Vec3::new(-0.681_17, 0.215_39, 0.699_76),
            Vec3::new(0.193_07, -0.887_93, -0.417_52),
            Vec3::new(-0.315_83, -0.392_61, 0.863_77),
            Vec3::new(0.788_42, 0.317_05, -0.527_13),
        ];
        let mut inside = 0;
        for (i, d) in DIRS.iter().enumerate() {
            if self.ray_crossings(p, d.normalize()) % 2 == 1 {
                inside += 1;
            }
            let outside = i + 1 - inside;
            if inside >= 3 || outside >= 3 {
                break;
            }
        }
        inside >= 3
    }

    /// Closest point on the surface within `max_dist`: (distance, point, tri).
    pub fn closest(&self, p: Vec3, max_dist: f32) -> Option<(f32, Vec3, u32)> {
        let mut best_d2 = max_dist * max_dist;
        let mut best = None;
        let mut stack = Vec::with_capacity(64);
        stack.push(0usize);
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            if box_dist2(&n.bb, p) > best_d2 {
                continue;
            }
            if n.count > 0 {
                for &t in &self.order[n.start as usize..(n.start + n.count) as usize] {
                    let q = closest_on_tri(p, &self.tris[t as usize]);
                    let d2 = q.distance_squared(p);
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best = Some((q, t));
                    }
                }
            } else {
                let (l, r) = (n.start as usize, n.start as usize + 1);
                let (dl, dr) = (box_dist2(&self.nodes[l].bb, p), box_dist2(&self.nodes[r].bb, p));
                if dl < dr {
                    stack.push(r);
                    stack.push(l);
                } else {
                    stack.push(l);
                    stack.push(r);
                }
            }
        }
        best.map(|(q, t)| (best_d2.sqrt(), q, t))
    }

    /// Calls `f` for every triangle whose bounding box overlaps the sphere.
    pub fn for_each_near(&self, c: Vec3, r: f32, mut f: impl FnMut(u32)) {
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            if box_dist2(&n.bb, c) > r * r {
                continue;
            }
            if n.count > 0 {
                for &t in &self.order[n.start as usize..(n.start + n.count) as usize] {
                    if closest_on_tri(c, &self.tris[t as usize]).distance_squared(c) <= r * r {
                        f(t);
                    }
                }
            } else {
                stack.push(n.start as usize);
                stack.push(n.start as usize + 1);
            }
        }
    }
}

#[inline]
fn slab(bb: &Aabb, o: Vec3, inv: Vec3) -> Option<f32> {
    let t0 = (bb.min - o) * inv;
    let t1 = (bb.max - o) * inv;
    let tmin = t0.min(t1).max_element().max(0.0);
    let tmax = t0.max(t1).min_element();
    if tmin <= tmax { Some(tmin) } else { None }
}

#[inline]
fn box_dist2(bb: &Aabb, p: Vec3) -> f32 {
    let d = (bb.min - p).max(p - bb.max).max(Vec3::ZERO);
    d.length_squared()
}

/// Möller–Trumbore; returns t (may be negative).
#[inline]
pub fn ray_tri(o: Vec3, d: Vec3, t: &[Vec3; 3]) -> Option<f32> {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - t[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(e2.dot(q) * inv)
}

/// Closest point on a triangle (Ericson, Real-Time Collision Detection 5.1.5).
pub fn closest_on_tri(p: Vec3, t: &[Vec3; 3]) -> Vec3 {
    let (a, b, c) = (t[0], t[1], t[2]);
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;

    #[test]
    fn ray_and_contains() {
        let m = shapes::cube(Vec3::splat(10.0));
        let b = Bvh::new(&m);
        let h = b.ray(Vec3::new(5.0, 5.0, -5.0), Vec3::Z, 100.0).unwrap();
        assert!((h.t - 5.0).abs() < 1e-5);
        assert!(h.normal.z < 0.0);
        assert!(b.contains(Vec3::splat(5.0)));
        assert!(!b.contains(Vec3::splat(15.0)));
    }

    #[test]
    fn closest_distance() {
        let m = shapes::uv_sphere(10.0, 48, 24);
        let b = Bvh::new(&m);
        let (d, _, _) = b.closest(Vec3::new(0.0, 0.0, 15.0), 100.0).unwrap();
        assert!((d - 5.0).abs() < 0.1);
        assert!(b.closest(Vec3::new(0.0, 0.0, 15.0), 1.0).is_none());
    }
}
