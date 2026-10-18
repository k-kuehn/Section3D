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
        s3d_core::split::SplitInput { mesh: std::sync::Arc::new(mesh), settings, zones: Default::default(), edits: vec![], keep: vec![], preview: None },
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

#[test]
fn triangle_split_sample() {
    let Some(path) = sample() else { return };
    let mesh = s3d_core::io::load(&path).unwrap();
    let mesh = s3d_core::repair::repair(mesh, &JobCtx::new()).unwrap().mesh;
    let mut settings = s3d_core::settings::Settings::default();
    settings.printer.printer = s3d_core::printers::Printer::new("X1C", 256.0, 256.0, 256.0);
    settings.split.mode = s3d_core::settings::SplitMode::Triangle;
    settings.split.time_limit = std::env::var("S3D_TIME").ok().and_then(|v| v.parse().ok()).unwrap_or(20.0);
    let t = Instant::now();
    let first = std::sync::Arc::new(std::sync::Mutex::new(None::<std::time::Duration>));
    let f2 = first.clone();
    let t0 = t;
    let preview: s3d_core::split::triangle::PreviewFn = std::sync::Arc::new(move |edges, msg| {
        let mut g = f2.lock().unwrap();
        if g.is_none() {
            *g = Some(t0.elapsed());
        }
        eprintln!("[preview {:?}] {} edges, {}", t0.elapsed(), edges.len(), msg);
    });
    let job = JobCtx::new();
    let out = s3d_core::split::run(
        s3d_core::split::SplitInput { mesh: std::sync::Arc::new(mesh), settings, zones: Default::default(), edits: vec![], keep: vec![], preview: Some(preview) },
        &job,
    )
    .unwrap();
    let p = &out.partition;
    let mut kinds = std::collections::BTreeMap::new();
    for q in &p.pieces {
        for f in &q.flags {
            *kinds.entry(f.short()).or_insert(0) += 1;
        }
    }
    eprintln!("triangle: {} pieces in {:?}; first preview at {:?}; flags {:?}", p.pieces.len(), t.elapsed(), first.lock().unwrap(), kinds);
    let mut dims: Vec<f32> = p.pieces.iter().map(|q| q.orient.map(|o| o.size.max_element()).unwrap_or(0.0)).collect();
    dims.sort_by(|a, b| a.total_cmp(b));
    let q = |f: f32| dims[((dims.len() - 1) as f32 * f) as usize];
    eprintln!("piece max dim mm: p10 {:.0} p25 {:.0} median {:.0} p75 {:.0} p90 {:.0}", q(0.1), q(0.25), q(0.5), q(0.75), q(0.9));
    let area: f64 = p.pieces.iter().map(|q| q.mesh.area()).sum::<f64>() / 2.0;
    eprintln!("surface area per piece (half of total area): {:.0} mm2", area / p.pieces.len() as f64);
}

#[test]
fn export_sample() {
    let Some(path) = sample() else { return };
    let Some(out_dir) = std::env::var_os("S3D_EXPORT_DIR").map(std::path::PathBuf::from) else { return };
    let mesh = s3d_core::io::load(&path).unwrap();
    let mesh = s3d_core::repair::repair(mesh, &JobCtx::new()).unwrap().mesh;
    let mut s = s3d_core::settings::Settings::default();
    s.printer.printer = s3d_core::printers::Printer::new("X1C", 256.0, 256.0, 256.0);
    s.split.time_limit = 6.0;
    s.connectors.diameter = 2.0;
    s.connectors.length = 8.0;
    let job = JobCtx::new();
    let out = s3d_core::split::run(s3d_core::split::SplitInput { mesh: std::sync::Arc::new(mesh), settings: s.clone(), zones: Default::default(), edits: vec![], keep: vec![], preview: None }, &job).unwrap();
    let mut part = out.partition;
    let ids: Vec<u32> = part.pieces.iter().map(|q| q.id).collect();
    let t = Instant::now();
    s3d_core::connectors::finish(&mut part, &ids, &s, &job).unwrap();
    eprintln!("connectors {:?}", t.elapsed());
    let t = Instant::now();
    let items: Vec<s3d_core::pack::PackItem> = part.pieces.iter().map(|q| s3d_core::pack::PackItem { id: q.id, mesh: q.display_mesh().clone(), print: q.orient.map(|o| o.matrix()).unwrap_or_default() }).collect();
    let u = s.printer.usable();
    let layout = s3d_core::pack::pack(&items, &s3d_core::pack::PackParams { bed: [u[0], u[1]], spacing: 4.0, keep_order: false, angle_step: 15.0 });
    eprintln!("pack {:?}: {} pieces on {} beds ({} unplaced)", t.elapsed(), part.pieces.len(), layout.beds.len(), layout.unplaced.len());
    let t = Instant::now();
    let r = s3d_core::export::run(&out_dir, "Body", &part, &layout, &s, &job).unwrap();
    eprintln!("export {:?}: {}", t.elapsed(), r.summary());
}
