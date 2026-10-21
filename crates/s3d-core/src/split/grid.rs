//! Grid mode: the model cut by planes square to X, Y and Z, evenly spaced so
//! every cell fits the piece size, or wherever the user placed them. The
//! pieces are only checked afterwards, never changed.

use crate::bsp::Plan;
use crate::mesh::Aabb;
use crate::plane::Plane;
use crate::settings::Settings;

/// Largest cell along each axis: the piece size, capped by the printer.
fn cell(s: &Settings) -> [f32; 3] {
    let bed = s.printer.usable();
    [0, 1, 2].map(|a| s.split.target_size.map_or(bed[a], |t| t.min(bed[a])))
}

/// `n` cuts spread evenly between `lo` and `hi`.
pub fn even(lo: f32, hi: f32, n: usize) -> Vec<f32> {
    (1..=n).map(|k| lo + (hi - lo) * k as f32 / (n + 1) as f32).collect()
}

/// Fewest cuts per axis so that no cell is longer than the piece size.
pub fn default_counts(bb: &Aabb, s: &Settings) -> [usize; 3] {
    let c = cell(s);
    [0, 1, 2].map(|a| (((bb.max[a] - bb.min[a]) / c[a] - 1e-4).ceil().max(1.0) as usize) - 1)
}

/// Cut positions along X, Y and Z: the user's, or evenly spaced.
pub fn cuts(bb: &Aabb, s: &Settings) -> [Vec<f32>; 3] {
    match &s.split.grid {
        Some(g) => g.clone(),
        None => {
            let n = default_counts(bb, s);
            [0, 1, 2].map(|a| even(bb.min[a], bb.max[a], n[a]))
        }
    }
}

/// The cutting plan: slabs along X, each cut into rows along Y, each into
/// blocks along Z. Cuts outside the model or closer than 0.5 mm to another
/// are skipped.
pub fn plan(bb: &Aabb, cuts: &[Vec<f32>; 3]) -> Plan {
    let at: Vec<Vec<f64>> = (0..3)
        .map(|a| {
            let mut v: Vec<f64> = cuts[a].iter().filter(|x| **x > bb.min[a] + 0.5 && **x < bb.max[a] - 0.5).map(|x| *x as f64).collect();
            v.sort_by(f64::total_cmp);
            v.dedup_by(|x, y| (*x - *y).abs() < 0.5);
            v
        })
        .collect();
    let chain = |axis: usize, child: &dyn Fn() -> Plan| {
        if at[axis].is_empty() {
            return child();
        }
        let planes: Vec<Plane> = at[axis].iter().map(|&v| Plane::axis(axis, v)).collect();
        let children = (0..=planes.len()).map(|_| child()).collect();
        Plan::slabs(planes, children)
    };
    chain(0, &|| chain(1, &|| chain(2, &|| Plan::Leaf)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn even_cells_fit_and_user_cuts_are_kept() {
        let bb = Aabb { min: Vec3::ZERO, max: Vec3::new(500.0, 200.0, 100.0) };
        let mut s = Settings::default();
        s.printer.volume = [210.0, 210.0, 210.0];
        // 500 mm in cells of at most 200 mm: 3 cells, 2 cuts.
        let c = cuts(&bb, &s);
        assert_eq!(c[0].len(), 2);
        assert!((c[0][0] - 166.67).abs() < 0.1, "{c:?}");
        assert!(c[1].is_empty() && c[2].is_empty(), "{c:?}");
        assert_eq!(plan(&bb, &c).leaf_count(), 3);
        s.split.grid = Some([vec![300.0, 100.0], vec![50.0], vec![]]);
        assert_eq!(cuts(&bb, &s)[0], vec![300.0, 100.0]);
        assert_eq!(plan(&bb, &cuts(&bb, &s)).leaf_count(), 6);
    }
}
