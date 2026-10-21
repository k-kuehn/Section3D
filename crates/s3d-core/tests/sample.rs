//! Checks on the real 1M-triangle sample model. Set S3D_SAMPLE to its STL
//! path (skipped otherwise); S3D_SHOTS to a folder also saves renders of the
//! split, S3D_EXPORT_DIR to a folder also exports it. S3D_TIME sets the
//! Shatter time limit (s). S3D_PROJECT checks part numbers on a saved project.

use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use s3d_core::job::JobCtx;
use s3d_core::mesh::{Aabb, Mesh};
use s3d_core::partition::{Flag, Partition};
use s3d_core::render::{DrawMesh, render};
use s3d_core::settings::{Settings, SplitMode};

fn env_path(k: &str) -> Option<std::path::PathBuf> {
    std::env::var_os(k).map(std::path::PathBuf::from)
}

fn sample() -> Option<Mesh> {
    let path = env_path("S3D_SAMPLE").filter(|p| p.exists())?;
    let t = Instant::now();
    let mesh = s3d_core::io::load(&path).unwrap();
    let load = t.elapsed();
    let r = s3d_core::repair::repair(mesh, &JobCtx::new()).unwrap();
    eprintln!("load {load:?}, check/repair {:?}, {} triangles, {}", t.elapsed() - load, r.mesh.tris.len(), r.health.label());
    assert!(load.as_secs_f32() < 1.0, "opening took {load:?}");
    Some(r.mesh)
}

fn settings(mode: SplitMode) -> Settings {
    let mut s = Settings::default();
    s.split.mode = mode;
    s.split.time_limit = std::env::var("S3D_TIME").ok().and_then(|v| v.parse().ok()).unwrap_or(30.0);
    s
}

fn split(mesh: &Mesh, s: &Settings) -> Partition {
    let t = Instant::now();
    let job = JobCtx::new();
    let out = s3d_core::split::run(s3d_core::split::SplitInput { mesh: Arc::new(mesh.clone()), settings: s.clone(), zones: Default::default(), keep: vec![], keep_planes: vec![], preview: None }, &job).unwrap();
    let mut p = out.partition;
    let cut = t.elapsed();
    let ids: Vec<u32> = p.pieces.iter().map(|q| q.id).collect();
    s3d_core::connectors::finish(&mut p, &ids, s, &job).unwrap();
    let mut kinds = std::collections::BTreeMap::new();
    for f in p.pieces.iter().flat_map(|q| &q.flags) {
        *kinds.entry(f.short()).or_insert(0) += 1;
    }
    let numbered = p.pieces.iter().filter(|q| q.label_place.is_some()).count();
    eprintln!("{:?}: {} pieces in {cut:?} (+ features {:?}); flags {kinds:?}; {numbered} numbered; {:?}", s.split.mode, p.pieces.len(), t.elapsed() - cut, out.stats);
    assert!((p.total_volume() - mesh.volume()).abs() / mesh.volume() < 2e-3);
    assert!(p.pieces.iter().all(|q| !q.flags.contains(&Flag::TooBig)));
    // Every piece gets a part number, except chips and slivers too small to
    // carry one (only Grid, which never re-cuts, leaves those).
    let unnumbered: Vec<_> = p.pieces.iter().filter(|q| q.label_place.is_none()).map(|q| (q.label, q.bbox.size())).collect();
    assert!(unnumbered.iter().all(|(_, s)| s.min_element() < 10.0 && s.max_element() < 100.0), "{unnumbered:?}");
    p
}

/// Renders every piece (flagged ones red) from the front-right.
fn shot(p: &Partition, name: &str) {
    let Some(dir) = env_path("S3D_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let bb = p.pieces.iter().fold(Aabb::EMPTY, |b, q| b.union(&q.bbox));
    let color = |q: &s3d_core::partition::Piece| {
        if q.flags.iter().any(|f| *f != Flag::SkippedDowel) { [0.95, 0.2, 0.15] } else { [0.6 + 0.3 * (q.label as f32 * 0.618).fract(), 0.75, 0.82] }
    };
    let items: Vec<DrawMesh> = p.pieces.iter().map(|q| DrawMesh { mesh: q.display_mesh(), model: Mat4::IDENTITY, color: color(q), id: q.id }).collect();
    let img = render(&items, s3d_core::render::iso_camera(&bb, 1.4), 1400, 1000, [250, 250, 252]);
    std::fs::write(dir.join(format!("{name}.png")), img.to_png()).unwrap();
}

/// The sample is modelled Y-up: turn it Z-up, centred, on the bed.
fn z_up(mut m: Mesh) -> Mesh {
    m.transform(&Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2));
    let bb = m.bbox();
    m.translate(Vec3::new(-bb.center().x, -bb.center().y, -bb.min.z));
    m
}

#[test]
fn sample_grid_and_shatter() {
    let Some(mesh) = sample() else { return };
    let mesh = z_up(mesh);
    let grid = split(&mesh, &settings(SplitMode::Grid));
    shot(&grid, "grid");
    let shatter = split(&mesh, &settings(SplitMode::Shatter));
    shot(&shatter, "shatter");
    if let Some(dir) = env_path("S3D_EXPORT_DIR") {
        let s = settings(SplitMode::Shatter);
        let items: Vec<s3d_core::pack::PackItem> = shatter.pieces.iter().map(|q| s3d_core::pack::PackItem { id: q.id, mesh: q.display_mesh().clone(), print: q.orient.map(|o| o.matrix()).unwrap_or_default() }).collect();
        let u = s.printer.usable();
        let t = Instant::now();
        let layout = s3d_core::pack::pack(&items, &s3d_core::pack::PackParams { bed: [u[0], u[1]], spacing: 4.0, keep_order: false, angle_step: 15.0 });
        let r = s3d_core::export::run(&dir, "Sample", &shatter, &layout, &s, &JobCtx::new()).unwrap();
        eprintln!("pack + export {:?}: {} beds; {}", t.elapsed(), layout.beds.len(), r.summary());
    }
}

/// Part numbers on a saved project (S3D_PROJECT; S3D_LABEL sets the text
/// size, S3D_EMBOSS embosses): every piece must take one on its convex and
/// on its concave side, and the two must be on opposite skins.
#[test]
fn project_numbers_both_sides() {
    use s3d_core::settings::{LabelFace, LabelMode};
    let Some(path) = env_path("S3D_PROJECT").filter(|p| p.exists()) else { return };
    let d = s3d_core::project::load(&path, &JobCtx::new()).unwrap();
    let p = d.split.unwrap().partition;
    let mut c = d.settings.connectors.clone();
    c.labels = if std::env::var_os("S3D_EMBOSS").is_some() { LabelMode::Emboss } else { LabelMode::Deboss };
    if let Some(v) = std::env::var("S3D_LABEL").ok().and_then(|v| v.parse().ok()) {
        c.label_size = v;
    }
    let t = Instant::now();
    let model = s3d_core::labels::Surroundings::new(p.pieces.iter().map(|q| &*q.mesh));
    let (mut missing, mut same_skin, mut smaller) = (Vec::new(), Vec::new(), 0);
    for q in &p.pieces {
        let planes: Vec<_> = p.seams_of(q.id).map(|s| s.plane).collect();
        let mut spots = Vec::new();
        for face in [LabelFace::Convex, LabelFace::Concave] {
            c.label_face = face;
            match s3d_core::labels::place_label(&q.mesh, &planes, q.orient.as_ref(), &q.label.to_string(), &c, false, &model) {
                Some(l) if l.convex == (face == LabelFace::Convex) && !l.on_cut => {
                    smaller += (l.size < c.label_size) as usize;
                    spots.push((l.center, l.z));
                    let text = s3d_core::labels::label_solid_at(&l).unwrap();
                    let out = if l.mode == LabelMode::Emboss { q.solid.union(&text) } else { q.solid.subtract(&text) }.unwrap();
                    assert!((out.volume() - q.solid.volume()).abs() > 1e-3, "piece {}: the number does not cut", q.label);
                }
                _ => missing.push((q.label, face)),
            }
        }
        // Both on one skin: facing the same way in the same surface.
        if let [(c1, n1), (c2, n2)] = spots[..] {
            if n1.dot(n2) > 0.95 && (c2 - c1).dot(n1).abs() < 1.0 {
                same_skin.push(q.label);
            }
        }
    }
    eprintln!("{} pieces in {:?} ({} mm {:?}): {smaller} numbers smaller than asked; no room on {missing:?}; both on one skin on {same_skin:?}", p.pieces.len(), t.elapsed(), c.label_size, c.labels);
    assert!(missing.is_empty());
    assert!(same_skin.is_empty(), "{same_skin:?}");
}
