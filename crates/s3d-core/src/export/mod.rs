//! Export: one 3MF per bed, optional single-part STLs, a print list (CSV),
//! an assembly guide (PDF) and an offline viewer (HTML).

pub mod pdf;
pub mod threemf;
pub mod viewer;

use std::path::{Path, PathBuf};

use glam::{Mat4, Vec3};
use rayon::prelude::*;

use crate::assembly;
use crate::job::JobCtx;
use crate::mesh::{Aabb, Mesh};
use crate::pack::{self, Layout};
use crate::partition::{Flag, Partition, Piece};
use crate::render::{self, DrawMesh};
use crate::settings::Settings;

pub struct ExportResult {
    pub files: Vec<PathBuf>,
    pub folder: PathBuf,
}

impl ExportResult {
    pub fn summary(&self) -> String {
        format!("Exported {} files to {}", self.files.len(), self.folder.display())
    }
}

/// Fills a name template ({project}, {bed}, {part}) and makes it file-safe.
pub fn file_name(template: &str, project: &str, bed: Option<usize>, part: Option<u32>) -> String {
    let mut s = template.replace("{project}", project);
    s = s.replace("{bed}", &bed.map(|b| format!("{:02}", b + 1)).unwrap_or_default());
    s = s.replace("{part}", &part.map(|p| format!("{p:03}")).unwrap_or_default());
    let clean: String = s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let t = clean.trim().trim_end_matches('.').to_string();
    if t.is_empty() { "section3d".into() } else { t }
}

/// Shrinkage compensation: scales a print-oriented mesh about its footprint
/// centre on the bed (X/Y) and the bed surface (Z).
fn shrink(m: &Mesh, s: &Settings, centre: Vec3) -> Mesh {
    let (kx, kz) = (1.0 + s.export.shrink_xy / 100.0, 1.0 + s.export.shrink_z / 100.0);
    if (kx - 1.0).abs() < 1e-6 && (kz - 1.0).abs() < 1e-6 {
        return m.clone();
    }
    let t = Mat4::from_translation(Vec3::new(centre.x, centre.y, 0.0)) * Mat4::from_scale(Vec3::new(kx, kx, kz)) * Mat4::from_translation(Vec3::new(-centre.x, -centre.y, 0.0));
    m.transformed(&t)
}

pub fn piece_print_matrix(q: &Piece) -> Mat4 {
    q.orient.map(|o| o.matrix()).unwrap_or(Mat4::IDENTITY)
}

pub fn weight_g(q: &Piece, s: &Settings) -> f64 {
    q.volume / 1000.0 * s.export.density as f64
}

fn flags_text(q: &Piece) -> String {
    q.flags.iter().map(|f| f.short().to_lowercase()).collect::<Vec<_>>().join("; ")
}

fn piece_color(label: u32) -> [f32; 3] {
    let h = (label as f32 * 0.618_034).fract();
    let (s, v) = (0.35, 0.85);
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

pub fn run(folder: &Path, title: &str, part: &Partition, layout: &Layout, s: &Settings, job: &JobCtx) -> Result<ExportResult, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
    let mut files = Vec::new();
    let write = |name: &str, bytes: &[u8], files: &mut Vec<PathBuf>| -> Result<(), String> {
        let p = folder.join(name);
        std::fs::write(&p, bytes).map_err(|e| format!("cannot write {}: {e}", p.display()))?;
        files.push(p);
        Ok(())
    };
    let margin = s.printer.margin;
    // Beds.
    job.set_stage("Writing beds");
    let beds: Vec<(usize, Vec<u8>)> = layout
        .beds
        .par_iter()
        .enumerate()
        .map(|(b, placements)| {
            let meshes: Vec<(String, Mesh)> = placements
                .iter()
                .filter_map(|pl| {
                    let q = part.piece(pl.piece)?;
                    let m = pack::bed_transform(&piece_print_matrix(q), pl);
                    let placed = q.display_mesh().transformed(&(Mat4::from_translation(Vec3::new(margin, margin, 0.0)) * m));
                    let placed = shrink(&placed, s, Vec3::new(pl.x + margin, pl.y + margin, 0.0));
                    Some((format!("Part {}", q.label), placed))
                })
                .collect();
            let refs: Vec<(String, &Mesh)> = meshes.iter().map(|(n, m)| (n.clone(), m)).collect();
            (b, threemf::write(&refs, &format!("{title} bed {}", b + 1)))
        })
        .collect();
    for (b, bytes) in beds {
        job.check().map_err(|e| e.to_string())?;
        write(&format!("{}.3mf", file_name(&s.export.bed_name, title, Some(b), None)), &bytes, &mut files)?;
    }
    if !layout.unplaced.is_empty() {
        // Pieces too big for a bed still get exported, each on its own.
        for id in &layout.unplaced {
            if let Some(q) = part.piece(*id) {
                let m = q.display_mesh().transformed(&piece_print_matrix(q));
                write(&format!("{}_too_big.3mf", file_name(&s.export.part_name, title, None, Some(q.label))), &threemf::write(&[(format!("Part {}", q.label), &m)], title), &mut files)?;
            }
        }
    }
    job.set_progress(0.3);
    // Single-part STLs in print orientation.
    if s.export.single_stls {
        job.set_stage("Writing STL files");
        let stls: Vec<(String, Vec<u8>)> = part
            .pieces
            .par_iter()
            .map(|q| {
                let m = shrink(&q.display_mesh().transformed(&piece_print_matrix(q)), s, Vec3::ZERO);
                (format!("{}.stl", file_name(&s.export.part_name, title, None, Some(q.label))), crate::io::stl::write_binary(&m, &format!("part {}", q.label)))
            })
            .collect();
        for (n, b) in stls {
            write(&n, &b, &mut files)?;
        }
    }
    // Print list.
    let bed_of = |id| layout.find(id).map(|(b, _)| (b + 1).to_string()).unwrap_or_else(|| "-".into());
    let mut csv = String::from("label,size_x_mm,size_y_mm,size_z_mm,weight_g,bed,flags\n");
    let mut sorted: Vec<&Piece> = part.pieces.iter().collect();
    sorted.sort_by_key(|q| q.label);
    for q in &sorted {
        let sz = q.orient.map(|o| o.size).unwrap_or(q.bbox.size());
        csv.push_str(&format!("{},{:.1},{:.1},{:.1},{:.1},{},\"{}\"\n", q.label, sz.x, sz.y, sz.z, weight_g(q, s), bed_of(q.id), flags_text(q)));
    }
    write(&format!("{}_print_list.csv", file_name("{project}", title, None, None)), csv.as_bytes(), &mut files)?;
    job.set_progress(0.4);
    let asm = assembly::plan(part);
    if s.export.guide {
        job.set_stage("Writing assembly guide");
        let pdf = guide(title, part, &asm, layout, s, job)?;
        write(&format!("{}_assembly_guide.pdf", file_name("{project}", title, None, None)), &pdf, &mut files)?;
    }
    job.set_progress(0.85);
    if s.export.viewer {
        job.set_stage("Writing 3D viewer");
        let budget = (400_000 / part.pieces.len().max(1)).max(400);
        let simplified: Vec<(u32, Mesh, usize, Vec3)> = part
            .pieces
            .par_iter()
            .map(|q| {
                let step = asm.order.iter().position(|x| *x == q.id).unwrap_or(0);
                (q.label, crate::simplify::cluster(q.display_mesh(), budget), step, asm.insert_dir.get(&q.id).copied().unwrap_or(-Vec3::Z))
            })
            .collect();
        let vparts: Vec<viewer::ViewerPart> = simplified.iter().map(|(l, m, st, d)| viewer::ViewerPart { label: *l, mesh: m, color: piece_color(*l), step: *st, insert: *d }).collect();
        write(&format!("{}_viewer.html", file_name("{project}", title, None, None)), viewer::write(title, &vparts).as_bytes(), &mut files)?;
    }
    job.set_progress(1.0);
    Ok(ExportResult { files, folder: folder.to_path_buf() })
}

/// The assembly guide: cover, parts checklist, then one step per page.
fn guide(title: &str, part: &Partition, asm: &assembly::Assembly, layout: &Layout, s: &Settings, job: &JobCtx) -> Result<Vec<u8>, String> {
    use pdf::{A4, Page, Pdf};
    let mut doc = Pdf::default();
    let bb = part.pieces.iter().fold(Aabb::EMPTY, |b, q| b.union(&q.bbox));
    let (iw, ih) = (900usize, 620usize);
    let vp = render::iso_camera(&bb, iw as f32 / ih as f32);
    // Light meshes for drawing.
    let budget = (300_000 / part.pieces.len().max(1)).max(300);
    let light: Vec<(u32, Mesh)> = part.pieces.par_iter().map(|q| (q.id, crate::simplify::cluster(q.display_mesh(), budget))).collect();
    let mesh_of = |id: u32| light.iter().find(|(i, _)| *i == id).map(|(_, m)| m);
    let label_of = |id: u32| part.piece(id).map(|q| q.label).unwrap_or(0);
    let total_g: f64 = part.pieces.iter().map(|q| weight_g(q, s)).sum();

    // Cover.
    let all: Vec<DrawMesh> = part.pieces.iter().filter_map(|q| Some(DrawMesh { mesh: mesh_of(q.id)?, model: Mat4::IDENTITY, color: piece_color(q.label), id: q.id })).collect();
    let cover_img = doc.add_image(&render::render(&all, vp, iw, ih, [255, 255, 255]));
    let mut p = Page::default();
    p.color(1.0, 0.42, 0.17);
    p.rect(0.0, A4.1 - 8.0, A4.0, 8.0, true);
    p.color(0.1, 0.1, 0.12);
    p.text(50.0, 780.0, 26.0, true, title);
    p.text(50.0, 756.0, 12.0, false, "Assembly guide · Section3D");
    p.image(cover_img, 50.0, 330.0, 495.0, 495.0 * ih as f32 / iw as f32);
    let lines = [
        format!("{} parts on {} bed(s), about {:.0} g of filament", part.pieces.len(), layout.beds.len(), total_g),
        format!("Printer: {}", s.printer.printer.name),
        if s.connectors.dowels { format!("Dowel pins: {:.1} mm diameter, {:.0} mm long", s.connectors.diameter, s.connectors.length) } else { "No dowel pins".into() },
        format!("{} assembly steps follow, one per page.", asm.order.len()),
    ];
    for (i, l) in lines.iter().enumerate() {
        p.text(50.0, 290.0 - i as f32 * 18.0, 12.0, false, l);
    }
    doc.pages.push(p);

    // Parts checklist.
    let mut sorted: Vec<&Piece> = part.pieces.iter().collect();
    sorted.sort_by_key(|q| q.label);
    for chunk in sorted.chunks(38) {
        let mut p = Page::default();
        p.color(0.1, 0.1, 0.12);
        p.text(50.0, 790.0, 18.0, true, "Parts checklist");
        let header = ["", "Part", "Size (mm)", "Weight", "Bed", "Notes"];
        let xs = [50.0, 72.0, 130.0, 260.0, 320.0, 360.0];
        for (h, x) in header.iter().zip(xs) {
            p.text(x, 760.0, 10.0, true, h);
        }
        for (i, q) in chunk.iter().enumerate() {
            let y = 740.0 - i as f32 * 18.0;
            p.color(0.5, 0.5, 0.55);
            p.rect(50.0, y - 2.0, 10.0, 10.0, false);
            p.color(0.1, 0.1, 0.12);
            let sz = q.orient.map(|o| o.size).unwrap_or(q.bbox.size());
            p.text(72.0, y, 10.0, true, &format!("#{}", q.label));
            p.text(130.0, y, 10.0, false, &format!("{:.0} × {:.0} × {:.0}", sz.x, sz.y, sz.z));
            p.text(260.0, y, 10.0, false, &format!("{:.0} g", weight_g(q, s)));
            p.text(320.0, y, 10.0, false, &layout.find(q.id).map(|(b, _)| (b + 1).to_string()).unwrap_or_else(|| "-".into()));
            let notes: Vec<String> = q.flags.iter().filter(|f| !matches!(f, Flag::SkippedDowel)).map(|f| f.short().to_lowercase()).collect();
            p.text(360.0, y, 9.0, false, &notes.join(", "));
        }
        doc.pages.push(p);
    }

    // Steps (rendered in parallel).
    job.set_stage("Rendering assembly steps");
    let imgs: Vec<render::Image> = (0..asm.order.len())
        .into_par_iter()
        .map(|k| {
            let cur = asm.order[k];
            let dir = asm.insert_dir.get(&cur).copied().unwrap_or(-Vec3::Z);
            let lift = part.piece(cur).map(|q| q.bbox.size().max_element() * 0.35).unwrap_or(0.0);
            let items: Vec<DrawMesh> = asm.order[..=k]
                .iter()
                .filter_map(|id| {
                    let m = mesh_of(*id)?;
                    let is_cur = *id == cur;
                    Some(DrawMesh {
                        mesh: m,
                        model: if is_cur { Mat4::from_translation(-dir * lift) } else { Mat4::IDENTITY },
                        color: if is_cur { [1.0, 0.45, 0.2] } else { [0.78, 0.8, 0.84] },
                        id: *id,
                    })
                })
                .collect();
            render::render(&items, vp, iw, ih, [255, 255, 255])
        })
        .collect();
    job.check().map_err(|e| e.to_string())?;
    for (k, img) in imgs.iter().enumerate() {
        let cur = asm.order[k];
        let idx = doc.add_image(img);
        let mut p = Page::default();
        p.color(0.1, 0.1, 0.12);
        p.text(50.0, 790.0, 20.0, true, &format!("Step {} of {}", k + 1, asm.order.len()));
        p.text(50.0, 765.0, 14.0, false, &format!("Add part #{}", label_of(cur)));
        p.image(idx, 50.0, 330.0, 495.0, 495.0 * ih as f32 / iw as f32);
        let mut y = 300.0;
        let joins: Vec<u32> = part.neighbours(cur).into_iter().filter(|n| asm.order[..k].contains(n)).map(label_of).collect();
        if !joins.is_empty() {
            let pins = if s.connectors.dowels { " Glue the dowel pins into the holes first." } else { "" };
            p.text(50.0, y, 12.0, false, &format!("Joins part(s) {}.{pins}", joins.iter().map(|l| format!("#{l}")).collect::<Vec<_>>().join(", ")));
            y -= 18.0;
        }
        if asm.blocked.contains(&cur) {
            p.color(0.8, 0.45, 0.0);
            p.text(50.0, y, 12.0, true, "No straight path: this part cannot slide onto all pins at once. Fit it before its neighbours or glue without pins.");
        }
        doc.pages.push(p);
    }
    Ok(doc.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::Solid;
    use crate::plane::Plane;
    use glam::DVec3;

    #[test]
    fn names_are_safe() {
        assert_eq!(file_name("{project}_bed{bed}", "My: model?", Some(0), None), "My_ model__bed01");
        assert_eq!(file_name("{project}_{part}", "x", None, Some(7)), "x_007");
    }

    #[test]
    fn export_writes_all_files() {
        let mut p = Partition::from_solids(vec![Solid::cube(DVec3::new(120.0, 80.0, 60.0), false).unwrap()]);
        p.split_piece(0, Plane::axis(0, 60.0)).unwrap();
        let mut s = Settings::default();
        s.export.single_stls = true;
        let params = crate::split::orient_params(&s);
        let ids: Vec<u32> = p.pieces.iter().map(|q| q.id).collect();
        crate::split::orient_pieces(&mut p, &ids, &params);
        crate::split::number_pieces(&mut p);
        let items: Vec<pack::PackItem> = p.pieces.iter().map(|q| pack::PackItem { id: q.id, mesh: q.display_mesh().clone(), print: piece_print_matrix(q) }).collect();
        let u = s.printer.usable();
        let layout = pack::pack(&items, &pack::PackParams { bed: [u[0], u[1]], spacing: 4.0, keep_order: false, angle_step: 15.0 });
        let dir = std::env::temp_dir().join(format!("s3d-export-{}", std::process::id()));
        let r = run(&dir, "cube", &p, &layout, &s, &JobCtx::new()).unwrap();
        let names: Vec<String> = r.files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.iter().any(|n| n.ends_with(".3mf")), "{names:?}");
        assert!(names.iter().any(|n| n.ends_with(".pdf")));
        assert!(names.iter().any(|n| n.ends_with(".csv")));
        assert!(names.iter().any(|n| n.ends_with(".html")));
        assert_eq!(names.iter().filter(|n| n.ends_with(".stl")).count(), 2);
        // The bed file contains both parts, inside the bed.
        let bed = crate::io::threemf::read(&std::fs::read(&r.files[0]).unwrap()).unwrap();
        let bb = bed.bbox();
        assert!(bb.min.x >= 0.0 && bb.max.x <= s.printer.printer.volume[0] && bb.min.z.abs() < 1e-3, "{bb:?}");
        assert!((bed.volume() - 120.0 * 80.0 * 60.0).abs() < 1.0);
        std::fs::remove_dir_all(dir).ok();
    }
}
