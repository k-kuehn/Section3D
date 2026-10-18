//! Assembly order: start from the largest, lowest piece and keep adding a
//! touching piece that can slide straight into place along its pins.

use std::collections::HashMap;

use glam::{DVec3, Vec3};
use serde::{Deserialize, Serialize};

use crate::partition::{Partition, PieceId};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Assembly {
    pub order: Vec<PieceId>,
    /// Direction each piece moves while being inserted (unit).
    pub insert_dir: HashMap<PieceId, Vec3>,
    /// Pieces with no straight insertion path (pins on non-parallel faces).
    pub blocked: Vec<PieceId>,
}

/// Insertion direction of `c` against already placed pieces, if one exists.
fn insertion(p: &Partition, c: PieceId, placed: &[PieceId]) -> (Option<DVec3>, f64) {
    let mut dirs: Vec<(DVec3, f64)> = Vec::new();
    for s in p.seams_of(c) {
        let o = s.other(c);
        if !placed.contains(&o) {
            continue;
        }
        let d = if s.a == c { -s.plane.normal } else { s.plane.normal };
        dirs.push((d, s.area));
    }
    let area: f64 = dirs.iter().map(|d| d.1).sum();
    if dirs.is_empty() {
        return (None, 0.0);
    }
    let first = dirs[0].0;
    if dirs.iter().all(|(d, _)| d.dot(first) > 0.996) {
        let avg = dirs.iter().fold(DVec3::ZERO, |acc, (d, a)| acc + *d * *a).normalize();
        (Some(avg), area)
    } else {
        (None, area)
    }
}

pub fn plan(p: &Partition) -> Assembly {
    let mut a = Assembly::default();
    if p.pieces.is_empty() {
        return a;
    }
    let zmin = p.pieces.iter().map(|q| q.bbox.min.z).fold(f32::INFINITY, f32::min);
    let height = p.pieces.iter().map(|q| q.bbox.max.z).fold(f32::NEG_INFINITY, f32::max) - zmin;
    let tol = (height * 0.02).max(1.0);
    let start = p
        .pieces
        .iter()
        .filter(|q| q.bbox.min.z <= zmin + tol)
        .max_by(|x, y| x.volume.total_cmp(&y.volume))
        .or_else(|| p.pieces.iter().max_by(|x, y| x.volume.total_cmp(&y.volume)))
        .unwrap()
        .id;
    a.order.push(start);
    a.insert_dir.insert(start, -Vec3::Z);
    let mut left: Vec<PieceId> = p.pieces.iter().map(|q| q.id).filter(|id| *id != start).collect();
    while !left.is_empty() {
        // Feasible candidates first, then most contact, then lowest.
        let mut best: Option<(bool, f64, f32, usize, Option<DVec3>)> = None;
        for (i, &c) in left.iter().enumerate() {
            let (dir, area) = insertion(p, c, &a.order);
            if area <= 0.0 {
                continue;
            }
            let z = p.piece(c).map(|q| q.bbox.min.z).unwrap_or(0.0);
            let key = (dir.is_some(), area, -z);
            let better = match &best {
                None => true,
                Some(b) => (key.0, key.1, key.2).partial_cmp(&(b.0, b.1, b.2)) == Some(std::cmp::Ordering::Greater),
            };
            if better {
                best = Some((key.0, key.1, key.2, i, dir));
            }
        }
        let (i, dir) = match best {
            Some(b) => (b.3, b.4),
            // Disconnected island: continue with its lowest piece.
            None => {
                let i = (0..left.len()).min_by(|&x, &y| p.piece(left[x]).unwrap().bbox.min.z.total_cmp(&p.piece(left[y]).unwrap().bbox.min.z)).unwrap();
                (i, Some(-DVec3::Z))
            }
        };
        let c = left.swap_remove(i);
        match dir {
            Some(d) => {
                a.insert_dir.insert(c, d.as_vec3());
            }
            None => {
                a.blocked.push(c);
                a.insert_dir.insert(c, -Vec3::Z);
            }
        }
        a.order.push(c);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::Solid;
    use crate::plane::Plane;

    #[test]
    fn stack_assembles_bottom_up() {
        let mut p = Partition::from_solids(vec![Solid::cube(DVec3::splat(90.0), false).unwrap()]);
        p.split_piece(0, Plane::axis(2, 30.0)).unwrap();
        let top = p.pieces.iter().find(|q| q.centroid.z > 30.0).unwrap().id;
        p.split_piece(top, Plane::axis(2, 60.0)).unwrap();
        let a = plan(&p);
        assert_eq!(a.order.len(), 3);
        let z: Vec<f32> = a.order.iter().map(|id| p.piece(*id).unwrap().centroid.z).collect();
        assert!(z[0] < z[1] && z[1] < z[2], "{z:?}");
        assert!(a.blocked.is_empty());
    }
}
