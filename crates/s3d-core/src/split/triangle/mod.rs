//! Triangle mode: brick-staggered triangular pieces that print without
//! supports.
//!
//! The search runs on a simplified proxy: a coarse sweep over frames, piece
//! sizes and phases, then local moves of single cuts (shift and tilt) and row
//! heights. The best layout so far is streamed to a preview callback; the
//! final cuts are applied to the full mesh once by the caller.

pub mod eval;
pub mod layout;

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use self::eval::{EvalParams, FrameData, Proxy, Score, bucket_rows, eval_layout, eval_row};
use self::layout::{Frame, LatticeParams, Layout, Line};
use crate::bsp::Plan;
use crate::job::JobCtx;
use crate::mesh::Mesh;
use crate::settings::Settings;
use crate::zones::Zones;

/// Cut lines on the model surface, for showing the layout while searching.
pub type PreviewFn = Arc<dyn Fn(Vec<[Vec3; 2]>, String) + Send + Sync>;

/// Candidate frames: world up, the model's principal axes and its largest
/// flat area, each with rows along the frame's v axis.
fn frames(p: &Proxy) -> Vec<Frame> {
    let axes = super::fix::pca_axes(&p.mesh);
    // Horizontal principal directions (for upright models).
    let horiz: Vec<DVec3> = axes.iter().map(|a| DVec3::new(a.x, a.y, 0.0)).filter(|a| a.length() > 0.3).map(|a| a.normalize()).collect();
    let mut out: Vec<Frame> = Vec::new();
    let ext = |d: DVec3| {
        let (lo, hi) = p.mesh.verts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            let x = v.as_dvec3().dot(d);
            (lo.min(x), hi.max(x))
        });
        hi - lo
    };
    // World up; depth along the thinner horizontal axis first.
    let mut hs = horiz.clone();
    hs.sort_by(|a, b| ext(*a).total_cmp(&ext(*b)));
    if let Some(&thin) = hs.first() {
        out.push(Frame::new(DVec3::Z, thin));
        if let Some(&wide) = hs.last() {
            out.push(Frame::new(DVec3::Z, wide));
        }
    } else {
        out.push(Frame::new(DVec3::Z, DVec3::Y));
    }
    // Principal axes as row direction, depth along the thinnest remaining axis.
    for (i, v) in axes.iter().enumerate() {
        let others: Vec<DVec3> = axes.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, a)| *a).collect();
        let w = if ext(others[0]) < ext(others[1]) { others[0] } else { others[1] };
        out.push(Frame::new(*v, w));
    }
    // Largest flat area: rows parallel to it, so it can become a base.
    let mut groups: Vec<(DVec3, f64)> = Vec::new();
    for (t, n) in p.normals.iter().enumerate() {
        let n = n.as_dvec3();
        if n.length_squared() < 0.5 {
            continue;
        }
        let a = p.area[t] as f64;
        match groups.iter_mut().find(|g| g.0.dot(n) > 0.995) {
            Some(g) => g.1 += a,
            None => {
                if groups.len() < 512 {
                    groups.push((n, a))
                }
            }
        }
    }
    if let Some(g) = groups.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
        let total: f64 = p.area.iter().map(|a| *a as f64).sum();
        if g.1 > total * 0.08 {
            out.push(Frame::new(g.0, g.0.any_orthonormal_vector()));
        }
    }
    // Drop near duplicates (same v axis up to sign and same w).
    let mut uniq: Vec<Frame> = Vec::new();
    for f in out {
        if !uniq.iter().any(|u| u.v.dot(f.v).abs() > 0.985 && u.w.dot(f.w).abs() > 0.985) {
            uniq.push(f);
        }
    }
    uniq.truncate(6);
    uniq
}

pub struct SearchResult {
    pub layout: Option<Layout>,
    pub score: Score,
}

fn edges_of(rows: &[eval::RowEval]) -> Vec<[Vec3; 2]> {
    rows.iter().flat_map(|r| r.edges.iter().copied()).collect()
}

/// Runs the search and returns the best layout found.
pub fn search(mesh: &Mesh, s: &Settings, zones: &Zones, job: &JobCtx, preview: Option<&PreviewFn>) -> SearchResult {
    let started = Instant::now();
    let limit = Duration::from_secs_f32(s.split.time_limit.max(1.0));
    let usable = s.printer.usable();
    let bed = [usable[0] as f64, usable[1] as f64, usable[2] as f64];
    let theta = (s.split.side_angle as f64).clamp(30.0, 70.0).to_radians();
    let ep = EvalParams { bed, sin_overhang: (s.split.overhang as f64).to_radians().sin(), min_piece: s.split.min_piece as f64 };

    job.set_stage("Triangle search: preparing model");
    let coarse = Proxy::new(mesh, 12_000, zones, job);
    let fine = Proxy::new(mesh, 60_000, zones, job);
    if job.is_cancelled() {
        return SearchResult { layout: None, score: Score::default() };
    }
    let frames = frames(&fine);
    let coarse_fd: Vec<FrameData> = frames.par_iter().map(|f| FrameData::new(&coarse, *f)).collect();

    // Piece sizes: from the target (or the largest that fits) downwards.
    let (bx, by, bz) = (bed[0].max(bed[1]), bed[0].min(bed[1]), bed[2]);
    let l_cap = match s.split.target_size {
        Some(t) => t as f64,
        None => (bx * bx + by * by).sqrt() * 0.95,
    };
    // Base lengths for a side angle: the largest that keeps the height on the bed.
    let sizes_for = |th: f64| -> Vec<f64> {
        let l_max = l_cap.min(2.0 * bz / th.tan());
        let mut ls: Vec<f64> = vec![l_max, l_max.min(bx), l_max.min(bx) * 0.85, l_max.min(bx) * 0.7];
        ls.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        ls
    };
    let depths: Vec<Option<f64>> = vec![None, Some(by), Some(by * 0.8)];
    let phases = [0.0, 0.25, 0.5, 0.75];
    // Side angles from the setting up to ~11° steeper (height 0.5-0.75 of
    // the base), which allows bigger triangles under the same bed height.
    let thetas: Vec<f64> = [0.0f64, 6.0, 11.0].iter().map(|d| (theta.to_degrees() + d).min(62.0).to_radians()).collect();

    // Coarse sweep, stage 1: frames x angles x sizes x depth at zero phase.
    job.set_stage("Triangle search: trying layouts");
    let score_all = |cands: &[(usize, LatticeParams)]| -> Vec<(Score, usize, LatticeParams)> {
        let mut v: Vec<(Score, usize, LatticeParams)> = cands
            .par_iter()
            .filter_map(|(fi, lp)| {
                if job.is_cancelled() {
                    return None;
                }
                let fd = &coarse_fd[*fi];
                let lay = Layout::lattice(fd.frame, fd.lo, fd.hi, lp);
                if lay.cell_count() > 20_000 {
                    return None;
                }
                let (sc, _) = eval_layout(&coarse, fd, &lay, &ep, false);
                Some((sc, *fi, *lp))
            })
            .collect();
        v.sort_by(|a, b| if a.0.better_than(&b.0) { std::cmp::Ordering::Less } else if b.0.better_than(&a.0) { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Equal });
        v
    };
    let mut stage1: Vec<(usize, LatticeParams)> = Vec::new();
    for fi in 0..frames.len() {
        for &th in &thetas {
            for l in sizes_for(th) {
                for &depth in &depths {
                    stage1.push((fi, LatticeParams { l, theta: th, phase_v: 0.0, phase_u: 0.0, depth: depth.map(|d| (d, 0.0)) }));
                }
            }
        }
    }
    let r1 = score_all(&stage1);
    if r1.is_empty() || job.is_cancelled() {
        return SearchResult { layout: None, score: Score::default() };
    }
    // Stage 2: phases for the best configurations.
    let mut stage2: Vec<(usize, LatticeParams)> = Vec::new();
    for (_, fi, lp) in r1.iter().take(10) {
        for &pv in &phases {
            for &pu in &[0.0, 0.25, 0.5, 0.75] {
                stage2.push((*fi, LatticeParams { phase_v: pv, phase_u: pu, ..*lp }));
            }
        }
    }
    let mut ranked = score_all(&stage2);
    if ranked.is_empty() || job.is_cancelled() {
        return SearchResult { layout: None, score: Score::default() };
    }
    ranked.truncate(12);
    job.set_progress(0.15);

    // Re-score the best few on the fine proxy and keep the best.
    let fine_fd: Vec<FrameData> = frames.par_iter().map(|f| FrameData::new(&fine, *f)).collect();
    let top: Vec<(Score, Layout, usize)> = ranked
        .iter()
        .take(6)
        .collect::<Vec<_>>()
        .par_iter()
        .map(|(_, fi, lp)| {
            let fd = &fine_fd[*fi];
            let lay = Layout::lattice(fd.frame, fd.lo, fd.hi, lp);
            let (sc, _) = eval_layout(&fine, fd, &lay, &ep, false);
            (sc, lay, *fi)
        })
        .collect();
    let (mut best_score, mut best, mut best_fi) = top.into_iter().reduce(|a, b| if b.0.better_than(&a.0) { b } else { a }).unwrap();
    let report = |lay: &Layout, fi: usize, sc: &Score| {
        if let Some(pf) = preview {
            let (_, rows) = eval_layout(&fine, &fine_fd[fi], lay, &ep, true);
            pf(edges_of(&rows), describe(sc));
        }
    };
    report(&best, best_fi, &best_score);

    // Local refinement until the time limit or everything prints cleanly.
    job.set_stage("Triangle search: refining cuts");
    let fd = &fine_fd[best_fi];
    let mut buckets = bucket_rows(fd, &best);
    let mut row_scores: Vec<Score> = (0..best.rows.len()).into_par_iter().map(|ri| eval_row(&fine, fd, &best, ri, &buckets[ri], &ep, false).score).collect();
    let mut last_report = Instant::now();
    let mut step = best.l / 8.0;
    while started.elapsed() < limit && !job.is_cancelled() && !best_score.printable() {
        job.set_progress(0.15 + 0.8 * (started.elapsed().as_secs_f32() / limit.as_secs_f32()).min(1.0));
        let mut improved = false;
        // Line moves, rows in parallel: best improving move per row.
        let moves: Vec<Option<(usize, Vec<Line>, Score)>> = (0..best.rows.len())
            .into_par_iter()
            .map(|ri| {
                let row = &best.rows[ri];
                let mut row_best: Option<(Vec<Line>, Score)> = None;
                let cur = row_scores[ri];
                for li in 0..row.lines.len() {
                    for (db, dt) in [(step, step), (-step, -step), (step, 0.0), (-step, 0.0), (0.0, step), (0.0, -step)] {
                        let mut lines = row.lines.clone();
                        lines[li].xb += db;
                        lines[li].xt += dt;
                        if !valid_lines(&lines, li, row.v1 - row.v0) {
                            continue;
                        }
                        let mut lay = best.clone();
                        lay.rows[ri].lines = lines.clone();
                        let sc = eval_row(&fine, fd, &lay, ri, &buckets[ri], &ep, false).score;
                        let base = row_best.as_ref().map(|b| b.1).unwrap_or(cur);
                        if sc.better_than(&base) {
                            row_best = Some((lines, sc));
                        }
                    }
                }
                row_best.map(|(l, s)| (ri, l, s))
            })
            .collect();
        for (ri, lines, sc) in moves.into_iter().flatten() {
            best.rows[ri].lines = lines;
            row_scores[ri] = sc;
            improved = true;
        }
        // Row boundary moves (shift a base cut between two rows).
        for k in 1..best.rows.len() {
            if job.is_cancelled() {
                break;
            }
            let h = best.h;
            let mut pair_best: Option<(Layout, Score, Score)> = None;
            for dv in [h / 6.0, -h / 6.0, h / 12.0, -h / 12.0] {
                let mut lay = best.clone();
                let nv = lay.rows[k].v0 + dv;
                if nv - lay.rows[k - 1].v0 < 0.4 * h || lay.rows[k].v1 - nv < 0.4 * h {
                    continue;
                }
                lay.rows[k - 1].v1 = nv;
                lay.rows[k].v0 = nv;
                let b = bucket_rows(fd, &lay);
                let s0 = eval_row(&fine, fd, &lay, k - 1, &b[k - 1], &ep, false).score;
                let s1 = eval_row(&fine, fd, &lay, k, &b[k], &ep, false).score;
                let mut new = s0;
                new.add(&s1);
                let mut old = pair_best.as_ref().map(|p| {
                    let mut x = p.1;
                    x.add(&p.2);
                    x
                }).unwrap_or(row_scores[k - 1]);
                if pair_best.is_none() {
                    old.add(&row_scores[k]);
                }
                if new.better_than(&old) {
                    pair_best = Some((lay, s0, s1));
                }
            }
            if let Some((lay, s0, s1)) = pair_best {
                best = lay;
                row_scores[k - 1] = s0;
                row_scores[k] = s1;
                buckets = bucket_rows(fd, &best);
                improved = true;
            }
        }
        let mut total = Score::default();
        for r in &row_scores {
            total.add(r);
        }
        best_score = total;
        if last_report.elapsed() > Duration::from_millis(400) {
            report(&best, best_fi, &best_score);
            last_report = Instant::now();
        }
        if !improved {
            if step > best.l / 64.0 {
                step /= 2.0;
            } else {
                break;
            }
        }
    }
    best_fi = best_fi.min(fine_fd.len() - 1);
    report(&best, best_fi, &best_score);
    SearchResult { layout: Some(best), score: best_score }
}

/// Lines stay ordered (non-crossing) and keep a reasonable side angle.
fn valid_lines(lines: &[Line], i: usize, row_h: f64) -> bool {
    let l = lines[i];
    if i > 0 && (lines[i - 1].xb > l.xb + 1e-9 || lines[i - 1].xt > l.xt + 1e-9) {
        return false;
    }
    if i + 1 < lines.len() && (l.xb > lines[i + 1].xb + 1e-9 || l.xt > lines[i + 1].xt + 1e-9) {
        return false;
    }
    // Angle from the base between 35° and 80°.
    let ang = row_h.atan2((l.xt - l.xb).abs()).to_degrees();
    (35.0..=80.0).contains(&ang)
}

fn describe(s: &Score) -> String {
    let mut parts = vec![format!("~{:.0} pieces", s.pieces)];
    if s.unfit > 0 {
        parts.push(format!("{} too big", s.unfit));
    }
    if s.support_pieces > 0 {
        parts.push(format!("{} need support", s.support_pieces));
    }
    if s.tall > 0 {
        parts.push(format!("{} tall", s.tall));
    }
    parts.join(" · ")
}

/// Plan entry point used by the Split job.
pub fn plan(mesh: &Mesh, s: &Settings, zones: &Zones, job: &JobCtx, preview: Option<&PreviewFn>) -> Plan {
    let r = search(mesh, s, zones, job, preview);
    match r.layout {
        Some(l) => l.plan(),
        None => Plan::Leaf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::Solid;

    #[test]
    fn statue_like_block_gets_a_triangle_layout() {
        let a = Solid::cube(DVec3::new(500.0, 160.0, 700.0), true).unwrap();
        let b = Solid::sphere(170.0, 48).unwrap().translate(DVec3::new(0.0, 0.0, 420.0)).unwrap();
        let m = a.union(&b).unwrap().to_mesh();
        let mut s = Settings::default();
        s.printer.printer = crate::printers::Printer::new("t", 256.0, 256.0, 256.0);
        s.split.time_limit = 3.0;
        let r = search(&m, &s, &Zones::default(), &JobCtx::new(), None);
        let lay = r.layout.unwrap();
        assert!(lay.rows.len() >= 3, "{}", lay.rows.len());
        assert_eq!(r.score.unfit, 0, "{:?}", r.score);
        eprintln!("score {:?}", r.score);
    }
}
