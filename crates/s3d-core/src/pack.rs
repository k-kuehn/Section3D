//! Packs pieces (in print orientation) onto as few beds as possible.
//!
//! Each piece's true silhouette is rasterised and dilated by the spacing, so
//! irregular pieces nest closely. Placement is bottom-left first fit over a
//! set of rotations; several piece orders are tried in parallel and the
//! layout with the fewest beds (then the emptiest last bed) wins.

use glam::{Mat4, Vec2, Vec3};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::mesh::Mesh;
use crate::partition::PieceId;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub piece: PieceId,
    /// Centre of the piece's footprint on the bed (mm, bed origin at a corner).
    pub x: f32,
    pub y: f32,
    /// Extra rotation about Z after the print orientation (radians).
    pub angle: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub beds: Vec<Vec<Placement>>,
    /// Bed size used (X, Y).
    pub bed: [f32; 2],
    /// Pieces that fit on no bed.
    pub unplaced: Vec<PieceId>,
}

impl Layout {
    pub fn find(&self, piece: PieceId) -> Option<(usize, &Placement)> {
        self.beds.iter().enumerate().find_map(|(b, v)| v.iter().find(|p| p.piece == piece).map(|p| (b, p)))
    }
}

/// A piece ready for packing: its mesh already in print orientation, centred
/// on the origin in XY.
pub struct PackItem {
    pub id: PieceId,
    pub mesh: std::sync::Arc<Mesh>,
    pub print: Mat4,
}

/// Bitmap with one u64 word per 64 cells of a row.
#[derive(Clone)]
struct Raster {
    w: usize,
    h: usize,
    words: usize,
    bits: Vec<u64>,
}

impl Raster {
    fn new(w: usize, h: usize) -> Raster {
        let words = w.div_ceil(64).max(1);
        Raster { w, h, words, bits: vec![0; words * h] }
    }
    fn set(&mut self, x: usize, y: usize) {
        if x < self.w && y < self.h {
            self.bits[y * self.words + x / 64] |= 1 << (x % 64);
        }
    }
    fn get(&self, x: usize, y: usize) -> bool {
        x < self.w && y < self.h && self.bits[y * self.words + x / 64] & (1 << (x % 64)) != 0
    }
    /// Row `y` shifted right by `dx` cells, word `k`.
    #[inline]
    fn shifted_word(&self, y: usize, k: usize, dx: usize) -> u64 {
        let (ws, bs) = (dx / 64, dx % 64);
        if k < ws {
            return 0;
        }
        let src = k - ws;
        let lo = if src < self.words { self.bits[y * self.words + src] << bs } else { 0 };
        let hi = if bs > 0 && src >= 1 && src - 1 < self.words { self.bits[y * self.words + src - 1] >> (64 - bs) } else { 0 };
        lo | hi
    }
}

/// Footprint raster of a shape at one rotation.
struct Shape {
    r: Raster,
    /// Offset of the footprint centre from raster cell (0, 0), in cells.
    centre: Vec2,
    area: usize,
}

fn rasterize(pts2: &[[Vec2; 3]], angle: f32, cell: f32, dilate_cells: usize) -> Shape {
    let (s, c) = angle.sin_cos();
    let rot = |p: Vec2| Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y);
    let tris: Vec<[Vec2; 3]> = pts2.iter().map(|t| t.map(rot)).collect();
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for t in &tris {
        for p in t {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    let pad = dilate_cells as f32 * cell;
    let origin = lo - Vec2::splat(pad);
    let w = (((hi.x - lo.x) + 2.0 * pad) / cell).ceil() as usize + 1;
    let h = (((hi.y - lo.y) + 2.0 * pad) / cell).ceil() as usize + 1;
    let mut base = Raster::new(w, h);
    // Conservative coverage: mark every cell the triangle's bbox touches whose
    // centre is inside or which the triangle crosses.
    for t in &tris {
        let tl = t.iter().fold(Vec2::splat(f32::INFINITY), |a, p| a.min(*p));
        let th = t.iter().fold(Vec2::splat(f32::NEG_INFINITY), |a, p| a.max(*p));
        let (x0, y0) = (((tl.x - origin.x) / cell).floor() as usize, ((tl.y - origin.y) / cell).floor() as usize);
        let (x1, y1) = (((th.x - origin.x) / cell).floor() as usize, ((th.y - origin.y) / cell).floor() as usize);
        for y in y0..=y1.min(h - 1) {
            for x in x0..=x1.min(w - 1) {
                let cmin = origin + Vec2::new(x as f32, y as f32) * cell;
                if tri_box_overlap(t, cmin, cmin + Vec2::splat(cell)) {
                    base.set(x, y);
                }
            }
        }
    }
    // Dilate (square structuring element).
    let r = if dilate_cells == 0 {
        base
    } else {
        let mut d = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                if base.get(x, y) {
                    for yy in y.saturating_sub(dilate_cells)..=(y + dilate_cells).min(h - 1) {
                        for xx in x.saturating_sub(dilate_cells)..=(x + dilate_cells).min(w - 1) {
                            d.set(xx, yy);
                        }
                    }
                }
            }
        }
        d
    };
    let area = r.bits.iter().map(|b| b.count_ones() as usize).sum();
    let centre = (Vec2::ZERO - origin) / cell;
    Shape { r, centre, area }
}

fn tri_box_overlap(t: &[Vec2; 3], lo: Vec2, hi: Vec2) -> bool {
    // Separating axis test on box axes and triangle edge normals.
    let tl = t.iter().fold(Vec2::splat(f32::INFINITY), |a, p| a.min(*p));
    let th = t.iter().fold(Vec2::splat(f32::NEG_INFINITY), |a, p| a.max(*p));
    if tl.x > hi.x || th.x < lo.x || tl.y > hi.y || th.y < lo.y {
        return false;
    }
    let corners = [lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)];
    for i in 0..3 {
        let e = t[(i + 1) % 3] - t[i];
        let n = Vec2::new(-e.y, e.x);
        let tp: Vec<f32> = t.iter().map(|p| n.dot(*p)).collect();
        let (tmin, tmax) = (tp.iter().cloned().fold(f32::INFINITY, f32::min), tp.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
        let bp: Vec<f32> = corners.iter().map(|p| n.dot(*p)).collect();
        let (bmin, bmax) = (bp.iter().cloned().fold(f32::INFINITY, f32::min), bp.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
        if tmax < bmin || bmax < tmin {
            return false;
        }
    }
    true
}

struct Bed {
    r: Raster,
    placed: Vec<Placement>,
    used: usize,
}

impl Bed {
    fn fits(&self, s: &Raster, x: usize, y: usize) -> bool {
        if x + s.w > self.r.w || y + s.h > self.r.h {
            return false;
        }
        for sy in 0..s.h {
            let by = y + sy;
            let k0 = x / 64;
            let k1 = (x + s.w - 1) / 64;
            for k in k0..=k1 {
                if s.shifted_word(sy, k, x) & self.r.bits[by * self.r.words + k] != 0 {
                    return false;
                }
            }
        }
        true
    }

    fn stamp(&mut self, s: &Raster, x: usize, y: usize) {
        for sy in 0..s.h {
            let k1 = (x + s.w - 1) / 64;
            for k in x / 64..=k1.min(self.r.words - 1) {
                self.r.bits[(y + sy) * self.r.words + k] |= s.shifted_word(sy, k, x);
            }
        }
    }

    /// Bottom-left position for `s`, scanning rows then columns.
    fn find(&self, s: &Raster) -> Option<(usize, usize)> {
        if s.w > self.r.w || s.h > self.r.h {
            return None;
        }
        for y in 0..=self.r.h - s.h {
            for x in 0..=self.r.w - s.w {
                if self.fits(s, x, y) {
                    return Some((x, y));
                }
            }
        }
        None
    }
}

pub struct PackParams {
    /// Usable bed size X, Y (mm).
    pub bed: [f32; 2],
    pub spacing: f32,
    /// Keep the given item order (assembly order) instead of sorting.
    pub keep_order: bool,
    /// Rotation step in degrees (e.g. 15).
    pub angle_step: f32,
}

pub fn pack(items: &[PackItem], p: &PackParams) -> Layout {
    let cell = (p.bed[0].max(p.bed[1]) / 220.0).clamp(1.0, 4.0);
    let dilate = ((p.spacing * 0.5) / cell).ceil() as usize;
    let (bw, bh) = ((p.bed[0] / cell).floor() as usize, (p.bed[1] / cell).floor() as usize);
    let angles: Vec<f32> = {
        let n = (360.0 / p.angle_step.max(5.0)).round() as usize;
        (0..n).map(|i| (i as f32 * 360.0 / n as f32).to_radians()).collect()
    };
    // Rasterise every item at every angle, in parallel.
    let shapes: Vec<Vec<Shape>> = items
        .par_iter()
        .map(|it| {
            let pts: Vec<[Vec2; 3]> = it
                .mesh
                .tris
                .iter()
                .map(|t| t.map(|i| {
                    let q: Vec3 = it.print.transform_point3(it.mesh.verts[i as usize]);
                    Vec2::new(q.x, q.y)
                }))
                .collect();
            angles.iter().map(|&a| rasterize(&pts, a, cell, dilate)).collect()
        })
        .collect();
    let area: Vec<usize> = shapes.iter().map(|s| s.iter().map(|x| x.area).min().unwrap_or(0)).collect();
    let mut orders: Vec<Vec<usize>> = Vec::new();
    let natural: Vec<usize> = (0..items.len()).collect();
    if p.keep_order {
        orders.push(natural);
    } else {
        let mut by_area = natural.clone();
        by_area.sort_by(|&a, &b| area[b].cmp(&area[a]));
        let mut by_h = natural.clone();
        by_h.sort_by(|&a, &b| shapes[b][0].r.h.max(shapes[b][0].r.w).cmp(&shapes[a][0].r.h.max(shapes[a][0].r.w)));
        let mut by_w = natural.clone();
        by_w.sort_by(|&a, &b| shapes[b][0].r.w.min(shapes[b][0].r.h).cmp(&shapes[a][0].r.w.min(shapes[a][0].r.h)));
        orders.extend([by_area, by_h, by_w]);
    }
    let run = |order: &Vec<usize>| -> (Vec<Bed>, Vec<PieceId>) {
        let mut beds: Vec<Bed> = Vec::new();
        let mut unplaced = Vec::new();
        for &i in order {
            let mut done = false;
            let first_bed = if p.keep_order { beds.len().saturating_sub(1) } else { 0 };
            for b in first_bed..=beds.len() {
                if b == beds.len() {
                    beds.push(Bed { r: Raster::new(bw, bh), placed: Vec::new(), used: 0 });
                }
                // Best rotation on this bed: lowest y, then lowest x.
                let best = shapes[i].iter().enumerate().filter_map(|(ai, s)| beds[b].find(&s.r).map(|pos| (pos.1, pos.0, ai))).min();
                if let Some((y, x, ai)) = best {
                    let s = &shapes[i][ai];
                    beds[b].stamp(&s.r, x, y);
                    beds[b].used += s.area;
                    let c = (Vec2::new(x as f32, y as f32) + s.centre) * cell;
                    beds[b].placed.push(Placement { piece: items[i].id, x: c.x, y: c.y, angle: angles[ai] });
                    done = true;
                    break;
                }
                if beds[b].placed.is_empty() {
                    beds.pop();
                    break;
                }
            }
            if !done {
                unplaced.push(items[i].id);
            }
        }
        (beds, unplaced)
    };
    let results: Vec<(Vec<Bed>, Vec<PieceId>)> = orders.par_iter().map(run).collect();
    let (beds, unplaced) = results
        .into_iter()
        .min_by_key(|(b, u)| (u.len(), b.len(), b.last().map(|x| x.used).unwrap_or(0)))
        .unwrap_or_default();
    Layout { beds: beds.into_iter().map(|b| b.placed).collect(), bed: p.bed, unplaced }
}

/// Transform taking a piece from its assembled pose to its spot on the bed
/// (bed origin at the front-left corner).
pub fn bed_transform(print: &Mat4, pl: &Placement) -> Mat4 {
    Mat4::from_translation(Vec3::new(pl.x, pl.y, 0.0)) * Mat4::from_rotation_z(pl.angle) * *print
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;
    use std::sync::Arc;

    fn item(id: u32, size: Vec3) -> PackItem {
        let mut m = shapes::cube(size);
        m.translate(Vec3::new(-size.x / 2.0, -size.y / 2.0, 0.0));
        PackItem { id, mesh: Arc::new(m), print: Mat4::IDENTITY }
    }

    #[test]
    fn four_quarters_fill_one_bed() {
        let items: Vec<PackItem> = (0..4).map(|i| item(i, Vec3::new(95.0, 95.0, 10.0))).collect();
        let l = pack(&items, &PackParams { bed: [200.0, 200.0], spacing: 4.0, keep_order: false, angle_step: 90.0 });
        assert_eq!(l.beds.len(), 1, "{:?}", l);
        assert!(l.unplaced.is_empty());
    }

    #[test]
    fn triangles_nest() {
        // Two right triangles (half squares) fit together on one bed only by rotating.
        let tri = |id| {
            let m = Mesh::new(
                vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(180.0, 0.0, 0.0), Vec3::new(0.0, 180.0, 0.0), Vec3::new(0.0, 0.0, 10.0), Vec3::new(180.0, 0.0, 10.0), Vec3::new(0.0, 180.0, 10.0)],
                vec![[0, 2, 1], [3, 4, 5], [0, 1, 4], [0, 4, 3], [1, 2, 5], [1, 5, 4], [2, 0, 3], [2, 3, 5]],
            );
            PackItem { id, mesh: Arc::new(m), print: Mat4::from_translation(Vec3::new(-60.0, -60.0, 0.0)) }
        };
        let l = pack(&[tri(0), tri(1)], &PackParams { bed: [200.0, 200.0], spacing: 2.0, keep_order: false, angle_step: 15.0 });
        assert_eq!(l.beds.len(), 1, "{:?}", l);
    }

    #[test]
    fn too_big_is_unplaced() {
        let l = pack(&[item(7, Vec3::new(300.0, 300.0, 5.0))], &PackParams { bed: [200.0, 200.0], spacing: 2.0, keep_order: false, angle_step: 90.0 });
        assert_eq!(l.unplaced, vec![7]);
    }
}
