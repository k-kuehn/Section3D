//! Project files (.s3d): a zip with a JSON manifest and binary meshes.
//! Results are stored, so opening a project never re-cuts.

use std::path::Path;
use std::sync::Arc;

use glam::Vec3;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::edits::{EditOp, EditStatus};
use crate::io::zip::{ZipReader, ZipWriter};
use crate::job::JobCtx;
use crate::manifold::Solid;
use crate::mesh::Mesh;
use crate::partition::{Flag, Orientation, Partition, Piece, PieceId, Seam};
use crate::repair::Health;
use crate::settings::Settings;
use crate::zones::Zones;

pub const FORMAT_VERSION: u32 = 1;

pub struct SourceData {
    pub name: String,
    pub path: Option<String>,
    pub health: Health,
    pub mesh: Arc<Mesh>,
}

pub struct SplitData {
    pub key: u64,
    pub conn_key: Option<u64>,
    pub partition: Partition,
}

pub struct ProjectData {
    pub settings: Settings,
    pub edits: Vec<EditOp>,
    pub edit_status: Vec<EditStatus>,
    pub zones: Zones,
    pub source: Option<SourceData>,
    pub hollow: Option<(u64, Arc<Mesh>)>,
    pub split: Option<SplitData>,
    /// Application view state (bed layout etc.), stored verbatim.
    pub extra: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct PieceMeta {
    id: PieceId,
    label: u32,
    locked: bool,
    rev: u64,
    flags: Vec<Flag>,
    orient: Option<Orientation>,
    finished: bool,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    app: String,
    settings: Settings,
    edits: Vec<EditOp>,
    edit_status: Vec<EditStatus>,
    zones: Zones,
    source: Option<(String, Option<String>, Health)>,
    hollow_key: Option<u64>,
    split: Option<SplitMeta>,
    extra: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct SplitMeta {
    key: u64,
    conn_key: Option<u64>,
    pieces: Vec<PieceMeta>,
    seams: Vec<Seam>,
    next_piece: u32,
    next_seam: u32,
    next_rev: u64,
}

pub fn mesh_to_bytes(m: &Mesh) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + m.verts.len() * 12 + m.tris.len() * 12);
    out.extend_from_slice(&(m.verts.len() as u32).to_le_bytes());
    out.extend_from_slice(&(m.tris.len() as u32).to_le_bytes());
    out.extend_from_slice(bytemuck::cast_slice(&m.verts));
    out.extend_from_slice(bytemuck::cast_slice(&m.tris));
    out
}

pub fn mesh_from_bytes(b: &[u8]) -> Result<Mesh, String> {
    let bad = || "corrupt mesh data in project".to_string();
    if b.len() < 8 {
        return Err(bad());
    }
    let nv = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
    let nt = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    if b.len() != 8 + nv * 12 + nt * 12 {
        return Err(bad());
    }
    let verts: Vec<Vec3> = b[8..8 + nv * 12].chunks_exact(12).map(|c| Vec3::new(f32::from_le_bytes(c[0..4].try_into().unwrap()), f32::from_le_bytes(c[4..8].try_into().unwrap()), f32::from_le_bytes(c[8..12].try_into().unwrap()))).collect();
    let tris: Vec<[u32; 3]> = b[8 + nv * 12..].chunks_exact(12).map(|c| [0, 4, 8].map(|o| u32::from_le_bytes(c[o..o + 4].try_into().unwrap()))).collect();
    if tris.iter().flatten().any(|&i| i as usize >= nv) {
        return Err(bad());
    }
    Ok(Mesh::new(verts, tris))
}

pub fn save(path: &Path, d: &ProjectData, job: &JobCtx) -> Result<(), String> {
    job.set_stage("Writing project");
    let split_meta = d.split.as_ref().map(|s| SplitMeta {
        key: s.key,
        conn_key: s.conn_key,
        pieces: s.partition.pieces.iter().map(|p| PieceMeta { id: p.id, label: p.label, locked: p.locked, rev: p.rev, flags: p.flags.clone(), orient: p.orient, finished: p.finished.is_some() }).collect(),
        seams: s.partition.seams.clone(),
        next_piece: s.partition.next_piece,
        next_seam: s.partition.next_seam,
        next_rev: s.partition.next_rev,
    });
    let manifest = Manifest {
        version: FORMAT_VERSION,
        app: format!("Section3D {}", env!("CARGO_PKG_VERSION")),
        settings: d.settings.clone(),
        edits: d.edits.clone(),
        edit_status: d.edit_status.clone(),
        zones: d.zones.clone(),
        source: d.source.as_ref().map(|s| (s.name.clone(), s.path.clone(), s.health.clone())),
        hollow_key: d.hollow.as_ref().map(|h| h.0),
        split: split_meta,
        extra: d.extra.clone(),
    };
    // Compress blobs in parallel, then write them in a fixed order.
    let mut blobs: Vec<(String, Arc<Mesh>)> = Vec::new();
    if let Some(s) = &d.source {
        blobs.push(("source.bin".into(), s.mesh.clone()));
    }
    if let Some(h) = &d.hollow {
        blobs.push(("hollow.bin".into(), h.1.clone()));
    }
    if let Some(s) = &d.split {
        for p in &s.partition.pieces {
            blobs.push((format!("pieces/{}.bin", p.id), p.mesh.clone()));
            if let Some(f) = &p.finished {
                blobs.push((format!("pieces/{}.fin.bin", p.id), f.clone()));
            }
        }
    }
    let packed: Vec<(String, Vec<u8>)> = blobs.par_iter().map(|(n, m)| (n.clone(), mesh_to_bytes(m))).collect();
    job.set_progress(0.4);
    let mut z = ZipWriter::new();
    z.add("manifest.json", &serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?, true);
    for (i, (n, b)) in packed.iter().enumerate() {
        job.check().map_err(|e| e.to_string())?;
        z.add_level(n, b, 1);
        job.step(0.4, 0.95, i, packed.len());
    }
    let bytes = z.finish();
    let tmp = path.with_extension("s3d.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    job.set_progress(1.0);
    Ok(())
}

pub fn load(path: &Path, job: &JobCtx) -> Result<ProjectData, String> {
    job.set_stage("Reading project");
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let zip = ZipReader::new(&bytes).map_err(|e| e.to_string())?;
    let manifest: Manifest = serde_json::from_slice(&zip.read_name("manifest.json").map_err(|e| e.to_string())?).map_err(|e| format!("bad project manifest: {e}"))?;
    if manifest.version > FORMAT_VERSION {
        return Err("this project was saved by a newer version of Section3D".into());
    }
    let read_mesh = |name: &str| -> Result<Mesh, String> { mesh_from_bytes(&zip.read_name(name).map_err(|e| e.to_string())?) };
    let source = match manifest.source {
        Some((name, p, health)) => Some(SourceData { name, path: p, health, mesh: Arc::new(read_mesh("source.bin")?) }),
        None => None,
    };
    let hollow = match manifest.hollow_key {
        Some(k) => Some((k, Arc::new(read_mesh("hollow.bin")?))),
        None => None,
    };
    job.set_progress(0.3);
    let split = match manifest.split {
        Some(sm) => {
            job.set_stage("Loading pieces");
            let raw: Vec<(PieceMeta, Vec<u8>, Option<Vec<u8>>)> = sm
                .pieces
                .into_iter()
                .map(|m| {
                    let a = zip.read_name(&format!("pieces/{}.bin", m.id)).map_err(|e| e.to_string())?;
                    let f = if m.finished { Some(zip.read_name(&format!("pieces/{}.fin.bin", m.id)).map_err(|e| e.to_string())?) } else { None };
                    Ok((m, a, f))
                })
                .collect::<Result<_, String>>()?;
            let pieces: Vec<Piece> = raw
                .into_par_iter()
                .map(|(m, a, f)| {
                    let mesh = mesh_from_bytes(&a)?;
                    let solid = Solid::from_mesh(&mesh).map_err(|e| format!("piece {}: {e}", m.label))?;
                    let mut p = Piece::from_parts(m.id, Arc::new(solid), Arc::new(mesh), m.rev);
                    p.label = m.label;
                    p.locked = m.locked;
                    p.flags = m.flags;
                    p.orient = m.orient;
                    p.finished = match f {
                        Some(b) => Some(Arc::new(mesh_from_bytes(&b)?)),
                        None => None,
                    };
                    Ok(p)
                })
                .collect::<Result<_, String>>()?;
            let partition = Partition { pieces, seams: sm.seams, next_piece: sm.next_piece, next_seam: sm.next_seam, next_rev: sm.next_rev };
            Some(SplitData { key: sm.key, conn_key: sm.conn_key, partition })
        }
        None => None,
    };
    job.set_progress(1.0);
    Ok(ProjectData { settings: manifest.settings, edits: manifest.edits, edit_status: manifest.edit_status, zones: manifest.zones, source, hollow, split, extra: manifest.extra })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plane::Plane;
    use glam::DVec3;

    #[test]
    fn roundtrip_keeps_results() {
        let mut part = Partition::from_solids(vec![Solid::cube(DVec3::splat(50.0), false).unwrap()]);
        part.split_piece(0, Plane::axis(0, 20.0)).unwrap();
        part.pieces[0].flags.push(Flag::Tall);
        let src = Arc::new(crate::mesh::shapes::cube(Vec3::splat(50.0)));
        let d = ProjectData {
            settings: Settings::default(),
            edits: vec![],
            edit_status: vec![],
            zones: Zones::default(),
            source: Some(SourceData { name: "cube".into(), path: None, health: Health::Clean, mesh: src }),
            hollow: None,
            split: Some(SplitData { key: 42, conn_key: None, partition: part }),
            extra: serde_json::json!({"beds": 3}),
        };
        let dir = std::env::temp_dir().join(format!("s3d-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.s3d");
        save(&path, &d, &JobCtx::new()).unwrap();
        let back = load(&path, &JobCtx::new()).unwrap();
        let s = back.split.unwrap();
        assert_eq!(s.key, 42);
        assert_eq!(s.partition.pieces.len(), 2);
        assert_eq!(s.partition.seams.len(), 1);
        assert_eq!(s.partition.pieces[0].flags, vec![Flag::Tall]);
        assert!((s.partition.total_volume() - 125000.0).abs() < 1e-3);
        assert_eq!(back.extra["beds"], 3);
        std::fs::remove_dir_all(dir).ok();
    }
}
