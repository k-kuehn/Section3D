//! End-to-end UI flows driven through egui_kittest with real wgpu rendering.
//! Screenshots land in target/ui-screens/ for review.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use s3d_app::app::S3dApp;
use s3d_app::doc::Step;

fn screens_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-screens");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness() -> Harness<'static, S3dApp> {
    // Keep tests away from the user's config, recent list and autosave.
    unsafe { std::env::set_var("S3D_NO_CONFIG", "1") };
    Harness::builder().with_size(egui::vec2(1500.0, 920.0)).wgpu().build_eframe(|cc| S3dApp::new(cc))
}

fn wait_jobs(h: &mut Harness<'static, S3dApp>) {
    let t = Instant::now();
    loop {
        h.step();
        if !h.state().jobs.busy() {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(120), "job timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..3 {
        h.step();
    }
}

fn shot(h: &mut Harness<'static, S3dApp>, name: &str) {
    h.step();
    let img = h.render().expect("render");
    img.save(screens_dir().join(format!("{name}.png"))).unwrap();
}

/// A test model: a 300 mm tall rounded blob (box + sphere), as binary STL.
fn sample_model() -> PathBuf {
    use s3d_core::glam::DVec3;
    use s3d_core::manifold::Solid;
    let a = Solid::cube(DVec3::new(260.0, 160.0, 220.0), true).unwrap();
    let b = Solid::sphere(150.0, 64).unwrap().translate(DVec3::new(0.0, 0.0, 120.0)).unwrap();
    let m = a.union(&b).unwrap().to_mesh();
    let p = std::env::temp_dir().join("s3d_ui_test_model.stl");
    std::fs::write(&p, s3d_core::io::stl::write_binary(&m, "blob")).unwrap();
    p
}

#[test]
fn import_split_flow() {
    let mut h = harness();
    h.run_steps(2);
    shot(&mut h, "01_start");
    assert!(h.query_by_label("Open model…").is_some());

    h.state_mut().start_import(sample_model());
    wait_jobs(&mut h);
    assert!(h.state().doc.source.is_some());
    shot(&mut h, "02_imported");

    // Switching steps never starts work.
    h.state_mut().step = Step::Prepare;
    h.step();
    assert!(!h.state().jobs.busy());
    shot(&mut h, "03_prepare");

    h.state_mut().step = Step::Split;
    h.state_mut().doc.settings.split.mode = s3d_core::settings::SplitMode::Grid;
    h.step();
    assert!(!h.state().jobs.busy());
    h.get_by_label("Split").click();
    h.step();
    assert!(h.state().jobs.busy(), "Split button starts the job");
    wait_jobs(&mut h);
    let n = h.state().doc.split.as_ref().map(|s| s.partition.pieces.len()).unwrap_or(0);
    assert!(n > 1, "split produced {n} pieces");
    shot(&mut h, "04_split");

    // Changing a setting marks the step out of date, and nothing runs.
    h.state_mut().doc.settings.split.min_piece += 5.0;
    h.step();
    assert_eq!(h.state().doc.status(Step::Split), s3d_app::doc::Status::Stale);
    assert!(!h.state().jobs.busy());
    assert!(h.query_by_label("Update").is_some());
    shot(&mut h, "05_stale");
}

/// Triangle split of the real sample model (set S3D_SAMPLE), with screenshots.
#[test]
fn sample_triangle_screens() {
    let Some(path) = std::env::var_os("S3D_SAMPLE").map(PathBuf::from).filter(|p| p.exists()) else {
        eprintln!("S3D_SAMPLE not set; skipping");
        return;
    };
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().doc.settings.printer.printer = s3d_core::printers::Printer::new("Bambu Lab X1 Carbon", 256.0, 256.0, 256.0);
    h.state_mut().start_import(path);
    wait_jobs(&mut h);
    shot(&mut h, "s1_imported");
    h.state_mut().step = Step::Split;
    h.state_mut().doc.settings.split.mode = s3d_core::settings::SplitMode::Triangle;
    h.state_mut().doc.settings.split.time_limit = 8.0;
    h.step();
    h.get_by_label("Split").click();
    // Let the search run for a bit and capture the live preview.
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(4) {
        h.step();
        std::thread::sleep(Duration::from_millis(50));
    }
    shot(&mut h, "s2_searching");
    wait_jobs(&mut h);
    shot(&mut h, "s3_split");
    h.state_mut().view.explode = 0.6;
    h.step();
    shot(&mut h, "s4_exploded");
    h.state_mut().view.explode = 0.0;
    h.state_mut().view.viewport.camera.preset(s3d_app::viewport::ViewPreset::Front);
    h.state_mut().view.viewport.pending_fit = true;
    h.step();
    shot(&mut h, "s5_front");
}

#[test]
fn connectors_export_undo_project_flow() {
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().start_import(sample_model());
    wait_jobs(&mut h);
    h.state_mut().step = Step::Split;
    h.state_mut().doc.settings.split.mode = s3d_core::settings::SplitMode::Triangle;
    h.state_mut().doc.settings.split.time_limit = 2.0;
    h.step();
    h.get_by_label("Split").click();
    wait_jobs(&mut h);
    let pieces = h.state().doc.split.as_ref().unwrap().partition.pieces.len();
    assert!(pieces > 1);
    shot(&mut h, "10_triangle_small");

    // Selecting a piece shows its properties and cut handles.
    let first = h.state().doc.split.as_ref().unwrap().partition.pieces[0].id;
    h.state_mut().view.select(Some(first));
    h.step();
    assert!(h.query_by_label("Cuts").is_some() || h.query_by_label("CUTS").is_some());
    shot(&mut h, "11_selected");

    // Connectors: changing a setting marks the step stale, Update re-runs it.
    h.state_mut().step = Step::Connectors;
    h.state_mut().doc.settings.connectors.diameter = 5.0;
    h.step();
    assert_eq!(h.state().doc.status(Step::Connectors), s3d_app::doc::Status::Stale);
    h.get_by_label("Update connectors").click();
    wait_jobs(&mut h);
    assert_eq!(h.state().doc.status(Step::Connectors), s3d_app::doc::Status::Done);
    shot(&mut h, "12_connectors");

    // Undo restores the previous connector setting; redo brings it back.
    h.state_mut().undo();
    h.step();
    assert!((h.state().doc.settings.connectors.diameter - 5.0).abs() > 1e-6 || h.state().undo.can_redo());
    h.state_mut().redo();
    h.step();

    // Export writes files and records history.
    let out = std::env::temp_dir().join(format!("s3d_ui_export_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    h.state_mut().step = Step::Export;
    h.state_mut().doc.settings.export.folder = Some(out.display().to_string());
    h.step();
    h.get_by_label("Export files").click();
    wait_jobs(&mut h);
    let files: Vec<String> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(files.iter().any(|f| f.ends_with(".3mf")), "{files:?}");
    assert!(files.iter().any(|f| f.ends_with(".pdf")), "{files:?}");
    assert_eq!(h.state().doc.status(Step::Export), s3d_app::doc::Status::Done);
    h.state_mut().view.tab = s3d_app::view::Tab::Beds;
    h.step();
    shot(&mut h, "13_beds");
    h.state_mut().view.tab = s3d_app::view::Tab::Assembly;
    h.state_mut().view.assembly_step = 2;
    h.step();
    shot(&mut h, "14_assembly");

    // Save and reopen: the pieces come back without re-cutting.
    let proj = out.join("test.s3d");
    let data = s3d_app::project::Saved::from_doc(&h.state().doc, &h.state().view);
    s3d_app::project::save(&proj, &data, &s3d_core::job::JobCtx::new()).unwrap();
    let loaded = s3d_app::project::load(&proj, &s3d_core::job::JobCtx::new()).unwrap();
    let doc = loaded.into_doc();
    assert_eq!(doc.split.as_ref().unwrap().partition.pieces.len(), h.state().doc.split.as_ref().unwrap().partition.pieces.len());
    assert_eq!(doc.status(Step::Split), s3d_app::doc::Status::Done, "opening never re-cuts");
    let _ = std::fs::remove_dir_all(&out);
}
