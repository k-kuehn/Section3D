//! Grid mode: box cells sized to the printer.


use crate::bsp::Plan;
use crate::mesh::Aabb;
use crate::plane::Plane;
use crate::zones::Zones;

/// Evenly spaced cut positions along one axis so that no cell exceeds `cell`.
fn positions(lo: f32, hi: f32, cell: f32) -> Vec<f64> {
    let ext = (hi - lo) as f64;
    let n = (ext / cell as f64 - 1e-6).ceil().max(1.0) as usize;
    (1..n).map(|k| lo as f64 + ext * k as f64 / n as f64).collect()
}

/// Moves each cut to avoid painted zones, staying within `slack` of its spot.
fn nudge(axis: usize, pos: &mut [f64], slack: f64, zones: &Zones) {
    if zones.is_empty() {
        return;
    }
    for p in pos.iter_mut() {
        let base = *p;
        let mut best = (zones.plane_cost(&Plane::axis(axis, base)), 0.0f64);
        if best.0 == 0.0 {
            continue;
        }
        for k in 1..=24 {
            for sgn in [-1.0, 1.0] {
                let d = sgn * slack * k as f64 / 24.0;
                let c = zones.plane_cost(&Plane::axis(axis, base + d));
                if c < best.0 - 1e-6 || (c <= best.0 + 1e-6 && d.abs() < best.1.abs()) {
                    best = (c, d);
                }
            }
        }
        *p = base + best.1;
    }
}

/// Builds the grid plan. `cell` is the largest allowed cell size per axis.
pub fn plan(bb: &Aabb, cell: [f32; 3], zones: &Zones) -> Plan {
    let mut axes: Vec<Vec<f64>> = (0..3)
        .map(|a| {
            // Shrink cells a little so nudged cuts still keep pieces within the bed.
            let c = cell[a] * 0.92;
            let mut p = positions(bb.min[a], bb.max[a], c);
            nudge(a, &mut p, (cell[a] * 0.08) as f64 * 0.95, zones);
            p
        })
        .collect();
    let z = std::mem::take(&mut axes[2]);
    let y = std::mem::take(&mut axes[1]);
    let x = std::mem::take(&mut axes[0]);
    let chain = |axis: usize, at: &[f64], child: &dyn Fn() -> Plan| {
        if at.is_empty() {
            return child();
        }
        let planes: Vec<Plane> = at.iter().map(|&v| Plane::axis(axis, v)).collect();
        let children = (0..=at.len()).map(|_| child()).collect();
        Plan::slabs(planes, children)
    };
    chain(0, &x, &|| chain(1, &y, &|| chain(2, &z, &|| Plan::Leaf)))
}

