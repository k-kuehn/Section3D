//! Assembly plan: small sections first, then sections to sections.
//!
//! Pieces are grouped bottom-up. Each step joins the two touching sections
//! with the longest shared seam for their size, so early steps join single
//! pieces into small panels and later steps join panels into larger ones.
//! Sections are kept fairly flat while they are small: a flat panel lies on
//! the bench with its inside facing up, where the seam can be heat-stapled
//! (or glued) easily. Steps run from the smallest result to the largest, so
//! every section is finished before it is used.

use std::collections::HashMap;

use glam::{DMat3, DVec3, Vec3};
use serde::{Deserialize, Serialize};

use crate::partition::{Partition, PieceId, SeamId};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Assembly {
    /// Pieces in the order they are first used (bed order, numbering).
    pub order: Vec<PieceId>,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    /// Name of the section this step makes ("S4").
    pub name: String,
    /// The two parts joined: a piece ("#12") or an earlier section ("S2").
    pub a_name: String,
    pub b_name: String,
    pub a: Vec<PieceId>,
    pub b: Vec<PieceId>,
    /// Seams closed in this step and their total length on the surface (mm).
    pub seams: Vec<SeamId>,
    pub length: f32,
    /// Direction from `a` to `b` across the joint (for exploded views).
    pub apart: Vec3,
}

impl Assembly {
}

struct Sec {
    pieces: Vec<PieceId>,
    name: String,
    /// Area-weighted normal tensor of the surface (sign-free).
    normals: DMat3,
}

/// Flatness loss of a section: 0 for a flat panel, up to 2/3 when its
/// surface faces every way.
fn spread(t: &DMat3) -> f64 {
    let tr = t.x_axis.x + t.y_axis.y + t.z_axis.z;
    if tr <= 0.0 {
        return 0.0;
    }
    let (vals, _) = crate::split::refine::sym_eigen(*t);
    1.0 - vals[0] / tr
}

fn normal_tensor(p: &Partition, id: PieceId) -> DMat3 {
    let Some(q) = p.piece(id) else { return DMat3::ZERO };
    let m = &q.mesh;
    let mut t = DMat3::ZERO;
    for i in 0..m.tris.len() {
        let c = m.tri_cross(i).as_dvec3();
        let a = c.length();
        if a < 1e-12 {
            continue;
        }
        let n = c / a;
        t += DMat3::from_cols(n * n.x, n * n.y, n * n.z) * (a * 0.5);
    }
    t
}

/// Length of a seam along the surface: the longer extent of its contact.
fn seam_length(p: &Partition, sid: SeamId) -> f64 {
    let pts = crate::split::quality::seam_points(p, sid);
    let Some(s) = p.seam(sid) else { return 0.0 };
    let l = crate::split::quality::run_length(&pts, &s.plane);
    if l > 0.0 { l } else { s.area.sqrt() }
}

pub fn plan(p: &Partition) -> Assembly {
    let mut a = Assembly::default();
    if p.pieces.is_empty() {
        return a;
    }
    let lens: HashMap<SeamId, f64> = {
        use rayon::prelude::*;
        p.seams.par_iter().map(|s| (s.id, seam_length(p, s.id))).collect()
    };
    let mut secs: Vec<Option<Sec>> = p
        .pieces
        .iter()
        .map(|q| Some(Sec { pieces: vec![q.id], name: format!("#{}", q.label), normals: normal_tensor(p, q.id) }))
        .collect();
    let index: HashMap<PieceId, usize> = p.pieces.iter().enumerate().map(|(i, q)| (q.id, i)).collect();
    // Joints between sections: (section a, section b) -> seams.
    let mut joints: HashMap<(usize, usize), Vec<SeamId>> = HashMap::new();
    for s in &p.seams {
        let (Some(&i), Some(&j)) = (index.get(&s.a), index.get(&s.b)) else { continue };
        if i != j {
            joints.entry((i.min(j), i.max(j))).or_default().push(s.id);
        }
    }
    let mut made: Vec<(usize, Step)> = Vec::new();
    loop {
        // Best joint: long seams, small sections, and while small, flat.
        let mut best: Option<(f64, (usize, usize))> = None;
        for (&(i, j), seams) in &joints {
            let (Some(si), Some(sj)) = (&secs[i], &secs[j]) else { continue };
            let len: f64 = seams.iter().map(|s| lens.get(s).copied().unwrap_or(0.0)).sum();
            let n = (si.pieces.len() + sj.pieces.len()) as f64;
            let flat = 1.0 - spread(&(si.normals + sj.normals)) * 1.5;
            // Flatness matters for small sections; big ones join anyway.
            let w = if n <= 12.0 { flat.max(0.05) } else { flat.max(0.5) };
            let score = (len + 1.0) * w / n;
            if best.is_none_or(|b| score > b.0 + 1e-9 || ((score - b.0).abs() <= 1e-9 && (i, j) < b.1)) {
                best = Some((score, (i, j)));
            }
        }
        let Some((_, (i, j))) = best else { break };
        let seams = joints.remove(&(i, j)).unwrap_or_default();
        let (si, sj) = (secs[i].take().unwrap(), secs[j].take().unwrap());
        // Name the new section; it takes slot i.
        let name = format!("S{}", made.len() + 1);
        let length: f64 = seams.iter().map(|s| lens.get(s).copied().unwrap_or(0.0)).sum();
        let mut apart = DVec3::ZERO;
        for sid in &seams {
            if let Some(s) = p.seam(*sid) {
                // `s.a` lies on the positive side of the plane.
                let a_in_i = si.pieces.contains(&s.a);
                apart += if a_in_i { -s.plane.normal } else { s.plane.normal } * s.area;
            }
        }
        let apart = apart.normalize_or(DVec3::Z).as_vec3();
        made.push((si.pieces.len() + sj.pieces.len(), Step { name: name.clone(), a_name: si.name.clone(), b_name: sj.name.clone(), a: si.pieces.clone(), b: sj.pieces.clone(), seams, length: length as f32, apart }));
        let mut pieces = si.pieces;
        pieces.extend(sj.pieces);
        secs[i] = Some(Sec { pieces, name, normals: si.normals + sj.normals });
        // Re-key joints that touched j.
        let moved: Vec<((usize, usize), Vec<SeamId>)> = joints.iter().filter(|(k, _)| k.0 == j || k.1 == j || k.0 == i || k.1 == i).map(|(k, v)| (*k, v.clone())).collect();
        for (k, v) in moved {
            joints.remove(&k);
            let other = if k.0 == i || k.0 == j { k.1 } else { k.0 };
            if other == i || other == j {
                continue;
            }
            joints.entry((i.min(other), i.max(other))).or_default().extend(v);
        }
    }
    // Smallest results first; a section is always smaller than its parent.
    made.sort_by_key(|m| m.0);
    a.steps = made.into_iter().map(|m| m.1).collect();
    for s in &a.steps {
        for id in s.a.iter().chain(&s.b) {
            if !a.order.contains(id) {
                a.order.push(*id);
            }
        }
    }
    for q in &p.pieces {
        if !a.order.contains(&q.id) {
            a.order.push(q.id);
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::Solid;
    use crate::plane::Plane;

    #[test]
    fn panels_join_before_rows() {
        // A 4 x 2 grid of plates: pairs first, then larger sections.
        let mut p = Partition::from_solids(vec![Solid::cube(DVec3::new(400.0, 200.0, 10.0), false).unwrap()]);
        for x in [100.0, 200.0, 300.0] {
            let ids: Vec<PieceId> = p.pieces.iter().map(|q| q.id).collect();
            for id in ids {
                p.split_piece(id, Plane::axis(0, x)).unwrap();
            }
        }
        let ids: Vec<PieceId> = p.pieces.iter().map(|q| q.id).collect();
        for id in ids {
            p.split_piece(id, Plane::axis(1, 100.0)).unwrap();
        }
        assert_eq!(p.pieces.len(), 8);
        let a = plan(&p);
        assert_eq!(a.steps.len(), 7);
        assert_eq!(a.order.len(), 8);
        let sizes: Vec<usize> = a.steps.iter().map(|s| s.a.len() + s.b.len()).collect();
        assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "{sizes:?}");
        assert_eq!(*sizes.last().unwrap(), 8);
        // Every step joins parts that touch.
        for s in &a.steps {
            assert!(!s.seams.is_empty());
            assert!(s.length > 0.0);
        }
        // The first steps join single pieces.
        assert_eq!(sizes[0], 2);
    }
}
