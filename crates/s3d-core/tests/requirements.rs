//! End-to-end checks of the requirements on procedural models.

use std::sync::Arc;

use glam::{DVec3, Vec3};
use s3d_core::edits::{EditOp, EditStatus, PieceRef, SeamRef};
use s3d_core::job::JobCtx;
use s3d_core::manifold::Solid;
use s3d_core::mesh::Mesh;
use s3d_core::partition::Flag;
use s3d_core::printers::Printer;
use s3d_core::settings::{Settings, SplitMode};
use s3d_core::split::{self, SplitInput};
use s3d_core::zones::{Dab, Zones};

fn settings(mode: SplitMode) -> Settings {
    let mut s = Settings::default();
    s.printer.printer = Printer::new("test", 200.0, 200.0, 200.0);
    s.split.mode = mode;
    s.split.time_limit = 3.0;
    s
}

fn block(x: f64, y: f64, z: f64) -> Mesh {
    let s = Solid::cube(DVec3::new(x, y, z), false).unwrap();
    s.to_mesh()
}

fn run(mesh: &Mesh, s: &Settings, zones: Zones, edits: Vec<EditOp>, keep: Vec<Arc<Solid>>) -> split::SplitOutput {
    split::run(SplitInput { mesh: Arc::new(mesh.clone()), settings: s.clone(), zones, edits, keep, preview: None }, &JobCtx::new()).unwrap()
}

fn assert_valid(p: &s3d_core::partition::Partition, source_volume: f64) {
    for q in &p.pieces {
        assert!(s3d_core::repair::edge_stats(&q.mesh).is_closed_manifold(), "piece {} not closed", q.label);
        assert!(Solid::from_mesh(&q.mesh).is_ok(), "piece {} not a valid solid", q.label);
        assert!(!q.flags.contains(&Flag::TooBig), "piece {} too big: {:?}", q.label, q.orient.map(|o| o.size));
    }
    let v = p.total_volume();
    assert!((v - source_volume).abs() / source_volume < 2e-3, "volume {v} vs {source_volume}");
}

#[test]
fn grid_pieces_fit_and_are_solid() {
    let m = block(500.0, 300.0, 250.0);
    let s = settings(SplitMode::Grid);
    let out = run(&m, &s, Zones::default(), vec![], vec![]);
    assert!(out.partition.pieces.len() >= 6);
    assert_valid(&out.partition, m.volume());
}

#[test]
fn triangle_pieces_fit_print_and_stagger() {
    let m = block(600.0, 120.0, 450.0);
    let s = settings(SplitMode::Triangle);
    let out = run(&m, &s, Zones::default(), vec![], vec![]);
    let p = &out.partition;
    assert_valid(p, m.volume());
    // A solid block: every piece prints without support.
    let support = p.pieces.iter().filter(|q| q.flags.iter().any(|f| matches!(f, Flag::NeedsSupport(_)))).count();
    assert_eq!(support, 0, "{} pieces need support", support);
    // Most pieces are bounded by three or four cuts (triangles).
    let few_cuts = p.pieces.iter().filter(|q| s3d_core::partition::dedup_planes(p.seams_of(q.id).map(|s| s.plane)).len() <= 4).count();
    assert!(few_cuts * 2 >= p.pieces.len(), "{few_cuts} of {}", p.pieces.len());
}

#[test]
fn no_cut_zone_moves_grid_cut() {
    let m = block(300.0, 100.0, 100.0);
    let s = settings(SplitMode::Grid);
    let plain = run(&m, &s, Zones::default(), vec![], vec![]);
    let x_cuts = |o: &split::SplitOutput| -> Vec<f64> {
        let mut v: Vec<f64> = o.partition.seams.iter().filter(|s| s.plane.normal.x.abs() > 0.99).map(|s| s.plane.offset.abs()).collect();
        v.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        v
    };
    let base = x_cuts(&plain);
    assert_eq!(base.len(), 1);
    // Paint right where the cut was.
    let zones = Zones { dabs: vec![Dab { p: Vec3::new(base[0] as f32, 50.0, 100.0), radius: 8.0 }] };
    let moved = x_cuts(&run(&m, &s, zones, vec![], vec![]));
    assert!((moved[0] - base[0]).abs() >= 8.0, "cut at {} still crosses the zone at {}", moved[0], base[0]);
}

#[test]
fn edits_replay_after_resplit() {
    let m = block(360.0, 100.0, 100.0);
    let s = settings(SplitMode::Grid);
    let first = run(&m, &s, Zones::default(), vec![], vec![]);
    let seam = first.partition.seams[0].clone();
    let moved = seam.plane.moved(20.0);
    let edit = EditOp::MoveSeam { seam: SeamRef::of(&first.partition, seam.id).unwrap(), plane: moved };
    // Changing an unrelated setting re-splits; the edit is replayed.
    let mut s2 = s.clone();
    s2.split.min_piece = 10.0;
    let again = run(&m, &s2, Zones::default(), vec![edit], vec![]);
    assert_eq!(again.edit_status, vec![EditStatus::Applied]);
    assert!(again.partition.seams.iter().any(|x| x.plane.coincident(&moved, 1e-6, 1e-3)));
}

#[test]
fn locked_piece_survives_resplit() {
    let m = block(360.0, 100.0, 100.0);
    let s = settings(SplitMode::Grid);
    let first = run(&m, &s, Zones::default(), vec![], vec![]);
    let locked = &first.partition.pieces[0];
    let mut s2 = s.clone();
    s2.printer.printer = Printer::new("smaller", 150.0, 150.0, 150.0);
    let again = run(&m, &s2, Zones::default(), vec![], vec![locked.solid.clone()]);
    let kept = again.partition.pieces.iter().find(|q| q.locked).expect("locked piece kept");
    assert!((kept.volume - locked.volume).abs() < 1e-3);
    assert!((kept.centroid - locked.centroid).length() < 1e-3);
    assert_valid(&again.partition, m.volume());
}

#[test]
fn islands_become_separate_pieces() {
    let a = Solid::cube(DVec3::splat(50.0), false).unwrap();
    let b = Solid::cube(DVec3::splat(50.0), false).unwrap().translate(DVec3::new(100.0, 0.0, 0.0)).unwrap();
    let m = a.union(&b).unwrap().to_mesh();
    let out = run(&m, &settings(SplitMode::Grid), Zones::default(), vec![], vec![]);
    assert_eq!(out.partition.pieces.len(), 2);
}

#[test]
fn hollowed_model_splits_into_valid_pieces() {
    let m = block(300.0, 150.0, 150.0);
    let h = s3d_core::hollow::hollow(&m, 4.0, &JobCtx::new()).unwrap();
    assert!(h.volume() < m.volume() * 0.5);
    let out = run(&h, &settings(SplitMode::Grid), Zones::default(), vec![], vec![]);
    assert_valid(&out.partition, h.volume());
}

#[test]
fn merge_and_cut_edits() {
    let m = block(360.0, 100.0, 100.0);
    let s = settings(SplitMode::Grid);
    let mut p = run(&m, &s, Zones::default(), vec![], vec![]).partition;
    let n = p.pieces.len();
    let (a, b) = (p.seams[0].a, p.seams[0].b);
    let op = EditOp::Merge { pieces: vec![PieceRef::of(&p, a).unwrap(), PieceRef::of(&p, b).unwrap()] };
    let (st, _) = s3d_core::edits::apply(&mut p, &op);
    assert_eq!(st, EditStatus::Applied);
    assert_eq!(p.pieces.len(), n - 1);
    let target = p.pieces[0].id;
    let c = p.piece(target).unwrap().centroid.as_dvec3();
    let op = EditOp::Cut { piece: PieceRef::of(&p, target).unwrap(), plane: s3d_core::plane::Plane::through(c, DVec3::Y) };
    let (st, _) = s3d_core::edits::apply(&mut p, &op);
    assert_eq!(st, EditStatus::Applied);
    assert_eq!(p.pieces.len(), n);
}

#[test]
fn support_flag_only_when_unavoidable() {
    // A T shape: the arms overhang in every orientation that keeps it whole.
    let stem = Solid::cube(DVec3::new(20.0, 20.0, 100.0), false).unwrap();
    let bar = Solid::cube(DVec3::new(120.0, 20.0, 20.0), false).unwrap().translate(DVec3::new(-50.0, 0.0, 100.0)).unwrap();
    let t = stem.union(&bar).unwrap().to_mesh();
    let params = s3d_core::orient::OrientParams { bed: [200.0, 200.0, 200.0], overhang_deg: 45.0 };
    let (_, flags) = s3d_core::orient::best(&t, &[], &params);
    // Lying on its side (the T flat on the bed) needs no support.
    assert!(!flags.iter().any(|f| matches!(f, Flag::NeedsSupport(_))), "{flags:?}");
}

#[test]
fn export_shrinkage_scales_parts() {
    let mut p = s3d_core::partition::Partition::from_solids(vec![Solid::cube(DVec3::splat(100.0), false).unwrap()]);
    let mut s = Settings::default();
    s.export.shrink_xy = 2.0;
    s.export.shrink_z = 1.0;
    s.export.guide = false;
    s.export.viewer = false;
    let params = split::orient_params(&s);
    split::orient_pieces(&mut p, &[0], &params);
    let items: Vec<s3d_core::pack::PackItem> = p.pieces.iter().map(|q| s3d_core::pack::PackItem { id: q.id, mesh: q.mesh.clone(), print: q.orient.unwrap().matrix() }).collect();
    let layout = s3d_core::pack::pack(&items, &s3d_core::pack::PackParams { bed: [200.0, 200.0], spacing: 4.0, keep_order: false, angle_step: 90.0 });
    let dir = std::env::temp_dir().join(format!("s3d-shrink-{}", std::process::id()));
    let r = s3d_core::export::run(&dir, "cube", &p, &layout, &s, &JobCtx::new()).unwrap();
    let bed = s3d_core::io::threemf::read(&std::fs::read(&r.files[0]).unwrap()).unwrap();
    let size = bed.bbox().size();
    assert!((size.x - 102.0).abs() < 0.01 && (size.y - 102.0).abs() < 0.01 && (size.z - 101.0).abs() < 0.01, "{size:?}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn packing_keeps_assembly_order() {
    let items: Vec<s3d_core::pack::PackItem> = (0..6)
        .map(|i| {
            let mut m = block(90.0, 90.0, 10.0);
            m.translate(Vec3::new(-45.0, -45.0, 0.0));
            s3d_core::pack::PackItem { id: i, mesh: Arc::new(m), print: glam::Mat4::IDENTITY }
        })
        .collect();
    let l = s3d_core::pack::pack(&items, &s3d_core::pack::PackParams { bed: [200.0, 200.0], spacing: 4.0, keep_order: true, angle_step: 90.0 });
    // Beds hold consecutive pieces in order.
    let flat: Vec<u32> = l.beds.iter().flat_map(|b| b.iter().map(|p| p.piece)).collect();
    assert_eq!(flat, (0..6).collect::<Vec<_>>());
}
