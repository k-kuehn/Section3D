//! The Split job: plan cuts, apply them, clean up, replay edits, orient.

pub mod cleanup;
pub mod fix;
pub mod grid;
pub mod triangle;

use std::sync::Arc;

use glam::DVec3;
use rayon::prelude::*;

use crate::bsp::{self, Plan};
use crate::edits::{self, EditOp, EditStatus};
use crate::job::JobCtx;
use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::orient::{self, OrientParams};
use crate::partition::{Flag, Partition, PieceId};
use crate::plane::Plane;
use crate::settings::{Settings, SplitMode};
use crate::zones::Zones;

pub struct SplitInput {
    /// Prepared (transformed, maybe hollowed) model.
    pub mesh: Arc<Mesh>,
    pub settings: Settings,
    /// No-cut zones in prepared coordinates.
    pub zones: Zones,
    pub edits: Vec<EditOp>,
    /// Locked pieces carried over unchanged from the previous result.
    pub keep: Vec<Arc<Solid>>,
    /// Receives the best Triangle layout (as cut lines) while searching.
    pub preview: Option<triangle::PreviewFn>,
}

pub struct SplitOutput {
    pub partition: Partition,
    pub edit_status: Vec<EditStatus>,
}

pub fn orient_params(s: &Settings) -> OrientParams {
    OrientParams { bed: s.printer.usable(), overhang_deg: s.split.overhang }
}

/// Outward normals of a piece's cut faces.
pub fn cut_normals(p: &Partition, id: PieceId) -> Vec<DVec3> {
    p.seams_of(id).map(|s| if s.a == id { -s.plane.normal } else { s.plane.normal }).collect()
}

/// (Re)computes orientation and orientation flags for the given pieces.
pub fn orient_pieces(p: &mut Partition, ids: &[PieceId], params: &OrientParams) {
    let work: Vec<(usize, Vec<DVec3>)> = p.pieces.iter().enumerate().filter(|(_, q)| ids.contains(&q.id)).map(|(i, q)| (i, cut_normals(p, q.id))).collect();
    let results: Vec<(usize, crate::partition::Orientation, Vec<Flag>)> = work
        .into_par_iter()
        .map(|(i, normals)| {
            let (o, f) = orient::best(&p.pieces[i].mesh, &normals, params);
            (i, o, f)
        })
        .collect();
    for (i, o, f) in results {
        let q = &mut p.pieces[i];
        q.orient = Some(o);
        q.flags.retain(|x| !matches!(x, Flag::TooBig | Flag::NeedsSupport(_) | Flag::Tall));
        q.flags.extend(f);
    }
}

/// Numbers pieces bottom-up, row by row, starting at 1.
pub fn number_pieces(p: &mut Partition) {
    let mut order: Vec<usize> = (0..p.pieces.len()).collect();
    let row = p.pieces.iter().map(|q| q.bbox.size().z).fold(1.0f32, f32::max) * 0.5;
    order.sort_by(|&a, &b| {
        let (pa, pb) = (&p.pieces[a], &p.pieces[b]);
        let (ra, rb) = ((pa.bbox.min.z / row).floor(), (pb.bbox.min.z / row).floor());
        ra.total_cmp(&rb).then(pa.centroid.y.total_cmp(&pb.centroid.y)).then(pa.centroid.x.total_cmp(&pb.centroid.x))
    });
    for (n, i) in order.into_iter().enumerate() {
        p.pieces[i].label = n as u32 + 1;
    }
}

/// Builds the cut plan for the chosen mode.
pub fn plan(mesh: &Mesh, s: &Settings, zones: &Zones, job: &JobCtx, preview: Option<&triangle::PreviewFn>) -> Plan {
    let bed = s.printer.usable();
    let cell = |a: usize| s.split.target_size.map_or(bed[a], |t| t.min(bed[a]));
    match s.split.mode {
        SplitMode::Grid => grid::plan(&mesh.bbox(), [cell(0), cell(1), cell(2)], zones),
        SplitMode::Triangle => triangle::plan(mesh, s, zones, job, preview),
    }
}

/// Adds a fourth cut to pieces that still do not fit, need support or are
/// tall (Triangle mode), within a time budget.
pub fn fix_flagged(part: &mut Partition, params: &OrientParams, budget: std::time::Duration, job: &JobCtx) -> usize {
    let start = std::time::Instant::now();
    let mut fixed = 0;
    let mut tried: Vec<PieceId> = Vec::new();
    loop {
        if job.is_cancelled() || start.elapsed() > budget {
            break;
        }
        // Worst first: too big, then most support, then tall.
        let next = part
            .pieces
            .iter()
            .filter(|q| !q.locked && !tried.contains(&q.id))
            .filter_map(|q| {
                let rank = q.flags.iter().map(|f| match f {
                    Flag::TooBig => 3e9,
                    Flag::NeedsSupport(a) => 1e6 + *a as f64,
                    Flag::Tall => 1.0,
                    _ => 0.0,
                }).fold(0.0, f64::max);
                (rank > 0.0).then_some((rank, q.id))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, id)) = next else { break };
        tried.push(id);
        if let Some(plane) = fix::best_cut(part, id, params, job) {
            if let Ok(ids) = part.split_piece(id, plane) {
                orient_pieces(part, &ids, params);
                let mut nb: Vec<PieceId> = ids.iter().flat_map(|i| part.neighbours(*i)).collect();
                nb.sort_unstable();
                nb.dedup();
                orient_pieces(part, &nb, params);
                fixed += 1;
            }
        }
    }
    fixed
}

pub fn run(input: SplitInput, job: &JobCtx) -> Result<SplitOutput, String> {
    let s = &input.settings;
    job.set_progress(0.02);
    // Build the full-resolution solid while the (proxy-based) planning runs.
    let (solid, plan) = rayon::join(|| Solid::from_mesh(&input.mesh), || {
        job.set_stage("Planning cuts");
        plan(&input.mesh, s, &input.zones, job, input.preview.as_ref())
    });
    let mut solid = solid.map_err(|e| format!("model is not a valid solid: {e}"))?;
    if !input.keep.is_empty() {
        let refs: Vec<&Solid> = input.keep.iter().map(|k| k.as_ref()).collect();
        let locked = Solid::batch(&refs, crate::manifold::Op::Union).map_err(|e| e.to_string())?;
        solid = solid.subtract(&locked).map_err(|e| e.to_string())?;
    }
    job.check().map_err(|e| e.to_string())?;
    job.set_stage("Cutting");
    job.set_progress(0.4);
    let leaves = bsp::apply(solid, &plan, job).map_err(|e| match e {
        bsp::ApplyError::Cancelled => "cancelled".to_string(),
        bsp::ApplyError::Geometry(g) => g.to_string(),
    })?;
    let mut bodies: Vec<Solid> = leaves.into_par_iter().flat_map_iter(|l| l.bodies()).filter(|b| b.volume() > 1e-3).collect();
    let n_new = bodies.len();
    bodies.extend(input.keep.iter().map(|k| (**k).clone()));
    let mut part = Partition::from_solids(bodies);
    for q in part.pieces.iter_mut().skip(n_new) {
        q.locked = true;
    }
    let mut planes = Vec::new();
    plan.planes(&mut planes);
    job.set_stage("Finding seams");
    job.set_progress(0.6);
    part.rebuild_seams(&planes);
    job.check().map_err(|e| e.to_string())?;
    job.set_stage("Merging slivers");
    job.set_progress(0.7);
    let params = orient_params(s);
    let stuck = cleanup::merge_slivers(&mut part, s.split.min_piece as f64, &params, job).map_err(|e| e.to_string())?;
    job.set_stage("Applying edits");
    let (edit_status, _) = edits::replay(&mut part, &input.edits);
    job.set_stage("Orienting pieces");
    job.set_progress(0.85);
    let all: Vec<PieceId> = part.pieces.iter().map(|q| q.id).collect();
    orient_pieces(&mut part, &all, &params);
    if s.split.mode == SplitMode::Triangle {
        job.set_stage("Fixing pieces that need support");
        let budget = std::time::Duration::from_secs_f32((s.split.time_limit * 0.5).clamp(2.0, 60.0));
        if fix_flagged(&mut part, &params, budget, job) > 0 {
            cleanup::merge_slivers(&mut part, s.split.min_piece as f64, &params, job).map_err(|e| e.to_string())?;
        }
        // Fold fragments (e.g. at the model's silhouette) into neighbours.
        job.set_stage("Merging small pieces");
        let bed = s.printer.usable();
        let target = s.split.target_size.unwrap_or(bed[0].max(bed[1])) as f64;
        cleanup::merge_undersized(&mut part, target, 0.55, &params, job).map_err(|e| e.to_string())?;
    }
    for id in stuck {
        if let Some(q) = part.piece_mut(id) {
            q.flags.push(Flag::Small);
        }
    }
    number_pieces(&mut part);
    job.set_stage("Done");
    job.set_progress(1.0);
    Ok(SplitOutput { partition: part, edit_status })
}

/// Distinct cut planes of a plan.
pub fn plan_planes(plan: &Plan) -> Vec<Plane> {
    let mut v = Vec::new();
    plan.planes(&mut v);
    crate::partition::dedup_planes(v.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;
    use crate::printers::Printer;
    use glam::Vec3;

    #[test]
    fn grid_split_fits_and_conserves_volume() {
        let mesh = Arc::new(shapes::uv_sphere(200.0, 48, 24));
        let mut settings = Settings::default();
        settings.printer.printer = Printer::new("t", 180.0, 180.0, 180.0);
        settings.split.mode = SplitMode::Grid;
        let out = run(SplitInput { mesh: mesh.clone(), settings, zones: Zones::default(), edits: vec![], keep: vec![], preview: None }, &JobCtx::new()).unwrap();
        let p = &out.partition;
        assert!(p.pieces.len() >= 8, "{}", p.pieces.len());
        let v: f64 = p.total_volume();
        assert!((v - mesh.volume()).abs() / mesh.volume() < 1e-3);
        for q in &p.pieces {
            assert!(!q.flags.contains(&Flag::TooBig), "piece {} {:?}", q.id, q.orient.map(|o| o.size));
            assert!(crate::repair::edge_stats(&q.mesh).is_closed_manifold());
        }
        assert!(!p.seams.is_empty());
        let _ = Vec3::ZERO;
    }
}
