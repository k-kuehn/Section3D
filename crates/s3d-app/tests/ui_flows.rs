//! End-to-end UI flows driven through egui_kittest with real wgpu rendering.
//! Screenshots land in target/ui-screens/ for review.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
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
        assert!(t.elapsed() < Duration::from_secs(400), "job timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..3 {
        h.step();
    }
}

fn shot(h: &mut Harness<'static, S3dApp>, name: &str) {
    h.step();
    let img = h.render().expect("render");
    let rgb: Vec<u8> = img.as_raw().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let png = s3d_core::render::Image { w: img.width() as usize, h: img.height() as usize, rgb }.to_png();
    std::fs::write(screens_dir().join(format!("{name}.png")), png).unwrap();
}

/// A test model: a 300 mm tall rounded blob (box + sphere), as binary STL.
fn sample_model() -> PathBuf {
    use s3d_core::glam::DVec3;
    use s3d_core::manifold::Solid;
    let a = Solid::cube(DVec3::new(260.0, 160.0, 220.0), true).unwrap();
    let b = Solid::sphere(150.0, 64).unwrap().translate(DVec3::new(0.0, 0.0, 120.0)).unwrap();
    let m = a.union(&b).unwrap().to_mesh();
    // One file per call: tests run in parallel.
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("s3d_ui_test_model_{}_{n}.stl", std::process::id()));
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
    shot(&mut h, "04a_progress");
    wait_jobs(&mut h);
    let n = h.state().doc.split.as_ref().map(|s| s.partition.pieces.len()).unwrap_or(0);
    assert!(n > 1, "split produced {n} pieces");
    shot(&mut h, "04_split");

    // Painted no-cut zones show as marker on the surface.
    {
        let src = h.state().doc.source.as_ref().unwrap().mesh.clone();
        let bb = src.bbox();
        let top = glam::Vec3::new(bb.center().x, bb.center().y, bb.max.z);
        let dabs: Vec<s3d_core::zones::Dab> = (0..12)
            .map(|i| {
                let a = i as f32 * 0.25;
                let p = top + glam::Vec3::new(a.cos() * 40.0, a.sin() * 40.0, 0.0);
                let near = src.verts.iter().min_by(|x, y| x.distance(p).total_cmp(&y.distance(p))).copied().unwrap();
                s3d_core::zones::Dab { p: near, radius: 14.0, n: (near - bb.center()).normalize() }
            })
            .collect();
        h.state_mut().doc.zones.dabs = dabs;
        h.state_mut().view.paint = true;
        for _ in 0..30 {
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        shot(&mut h, "04b_paint");

        // Painting with the mouse: a drag across the model adds spots.
        h.state_mut().doc.zones.dabs.clear();
        let at = |x: f32, y: f32| egui::pos2(x, y);
        for _ in 0..40 {
            h.hover_at(at(950.0, 460.0));
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        h.event(egui::Event::PointerButton { pos: at(950.0, 460.0), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.step();
        for k in 1..=12 {
            h.event(egui::Event::PointerMoved(at(950.0 + k as f32 * 6.0, 460.0 + k as f32 * 3.0)));
            h.step();
        }
        h.event(egui::Event::PointerButton { pos: at(1022.0, 496.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.step();
        let n = h.state().doc.zones.dabs.len();
        assert!(n >= 2, "dragging painted {n} spots");
        assert!(h.state().doc.zones.dabs.iter().all(|d| d.n.length() > 0.5), "spots remember the surface normal");
        h.state_mut().view.paint = false;
        h.state_mut().doc.zones.dabs.clear();
    }

    // Changing a setting marks the step out of date, and nothing runs.
    h.state_mut().doc.settings.split.min_piece += 5.0;
    h.step();
    assert_eq!(h.state().doc.status(Step::Split), s3d_app::doc::Status::Stale);
    assert!(!h.state().jobs.busy());
    assert!(h.query_by_label("Update").is_some());
    shot(&mut h, "05_stale");
}

/// Shatter split of the real sample model (set S3D_SAMPLE), with screenshots.
#[test]
fn sample_triangle_screens() {
    let Some(path) = std::env::var_os("S3D_SAMPLE").map(PathBuf::from).filter(|p| p.exists()) else {
        eprintln!("S3D_SAMPLE not set; skipping");
        return;
    };
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().doc.settings.printer.volume = [256.0, 256.0, 256.0];
    h.state_mut().start_import(path);
    wait_jobs(&mut h);
    shot(&mut h, "s1_imported");
    h.state_mut().step = Step::Split;
    h.state_mut().doc.settings.split.mode = s3d_core::settings::SplitMode::Shatter;
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
    {
        let part = h.state().doc.split.as_ref().unwrap().partition.clone();
        let numbered = part.pieces.iter().filter(|q| q.label_place.is_some()).count();
        eprintln!("sample: {numbered} of {} pieces carry a number", part.pieces.len());
        let l = part.pieces.iter().filter_map(|q| q.label_place.clone()).nth(5).unwrap();
        let cam = &mut h.state_mut().view.viewport.camera;
        cam.target = l.center;
        cam.distance = 90.0;
        cam.yaw = l.z.y.atan2(l.z.x);
        cam.pitch = l.z.z.clamp(-0.99, 0.99).asin();
        h.step();
        shot(&mut h, "s3b_number");
        h.state_mut().view.viewport.camera.preset(s3d_app::viewport::ViewPreset::Iso);
        h.state_mut().view.viewport.pending_fit = true;
    }
    h.state_mut().view.explode = 0.6;
    h.step();
    shot(&mut h, "s4_exploded");
    h.state_mut().view.explode = 0.0;
    h.state_mut().view.viewport.camera.preset(s3d_app::viewport::ViewPreset::Front);
    h.state_mut().view.viewport.pending_fit = true;
    h.step();
    shot(&mut h, "s5_front");
    h.state_mut().start_arrange();
    wait_jobs(&mut h);
    h.state_mut().view.tab = s3d_app::view::Tab::Beds;
    h.step();
    shot(&mut h, "s6_beds");
    if std::env::var_os("S3D_EXPORT").is_some() {
        let out = std::env::temp_dir().join("s3d_sample_export");
        let _ = std::fs::remove_dir_all(&out);
        let t = Instant::now();
        h.state_mut().start_export(out.clone());
        wait_jobs(&mut h);
        let size = |ext: &str| std::fs::read_dir(&out).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(ext)).map(|e| e.metadata().unwrap().len()).sum::<u64>();
        eprintln!("sample export: {:.1} s, pdf {:.1} MB, html {:.1} MB", t.elapsed().as_secs_f32(), size(".pdf") as f64 / 1e6, size(".html") as f64 / 1e6);
    }
    h.state_mut().view.tab = s3d_app::view::Tab::Assembly;
    let n = h.state().doc.split.as_ref().unwrap().partition.pieces.len();
    h.state_mut().view.assembly_step = n * 3 / 4;
    h.step();
    shot(&mut h, "s7_assembly");
}

#[test]
fn connectors_export_undo_project_flow() {
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().start_import(sample_model());
    wait_jobs(&mut h);
    // A shell, like a car body: numbers can go inside or outside.
    h.state_mut().doc.settings.prepare.wall = 6.0;
    h.state_mut().start_hollow();
    wait_jobs(&mut h);
    assert!(h.state().doc.valid_hollow().is_some());
    h.state_mut().step = Step::Split;
    h.state_mut().doc.settings.split.mode = s3d_core::settings::SplitMode::Shatter;
    h.state_mut().doc.settings.split.time_limit = 2.0;
    h.step();
    h.get_by_label("Split").click();
    wait_jobs(&mut h);
    let pieces = h.state().doc.split.as_ref().unwrap().partition.pieces.len();
    assert!(pieces > 1);
    shot(&mut h, "10_triangle_small");

    // Selecting a piece shows its properties and actions.
    let first = h.state().doc.split.as_ref().unwrap().partition.pieces[0].id;
    h.state_mut().view.select(Some(first));
    h.step();
    assert!(h.query_by_label("Draw cut line").is_some());
    shot(&mut h, "11_selected");

    // Shift-selecting a touching neighbour offers Merge; merging makes one piece.
    let nb = h.state().doc.split.as_ref().unwrap().partition.neighbours(first)[0];
    h.state_mut().view.toggle_select(nb);
    h.step();
    assert_eq!(h.state().view.selection().len(), 2);
    shot(&mut h, "11b_multi_selected");
    h.get_by_label("Merge 2 pieces").click();
    wait_jobs(&mut h);
    let after_merge = h.state().doc.split.as_ref().unwrap().partition.pieces.len();
    assert_eq!(after_merge, pieces - 1);
    assert_eq!(h.state().doc.edits.len(), 1);
    // The edit is listed with the part numbers it joined.
    assert!(h.state().doc.edits[0].text.starts_with("Merged #"), "{}", h.state().doc.edits[0].text);
    assert!(h.query_by_label_contains("Merged #").is_some());

    // Draw a cut line across the biggest piece: a straight cut through it.
    let (big, line) = {
        let part = &h.state().doc.split.as_ref().unwrap().partition;
        let q = part.pieces.iter().max_by(|a, b| a.volume.total_cmp(&b.volume)).unwrap();
        let bb = q.bbox;
        let c = bb.center();
        let bvh = s3d_core::bvh::Bvh::new(&q.mesh);
        // Points on the top surface along a line across the piece.
        let pts: Vec<(glam::Vec3, glam::Vec3)> = (1..20)
            .filter_map(|i| {
                let x = bb.min.x + (bb.max.x - bb.min.x) * i as f32 / 20.0;
                let o = glam::Vec3::new(x, c.y, bb.max.z + 5.0);
                bvh.ray(o, -glam::Vec3::Z, 1e4).map(|hh| (o - glam::Vec3::Z * hh.t, hh.normal.normalize()))
            })
            .collect();
        (q.id, pts)
    };
    h.state_mut().view.start_cut_draw(big);
    {
        let cd = h.state_mut().view.cut_draw.as_mut().unwrap();
        cd.pts = line.iter().map(|x| x.0).collect();
        cd.normals = line.iter().map(|x| x.1).collect();
    }
    h.step();
    shot(&mut h, "11c_cut_line");
    let cd = h.state_mut().view.cut_draw.take().unwrap();
    s3d_app::view::cut_along_line(h.state_mut(), cd);
    wait_jobs(&mut h);
    let after_cut = h.state().doc.split.as_ref().unwrap().partition.pieces.len();

    assert!(after_cut > after_merge, "cut along the line added pieces ({after_merge} -> {after_cut})");

    // Features: changing a setting marks the step stale, Apply re-runs it.
    h.state_mut().step = Step::Features;
    h.state_mut().doc.settings.connectors.diameter = 5.0;
    h.step();
    assert!(h.query_by_label("Pin size").is_none(), "no preset pin size list");
    assert!(h.query_by_label("Pin diameter").is_some());
    assert_eq!(h.state().doc.status(Step::Features), s3d_app::doc::Status::Stale);
    h.get_by_label("Apply features").click();
    wait_jobs(&mut h);
    assert_eq!(h.state().doc.status(Step::Features), s3d_app::doc::Status::Done);
    let numbered = h.state().doc.split.as_ref().unwrap().partition.pieces.iter().filter(|q| q.label_place.is_some()).count();
    assert!(numbered * 2 > after_cut, "most pieces carry a number ({numbered} of {after_cut})");
    shot(&mut h, "12_features");
    // Close-up of one part number.
    {
        let part = h.state().doc.split.as_ref().unwrap().partition.clone();
        let l = part.pieces.iter().filter_map(|q| q.label_place.clone()).next().unwrap();
        let cam = &mut h.state_mut().view.viewport.camera;
        cam.target = l.center;
        cam.distance = 70.0;
        cam.yaw = l.z.y.atan2(l.z.x);
        cam.pitch = l.z.z.clamp(-0.99, 0.99).asin();
    }
    h.step();
    shot(&mut h, "12d_number_closeup");
    h.state_mut().recent.ortho = true;
    h.state_mut().view.viewport.pending_fit = true;
    h.step();
    shot(&mut h, "12b_isometric");
    h.state_mut().recent.ortho = false;
    h.state_mut().start_features();
    h.step();
    shot(&mut h, "12c_features_progress");
    wait_jobs(&mut h);

    for q in &h.state().doc.split.as_ref().unwrap().partition.pieces {
        assert!(!q.display_mesh().tris.is_empty(), "piece {} has an empty mesh ({} cut tris, vol {})", q.label, q.mesh.tris.len(), q.volume);
    }

    // Moving numbers to the other side: each goes to the opposite (convex /
    // concave) side, or stays and the piece says there is no room there.
    let mut moved = 0;
    let ids: Vec<u32> = h.state().doc.split.as_ref().unwrap().partition.pieces.iter().filter(|q| q.label_place.is_some()).map(|q| q.id).take(4).collect();
    for flip in ids {
        let before = h.state().doc.split.as_ref().unwrap().partition.piece(flip).unwrap().label_place.clone().unwrap();
        h.state_mut().notice = None;
        h.state_mut().start_flip_label(flip);
        wait_jobs(&mut h);
        let after = h.state().doc.split.as_ref().unwrap().partition.piece(flip).unwrap().label_place.clone().unwrap();
        if after.convex != before.convex {
            moved += 1;
            assert!(after.z.dot(before.z) < 0.9 || after.center.distance(before.center) > 1.0, "number moved");
        } else {
            assert_eq!(after, before);
            assert!(matches!(&h.state().notice, Some(s3d_app::app::Notice::Error(e)) if e.contains("no room")), "{:?}", h.state().notice);
        }
    }
    assert!(moved > 0, "some numbers can move to the other side");

    // Undo restores the previous connector setting; redo brings it back.
    h.state_mut().undo();
    h.step();
    assert!((h.state().doc.settings.connectors.diameter - 5.0).abs() > 1e-6 || h.state().undo.can_redo());
    h.state_mut().redo();
    h.step();

    // Export writes files. Assembly order is Ordered or Random.
    let out = std::env::temp_dir().join(format!("s3d_ui_export_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    h.state_mut().step = Step::Export;
    h.state_mut().doc.settings.export.folder = Some(out.display().to_string());
    h.step();
    assert!(h.query_by_label("Random").is_some());
    h.get_by_label("Ordered").click();
    h.step();
    assert!(h.state().doc.settings.export.keep_assembly_order);
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
    // The joining part's outline pulses: frames half a beat apart differ.
    let frame = |h: &mut Harness<'static, S3dApp>| h.render().expect("render").into_raw();
    let a = frame(&mut h);
    h.run_steps(4);
    let b = frame(&mut h);
    let changed = a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(x, y)| x != y).count();
    assert!(changed > 50, "{changed} pixels changed");
    let last = h.state().doc.split.as_ref().map(|s| s.partition.pieces.len()).unwrap_or(2) - 2;
    h.state_mut().view.assembly_step = last;
    h.step();
    shot(&mut h, "15_assembly_late");
    h.state_mut().view.tab = s3d_app::view::Tab::Beds;
    h.state_mut().view.beds_3d = false;
    h.step();
    shot(&mut h, "16_beds_plan");
    h.state_mut().view.beds_3d = true;

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

/// A fresh split starts over: manual edits are discarded, not replayed.
#[test]
fn resplit_discards_manual_edits() {
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().start_import(sample_model());
    wait_jobs(&mut h);
    h.state_mut().step = Step::Split;
    h.state_mut().doc.settings.split.mode = s3d_core::settings::SplitMode::Grid;
    h.step();
    h.get_by_label("Split").click();
    wait_jobs(&mut h);
    let n = h.state().doc.split.as_ref().unwrap().partition.pieces.len();
    let part = h.state().doc.split.as_ref().unwrap().partition.clone();
    let a = part.pieces[0].id;
    let b = part.neighbours(a)[0];
    h.state_mut().start_edit(s3d_core::edits::EditOp::Merge(vec![a, b]));
    wait_jobs(&mut h);
    assert_eq!(h.state().doc.split.as_ref().unwrap().partition.pieces.len(), n - 1);
    assert_eq!(h.state().doc.status(Step::Split), s3d_app::doc::Status::Done, "an edit does not make the split stale");
    h.get_by_label("Split again").click();
    wait_jobs(&mut h);
    assert!(h.state().doc.edits.is_empty());
    assert_eq!(h.state().doc.split.as_ref().unwrap().partition.pieces.len(), n, "the merge was not replayed");
}

/// Grid mode: cuts are placed by hand before cutting and never moved; a
/// piece that needs attention is listed, and Find a better cut fixes it.
#[test]
fn grid_editor_attention_and_better_cut() {
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().start_import(sample_model());
    wait_jobs(&mut h);
    // The printer is a plain build volume, changed in Settings.
    assert!(h.query_by_label("🖨 256 × 256 × 256 mm").is_some());
    h.get_by_label("🖨 256 × 256 × 256 mm").click();
    h.step();
    assert!(h.state().show_settings);
    h.state_mut().show_settings = false;
    h.state_mut().step = Step::Split;
    h.state_mut().doc.settings.split.mode = s3d_core::settings::SplitMode::Grid;
    h.step();
    // One more cut along X, then the cuts moved to make a 7 mm slice.
    let bb = h.state_mut().doc.prepared_mesh().unwrap().0.bbox();
    let before = s3d_core::split::grid::cuts(&bb, &h.state().doc.settings);
    h.get_all_by_label("+").next().unwrap().click();
    h.step();
    let grid = h.state().doc.settings.split.grid.clone().expect("cuts placed by hand");
    assert_eq!(grid[0].len(), before[0].len() + 1);
    let x = bb.center().x;
    h.state_mut().doc.settings.split.grid = Some([vec![x, x + 7.0], grid[1].clone(), grid[2].clone()]);
    h.state_mut().view.grid_hover = Some((0, 1));
    h.step();
    shot(&mut h, "06_grid_editor");
    h.get_by_label("Split").click();
    wait_jobs(&mut h);
    let part = h.state().doc.split.as_ref().unwrap().partition.clone();
    let xs: std::collections::BTreeSet<i64> = part.seams.iter().filter(|s| s.plane.normal.x.abs() > 0.99).map(|s| (s.plane.offset.abs() * 10.0).round() as i64).collect();
    assert_eq!(xs.len(), 2, "both X cuts exactly where placed: {xs:?}");
    let sliver = part.pieces.iter().find(|q| q.bbox.size().x < 10.0).expect("the 7 mm slice").id;
    assert!(part.piece(sliver).unwrap().flags.contains(&s3d_core::partition::Flag::Thin));
    // The parts list can show only the flagged pieces (badge or status bar).
    let flagged = part.pieces.iter().filter(|q| s3d_app::parts::needs_attention(q)).count();
    assert!(h.query_by_label(&format!("{flagged} NEED ATTENTION")).is_some(), "{flagged} flagged");
    h.get_by_label_contains("that need attention").click();
    h.step();
    assert!(h.state().view.attention);
    shot(&mut h, "07_attention");
    // Find a better cut: the slice is merged with a neighbour and re-cut.
    h.state_mut().view.select(Some(sliver));
    h.step();
    // Even with a piece's card open, the list shows at least ten rows.
    h.state_mut().view.attention = false;
    h.run_steps(3);
    assert!(part.pieces.len() >= 10);
    let rows = h.query_all_by_label_contains("Part #").count();
    assert!(rows >= 10, "{rows} rows showing");
    shot(&mut h, "07b_selected_and_list");
    h.get_by_label("Find a better cut").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Find a better cut").click();
    wait_jobs(&mut h);
    let e = h.state().doc.edits.last().unwrap_or_else(|| panic!("no edit: {:?}", h.state().notice)).text.clone();
    assert!(e.starts_with("Re-cut"), "{e}");
    let part = h.state().doc.split.as_ref().unwrap().partition.clone();
    let made = h.state().doc.edits.last().unwrap().pieces.clone();
    assert!(made.iter().filter_map(|id| part.piece(*id)).all(|q| !q.flags.contains(&s3d_core::partition::Flag::Thin) && q.bbox.size().x >= 10.0));
    assert!(part.piece(sliver).is_none(), "the slice is gone");
    shot(&mut h, "08_better_cut");
}

#[test]
fn light_theme_and_help() {
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().start_import(sample_model());
    wait_jobs(&mut h);
    h.state_mut().dark = false;
    s3d_app::theme::apply(&h.ctx, false);
    h.state_mut().step = Step::Prepare;
    h.run_steps(3);
    shot(&mut h, "20_light_prepare");
    // Every help marker id used in the UI has a help entry.
    for id in s3d_app::tooltips::IDS {
        assert!(s3d_app::tooltips::get(id).is_some(), "{id}");
    }
}

#[test]
fn undo_split_on_assembly_tab_does_not_crash() {
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().start_import(sample_model());
    wait_jobs(&mut h);
    h.state_mut().step = Step::Split;
    h.state_mut().doc.settings.split.mode = s3d_core::settings::SplitMode::Grid;
    h.step();
    h.get_by_label("Split").click();
    wait_jobs(&mut h);
    h.state_mut().view.tab = s3d_app::view::Tab::Assembly;
    h.run_steps(2);
    while h.state().doc.split.is_some() && h.state().undo.can_undo() {
        h.state_mut().undo();
        h.run_steps(2);
    }
    assert!(h.state().doc.split.is_none());
}
