//! The Split job: lay out cells (Shatter) or a grid, cut, then (Shatter
//! only) refine the pieces until they print well, and number them. Manual
//! edits made on an earlier result are not replayed: a fresh split starts
//! over.

pub mod grid;
pub mod quality;
pub mod refine;
pub mod shatter;

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use crate::bsp::{self, Plan};
use crate::job::JobCtx;
use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::orient::{self, OrientParams};
use crate::partition::{Flag, Partition, PieceId};
use crate::plane::Plane;
use crate::settings::{Settings, SplitMode};
use crate::zones::Zones;

/// Receives cut lines on the model surface and a status line while the
/// split runs, for a live preview.
pub type PreviewFn = Arc<dyn Fn(Vec<[Vec3; 2]>, String) + Send + Sync>;

/// Uniform random number in [0, 1) from a splitmix64 state.
pub fn rand01(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

pub struct SplitInput {
    /// Prepared (transformed, maybe hollowed) model.
    pub mesh: Arc<Mesh>,
    pub settings: Settings,
    /// No-cut zones in prepared coordinates.
    pub zones: Zones,
    /// Locked pieces carried over unchanged from the previous result.
    pub keep: Vec<Arc<Solid>>,
    /// Cut planes bounding the locked pieces (so their seams are found again).
    pub keep_planes: Vec<Plane>,
    /// Receives the cut lines while splitting.
    pub preview: Option<PreviewFn>,
}

pub struct SplitOutput {
    pub partition: Partition,
    pub stats: refine::RefineStats,
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

/// Full check of the given pieces: orientation, printability, shape flags
/// (sharp, thin) and cuts through no-cut zones (prepared coordinates).
pub fn check_pieces(p: &mut Partition, ids: &[PieceId], s: &Settings, zones: &Zones) {
    let q = quality::QualityParams::from_settings(s).with_zones(zones);
    let r = quality::analyze(p, ids, &q);
    quality::apply_reports(p, &r);
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

/// Cut lines where cut faces meet the surface, for previews.
pub fn seam_lines(p: &Partition) -> Vec<[Vec3; 2]> {
    p.pieces
        .par_iter()
        .flat_map_iter(|q| {
            let cuts = quality::piece_cuts(p, q.id);
            let cap = quality::cap_index(&q.mesh, &cuts);
            let m = &q.mesh;
            let mut out = Vec::new();
            for (a, b, t1, t2) in m.edge_pairs() {
                if (cap[t1 as usize] == u32::MAX) != (cap[t2 as usize] == u32::MAX) {
                    out.push([m.verts[a as usize], m.verts[b as usize]]);
                }
            }
            out
        })
        .collect()
}

pub fn run(input: SplitInput, job: &JobCtx) -> Result<SplitOutput, String> {
    let started = Instant::now();
    let s = &input.settings;
    let shatter = s.split.mode == SplitMode::Shatter;
    // Build the full-resolution solid while the layout is planned.
    enum Layout {
        Grid(Plan),
        Cells(Vec<shatter::Cell>),
    }
    let (solid, layout) = rayon::join(|| Solid::from_mesh(&input.mesh), || {
        job.set_stage("Planning cuts");
        if shatter {
            let (cells, seeds, spacing) = shatter::layout(&input.mesh, s, &input.zones, job);
            if let Some(pf) = &input.preview {
                let proxy = crate::simplify::cluster(&input.mesh, 40_000);
                pf(shatter::preview_lines(&proxy, &seeds, spacing), format!("{} cells", cells.len()));
            }
            Layout::Cells(cells)
        } else {
            let bb = input.mesh.bbox();
            Layout::Grid(grid::plan(&bb, &grid::cuts(&bb, s)))
        }
    });
    let mut solid = solid.map_err(|e| format!("model is not a valid solid: {e}"))?;
    if !input.keep.is_empty() {
        let refs: Vec<&Solid> = input.keep.iter().map(|k| k.as_ref()).collect();
        let locked = Solid::batch(&refs, crate::manifold::Op::Union).map_err(|e| e.to_string())?;
        solid = solid.subtract(&locked).map_err(|e| e.to_string())?;
    }
    job.check().map_err(|e| e.to_string())?;
    job.set_stage("Cutting");
    let mut planes = Vec::new();
    let regions: Vec<Solid> = match &layout {
        Layout::Grid(plan) => {
            plan.planes(&mut planes);
            bsp::apply(solid, plan, job).map_err(|e| match e {
                bsp::ApplyError::Cancelled => "cancelled".to_string(),
                bsp::ApplyError::Geometry(g) => g.to_string(),
            })?
        }
        Layout::Cells(cells) => {
            for c in cells {
                planes.extend(c.planes.iter().copied());
            }
            shatter::apply(solid, cells, job)?.into_iter().map(|x| x.1).collect()
        }
    };
    let mut bodies: Vec<Solid> = regions.into_par_iter().flat_map_iter(|l| l.bodies()).filter(|b| b.volume() > 1e-3).collect();
    let n_new = bodies.len();
    bodies.extend(input.keep.iter().map(|k| (**k).clone()));
    let mut part = Partition::from_solids(bodies);
    for q in part.pieces.iter_mut().skip(n_new) {
        q.locked = true;
    }
    planes.extend_from_slice(&input.keep_planes);
    let planes = crate::partition::dedup_planes(planes.into_iter());
    job.set_stage("Finding seams");
    part.rebuild_seams(&planes);
    job.check().map_err(|e| e.to_string())?;

    let q = quality::QualityParams::from_settings(s).with_zones(&input.zones);
    let stats = if shatter {
        // Improve until every piece prints well (or time runs out).
        let deadline = started + Duration::from_secs_f32(s.split.time_limit.max(1.0));
        let last = std::sync::Mutex::new(None::<Instant>);
        let preview = input.preview.clone();
        let report = |p: &Partition, msg: &str| {
            if let Some(pf) = &preview {
                let mut l = last.lock().unwrap();
                if l.is_none_or(|t| t.elapsed() > Duration::from_millis(1500)) {
                    pf(seam_lines(p), msg.to_string());
                    *l = Some(Instant::now());
                }
            }
        };
        refine::refine(&mut part, &q, &input.zones, deadline, job, &report)
    } else {
        // Grid pieces are only checked: the cuts stay where they were put.
        job.set_stage("Checking pieces");
        let all: Vec<PieceId> = part.pieces.iter().map(|x| x.id).collect();
        let reports = quality::analyze(&part, &all, &q);
        quality::apply_reports(&mut part, &reports);
        refine::flag_small(&mut part, &q);
        refine::RefineStats::default()
    };
    job.check().map_err(|e| e.to_string())?;
    number_pieces(&mut part);
    job.set_stage("Done");
    Ok(SplitOutput { partition: part, stats })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;
    use glam::Vec3;

    #[test]
    fn grid_split_fits_and_conserves_volume() {
        let mesh = Arc::new(shapes::uv_sphere(200.0, 48, 24));
        let mut settings = Settings::default();
        settings.printer.volume = [180.0, 180.0, 180.0];
        settings.split.mode = SplitMode::Grid;
        let out = run(SplitInput { mesh: mesh.clone(), settings, zones: Zones::default(), keep: vec![], keep_planes: vec![], preview: None }, &JobCtx::new()).unwrap();
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
