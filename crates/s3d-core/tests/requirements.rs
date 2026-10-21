//! End-to-end checks of the requirements on procedural models.

use std::sync::Arc;

use glam::{DVec3, Vec3};
use s3d_core::edits::EditOp;
use s3d_core::job::JobCtx;
use s3d_core::manifold::Solid;
use s3d_core::mesh::Mesh;
use s3d_core::partition::Flag;
use s3d_core::settings::{Settings, SplitMode};
use s3d_core::split::{self, SplitInput};
use s3d_core::zones::{Dab, Zones};

fn settings(mode: SplitMode) -> Settings {
    let mut s = Settings::default();
    s.printer.volume = [200.0, 200.0, 200.0];
    s.split.mode = mode;
    s.split.time_limit = 3.0;
    s
}

fn block(x: f64, y: f64, z: f64) -> Mesh {
    let s = Solid::cube(DVec3::new(x, y, z), false).unwrap();
    s.to_mesh()
}

fn run(mesh: &Mesh, s: &Settings, zones: Zones, keep: Vec<Arc<Solid>>) -> split::SplitOutput {
    run_keep(mesh, s, zones, keep, vec![])
}

fn run_keep(mesh: &Mesh, s: &Settings, zones: Zones, keep: Vec<Arc<Solid>>, keep_planes: Vec<s3d_core::plane::Plane>) -> split::SplitOutput {
    split::run(SplitInput { mesh: Arc::new(mesh.clone()), settings: s.clone(), zones, keep, keep_planes, preview: None }, &JobCtx::new()).unwrap()
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
    let out = run(&m, &s, Zones::default(), vec![]);
    assert!(out.partition.pieces.len() >= 6);
    assert_valid(&out.partition, m.volume());
}

/// Every Shatter-mode requirement on one result: pieces fit, print without
/// support, are not tall, have no sharp corners, feather edges or thin
/// strips, and no seam runs straight on into another.
fn assert_shatter_clean(p: &s3d_core::partition::Partition, s: &Settings) {
    let mut bad: Vec<String> = Vec::new();
    for q in &p.pieces {
        let f: Vec<&Flag> = q.flags.iter().filter(|f| !matches!(f, Flag::SkippedDowel)).collect();
        if !f.is_empty() {
            bad.push(format!("#{} {:?}", q.label, f));
        }
    }
    assert!(bad.is_empty(), "{} of {} pieces flagged: {bad:?}", bad.len(), p.pieces.len());
    let qp = s3d_core::split::quality::QualityParams::from_settings(s);
    let mut cache = s3d_core::split::quality::SeamCache::default();
    cache.update(p);
    let chains = s3d_core::split::quality::chains(p, &cache);
    let bad: Vec<_> = chains.iter().filter(|c| c.is_violation(&qp)).collect();
    assert!(bad.is_empty(), "aligned seams: {bad:?}");
}

#[test]
fn shatter_block_prints_well_without_aligned_seams() {
    let m = block(600.0, 120.0, 450.0);
    let mut s = settings(SplitMode::Shatter);
    s.split.time_limit = 60.0;
    let out = run(&m, &s, Zones::default(), vec![]);
    let p = &out.partition;
    assert_valid(p, m.volume());
    assert_shatter_clean(p, &s);
    // Seams meet at Y-shaped corners: most pieces have several neighbours.
    assert!(p.pieces.len() >= 8, "{}", p.pieces.len());
}

#[test]
fn shatter_shell_prints_well_without_aligned_seams() {
    // A hollow rounded box, like a body shell: pieces stand on a cut.
    let outer = Solid::sphere(260.0, 96).unwrap().transform(&glam::DMat4::from_scale(DVec3::new(1.0, 0.7, 0.55))).unwrap();
    let inner = Solid::sphere(254.0, 96).unwrap().transform(&glam::DMat4::from_scale(DVec3::new(1.0, 0.7, 0.55) * (254.0 - 6.0) / 254.0)).unwrap();
    let shell = outer.subtract(&inner).unwrap().translate(DVec3::new(0.0, 0.0, 150.0)).unwrap();
    let m = shell.to_mesh();
    let mut s = settings(SplitMode::Shatter);
    s.printer.volume = [220.0, 220.0, 220.0];
    s.split.time_limit = 90.0;
    let out = run(&m, &s, Zones::default(), vec![]);
    let p = &out.partition;
    assert_valid(p, m.volume());
    assert_shatter_clean(p, &s);
    eprintln!("shell: {} pieces, {:?}", p.pieces.len(), out.stats);
}

#[test]
fn grid_cuts_where_placed_and_only_checks() {
    let m = block(300.0, 100.0, 100.0);
    let mut s = settings(SplitMode::Grid);
    let x_cuts = |o: &split::SplitOutput| -> Vec<f64> {
        let mut v: Vec<f64> = o.partition.seams.iter().filter(|s| s.plane.normal.x.abs() > 0.99).map(|s| s.plane.offset.abs()).collect();
        v.sort_by(f64::total_cmp);
        v.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        v
    };
    // Evenly spaced by default: 300 mm in two cells of 150.
    let plain = run(&m, &s, Zones::default(), vec![]);
    assert_eq!(x_cuts(&plain), vec![150.0]);
    // Painted right on the cut: the cut stays, the pieces are flagged.
    let zones = Zones { dabs: vec![Dab { p: Vec3::new(150.0, 50.0, 100.0), radius: 8.0, n: Vec3::Z }] };
    let painted = run(&m, &s, zones, vec![]);
    assert_eq!(x_cuts(&painted), vec![150.0]);
    assert!(painted.partition.pieces.iter().all(|q| q.flags.contains(&Flag::InZone)));
    // Cuts placed by hand, any number per axis.
    s.split.grid = Some([vec![60.0, 200.0], vec![50.0], vec![]]);
    let placed = run(&m, &s, Zones::default(), vec![]);
    assert_eq!(x_cuts(&placed), vec![60.0, 200.0]);
    assert_eq!(placed.partition.pieces.len(), 6);
    assert_valid(&placed.partition, m.volume());
}

#[test]
fn shatter_cuts_avoid_painted_zone() {
    let m = block(380.0, 160.0, 120.0);
    let s = settings(SplitMode::Shatter);
    let plain = run(&m, &s, Zones::default(), vec![]);
    // Paint a strip on the top face across the middle of the first seam that
    // reaches it, then split again: no cut face may touch the paint.
    let top = plain.partition.seams.iter().find(|x| x.center.z > 20.0 && x.plane.normal.z.abs() < 0.5).expect("a seam reaching the top");
    let mut dabs = Vec::new();
    for k in -3..=3 {
        let along = top.plane.normal.cross(DVec3::Z).normalize().as_vec3();
        let mut c = top.plane.project(top.center.as_dvec3()).as_vec3() + along * (k as f32 * 8.0);
        c.z = 120.0;
        dabs.push(Dab { p: c, radius: 9.0, n: Vec3::Z });
    }
    let zones = Zones { dabs };
    let before: u32 = plain.partition.pieces.iter().map(|q| split::quality::zone_hits(&q.mesh, &split::quality::piece_cuts(&plain.partition, q.id), &zones)).sum();
    assert!(before > 0, "the paint should cross a cut of the plain split");
    let out = run(&m, &s, zones.clone(), vec![]);
    assert_valid(&out.partition, m.volume());
    for q in &out.partition.pieces {
        let cuts = split::quality::piece_cuts(&out.partition, q.id);
        let hits = split::quality::zone_hits(&q.mesh, &cuts, &zones);
        assert_eq!(hits, 0, "piece {} still cut through {hits} painted spot(s)", q.label);
        assert!(!q.flags.contains(&Flag::InZone));
    }
}

#[test]
fn locked_piece_survives_resplit() {
    let m = block(360.0, 100.0, 100.0);
    let s = settings(SplitMode::Grid);
    let first = run(&m, &s, Zones::default(), vec![]);
    let locked = &first.partition.pieces[0];
    let planes: Vec<s3d_core::plane::Plane> = first.partition.seams_of(locked.id).map(|s| s.plane).collect();
    let mut s2 = s.clone();
    s2.split.target_size = Some(90.0);
    let again = run_keep(&m, &s2, Zones::default(), vec![locked.solid.clone()], planes);
    let kept = again.partition.pieces.iter().find(|q| q.locked).expect("locked piece kept");
    assert!(again.partition.seams_of(kept.id).count() > 0, "locked piece keeps its seams");
    assert!((kept.volume - locked.volume).abs() < 1e-3);
    assert!((kept.centroid - locked.centroid).length() < 1e-3);
    assert_valid(&again.partition, m.volume());
}

#[test]
fn islands_become_separate_pieces() {
    let a = Solid::cube(DVec3::splat(50.0), false).unwrap();
    let b = Solid::cube(DVec3::splat(50.0), false).unwrap().translate(DVec3::new(100.0, 0.0, 0.0)).unwrap();
    let m = a.union(&b).unwrap().to_mesh();
    let out = run(&m, &settings(SplitMode::Grid), Zones::default(), vec![]);
    assert_eq!(out.partition.pieces.len(), 2);
}

#[test]
fn hollowed_model_splits_into_valid_pieces() {
    let m = block(300.0, 150.0, 150.0);
    let h = s3d_core::hollow::hollow(&m, 4.0, &JobCtx::new()).unwrap();
    assert!(h.volume() < m.volume() * 0.5);
    let out = run(&h, &settings(SplitMode::Grid), Zones::default(), vec![]);
    assert_valid(&out.partition, h.volume());
}

#[test]
fn merge_and_cut_edits() {
    let m = block(360.0, 100.0, 100.0);
    let s = settings(SplitMode::Grid);
    let mut p = run(&m, &s, Zones::default(), vec![]).partition;
    let n = p.pieces.len();
    let (a, b) = (p.seams[0].a, p.seams[0].b);
    s3d_core::edits::apply(&mut p, EditOp::Merge(vec![a, b])).unwrap();
    assert_eq!(p.pieces.len(), n - 1);
    let target = p.pieces[0].id;
    let c = p.piece(target).unwrap().centroid.as_dvec3();
    let (new, entry) = s3d_core::edits::apply(&mut p, EditOp::Cut(target, s3d_core::plane::Plane::through(c, DVec3::Y))).unwrap();
    assert_eq!(new.len(), 2);
    assert!(entry.text.starts_with("Cut #"), "{}", entry.text);
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

/// Area of a mesh's faces lying on a plane.
fn area_on(m: &Mesh, pl: &s3d_core::plane::Plane) -> f64 {
    (0..m.tris.len()).filter(|&t| m.tri(t).iter().all(|v| pl.dist(*v).abs() < 0.01)).map(|t| m.tri_cross(t).length() as f64 * 0.5).sum()
}

#[test]
fn holes_only_open_on_their_own_cut_face() {
    // Block cut at z = 50, then the top cut by a 45° plane (reviewer case).
    let mut p = s3d_core::partition::Partition::from_solids(vec![Solid::cube(DVec3::new(200.0, 100.0, 100.0), false).unwrap()]);
    p.split_piece(0, s3d_core::plane::Plane::axis(2, 50.0)).unwrap();
    let top = p.pieces.iter().find(|q| q.centroid.z > 50.0).unwrap().id;
    p.split_piece(top, s3d_core::plane::Plane::through(DVec3::new(25.0, 0.0, 50.0), DVec3::new(1.0, 0.0, 1.0))).unwrap();
    let mut s = Settings::default();
    s.connectors.labels = s3d_core::settings::LabelMode::Off;
    let params = split::orient_params(&s);
    let ids: Vec<u32> = p.pieces.iter().map(|q| q.id).collect();
    split::orient_pieces(&mut p, &ids, &params);
    s3d_core::connectors::finish(&mut p, &ids, &s, &JobCtx::new()).unwrap();
    let pins = s3d_core::connectors::all_pins(&p, &s.connectors);
    let r = s.connectors.hole_radius();
    for q in &p.pieces {
        let fin = q.finished.as_ref().unwrap();
        assert!(s3d_core::repair::edge_stats(fin).is_closed_manifold());
        for seam in p.seams_of(q.id) {
            let lost = area_on(&q.mesh, &seam.plane) - area_on(fin, &seam.plane);
            let n = pins.iter().filter(|x| x.seam == seam.id).count() as f64;
            let cos = pins.iter().find(|x| x.seam == seam.id).map(|x| x.axis.dot(seam.plane.normal).abs()).unwrap_or(1.0);
            let max = n * (std::f64::consts::PI * r * r + r * r) / cos * 1.3 + 1.0;
            assert!(lost <= max, "piece {} lost {lost:.1} mm² on a cut face with {n} pins (max {max:.1})", q.id);
        }
    }
}

#[test]
fn nudged_grid_cells_still_fit() {
    let m = block(524.4, 100.0, 100.0);
    let s = settings(SplitMode::Grid);
    let plain = run(&m, &s, Zones::default(), vec![]);
    // Paint just inside each cut so the cuts are pushed apart.
    let dabs: Vec<Dab> = plain
        .partition
        .seams
        .iter()
        .filter(|x| x.plane.normal.x.abs() > 0.99)
        .map(|x| x.plane.offset.abs() as f32)
        .map(|x| Dab { p: Vec3::new(x + if x > 262.0 { -3.0 } else { 3.0 }, 50.0, 100.0), radius: 6.0, n: Vec3::ZERO })
        .collect();
    let out = run(&m, &s, Zones { dabs }, vec![]);
    assert!(out.partition.pieces.iter().all(|q| !q.flags.contains(&Flag::TooBig)));
}

#[test]
fn drawn_cut_line_becomes_a_square_cut() {
    let m = block(220.0, 100.0, 60.0);
    let p = s3d_core::partition::Partition::from_solids(vec![Solid::from_mesh(&m).unwrap()]);
    // A slightly wobbly stroke across the top face near x = 90.
    let pts: Vec<Vec3> = (0..20).map(|i| Vec3::new(90.0 + if i % 2 == 0 { 1.5 } else { -1.5 }, 5.0 + i as f32 * 4.7, 60.0)).collect();
    let normals = vec![Vec3::Z; pts.len()];
    let q = split::quality::QualityParams::from_settings(&settings(SplitMode::Grid));
    let pl = split::refine::cut_along(&p, p.pieces[0].id, &pts, &normals, &q, &JobCtx::new()).expect("a cut");
    assert!(pl.normal.x.abs() > 0.97, "cut normal {:?}", pl.normal);
    let x = pl.offset * pl.normal.x.signum();
    assert!((x - 90.0).abs() <= 7.0, "cut at x = {x}");
}

#[test]
fn find_better_cut_moves_a_shared_cut_first() {
    // A 7 mm slice between two grid cuts is too thin.
    let m = block(300.0, 100.0, 100.0);
    let mut s = settings(SplitMode::Grid);
    s.split.grid = Some([vec![150.0, 157.0], vec![], vec![]]);
    let p = run(&m, &s, Zones::default(), vec![]).partition;
    let sliver = p.pieces.iter().find(|q| q.bbox.size().x < 10.0).expect("the slice");
    assert!(sliver.flags.contains(&Flag::Thin), "{:?}", sliver.flags);
    let q = split::quality::QualityParams::from_settings(&s);
    let imp = split::refine::improve_piece(&p, sliver.id, &q, &JobCtx::new()).expect("a better cut");
    // First choice: merged with a neighbour and cut again a little further on.
    assert_eq!(imp.old.len(), 2);
    assert_eq!(imp.planes.len(), 1);
    assert!(imp.planes[0].normal.x.abs() > 0.999, "{:?}", imp.planes[0]);
    let mut p2 = p.clone();
    let (changed, entry) = s3d_core::edits::apply(&mut p2, EditOp::Replace { old: imp.old, new: imp.new, planes: imp.planes }).unwrap();
    assert!(entry.text.starts_with("Re-cut"), "{}", entry.text);
    split::check_pieces(&mut p2, &changed, &s, &Zones::default());
    assert_eq!(p2.pieces.len(), 3);
    assert!(p2.pieces.iter().all(|q| !q.flags.contains(&Flag::Thin)), "{:?}", p2.pieces.iter().map(|q| (q.bbox.size().x, q.flags.clone())).collect::<Vec<_>>());
    assert_valid(&p2, m.volume());
}
