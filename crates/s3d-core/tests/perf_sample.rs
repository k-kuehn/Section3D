//! Performance checks on the real 1M-triangle sample model.
//! Set S3D_SAMPLE to the STL path; skipped otherwise.

use std::time::Instant;

use s3d_core::job::JobCtx;

fn sample() -> Option<std::path::PathBuf> {
    std::env::var_os("S3D_SAMPLE").map(Into::into).filter(|p: &std::path::PathBuf| p.exists())
}

#[test]
fn open_sample_under_one_second() {
    let Some(path) = sample() else {
        eprintln!("S3D_SAMPLE not set; skipping");
        return;
    };
    let t = Instant::now();
    let mesh = s3d_core::io::load(&path).unwrap();
    let t_load = t.elapsed();
    let t2 = Instant::now();
    let r = s3d_core::repair::repair(mesh, &JobCtx::new()).unwrap();
    let t_check = t2.elapsed();
    eprintln!("load {:?}, check/repair {:?}, tris {}, health {:?}", t_load, t_check, r.mesh.tris.len(), r.health.label());
    assert!(t_load.as_secs_f32() < 1.0);
}

#[test]
fn sample_diagnostics() {
    let Some(path) = sample() else { return };
    let mesh = s3d_core::io::load(&path).unwrap();
    let t = Instant::now();
    let st = s3d_core::repair::edge_stats(&mesh);
    eprintln!("edge_stats {:?} in {:?}", st, t.elapsed());
    let t = Instant::now();
    let s = s3d_core::manifold::Solid::from_mesh(&mesh);
    eprintln!("from_mesh ok={} in {:?}", s.is_ok(), t.elapsed());
    let t = Instant::now();
    eprintln!("volume {} in {:?}", mesh.volume(), t.elapsed());
}

#[test]
fn grid_split_sample() {
    let Some(path) = sample() else { return };
    let mesh = s3d_core::io::load(&path).unwrap();
    let mesh = s3d_core::repair::repair(mesh, &JobCtx::new()).unwrap().mesh;
    let mut settings = s3d_core::settings::Settings::default();
    settings.printer.printer = s3d_core::printers::Printer::new("X1C", 256.0, 256.0, 256.0);
    settings.split.mode = s3d_core::settings::SplitMode::Grid;
    let t = Instant::now();
    let job = JobCtx::new();
    let out = s3d_core::split::run(
        s3d_core::split::SplitInput { mesh: std::sync::Arc::new(mesh), settings, zones: Default::default(), edits: vec![], keep: vec![] },
        &job,
    )
    .unwrap();
    let p = &out.partition;
    let flagged = p.pieces.iter().filter(|q| !q.flags.is_empty()).count();
    eprintln!("grid: {} pieces, {} seams, {} flagged, in {:?}", p.pieces.len(), p.seams.len(), flagged, t.elapsed());
}
