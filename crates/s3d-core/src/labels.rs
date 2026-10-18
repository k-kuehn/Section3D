//! Part numbers: text outlines extruded on a flat outside face of a piece.
//! Never placed on a cut face, the bed face, or a downward-facing face.

use ab_glyph::{Font, FontRef, OutlineCurve};
use glam::{DMat4, DVec2, DVec3, Vec3};

use crate::bvh::Bvh;
use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::partition::{ON_PLANE, Orientation};
use crate::plane::Plane;
use crate::settings::{ConnectorSettings, LabelMode};

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

/// Finds a flat spot for the label and returns the text solid to union
/// (emboss) or subtract (deboss) there. `cut_planes` are the piece's cuts.
pub fn label_solid(mesh: &Mesh, cut_planes: &[Plane], orient: Option<&Orientation>, text: &str, c: &ConnectorSettings) -> Option<Solid> {
    let size = c.label_size as f64;
    let ext = text_extent(text, size);
    let radius = (ext.length() * 0.5 + 1.5) as f32;
    let depth = c.label_depth as f64;
    let up = orient.map(|o| o.rotation.inverse() * Vec3::Z).unwrap_or(Vec3::Z);
    let bvh = Bvh::new(mesh);
    let bed_down = orient.map(|o| o.rotation.inverse() * -Vec3::Z);
    // Candidate spots: centroids of larger non-cut triangles.
    let mut cands: Vec<(f32, usize)> = (0..mesh.tris.len())
        .filter_map(|t| {
            let tri = mesh.tri(t);
            if cut_planes.iter().any(|pl| tri.iter().all(|v| pl.dist(*v).abs() < ON_PLANE * 10.0)) {
                return None;
            }
            Some((mesh.tri_cross(t).length(), t))
        })
        .collect();
    cands.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut best: Option<(f32, Vec3, Vec3)> = None;
    for &(_, t) in cands.iter().take(400) {
        let tri = mesh.tri(t);
        let n = mesh.tri_cross(t).normalize_or_zero();
        let p = (tri[0] + tri[1] + tri[2]) / 3.0;
        // Not on the bed face, not facing down while printing.
        if bed_down.is_some_and(|d| n.dot(d) > 0.9) || n.dot(up) < -0.3 {
            continue;
        }
        // Keep clear of cut faces (the label must not touch a cut edge).
        if cut_planes.iter().any(|pl| (pl.dist(p).abs() as f32) < radius + 2.0) {
            continue;
        }
        // Flatness: every triangle under the label disc faces the same way and
        // lies within the label depth of the plane.
        let mut flat = true;
        let mut checked = 0;
        bvh.for_each_near(p, radius, |u| {
            if !flat {
                return;
            }
            checked += 1;
            let tt = bvh.triangle(u);
            let nn = (tt[1] - tt[0]).cross(tt[2] - tt[0]).normalize_or_zero();
            let off = tt.iter().map(|v| (*v - p).dot(n).abs()).fold(0.0f32, f32::max);
            if nn.dot(n) < 0.97 || off > (depth as f32) * 0.5 {
                flat = false;
            }
        });
        if !flat || checked == 0 {
            continue;
        }
        // Prefer sides (readable on the printed piece) and big flat areas.
        let score = 1.0 - n.dot(up).abs() * 0.3;
        if best.is_none_or(|b| score > b.0) {
            best = Some((score, p, n));
        }
        if score > 0.95 {
            break;
        }
    }
    let (_, p, n) = best?;
    let contours = text_contours(text, size);
    if contours.is_empty() {
        return None;
    }
    // Local frame: z along the outward normal, text "up" towards print up.
    let z = n.as_dvec3();
    let mut y = up.as_dvec3() - z * up.as_dvec3().dot(z);
    if y.length() < 0.3 {
        y = z.any_orthonormal_vector();
    }
    let y = y.normalize();
    let x = y.cross(z);
    let (z0, h) = match c.labels {
        LabelMode::Emboss => (-0.4, depth + 0.4),
        _ => (-depth, depth + 0.4),
    };
    let text = Solid::extrude(&contours, h).ok()?.translate(DVec3::new(0.0, 0.0, z0)).ok()?;
    let m = DMat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), p.as_dvec3().extend(1.0));
    text.transform(&m).ok()
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
        let l = label_solid(&m, &[cut], None, "12", &cs).expect("label placed");
        let b = l.bbox();
        assert!(b.min.x > 5.0, "{b:?}");
        let cube = Solid::from_mesh(&m).unwrap();
        let out = cube.subtract(&l).unwrap();
        assert!(out.volume() < cube.volume());
    }
}
