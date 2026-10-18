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

/// Footprint at angle 0: conservative raster of the projected triangles.
fn base_raster(tris: &[[Vec2; 3]], cell: f32) -> (Raster, Vec2) {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for t in tris {
        for p in t {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    let origin = lo - Vec2::splat(cell);
    let w = ((hi.x - lo.x) / cell).ceil() as usize + 3;
    let h = ((hi.y - lo.y) / cell).ceil() as usize + 3;
    let mut base = Raster::new(w, h);
    for t in tris {
        let tl = t.iter().fold(Vec2::splat(f32::INFINITY), |a, p| a.min(*p));
        let th = t.iter().fold(Vec2::splat(f32::NEG_INFINITY), |a, p| a.max(*p));
        let (x0, y0) = (((tl.x - origin.x) / cell).floor() as usize, ((tl.y - origin.y) / cell).floor() as usize);
        let (x1, y1) = (((th.x - origin.x) / cell).floor() as usize, ((th.y - origin.y) / cell).floor() as usize);
        for y in y0..=y1.min(h - 1) {
            for x in x0..=x1.min(w - 1) {
                if base.get(x, y) {
                    continue;
                }
                let cmin = origin + Vec2::new(x as f32, y as f32) * cell;
                if tri_box_overlap(t, cmin, cmin + Vec2::splat(cell)) {
                    base.set(x, y);
                }
            }
        }
    }
    (base, origin)
}

/// The base footprint rotated by `angle` (conservatively resampled) and
/// dilated by `dilate` cells.
fn rotated_shape(base: &Raster, origin: Vec2, cell: f32, angle: f32, dilate: usize) -> Shape {
    // Right angles: exact copies (the orientation step chose angle 0 to fit).
    let quarter = (angle / std::f32::consts::FRAC_PI_2).round();
    if (angle - quarter * std::f32::consts::FRAC_PI_2).abs() < 1e-4 {
        let q = (quarter as i32).rem_euclid(4);
        let (w, h) = if q % 2 == 0 { (base.w, base.h) } else { (base.h, base.w) };
        let pad = dilate;
        let mut r = Raster::new(w + 2 * pad, h + 2 * pad);
        for y in 0..base.h {
            for x in 0..base.w {
                if base.get(x, y) {
                    // Rotate cell (x, y) by q quarter turns counter-clockwise.
                    let (nx, ny) = match q {
                        0 => (x, y),
                        1 => (base.h - 1 - y, x),
                        2 => (base.w - 1 - x, base.h - 1 - y),
                        _ => (y, base.w - 1 - x),
                    };
                    r.set(nx + pad, ny + pad);
                }
            }
        }
        // Where the footprint centre (world origin) lands, in cells.
        let c0 = (Vec2::ZERO - origin) / cell;
        let cr = match q {
            0 => c0,
            1 => Vec2::new(base.h as f32 - c0.y, c0.x),
            2 => Vec2::new(base.w as f32 - c0.x, base.h as f32 - c0.y),
            _ => Vec2::new(c0.y, base.w as f32 - c0.x),
        } + Vec2::splat(pad as f32);
        if dilate > 0 {
            dilate_raster(&mut r, dilate);
        }
        let area = r.bits.iter().map(|b| b.count_ones() as usize).sum();
        return Shape { r, centre: cr, area };
    }
    let (s, c) = angle.sin_cos();
    let rot = |p: Vec2| Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y);
    let inv = |p: Vec2| Vec2::new(c * p.x + s * p.y, -s * p.x + c * p.y);
    let size = Vec2::new(base.w as f32, base.h as f32) * cell;
    let corners = [origin, origin + Vec2::new(size.x, 0.0), origin + size, origin + Vec2::new(0.0, size.y)].map(rot);
    let lo = corners.iter().fold(Vec2::splat(f32::INFINITY), |a, p| a.min(*p));
    let hi = corners.iter().fold(Vec2::splat(f32::NEG_INFINITY), |a, p| a.max(*p));
    let pad = dilate as f32 * cell;
    let o2 = lo - Vec2::splat(pad);
    let w = ((hi.x - lo.x + 2.0 * pad) / cell).ceil() as usize + 1;
    let h = ((hi.y - lo.y + 2.0 * pad) / cell).ceil() as usize + 1;
    let mut r = Raster::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let q = inv(o2 + (Vec2::new(x as f32, y as f32) + 0.5) * cell);
            let b = (q - origin) / cell - Vec2::splat(0.5);
            let (bx, by) = (b.x.floor() as isize, b.y.floor() as isize);
            // Any of the four nearest base cells: conservative under rotation.
            let hit = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().any(|(dx, dy)| {
                let (xx, yy) = (bx + dx, by + dy);
                xx >= 0 && yy >= 0 && base.get(xx as usize, yy as usize)
            });
            if hit {
                r.set(x, y);
            }
        }
    }
    if dilate > 0 {
        dilate_raster(&mut r, dilate);
    }
    let area = r.bits.iter().map(|b| b.count_ones() as usize).sum();
    Shape { r, centre: (Vec2::ZERO - o2) / cell, area }
}

/// Square dilation with row bit operations.
fn dilate_raster(r: &mut Raster, d: usize) {
    let words = r.words;
    let mut horiz = vec![0u64; r.bits.len()];
    let (mut tmp, mut dil) = (vec![0u64; words], vec![0u64; words]);
    for y in 0..r.h {
        let row = &r.bits[y * words..(y + 1) * words];
        // x set if any of x-d..=x+d: dilate towards lower x by 2d, shift up by d.
        dilate_right(row, 2 * d, &mut dil, &mut tmp);
        shift_left_into(&dil, d, &mut horiz[y * words..(y + 1) * words]);
    }
    for y in 0..r.h {
        for k in 0..words {
            let mut v = 0;
            for yy in y.saturating_sub(d)..=(y + d).min(r.h - 1) {
                v |= horiz[yy * words + k];
            }
            r.bits[y * words + k] = v;
        }
    }
    // Clear bits beyond the raster width.
    let extra = words * 64 - r.w;
    if extra > 0 {
        let mask = u64::MAX >> extra;
        for y in 0..r.h {
            r.bits[y * words + words - 1] &= mask;
        }
    }
}

/// out = row << j (bit x moves to x + j).
fn shift_left_into(row: &[u64], j: usize, out: &mut [u64]) {
    let (ws, bs) = (j / 64, j % 64);
    for k in 0..out.len() {
        let lo = if k >= ws { row[k - ws] << bs } else { 0 };
        let hi = if bs > 0 && k > ws { row[k - ws - 1] >> (64 - bs) } else { 0 };
        out[k] = lo | hi;
    }
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

    /// Bottom-left position for `s`: for each candidate row, all blocked x
    /// positions are computed at once with bit operations (each footprint
    /// row run blocks the bed bits dilated by its length).
    fn find(&self, s: &Raster) -> Option<(usize, usize)> {
        if s.w > self.r.w || s.h > self.r.h {
            return None;
        }
        let words = self.r.words;
        let runs = row_runs(s);
        let max_x = self.r.w - s.w;
        let mut blocked = vec![0u64; words];
        let mut tmp = vec![0u64; words];
        let mut dil = vec![0u64; words];
        for y in 0..=self.r.h - s.h {
            blocked.iter_mut().for_each(|w| *w = 0);
            let mut full = false;
            for (sy, row_runs) in runs.iter().enumerate() {
                let bed_row = &self.r.bits[(y + sy) * words..(y + sy + 1) * words];
                if bed_row.iter().all(|w| *w == 0) {
                    continue;
                }
                for &(s0, s1) in row_runs {
                    dilate_right(bed_row, s1 - s0, &mut dil, &mut tmp);
                    shift_right_or(&dil, s0, &mut blocked);
                }
                // Stop early once every x is blocked.
                if first_zero(&blocked, max_x).is_none() {
                    full = true;
                    break;
                }
            }
            if full {
                continue;
            }
            if let Some(x) = first_zero(&blocked, max_x) {
                debug_assert!(self.fits(s, x, y));
                return Some((x, y));
            }
        }
        None
    }
}

/// Runs of set bits per row: (first, last) column.
fn row_runs(s: &Raster) -> Vec<Vec<(usize, usize)>> {
    (0..s.h)
        .map(|y| {
            let mut out = Vec::new();
            let mut x = 0;
            while x < s.w {
                if s.get(x, y) {
                    let start = x;
                    while x + 1 < s.w && s.get(x + 1, y) {
                        x += 1;
                    }
                    out.push((start, x));
                }
                x += 1;
            }
            out
        })
        .collect()
}

/// out = row >> j for one multi-word bitset (bit x moves to x - j).
fn shift_right_into(row: &[u64], j: usize, out: &mut [u64]) {
    let (ws, bs) = (j / 64, j % 64);
    for k in 0..out.len() {
        let src = k + ws;
        let lo = if src < row.len() { row[src] >> bs } else { 0 };
        let hi = if bs > 0 && src + 1 < row.len() { row[src + 1] << (64 - bs) } else { 0 };
        out[k] = lo | hi;
    }
}

fn shift_right_or(row: &[u64], j: usize, out: &mut [u64]) {
    let (ws, bs) = (j / 64, j % 64);
    for k in 0..out.len() {
        let src = k + ws;
        let lo = if src < row.len() { row[src] >> bs } else { 0 };
        let hi = if bs > 0 && src + 1 < row.len() { row[src + 1] << (64 - bs) } else { 0 };
        out[k] |= lo | hi;
    }
}

/// out = OR over j in 0..=k of (row >> j): x is set if any of x..=x+k is.
fn dilate_right(row: &[u64], k: usize, out: &mut [u64], tmp: &mut [u64]) {
    out.copy_from_slice(row);
    let mut covered = 1;
    while covered * 2 <= k + 1 {
        shift_right_into(out, covered, tmp);
        for (o, t) in out.iter_mut().zip(tmp.iter()) {
            *o |= *t;
        }
        covered *= 2;
    }
    let rest = k + 1 - covered;
    if rest > 0 {
        shift_right_into(out, rest, tmp);
        for (o, t) in out.iter_mut().zip(tmp.iter()) {
            *o |= *t;
        }
    }
}

/// First clear bit at or below `max_x`.
fn first_zero(bits: &[u64], max_x: usize) -> Option<usize> {
    for (k, w) in bits.iter().enumerate() {
        if k * 64 > max_x {
            break;
        }
        let inv = !*w;
        if inv != 0 {
            let x = k * 64 + inv.trailing_zeros() as usize;
            return (x <= max_x).then_some(x);
        }
    }
    None
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
    // The bed raster is padded by the dilation (spacing applies between
    // pieces, not at the bed edge) plus two cells of rasterisation slack
    // (about 2 mm, well inside the printer margin).
    let pad = dilate + 2;
    let (bw, bh) = ((p.bed[0] / cell).floor() as usize + 2 * pad, (p.bed[1] / cell).floor() as usize + 2 * pad);
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
            let (base, origin) = base_raster(&pts, cell);
            angles.iter().map(|&a| rotated_shape(&base, origin, cell, a, dilate)).collect()
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
                // Skip beds without enough free area.
                let free = beds[b].r.w * beds[b].r.h - beds[b].used;
                if area[i] > free {
                    if beds[b].placed.is_empty() {
                        beds.pop();
                        break;
                    }
                    continue;
                }
                // Best rotation on this bed: lowest y, then lowest x.
                let bed_ref = &beds[b];
                let best = shapes[i].par_iter().enumerate().filter_map(|(ai, s)| bed_ref.find(&s.r).map(|pos| (pos.1, pos.0, ai))).min();
                if let Some((y, x, ai)) = best {
                    let s = &shapes[i][ai];
                    beds[b].stamp(&s.r, x, y);
                    beds[b].used += s.area;
                    let c = (Vec2::new(x as f32, y as f32) + s.centre - Vec2::splat(pad as f32)) * cell;
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
