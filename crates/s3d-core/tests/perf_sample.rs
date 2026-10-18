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
    let mut part = out.partition;
    eprintln!("grid split: {} pieces, {} seams in {:?}", part.pieces.len(), part.seams.len(), t.elapsed());
    let t2 = Instant::now();
    let ids: Vec<u32> = part.pieces.iter().map(|q| q.id).collect();
    let mut st = s3d_core::settings::Settings::default();
    st.printer.printer = s3d_core::printers::Printer::new("X1C", 256.0, 256.0, 256.0);
    st.connectors.diameter = std::env::var("S3D_PIN").ok().and_then(|v| v.parse().ok()).unwrap_or(6.0);
    st.connectors.length = std::env::var("S3D_PINLEN").ok().and_then(|v| v.parse().ok()).unwrap_or(30.0);
    s3d_core::connectors::finish(&mut part, &ids, &st, &job).unwrap();
    let flagged = part.pieces.iter().filter(|q| !q.flags.is_empty()).count();
    let skipped = part.pieces.iter().filter(|q| q.flags.contains(&s3d_core::partition::Flag::SkippedDowel)).count();
    eprintln!("connectors: {:?}; {} flagged, {} with skipped pins", t2.elapsed(), flagged, skipped);
    let mut kinds = std::collections::BTreeMap::new();
    for q in &part.pieces {
        for f in &q.flags {
            *kinds.entry(f.short()).or_insert(0) += 1;
        }
    }
    eprintln!("flags: {kinds:?}");
    let mut widths: Vec<f64> = part.seams.iter().map(|s| s3d_core::connectors::contact_width(&part, s)).collect();
    widths.sort_by(|a, b| a.total_cmp(b));
    let q = |f: f64| widths[((widths.len() - 1) as f64 * f) as usize];
    eprintln!("suggested pin diameter: {:?}", s3d_core::connectors::suggest_pin_size(&part, &st.connectors));
    eprintln!("seam contact widths mm: min {:.1} p25 {:.1} median {:.1} p75 {:.1} max {:.1}", q(0.0), q(0.25), q(0.5), q(0.75), q(1.0));
}
