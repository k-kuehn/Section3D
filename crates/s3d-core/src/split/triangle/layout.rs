//! Triangle lattice layouts: rows of alternating / and \ cuts.
//!
//! Coordinates are local to a frame (u, v, w): rows stack along v (the base
//! cuts are planes of constant v), side cuts are slanted lines in the (u, v)
//! plane extruded along w, and optional depth cuts are planes of constant w.

use glam::{DVec3, Vec3};

use crate::bsp::Plan;
use crate::plane::Plane;

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub u: DVec3,
    pub v: DVec3,
    pub w: DVec3,
}

impl Frame {
    /// Right-handed frame from a row direction `v` and a depth direction hint.
    pub fn new(v: DVec3, w_hint: DVec3) -> Frame {
        let v = v.normalize();
        let mut w = w_hint - v * w_hint.dot(v);
        if w.length() < 1e-6 {
            w = v.any_orthonormal_vector();
        }
        let w = w.normalize();
        let u = v.cross(w);
        Frame { u, v, w }
    }

    #[inline]
    pub fn local(&self, p: Vec3) -> DVec3 {
        let p = p.as_dvec3();
        DVec3::new(p.dot(self.u), p.dot(self.v), p.dot(self.w))
    }

    #[inline]
    pub fn local_dir(&self, d: Vec3) -> DVec3 {
        self.local(d)
    }

    #[inline]
    pub fn world(&self, l: DVec3) -> DVec3 {
        self.u * l.x + self.v * l.y + self.w * l.z
    }
}

/// A side cut within one row: u-intercepts at the row bottom and top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub xb: f64,
    pub xt: f64,
}

impl Line {
    /// u position of the line at height `t` in [0, 1] across the row.
    #[inline]
    pub fn at(&self, t: f64) -> f64 {
        self.xb + (self.xt - self.xb) * t
    }
    pub fn min(&self) -> f64 {
        self.xb.min(self.xt)
    }
    pub fn max(&self) -> f64 {
        self.xb.max(self.xt)
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub v0: f64,
    pub v1: f64,
    /// Ordered left to right; non-crossing inside the row.
    pub lines: Vec<Line>,
    /// Depth cut positions (w), ascending; empty for none.
    pub depth: Vec<f64>,
}

impl Row {
    pub fn cells(&self) -> usize {
        self.lines.len() + 1
    }
    pub fn depth_cells(&self) -> usize {
        self.depth.len() + 1
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub frame: Frame,
    pub rows: Vec<Row>,
    /// Nominal triangle base length and row height.
    pub l: f64,
    pub h: f64,
    /// Local bounds of the model: [min, max].
    pub lo: DVec3,
    pub hi: DVec3,
}

/// Parameters of a regular lattice.
#[derive(Clone, Copy, Debug)]
pub struct LatticeParams {
    pub l: f64,
    /// Side angle from the base (radians).
    pub theta: f64,
    /// Fractions in [0, 1) shifting rows along v and triangles along u.
    pub phase_v: f64,
    pub phase_u: f64,
    /// Depth layer thickness and phase (None = no depth cuts).
    pub depth: Option<(f64, f64)>,
}

impl Layout {
    pub fn lattice(frame: Frame, lo: DVec3, hi: DVec3, p: &LatticeParams) -> Layout {
        let l = p.l;
        let h = (l * p.theta.tan() * 0.5).max(1.0);
        let mut rows = Vec::new();
        let mut v0 = lo.y - p.phase_v * h;
        // Avoid a sliver row at the bottom: start at the model's base when the
        // phase would leave less than a quarter row below it.
        if lo.y - v0 < 0.25 * h && p.phase_v > 0.0 {
            v0 = lo.y;
        }
        let mut k = 0;
        while v0 < hi.y - 1e-6 {
            let v1 = v0 + h;
            // All rows share the same u origin: row k+1's base vertices then
            // sit halfway along row k's top bases (brick stagger).
            let o = lo.x - l - p.phase_u * l;
            let mut lines = Vec::new();
            let mut j = 0;
            loop {
                let x = o + j as f64 * l;
                if x > hi.x + l {
                    break;
                }
                lines.push(Line { xb: x, xt: x + 0.5 * l });
                lines.push(Line { xb: x + l, xt: x + 0.5 * l });
                j += 1;
            }
            let depth = match p.depth {
                Some((d, ph)) if d > 0.0 && hi.z - lo.z > d => {
                    // Stagger depth cuts by half a layer on alternate rows.
                    let shift = if k % 2 == 1 { 0.5 } else { 0.0 };
                    let mut z = lo.z - ((ph + shift) % 1.0) * d;
                    let mut out = Vec::new();
                    z += d;
                    while z < hi.z - 1e-6 {
                        if z > lo.z + 1e-6 {
                            out.push(z);
                        }
                        z += d;
                    }
                    out
                }
                _ => Vec::new(),
            };
            rows.push(Row { v0, v1, lines, depth });
            v0 = v1;
            k += 1;
        }
        Layout { frame, rows, l, h, lo, hi }
    }

    /// Row index containing local height `v` (clamped).
    pub fn row_at(&self, v: f64) -> usize {
        let i = self.rows.partition_point(|r| r.v1 <= v);
        i.min(self.rows.len().saturating_sub(1))
    }

    /// Side cut as a 3D plane with its normal towards +u.
    pub fn line_plane(&self, row: &Row, line: &Line) -> Plane {
        let f = &self.frame;
        let dir = f.u * (line.xt - line.xb) + f.v * (row.v1 - row.v0);
        let mut n = dir.cross(f.w).normalize();
        if n.dot(f.u) < 0.0 {
            n = -n;
        }
        let p = f.u * line.xb + f.v * row.v0;
        Plane { normal: n, offset: n.dot(p) }
    }

    pub fn base_plane(&self, v: f64) -> Plane {
        Plane { normal: self.frame.v, offset: v }
    }

    pub fn depth_plane(&self, w: f64) -> Plane {
        Plane { normal: self.frame.w, offset: w }
    }

    /// The BSP cut plan for this layout.
    pub fn plan(&self) -> Plan {
        if self.rows.is_empty() {
            return Plan::Leaf;
        }
        let base: Vec<Plane> = self.rows[1..].iter().map(|r| self.base_plane(r.v0)).collect();
        let rows: Vec<Plan> = self
            .rows
            .iter()
            .map(|r| {
                let sides: Vec<Plane> = r.lines.iter().map(|l| self.line_plane(r, l)).collect();
                let cells: Vec<Plan> = (0..=r.lines.len())
                    .map(|_| {
                        if r.depth.is_empty() {
                            Plan::Leaf
                        } else {
                            let d: Vec<Plane> = r.depth.iter().map(|w| self.depth_plane(*w)).collect();
                            let n = d.len() + 1;
                            Plan::slabs(d, (0..n).map(|_| Plan::Leaf).collect())
                        }
                    })
                    .collect();
                Plan::slabs(sides, cells)
            })
            .collect();
        Plan::slabs(base, rows)
    }

    /// True if a cell's bottom edge (at v0) is wider than its top: the base
    /// is at the bottom (point-up triangle).
    pub fn cell_points_up(&self, row: &Row, cell: usize) -> bool {
        let (l, r) = self.cell_lines(row, cell);
        let bottom = r.xb - l.xb;
        let top = r.xt - l.xt;
        bottom >= top
    }

    /// Left and right lines of a cell (virtual far lines at the ends).
    pub fn cell_lines(&self, row: &Row, cell: usize) -> (Line, Line) {
        let far = 1e7;
        let left = if cell == 0 { Line { xb: -far, xt: -far } } else { row.lines[cell - 1] };
        let right = if cell >= row.lines.len() { Line { xb: far, xt: far } } else { row.lines[cell] };
        (left, right)
    }

    /// w bounds of depth cell `d` in a row (model bounds at the ends).
    pub fn depth_bounds(&self, row: &Row, d: usize) -> (f64, f64) {
        let lo = if d == 0 { self.lo.z - 1.0 } else { row.depth[d - 1] };
        let hi = if d >= row.depth.len() { self.hi.z + 1.0 } else { row.depth[d] };
        (lo, hi)
    }

    pub fn cell_count(&self) -> usize {
        self.rows.iter().map(|r| r.cells() * r.depth_cells()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_stagger() {
        let f = Frame::new(DVec3::Z, DVec3::Y);
        let lo = DVec3::new(-200.0, 0.0, -50.0);
        let hi = DVec3::new(200.0, 300.0, 50.0);
        let lay = Layout::lattice(f, lo, hi, &LatticeParams { l: 200.0, theta: 45f64.to_radians(), phase_v: 0.0, phase_u: 0.0, depth: None });
        assert_eq!(lay.rows.len(), 3);
        assert!((lay.h - 100.0).abs() < 1e-9);
        let r = &lay.rows[0];
        // Lines are ordered and alternate / \.
        for w in r.lines.windows(2) {
            assert!(w[0].xb <= w[1].xb + 1e-9 && w[0].xt <= w[1].xt + 1e-9);
        }
        // Base vertices of the next row sit at the middle of this row's top bases.
        let tops: Vec<f64> = r.lines.iter().map(|l| l.xt).collect();
        let next_bottoms: Vec<f64> = lay.rows[1].lines.iter().map(|l| l.xb).collect();
        for b in &next_bottoms {
            assert!(tops.iter().all(|t| (t - b).abs() > 1.0), "diagonal seams continue at {b}");
        }
        let plan = lay.plan();
        assert!(plan.leaf_count() >= 3 * 4);
    }
}
