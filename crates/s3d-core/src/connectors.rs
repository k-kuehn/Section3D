//! Finishing pieces after cutting: dowel pin holes on both sides of every cut
//! and embossed/debossed part numbers.
//!
//! Pin centres are computed once per seam, so the holes in the two pieces
//! always line up.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DMat4, DVec2, DVec3, Vec3};
use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::job::JobCtx;
use crate::labels;
use crate::manifold::{Op, Solid};
use crate::mesh::Mesh;
use crate::partition::{Flag, ON_PLANE, Partition, PieceId, Seam, TriGrid, caps_on};
use crate::plane::Plane;
use crate::settings::{ConnectorSettings, LabelMode, Settings};

/// Minimum material around a hole (mm).
pub const MIN_WALL: f64 = 1.2;
/// Extra hole depth beyond half the pin length, for glue (mm).
pub const GLUE_ROOM: f64 = 0.5;

#[derive(Clone, Debug)]
pub struct Pin {
    pub seam: u32,
    /// Centre on the cut plane.
    pub center: DVec3,
    /// Hole direction into piece `a` (the insertion direction of the piece
    /// assembled later); the hole in `b` runs the opposite way.
    pub axis: DVec3,
}

impl ConnectorSettings {
    pub fn hole_radius(&self) -> f64 {
        (self.diameter + self.clearance) as f64 * 0.5
    }
    pub fn hole_depth(&self) -> f64 {
        self.length as f64 * 0.5 + GLUE_ROOM
    }
}

/// Raster of the contact region of a seam with a distance-to-edge field.
struct Contact {
    u: DVec3,
    v: DVec3,
    origin: DVec2,
    cell: f64,
    w: usize,
    h: usize,
    /// Distance to the region edge in mm (0 outside).
    dist: Vec<f64>,
}

fn contact_raster(p: &Partition, s: &Seam) -> Option<Contact> {
    let (a, b) = (p.piece(s.a)?, p.piece(s.b)?);
    let ca = caps_on(&a.mesh, &s.plane, -1.0);
    let cb = caps_on(&b.mesh, &s.plane, 1.0);
    if ca.is_empty() || cb.is_empty() {
        return None;
    }
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for t in &ca {
        for q in t {
            lo = lo.min(*q);
            hi = hi.max(*q);
        }
    }
    let ext = hi - lo;
    // Fine enough to resolve thin shell walls (a few mm) on large cuts.
    let cell = (ext.x.max(ext.y) / 400.0).clamp(0.2, 1.0);
    let (w, h) = ((ext.x / cell).ceil() as usize + 3, (ext.y / cell).ceil() as usize + 3);
    let origin = lo - DVec2::splat(cell);
    let (ga, gb) = (TriGrid::new(ca), TriGrid::new(cb));
    let inside: Vec<bool> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let q = origin + DVec2::new((i % w) as f64 + 0.5, (i / w) as f64 + 0.5) * cell;
            ga.contains(q) && gb.contains(q)
        })
        .collect();
    // Two-pass chamfer distance transform (in cells), then to mm.
    let big = 1e9;
    let mut d: Vec<f64> = inside.iter().map(|&x| if x { big } else { 0.0 }).collect();
    let (s1, s2) = (1.0, std::f64::consts::SQRT_2);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if d[i] == 0.0 {
                continue;
            }
            let mut m = d[i];
            if x > 0 { m = m.min(d[i - 1] + s1) }
            if y > 0 { m = m.min(d[i - w] + s1) }
            if x > 0 && y > 0 { m = m.min(d[i - w - 1] + s2) }
            if x + 1 < w && y > 0 { m = m.min(d[i - w + 1] + s2) }
            d[i] = m;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            if d[i] == 0.0 {
                continue;
            }
            let mut m = d[i];
            if x + 1 < w { m = m.min(d[i + 1] + s1) }
            if y + 1 < h { m = m.min(d[i + w] + s1) }
            if x + 1 < w && y + 1 < h { m = m.min(d[i + w + 1] + s2) }
            if x > 0 && y + 1 < h { m = m.min(d[i + w - 1] + s2) }
            d[i] = m;
        }
    }
    let dist = d.into_iter().map(|c| if c >= big { 0.0 } else { c * cell }).collect();
    let (u, v) = s.plane.basis();
    Some(Contact { u, v, origin, cell, w, h, dist })
}

/// Pin centres for one seam: spread along the contact region's long axis,
/// kept at least `clear` from its edges and `2 * diameter` apart.
pub fn place_pins(p: &Partition, s: &Seam, c: &ConnectorSettings) -> Vec<DVec3> {
    place_pins_checked(p, s, c, &|_| true)
}

/// Like `place_pins`, but each pin must pass `fits` (the 3D wall check); the
/// best few candidate spots per pin are tried in order.
pub fn place_pins_checked(p: &Partition, s: &Seam, c: &ConnectorSettings, fits: &(dyn Fn(DVec3) -> bool + Sync)) -> Vec<DVec3> {
    let Some(ct) = contact_raster(p, s) else { return Vec::new() };
    let r = c.hole_radius();
    let clear = r + MIN_WALL;
    let ok: Vec<(DVec2, f64)> = (0..ct.w * ct.h)
        .filter(|&i| ct.dist[i] >= clear)
        .map(|i| (ct.origin + DVec2::new((i % ct.w) as f64 + 0.5, (i / ct.w) as f64 + 0.5) * ct.cell, ct.dist[i]))
        .collect();
    if ok.is_empty() {
        return Vec::new();
    }
    // Principal axis of the allowed region.
    let mean = ok.iter().fold(DVec2::ZERO, |a, q| a + q.0) / ok.len() as f64;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for (q, _) in &ok {
        let d = *q - mean;
        sxx += d.x * d.x;
        sxy += d.x * d.y;
        syy += d.y * d.y;
    }
    let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let axis = DVec2::new(ang.cos(), ang.sin());
    let (lo, hi) = ok.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
        let t = (q.0 - mean).dot(axis);
        (lo.min(t), hi.max(t))
    });
    let spacing = c.diameter as f64 * 2.0 + 2.0 * r;
    let max_n = (((hi - lo) / spacing).floor() as u32 + 1).max(1);
    let n = c.per_cut.clamp(1, max_n);
    let base = s.plane.normal * s.plane.offset;
    let to3 = |q: DVec2| base + ct.u * q.x + ct.v * q.y;
    let mut pins: Vec<DVec2> = Vec::new();
    for k in 0..n {
        let target = lo + (hi - lo) * (k as f64 + 0.5) / n as f64;
        // Close to the target position along the axis, deep inside the region.
        let mut ranked: Vec<(f64, DVec2)> = ok
            .iter()
            .filter(|q| pins.iter().all(|p| p.distance(q.0) >= spacing * 0.9))
            .map(|q| {
                let t = (q.0 - mean).dot(axis);
                ((t - target).abs() - 0.35 * q.1, q.0)
            })
            .collect();
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Try well-spread candidates: skip near-duplicates of rejected spots.
        let mut tried: Vec<DVec2> = Vec::new();
        for (_, q) in ranked {
            if tried.len() >= 24 {
                break;
            }
            if tried.iter().any(|t| t.distance(q) < r) {
                continue;
            }
            tried.push(q);
            if fits(to3(q)) {
                pins.push(q);
                break;
            }
        }
    }
    pins.into_iter().map(to3).collect()
}

/// Checks that a hole going `depth` into the piece along `into` keeps a wall
/// of `MIN_WALL` everywhere. `bvh` excludes the cut face itself.
fn hole_fits(bvh: &Bvh, center: DVec3, into: DVec3, r: f64, depth: f64) -> bool {
    let need = (r + MIN_WALL) as f32;
    let steps = ((depth + r) / (r * 0.5).max(0.5)).ceil() as usize + 1;
    for k in 0..=steps {
        let t = (r * 0.5).max(0.3) + (depth + r - (r * 0.5).max(0.3)) * k as f64 / steps as f64;
        let p = (center + into * t).as_vec3();
        if let Some((d, q, _)) = bvh.closest(p, need) {
            if d < need {
                if std::env::var_os("S3D_DEBUG_PINS").is_some() {
                    use std::sync::atomic::{AtomicUsize, Ordering};
                    static N: AtomicUsize = AtomicUsize::new(0);
                    if N.fetch_add(1, Ordering::Relaxed) < 12 {
                        eprintln!("[pin] fail at depth {t:.2}/{:.2}: dist {d:.2} < {need:.2}; centre {center:?} into {into:?} nearest {q:?}", depth + r);
                    }
                }
                return false;
            }
        }
    }
    true
}

/// Hole profile in the hole's local XY: a circle, or a teardrop whose point
/// faces local +Y (set to "up" in print orientation for sideways holes).
fn hole_profile(r: f64, teardrop: bool) -> Vec<DVec2> {
    let n = 28;
    let mut pts: Vec<DVec2> = Vec::with_capacity(n + 1);
    if teardrop {
        // Circle below 45°, then a point at r*sqrt(2) above the centre.
        let start = std::f64::consts::FRAC_PI_4 * 3.0; // 135°
        let end = std::f64::consts::FRAC_PI_4 + std::f64::consts::TAU; // 405°
        for i in 0..=n {
            let a = start + (end - start) * i as f64 / n as f64;
            pts.push(DVec2::new(r * a.cos(), r * a.sin()));
        }
        pts.push(DVec2::new(0.0, r * std::f64::consts::SQRT_2));
    } else {
        for i in 0..n {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            pts.push(DVec2::new(r * a.cos(), r * a.sin()));
        }
    }
    pts
}

/// Solid of one blind hole: starts slightly outside the cut face and goes
/// `depth` into the piece, ending in a 45° cone so it prints without support.
fn hole_solid(center: DVec3, into: DVec3, r: f64, depth: f64, up_print: DVec3) -> Option<Solid> {
    let z = into.normalize();
    // Sideways holes get a teardrop whose tip points up while printing.
    let up_perp = up_print - z * up_print.dot(z);
    let teardrop = up_perp.length() > 0.5;
    let y = if teardrop { up_perp.normalize() } else { z.any_orthonormal_vector() };
    let x = y.cross(z);
    let prof = hole_profile(r, teardrop);
    let lead = 0.6;
    let body = Solid::extrude(&[prof.clone()], depth + lead).ok()?;
    let cone = Solid::extrude_scaled(&[prof], r, 0.0).ok()?.translate(DVec3::new(0.0, 0.0, depth + lead)).ok()?;
    let hole = body.union(&cone).ok()?;
    let m = DMat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), (center - z * lead).extend(1.0));
    hole.transform(&m).ok()
}

/// Per-piece BVHs of the surface without its cut faces, for wall checks.
fn wall_bvhs(p: &Partition, ids: &[PieceId]) -> HashMap<PieceId, Arc<Bvh>> {
    ids.par_iter()
        .filter_map(|id| {
            let q = p.piece(*id)?;
            let planes: Vec<Plane> = p.seams_of(*id).map(|s| s.plane).collect();
            let tris: Vec<[Vec3; 3]> = (0..q.mesh.tris.len())
                .map(|t| q.mesh.tri(t))
                .filter(|t| !planes.iter().any(|pl| t.iter().all(|v| pl.dist(*v).abs() < ON_PLANE * 10.0)))
                .collect();
            Some((*id, Arc::new(Bvh::from_triangles(tris))))
        })
        .collect()
}

/// Pins whose hole would break through a wall in either piece.
fn reject_pins(pins: &[(Seam, Vec<DVec3>)], bvhs: &HashMap<PieceId, Arc<Bvh>>, axes: &HashMap<u32, DVec3>, r: f64, depth: f64) -> Vec<(u32, DVec3)> {
    pins.par_iter()
        .flat_map_iter(|(sm, centres)| {
            let ax = axes.get(&sm.id).copied().unwrap_or(sm.plane.normal);
            centres.iter().filter_map(move |ctr| {
                let ok_a = bvhs.get(&sm.a).is_some_and(|b| hole_fits(b, *ctr, ax, r, depth));
                let ok_b = bvhs.get(&sm.b).is_some_and(|b| hole_fits(b, *ctr, -ax, r, depth));
                (!(ok_a && ok_b)).then_some((sm.id, *ctr))
            })
        })
        .collect()
}

/// Direction of the hole into piece `a` for every seam: along the insertion
/// direction of whichever piece is assembled later, so one straight move seats
/// all of its pins. Falls back to the face normal.
pub fn pin_axes(p: &Partition) -> HashMap<u32, DVec3> {
    let asm = crate::assembly::plan(p);
    let pos: HashMap<PieceId, usize> = asm.order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    p.seams
        .iter()
        .map(|s| {
            let (pa, pb) = (pos.get(&s.a).copied().unwrap_or(0), pos.get(&s.b).copied().unwrap_or(0));
            let later = if pa > pb { s.a } else { s.b };
            let d = asm.insert_dir.get(&later).map(|v| v.as_dvec3().normalize()).unwrap_or(DVec3::ZERO);
            // The later piece moves along d; d points into the earlier piece.
            let into_a = if later == s.b { d } else { -d };
            let ok = into_a.dot(s.plane.normal) >= 0.3;
            (s.id, if ok { into_a } else { s.plane.normal })
        })
        .collect()
}

/// Contact smaller than this gets no pin and no warning (mm²).
pub const MIN_PIN_CONTACT: f64 = 400.0;

/// Adds connectors and labels to the given pieces (`finished` meshes).
pub fn finish(p: &mut Partition, ids: &[PieceId], s: &Settings, job: &JobCtx) -> Result<(), String> {
    let c = &s.connectors;
    job.set_stage("Placing dowel pins");
    // Pins for every seam touching a piece being finished.
    let seams: Vec<Seam> = p.seams.iter().filter(|x| ids.contains(&x.a) || ids.contains(&x.b)).cloned().collect();
    let r = c.hole_radius();
    let depth = c.hole_depth();
    let mut involved: Vec<PieceId> = seams.iter().flat_map(|x| [x.a, x.b]).collect();
    involved.sort_unstable();
    involved.dedup();
    let bvhs = if c.dowels { wall_bvhs(p, &involved) } else { HashMap::new() };
    let axes = pin_axes(p);
    let pins: Vec<(Seam, Vec<DVec3>)> = if c.dowels {
        seams
            .par_iter()
            .filter(|x| x.area >= MIN_PIN_CONTACT)
            .map(|x| {
                let ax = axes.get(&x.id).copied().unwrap_or(x.plane.normal);
                let fits = |ctr: DVec3| bvhs.get(&x.a).is_some_and(|b| hole_fits(b, ctr, ax, r, depth)) && bvhs.get(&x.b).is_some_and(|b| hole_fits(b, ctr, -ax, r, depth));
                (x.clone(), place_pins_checked(p, x, c, &fits))
            })
            .collect()
    } else {
        Vec::new()
    };
    job.check().map_err(|e| e.to_string())?;
    job.set_stage("Drilling holes and adding part numbers");
    // Per-piece hole requests: (centre, direction into the piece).
    let mut requests: HashMap<PieceId, Vec<(DVec3, DVec3, u32)>> = HashMap::new();
    for (sm, centres) in &pins {
        for ctr in centres {
            // `a` is on the positive side: holes go +n into a, -n into b.
            let ax = axes.get(&sm.id).copied().unwrap_or(sm.plane.normal);
            requests.entry(sm.a).or_default().push((*ctr, ax, sm.id));
            requests.entry(sm.b).or_default().push((*ctr, -ax, sm.id));
        }
    }
    // Placement already checked the walls; this only guards against overlap
    // with other pins' holes being missed (kept for safety and stats).
    let rejected = reject_pins(&pins, &bvhs, &axes, r, depth);
    if std::env::var_os("S3D_TIMING").is_some() {
        let total: usize = pins.iter().map(|x| x.1.len()).sum();
        let empty = pins.iter().filter(|x| x.1.is_empty()).count();
        let empty_big = pins.iter().filter(|x| x.1.is_empty() && x.0.area > 1000.0).count();
        eprintln!("[pins] seams {} pins {} rejected {} seams-without-pins {} (big {})", pins.len(), total, rejected.len(), empty, empty_big);
    }
    let skipped_pieces: Vec<PieceId> = pins.iter().filter(|(sm, _)| rejected.iter().any(|r| r.0 == sm.id)).flat_map(|(sm, _)| [sm.a, sm.b]).collect();
    let no_room: Vec<PieceId> = pins.iter().filter(|(sm, c)| c.is_empty() && sm.area >= MIN_PIN_CONTACT).flat_map(|(sm, _)| [sm.a, sm.b]).collect();

    let work: Vec<usize> = (0..p.pieces.len()).filter(|&i| ids.contains(&p.pieces[i].id)).collect();
    let results: Vec<(usize, Result<Mesh, String>)> = work
        .par_iter()
        .map(|&i| {
            let q = &p.pieces[i];
            let up_print = q.orient.map(|o| (o.rotation.inverse() * Vec3::Z).as_dvec3()).unwrap_or(DVec3::Z);
            let mut cutters: Vec<Solid> = Vec::new();
            for (ctr, into, sid) in requests.get(&q.id).map(|v| v.as_slice()).unwrap_or(&[]) {
                if rejected.iter().any(|r| r.0 == *sid && r.1.distance(*ctr) < 1e-6) {
                    continue;
                }
                if let Some(h) = hole_solid(*ctr, *into, r, depth, up_print) {
                    cutters.push(h);
                }
            }
            let mut solid = (*q.solid).clone();
            if !cutters.is_empty() {
                let mut all: Vec<&Solid> = vec![&solid];
                all.extend(cutters.iter());
                solid = match Solid::batch(&all, Op::Subtract) {
                    Ok(s) => s,
                    Err(e) => return (i, Err(format!("piece {}: {e}", q.label))),
                };
            }
            if c.labels != LabelMode::Off {
                let planes: Vec<Plane> = p.seams_of(q.id).map(|s| s.plane).collect();
                if let Some(l) = labels::label_solid(&q.mesh, &planes, q.orient.as_ref(), &q.label.to_string(), c) {
                    let r = match c.labels {
                        LabelMode::Emboss => solid.union(&l),
                        _ => solid.subtract(&l),
                    };
                    if let Ok(s2) = r {
                        solid = s2;
                    }
                }
            }
            if job.is_cancelled() {
                return (i, Err("cancelled".into()));
            }
            (i, Ok(solid.to_mesh()))
        })
        .collect();
    for (i, r) in results {
        let m = r?;
        let q = &mut p.pieces[i];
        q.finished = Some(Arc::new(m));
        q.flags.retain(|f| *f != Flag::SkippedDowel);
        if skipped_pieces.contains(&q.id) || (c.dowels && no_room.contains(&q.id)) {
            q.flags.push(Flag::SkippedDowel);
        }
    }
    for id in ids {
        p.touch(*id);
    }
    Ok(())
}

/// Thickest point of a seam's contact region (twice the max edge distance).
pub fn contact_width(p: &Partition, s: &Seam) -> f64 {
    contact_raster(p, s).map(|c| c.dist.iter().cloned().fold(0.0, f64::max) * 2.0).unwrap_or(0.0)
}

/// Share of pins that fit, and share of sizeable cuts that get a pin.
fn pin_fit_ratio(p: &Partition, bvhs: &HashMap<PieceId, Arc<Bvh>>, cs: &ConnectorSettings) -> (f64, f64) {
    let (r, depth) = (cs.hole_radius(), cs.hole_depth());
    let axes = pin_axes(p);
    let pins: Vec<(Seam, Vec<DVec3>)> = p
        .seams
        .par_iter()
        .filter(|s| s.area >= MIN_PIN_CONTACT)
        .map(|s| {
            let ax = axes.get(&s.id).copied().unwrap_or(s.plane.normal);
            let fits = |ctr: DVec3| bvhs.get(&s.a).is_some_and(|b| hole_fits(b, ctr, ax, r, depth)) && bvhs.get(&s.b).is_some_and(|b| hole_fits(b, ctr, -ax, r, depth));
            (s.clone(), place_pins_checked(p, s, cs, &fits))
        })
        .collect();
    let seams_ok = pins.iter().filter(|(_, c)| !c.is_empty()).count();
    let ratio = seams_ok as f64 / pins.len().max(1) as f64;
    (ratio, ratio)
}

/// Advice when pins had to be skipped.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PinAdvice {
    pub diameter: f32,
    pub length: f32,
    /// Share of sizeable cuts that get at least one pin at this size.
    pub coverage: f32,
}

/// Largest pin size (no larger than the current one) at which at least 60%
/// of sizeable cuts get a working pin; else the size with the best coverage.
pub fn suggest_pin_size(p: &Partition, c: &ConnectorSettings) -> Option<PinAdvice> {
    let ids: Vec<PieceId> = p.pieces.iter().map(|q| q.id).collect();
    let bvhs = wall_bvhs(p, &ids);
    let lengths: Vec<f32> = [c.length, 24.0, 16.0, 12.0, 8.0].into_iter().filter(|l| *l <= c.length).collect();
    let mut best: Option<PinAdvice> = None;
    let mut d = (c.diameter * 2.0).floor() / 2.0;
    while d >= 2.0 {
        for &len in &lengths {
            let mut cs = c.clone();
            cs.diameter = d;
            cs.length = len;
            let (cov, _) = pin_fit_ratio(p, &bvhs, &cs);
            let a = PinAdvice { diameter: d, length: len, coverage: cov as f32 };
            if cov >= 0.6 {
                return Some(a);
            }
            if best.is_none_or(|b| a.coverage > b.coverage + 0.05) {
                best = Some(a);
            }
        }
        d -= 1.0;
    }
    best.filter(|b| b.coverage >= 0.3)
}

/// All pin positions for the current pieces (for drawing and tests).
pub fn all_pins(p: &Partition, c: &ConnectorSettings) -> Vec<Pin> {
    let axes = pin_axes(p);
    p.seams
        .iter()
        .flat_map(|s| {
            let axis = axes.get(&s.id).copied().unwrap_or(s.plane.normal);
            place_pins(p, s, c).into_iter().map(move |center| Pin { seam: s.id, center, axis })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plane::Plane;

    fn settings() -> Settings {
        let mut s = Settings::default();
        s.connectors.labels = LabelMode::Off;
        s
    }

    #[test]
    fn pins_line_up_and_holes_are_drilled() {
        let mut p = Partition::from_solids(vec![Solid::cube(DVec3::new(120.0, 80.0, 80.0), false).unwrap()]);
        p.split_piece(0, Plane::axis(0, 60.0)).unwrap();
        let s = settings();
        let pins = all_pins(&p, &s.connectors);
        assert_eq!(pins.len(), 2);
        for pin in &pins {
            assert!((pin.center.x - 60.0).abs() < 1e-6);
            assert!(pin.center.y > 10.0 && pin.center.y < 70.0);
        }
        let ids: Vec<u32> = p.pieces.iter().map(|q| q.id).collect();
        finish(&mut p, &ids, &s, &JobCtx::new()).unwrap();
        let vol_before = 120.0 * 80.0 * 80.0;
        let vol_after: f64 = p.pieces.iter().map(|q| q.finished.as_ref().unwrap().volume()).sum();
        let r = s.connectors.hole_radius();
        let removed = vol_before - vol_after;
        let expect = 4.0 * std::f64::consts::PI * r * r * s.connectors.hole_depth();
        assert!(removed > expect * 0.8 && removed < expect * 1.5, "removed {removed}, expected ~{expect}");
        for q in &p.pieces {
            let m = q.finished.as_ref().unwrap();
            assert!(crate::repair::edge_stats(m).is_closed_manifold());
            assert!(!q.flags.contains(&Flag::SkippedDowel));
        }
    }

    #[test]
    fn thin_wall_skips_pin() {
        // A thin plate cut across: no room for 6 mm pins in a 5 mm wall.
        let mut p = Partition::from_solids(vec![Solid::cube(DVec3::new(100.0, 100.0, 5.0), false).unwrap()]);
        p.split_piece(0, Plane::axis(0, 50.0)).unwrap();
        let ids: Vec<u32> = p.pieces.iter().map(|q| q.id).collect();
        finish(&mut p, &ids, &settings(), &JobCtx::new()).unwrap();
        assert!(p.pieces.iter().all(|q| q.flags.contains(&Flag::SkippedDowel)));
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;

    #[test]
    fn pins_fit_in_shell_walls() {
        let outer = Solid::cube(DVec3::new(200.0, 200.0, 200.0), true).unwrap();
        let inner = Solid::cube(DVec3::new(182.0, 182.0, 182.0), true).unwrap();
        let shell = outer.subtract(&inner).unwrap();
        let mut p = Partition::from_solids(vec![shell]);
        p.split_piece(0, Plane::axis(2, 10.0)).unwrap();
        let mut s = Settings::default();
        s.connectors.labels = LabelMode::Off;
        s.connectors.diameter = 5.0;
        let seam = p.seams[0].clone();
        eprintln!("seams {} width {:.2}", p.seams.len(), contact_width(&p, &seam));
        let pins = place_pins(&p, &seam, &s.connectors);
        eprintln!("pins {:?}", pins);
        assert!(!pins.is_empty());
        let ids: Vec<u32> = p.pieces.iter().map(|q| q.id).collect();
        finish(&mut p, &ids, &s, &JobCtx::new()).unwrap();
        for q in &p.pieces {
            eprintln!("piece {} flags {:?}", q.id, q.flags);
        }
        assert!(p.pieces.iter().all(|q| !q.flags.contains(&Flag::SkippedDowel)));
    }
}
