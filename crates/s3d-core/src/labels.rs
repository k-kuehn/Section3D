//! Part numbers: text outlines embossed or debossed on a nearly flat spot of
//! a piece's own surface (its convex or concave side), or cut into a cut
//! face when a piece has no room anywhere else.

use ab_glyph::{Font, FontRef, OutlineCurve};
use glam::{DMat4, DVec2, DVec3, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::bvh::Bvh;
use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::partition::{ON_PLANE, Orientation};
use crate::plane::Plane;
use crate::settings::{ConnectorSettings, LabelFace, LabelMode};

static FONT: &[u8] = include_bytes!("../assets/LabelFont.ttf");

/// Polygon contours of `text` with cap height `size`, centred on the origin,
/// outer contours counter-clockwise.
pub fn text_contours(text: &str, size: f64) -> Vec<Vec<DVec2>> {
    let font = FontRef::try_from_slice(FONT).expect("label font");
    // Scale so that digit height (outline bounds of '8') equals `size`.
    let cap = font.outline(font.glyph_id('8')).map(|o| (o.bounds.max.y - o.bounds.min.y).abs() as f64).unwrap_or(700.0);
    let k = size / cap.max(1.0);
    let mut contours: Vec<Vec<DVec2>> = Vec::new();
    let mut pen = 0.0f64;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        if let Some(outline) = font.outline(gid) {
            let mut cur: Vec<DVec2> = Vec::new();
            let mut last: Option<DVec2> = None;
            let to = |p: ab_glyph::Point| DVec2::new(pen + p.x as f64, p.y as f64) * k;
            for c in &outline.curves {
                let (start, pts): (DVec2, Vec<DVec2>) = match *c {
                    OutlineCurve::Line(a, b) => (to(a), vec![to(b)]),
                    OutlineCurve::Quad(a, b, c2) => {
                        let (a, b, c2) = (to(a), to(b), to(c2));
                        (a, (1..=6).map(|i| {
                            let t = i as f64 / 6.0;
                            a * (1.0 - t) * (1.0 - t) + b * 2.0 * t * (1.0 - t) + c2 * t * t
                        }).collect())
                    }
                    OutlineCurve::Cubic(a, b, c2, d) => {
                        let (a, b, c2, d) = (to(a), to(b), to(c2), to(d));
                        (a, (1..=8).map(|i| {
                            let t = i as f64 / 8.0;
                            let u = 1.0 - t;
                            a * u * u * u + b * 3.0 * u * u * t + c2 * 3.0 * u * t * t + d * t * t * t
                        }).collect())
                    }
                };
                if last.is_none_or(|l| l.distance(start) > 1e-9) {
                    if cur.len() > 2 {
                        contours.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    cur.push(start);
                }
                cur.extend(pts.iter().copied());
                last = pts.last().copied();
            }
            if cur.len() > 2 {
                contours.push(cur);
            }
        }
        pen += font.h_advance_unscaled(gid) as f64;
    }
    for c in &mut contours {
        if c.len() > 1 && c.first().unwrap().distance(*c.last().unwrap()) < 1e-9 {
            c.pop();
        }
    }
    if contours.is_empty() {
        return contours;
    }
    // Make the largest contour CCW; fonts keep a consistent winding.
    let area = |c: &Vec<DVec2>| (0..c.len()).map(|i| c[i].perp_dot(c[(i + 1) % c.len()])).sum::<f64>() * 0.5;
    let largest = contours.iter().max_by(|a, b| area(a).abs().total_cmp(&area(b).abs())).unwrap();
    if area(largest) < 0.0 {
        for c in &mut contours {
            c.reverse();
        }
    }
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for p in contours.iter().flatten() {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let c = (lo + hi) * 0.5;
    for p in contours.iter_mut().flatten() {
        *p -= c;
    }
    contours
}

/// Text width/height for a label.
fn text_extent(text: &str, size: f64) -> DVec2 {
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for p in text_contours(text, size).iter().flatten() {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    (hi - lo).max(DVec2::ZERO)
}

/// Where a part number sits on a piece: a frame on the surface (z along the
/// outward normal, y towards print "up") and how far the surface under the
/// text strays from that plane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelPlace {
    pub center: Vec3,
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
    /// Lowest and highest surface offset under the text along `z` (mm).
    pub lo: f32,
    pub hi: f32,
    pub text: String,
    pub size: f32,
    pub mode: LabelMode,
    pub depth: f32,
    /// On the convex side (facing away from the model's centre).
    #[serde(default)]
    pub convex: bool,
    /// On a cut face: the piece had no room on either side.
    #[serde(default)]
    pub on_cut: bool,
}

/// Signed bulge of the surface around `p` (normal `n`) over radius `r`:
/// positive where it curves away from the normal (convex), negative where it
/// curves towards it (concave), roughly in mm of sag.
fn bulge(bvh: &Bvh, p: Vec3, n: Vec3, r: f32) -> f32 {
    let (mut sum, mut w) = (0.0f32, 0.0f32);
    bvh.for_each_near(p, r, |u| {
        let t = bvh.triangle(u);
        let nn = (t[1] - t[0]).cross(t[2] - t[0]);
        let a = nn.length();
        if a < 1e-9 || nn.dot(n) < 0.3 * a {
            return;
        }
        let c = (t[0] + t[1] + t[2]) / 3.0;
        let off = (c - p).dot(n);
        // Same skin only (not the far side of a thin wall).
        if off.abs() > r * 0.5 {
            return;
        }
        let d = (c - p).length().max(1e-3);
        sum += -off * a / d;
        w += a / d;
    });
    if w > 0.0 { sum / w } else { 0.0 }
}

/// Text frame on a face with outward normal `n`: x along the text, y up.
fn frame(n: Vec3, up: Vec3) -> (Vec3, Vec3) {
    let mut y = up - n * up.dot(n);
    if y.length() < 0.3 {
        y = n.any_orthonormal_vector();
    }
    let y = y.normalize();
    (y.cross(n), y)
}

/// The whole model, which decides a piece's convex and concave sides.
pub struct Surroundings {
    /// All pieces' surfaces.
    bvh: Bvh,
    /// A line along the model's longest extent, stopping as far from the
    /// ends as the model is wide (a point for a round model).
    core: (Vec3, Vec3),
}

impl Surroundings {
    pub fn new<'a>(meshes: impl Iterator<Item = &'a Mesh>) -> Surroundings {
        let tris: Vec<[Vec3; 3]> = meshes.flat_map(|m| (0..m.tris.len()).map(|t| m.tri(t))).collect();
        let bvh = Bvh::from_triangles(tris);
        let (bb, s) = (bvh.bbox(), bvh.bbox().size());
        let k = if s.x >= s.y && s.x >= s.z { 0 } else if s.y >= s.z { 1 } else { 2 };
        let mut rest = [s.x, s.y, s.z];
        rest[k] = 0.0;
        let mut d = Vec3::ZERO;
        d[k] = ((s[k] - rest.iter().cloned().fold(0.0, f32::max)) * 0.5).max(0.0);
        Surroundings { bvh, core: (bb.center() - d, bb.center() + d) }
    }

    /// Share of directions around `n` in which `p` sees out of the model.
    fn openness(&self, p: Vec3, n: Vec3) -> f32 {
        let (u, v) = n.any_orthonormal_pair();
        let open = (0..16)
            .filter(|&k| {
                let (tilt, a) = ([0.3f32, 0.75, 1.2][k % 3], k as f32 * 2.4);
                let d = n * tilt.cos() + (u * a.cos() + v * a.sin()) * tilt.sin();
                self.bvh.ray(p + n * 0.05, d, f32::INFINITY).is_none()
            })
            .count();
        open as f32 / 16.0
    }

    /// How much the skin at `p` (normal `n`) is on the convex side: facing
    /// away from the core and, of the two skins of a wall, the one that
    /// sees more of the outside (the other one is hidden once assembled).
    /// Negative on the concave side. Both skins of a wall are judged from
    /// its middle, so they come out opposite; where they are square to the
    /// core, the one bulging away from the piece's `centre` is convex, and
    /// failing that the one facing up.
    fn outward(&self, p: Vec3, n: Vec3, centre: Vec3) -> f32 {
        let (mid, seen) = match self.bvh.ray(p - n * 0.01, -n, 60.0).filter(|h| h.normal.normalize_or_zero().dot(n) < -0.5) {
            Some(h) => {
                let q = p - n * (0.01 + h.t);
                ((p + q) * 0.5, self.openness(p, n) - self.openness(q, h.normal.normalize()))
            }
            None => (p, 0.0),
        };
        let (a, b) = self.core;
        let ab = b - a;
        let t = if ab == Vec3::ZERO { 0.0 } else { ((mid - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) };
        let facing = n.dot((mid - (a + ab * t)).normalize_or_zero());
        let bulge = n.dot((mid - centre).normalize_or_zero());
        facing + 1.5 * seen + 0.3 * bulge + 0.05 * n.dot(Vec3::new(0.12, 0.23, 0.96))
    }
}

/// Splits a piece's own surface (not its cut faces) into skins: smoothly
/// joined triangles turning less than 60° from where the skin started, so a
/// skin never wraps round the edge of a wall onto its other side.
fn skins(mesh: &Mesh, normals: &[Vec3], cut: &[bool]) -> Vec<Option<u32>> {
    let n = mesh.tris.len();
    let mut next: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (_, _, a, b) in mesh.edge_pairs() {
        if normals[a as usize].dot(normals[b as usize]) > 0.94 {
            next[a as usize].push(b);
            next[b as usize].push(a);
        }
    }
    let mut order: Vec<usize> = (0..n).filter(|&t| !cut[t]).collect();
    order.sort_by(|&a, &b| mesh.tri_cross(b).length_squared().total_cmp(&mesh.tri_cross(a).length_squared()));
    let mut skin: Vec<Option<u32>> = vec![None; n];
    let mut count = 0;
    for seed in order {
        if skin[seed].is_some() {
            continue;
        }
        let m = normals[seed];
        skin[seed] = Some(count);
        let mut stack = vec![seed];
        while let Some(t) = stack.pop() {
            for &u in &next[t] {
                let u = u as usize;
                if !cut[u] && skin[u].is_none() && normals[u].dot(m) > 0.5 {
                    skin[u] = Some(count);
                    stack.push(u);
                }
            }
        }
        count += 1;
    }
    skin
}

/// The piece's flat cut faces: connected triangles on one cut plane.
fn cut_faces(mesh: &Mesh, cut_of: &[Option<usize>]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..mesh.tris.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (_, _, a, b) in mesh.edge_pairs() {
        let (a, b) = (a as usize, b as usize);
        if cut_of[a].is_some() && cut_of[a] == cut_of[b] {
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            parent[ra] = rb;
        }
    }
    let mut faces: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for t in (0..mesh.tris.len()).filter(|&t| cut_of[t].is_some()) {
        let r = root(&mut parent, t);
        faces.entry(r).or_default().push(t);
    }
    faces.into_values().collect()
}

/// Text turns tried, upright first (radians).
const TURNS: [f32; 8] = [0.0, std::f32::consts::FRAC_PI_2, 0.785, -0.785, 0.393, -0.393, 1.178, -1.178];

/// Search passes, strict first: text scale, and whether the surface may be
/// less flat and the text come closer to a cut edge.
const PASSES: [(f32, bool); 4] = [(1.0, false), (1.0, true), (0.75, true), (0.5, true)];

/// Numbers are never made smaller than this (mm), unless asked for.
const MIN_TEXT: f32 = 3.0;

/// Finds a spot for a part number: on the model's own surface (never a cut
/// face), clear of cut edges, nearly flat under the text and with enough
/// wall behind it. Embossed numbers avoid faces pointing down while
/// printing when they can, and are cut in instead on the bed face.
///
/// The convex side faces away from the middle of the model and out of it,
/// the concave side faces in (see `Surroundings::outward`): on a shell
/// these are its outside and inside.
/// `c.label_face` picks the side and `flip` asks for the other one. When the
/// side has no room the number is turned, then made smaller (down to half
/// size, not under 3 mm) before it goes on the other side; a piece with no
/// room on either side gets it cut into a cut face, clear of pin holes.
pub fn place_label(mesh: &Mesh, cut_planes: &[Plane], orient: Option<&Orientation>, text: &str, c: &ConnectorSettings, flip: bool, model: &Surroundings) -> Option<LabelPlace> {
    if c.labels == LabelMode::Off || mesh.tris.is_empty() {
        return None;
    }
    let normals = mesh.face_normals();
    // The cut plane each cut-face triangle lies on.
    let cut_of: Vec<Option<usize>> = (0..mesh.tris.len()).map(|t| cut_planes.iter().position(|pl| mesh.tri(t).iter().all(|v| pl.dist(*v).abs() < ON_PLANE * 10.0))).collect();
    let cut: Vec<bool> = cut_of.iter().map(Option::is_some).collect();
    let bvh = Bvh::new(mesh);
    let up = orient.map(|o| o.rotation.inverse() * Vec3::Z).unwrap_or(Vec3::Z);
    let bed_down = orient.map(|o| o.rotation.inverse() * -Vec3::Z);
    let depth = c.label_depth;
    // Candidate spots: larger triangles spread over sizes, sorted by side.
    let mut v: Vec<(f32, usize)> = (0..mesh.tris.len()).filter(|&t| !cut[t]).map(|t| (mesh.tri_cross(t).length(), t)).collect();
    v.sort_by(|a, b| b.0.total_cmp(&a.0));
    let stride = (v.len() / 800).max(1);
    let picks: Vec<usize> = v.iter().step_by(stride).take(800).map(|x| x.1).collect();
    // One side per skin, settled against the skin behind it, so a skin is
    // one side throughout and the skin behind it the other.
    let centroid = |t: usize| {
        let [a, b, c] = mesh.tri(t);
        (a + b + c) / 3.0
    };
    let skin = skins(mesh, &normals, &cut);
    let n_skins = skin.iter().flatten().max().map_or(0, |m| *m as usize + 1);
    let centre = mesh.centroid();
    let (mut sum, mut weight, mut behind, mut seen) = (vec![0.0f32; n_skins], vec![0.0f32; n_skins], vec![None; n_skins], vec![0u8; n_skins]);
    for &t in &picks {
        let k = skin[t].unwrap() as usize;
        if seen[k] == 3 {
            continue;
        }
        seen[k] += 1;
        let (p, n, w) = (centroid(t), normals[t], mesh.tri_cross(t).length());
        sum[k] += model.outward(p, n, centre) * w;
        weight[k] += w;
        if behind[k].is_none() {
            behind[k] = bvh.ray(p - n * 0.01, -n, 60.0).filter(|h| h.normal.normalize_or_zero().dot(n) < -0.5).and_then(|h| skin[h.tri as usize]).filter(|&j| j as usize != k);
        }
    }
    let mean = |k: usize| sum[k] / weight[k].max(1e-9);
    let is_convex = |k: usize| behind[k].map_or(mean(k), |j| mean(k) - mean(j as usize)) >= 0.0;
    let (convex, concave): (Vec<(Vec3, usize)>, Vec<(Vec3, usize)>) = picks.iter().map(|&t| (centroid(t), t)).partition(|&(_, t)| is_convex(skin[t].unwrap() as usize));
    let best_on = |list: &[(Vec3, usize)], convex: bool, scale: f32, relaxed: bool| -> Option<LabelPlace> {
        let size = (c.label_size * scale).max(c.label_size.min(MIN_TEXT));
        let text_half = text_extent(text, size as f64).as_vec2() * 0.5;
        // The text with a small margin; the circle round it bounds the
        // surface checked under it.
        let half = text_half + Vec2::splat(0.5);
        let radius = half.length();
        let clear = if relaxed { 0.8 } else { 2.0 };
        // The surface may stray this far from flat under the text.
        let spread_max = (depth * 1.5).max(1.0) * if relaxed { 2.5 } else { 1.0 };
        let min_cos = if relaxed { 0.7 } else { 0.85 };
        let mut best: Option<(f32, LabelPlace)> = None;
        for &(p, t) in list {
            let n = normals[t];
            let down = -n.dot(up);
            // Raised text can't sit on the bed or on a cut face: it is cut in
            // there instead.
            let on_cut = cut_of[t];
            let mode = if c.labels == LabelMode::Emboss && (on_cut.is_some() || bed_down.is_some_and(|d| n.dot(d) > 0.9)) { LabelMode::Deboss } else { c.labels };
            if n == Vec3::ZERO || (mode == LabelMode::Emboss && !relaxed && down > 0.3) {
                continue;
            }
            // Nearby surface (with where it comes closest to the spot) and
            // the cut faces near the spot.
            let mut nearby: Vec<(u32, Vec3)> = Vec::new();
            let mut near: Vec<usize> = Vec::new();
            bvh.for_each_near(p, radius + clear, |u| match cut_of[u as usize] {
                Some(k) => {
                    if !near.contains(&k) {
                        near.push(k);
                    }
                }
                None => nearby.push((u, crate::bvh::closest_on_tri(p, &bvh.triangle(u)) - p)),
            });
            // Upright text if it fits, else turned (along a narrow strip, say).
            let (x0, y0) = frame(n, up);
            let at = |x: Vec3, y: Vec3, h: Vec2, i: f32, j: f32| p + x * (h.x * i) + y * (h.y * j);
            let fit = TURNS.iter().find_map(|&a| {
                let (x, y) = (x0 * a.cos() + y0 * a.sin(), y0 * a.cos() - x0 * a.sin());
                // Clear of the cut faces.
                let clear_of_cuts = near.iter().filter(|&&k| Some(k) != on_cut).all(|&k| {
                    let pl = &cut_planes[k];
                    let side = pl.dist(p).signum();
                    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().all(|&(i, j)| pl.dist(at(x, y, half, i, j)) * side >= clear as f64)
                });
                if !clear_of_cuts {
                    return None;
                }
                // Under the text: the same skin, close to the tangent plane,
                // and any other skin (the far side of a thin wall) well
                // behind it.
                let (mut lo, mut hi, mut behind) = (0.0f32, 0.0f32, f32::INFINITY);
                for &(u, foot) in &nearby {
                    if foot.dot(x).abs() > half.x || foot.dot(y).abs() > half.y {
                        continue;
                    }
                    let nn = normals[u as usize];
                    let offs = bvh.triangle(u).map(|v| (v - p).dot(n));
                    if nn.dot(n) > 0.3 {
                        if nn.dot(n) < min_cos {
                            return None;
                        }
                        lo = offs.iter().fold(lo, |m, o| m.min(*o));
                        hi = offs.iter().fold(hi, |m, o| m.max(*o));
                    } else {
                        behind = behind.min(-offs.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
                    }
                }
                let need = match mode {
                    LabelMode::Deboss => depth + (hi - lo) + 0.8,
                    _ => 0.8,
                };
                if hi - lo > spread_max || behind < need - lo.min(0.0) {
                    return None;
                }
                // The whole text area must lie on the surface (no holes or edges).
                let covered = [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (-1.0, 1.0), (-1.0, 0.0)]
                    .iter()
                    .all(|&(i, j)| bvh.ray(at(x, y, text_half, i, j) + n * (hi + 1.0), -n, hi - lo + 2.0).is_some_and(|h| h.normal.normalize_or_zero().dot(n) > 0.3));
                covered.then_some((x, y, lo, hi))
            });
            let Some((x, y, lo, hi)) = fit else {
                continue;
            };
            let k = (bulge(&bvh, p, n, radius * 3.0) / (radius * 0.05)).clamp(-1.0, 1.0);
            let flat = 1.0 - (hi - lo) / spread_max;
            // Sides read best on the printed piece, upright text too; faces
            // pointing down last.
            let read = 1.0 - n.dot(up).abs() * 0.3 - down.max(0.0) * 0.5 - (1.0 - y.dot(y0)) * 0.2;
            let score = 0.3 * read + 0.2 * flat + 0.2 * if convex { k } else { -k } - if mode != c.labels { 0.5 } else { 0.0 };
            if best.as_ref().is_none_or(|b| score > b.0) {
                best = Some((score, LabelPlace { center: p, x, y, z: n, lo, hi, text: text.to_string(), size, mode, depth, convex, on_cut: on_cut.is_some() }));
            }
        }
        best.map(|b| b.1)
    };
    let want = (c.label_face == LabelFace::Convex) != flip;
    // Last resort: the middle of each cut face, and its largest triangle.
    let faces: Vec<(Vec3, usize)> = cut_faces(mesh, &cut_of)
        .iter()
        .flat_map(|f| {
            let c = f.iter().fold(Vec3::ZERO, |a, &t| a + centroid(t) * mesh.tri_cross(t).length()) / f.iter().map(|&t| mesh.tri_cross(t).length()).sum::<f32>().max(1e-9);
            let near = *f.iter().min_by(|&&a, &&b| centroid(a).distance_squared(c).total_cmp(&centroid(b).distance_squared(c))).unwrap();
            let big = *f.iter().max_by(|&&a, &&b| mesh.tri_cross(a).length_squared().total_cmp(&mesh.tri_cross(b).length_squared())).unwrap();
            [(c, near), (centroid(big), big)]
        })
        .collect();
    let side = |convex_side: bool| if convex_side { &convex } else { &concave };
    [(want, side(want)), (!want, side(!want)), (want, &faces)]
        .into_iter()
        .find_map(|(convex_side, list)| PASSES.iter().find_map(|&(scale, relaxed)| best_on(list, convex_side, scale, relaxed)))
}

/// The text solid to union (emboss) or subtract (deboss) at a placement. It
/// spans the surface's spread, so curved spots keep the full depth.
pub fn label_solid_at(l: &LabelPlace) -> Option<Solid> {
    let contours = text_contours(&l.text, l.size as f64);
    if contours.is_empty() {
        return None;
    }
    let (lo, hi, depth) = (l.lo as f64, l.hi as f64, l.depth as f64);
    let (z0, z1) = match l.mode {
        LabelMode::Emboss => (lo - 0.4, hi + depth),
        _ => (lo - depth, hi + 0.4),
    };
    let text = Solid::extrude(&contours, z1 - z0).ok()?.translate(DVec3::new(0.0, 0.0, z0)).ok()?;
    let m = DMat4::from_cols(l.x.as_dvec3().extend(0.0), l.y.as_dvec3().extend(0.0), l.z.as_dvec3().extend(0.0), l.center.as_dvec3().extend(1.0));
    text.transform(&m).ok()
}

/// Outline of a placed label as line segments just above the surface, for
/// drawing the numbers in the 3D view.
pub fn outline(l: &LabelPlace) -> Vec<[Vec3; 2]> {
    let z = match l.mode {
        LabelMode::Emboss => l.hi + l.depth + 0.05,
        _ => l.hi.max(0.0) + 0.08,
    };
    let at = |q: DVec2| l.center + l.x * q.x as f32 + l.y * q.y as f32 + l.z * z;
    let mut out = Vec::new();
    for c in text_contours(&l.text, l.size as f64) {
        for i in 0..c.len() {
            out.push([at(c[i]), at(c[(i + 1) % c.len()])]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_have_holes_and_size() {
        let c = text_contours("80", 10.0);
        // '8' has an outer contour and two holes, '0' an outer and one hole.
        assert_eq!(c.len(), 5);
        let e = text_extent("80", 10.0);
        assert!((e.y - 10.0).abs() < 1.0, "{e:?}");
        let s = Solid::extrude(&c, 1.0).unwrap();
        assert!(s.volume() > 10.0, "{}", s.volume());
    }

    #[test]
    fn label_on_cube_side() {
        let m = crate::mesh::shapes::cube(Vec3::new(60.0, 60.0, 40.0));
        let mut cs = ConnectorSettings::default();
        cs.labels = LabelMode::Deboss;
        // The x = 0 face is a cut; the label must go elsewhere.
        let cut = Plane::axis(0, 0.0);
        let l = label_solid_at(&place_label(&m, &[cut], None, "12", &cs, false, &Surroundings::new([&m].into_iter())).expect("label placed")).unwrap();
        let b = l.bbox();
        assert!(b.min.x > 5.0, "{b:?}");
        let cube = Solid::from_mesh(&m).unwrap();
        let out = cube.subtract(&l).unwrap();
        assert!(out.volume() < cube.volume());
    }

    #[test]
    fn convex_or_concave_side_of_a_shell() {
        // Top half of a 6 mm thick spherical shell.
        let outer = Solid::sphere(90.0, 160).unwrap();
        let inner = Solid::sphere(84.0, 160).unwrap();
        let (top, _) = outer.subtract(&inner).unwrap().split_by_plane(DVec3::Z, 0.0).unwrap();
        let m = top.to_mesh();
        let cut = Plane { normal: -DVec3::Z, offset: 0.0 };
        let mut cs = ConnectorSettings::default();
        cs.labels = LabelMode::Deboss;
        for (face, flip, outward) in [(LabelFace::Convex, false, true), (LabelFace::Concave, false, false), (LabelFace::Concave, true, true), (LabelFace::Convex, true, false)] {
            cs.label_face = face;
            let l = place_label(&m, &[cut], None, "7", &cs, flip, &Surroundings::new([&m].into_iter())).expect("label placed");
            let out = l.z.dot(l.center.normalize()) > 0.0;
            assert_eq!(out, outward, "{face:?} flip {flip}: normal {:?} at {:?}", l.z, l.center);
            assert_eq!(l.convex, outward);
            assert!(!outline(&l).is_empty());
            let body = Solid::from_mesh(&m).unwrap();
            assert!(body.subtract(&label_solid_at(&l).unwrap()).unwrap().volume() < body.volume());
        }
    }

    #[test]
    fn narrow_strips_get_a_turned_or_smaller_number_on_either_side() {
        // Half of a 6 mm thick ring: on a 12 mm strip the 8 mm number turns
        // to run along it; on a 9 mm strip it also gets smaller.
        for (height, turned) in [(12.0, true), (9.0, false)] {
            let ring = Solid::cylinder(height, 90.0, 90.0, 160, false).unwrap().subtract(&Solid::cylinder(height, 84.0, 84.0, 160, false).unwrap()).unwrap();
            let (half, _) = ring.split_by_plane(DVec3::Y, 0.0).unwrap();
            let m = half.to_mesh();
            let cut = Plane { normal: -DVec3::Y, offset: 0.0 };
            let mut cs = ConnectorSettings::default();
            cs.labels = LabelMode::Deboss;
            cs.label_size = 8.0;
            let model = Surroundings::new([&m].into_iter());
            for face in [LabelFace::Convex, LabelFace::Concave] {
                cs.label_face = face;
                let l = place_label(&m, &[cut], None, "7", &cs, false, &model).expect("label placed");
                assert_eq!(l.convex, face == LabelFace::Convex);
                // Outside faces away from the ring's axis.
                assert_eq!(Vec3::new(l.center.x, l.center.y, 0.0).dot(l.z) > 0.0, l.convex);
                if turned {
                    assert!(l.size == 8.0 && l.y.z.abs() < 0.5, "{height} mm: size {} up {:?}", l.size, l.y);
                } else {
                    assert!(l.size < 8.0 && l.size >= 4.0, "{height} mm: size {}", l.size);
                }
                assert!(half.subtract(&label_solid_at(&l).unwrap()).unwrap().volume() < half.volume());
            }
        }
    }

    #[test]
    fn raised_number_on_the_bed_face_is_cut_in() {
        let m = crate::mesh::shapes::cube(Vec3::new(60.0, 60.0, 5.0));
        let flat = Orientation { rotation: glam::Quat::IDENTITY, translation: Vec3::ZERO, size: Vec3::new(60.0, 60.0, 5.0), support_area: 0.0 };
        let mut cs = ConnectorSettings::default();
        cs.labels = LabelMode::Emboss;
        let model = Surroundings::new([&m].into_iter());
        for (face, mode) in [(LabelFace::Convex, LabelMode::Emboss), (LabelFace::Concave, LabelMode::Deboss)] {
            cs.label_face = face;
            let l = place_label(&m, &[], Some(&flat), "3", &cs, false, &model).expect("label placed");
            assert_eq!((l.convex, l.mode), (face == LabelFace::Convex, mode), "{face:?}");
        }
    }

    #[test]
    fn piece_with_only_cut_faces_gets_its_number_cut_into_one() {
        let m = crate::mesh::shapes::cube(Vec3::new(30.0, 20.0, 20.0));
        let cuts: Vec<Plane> = (0..3).flat_map(|k| [Plane::axis(k, 0.0), Plane::axis(k, [30.0, 20.0, 20.0][k])]).collect();
        let mut cs = ConnectorSettings::default();
        cs.labels = LabelMode::Emboss;
        let l = place_label(&m, &cuts, None, "12", &cs, false, &Surroundings::new([&m].into_iter())).expect("label placed");
        assert!(l.on_cut && l.mode == LabelMode::Deboss, "{l:?}");
        let body = Solid::from_mesh(&m).unwrap();
        assert!(body.subtract(&label_solid_at(&l).unwrap()).unwrap().volume() < body.volume());
    }
}
