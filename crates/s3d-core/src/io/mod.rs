//! Mesh file import/export.

pub mod obj;
pub mod stl;
pub mod threemf;
pub mod zip;

use std::path::Path;

use glam::Vec3;
use rayon::prelude::*;

use crate::mesh::Mesh;

#[derive(Debug)]
pub enum IoError {
    Io(std::io::Error),
    Format(String),
    Unsupported(String),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::Io(e) => write!(f, "{e}"),
            IoError::Format(s) => write!(f, "{s}"),
            IoError::Unsupported(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for IoError {}
impl From<std::io::Error> for IoError {
    fn from(e: std::io::Error) -> Self {
        IoError::Io(e)
    }
}

pub const MESH_EXTENSIONS: &[&str] = &["stl", "obj", "3mf"];

/// Reads a mesh file, picking the format from the extension.
pub fn load(path: &Path) -> Result<Mesh, IoError> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let bytes = std::fs::read(path)?;
    load_bytes(&bytes, &ext)
}

pub fn load_bytes(bytes: &[u8], ext: &str) -> Result<Mesh, IoError> {
    let mut m = match ext {
        "stl" => stl::read(bytes)?,
        "obj" => obj::read(bytes)?,
        "3mf" => threemf::read(bytes)?,
        "step" | "stp" => return Err(IoError::Unsupported("STEP files are not supported. Export the model as STL, OBJ or 3MF.".into())),
        _ => return Err(IoError::Unsupported(format!("unsupported file type '.{ext}'"))),
    };
    if m.verts.iter().any(|v| !v.is_finite()) {
        return Err(IoError::Format("mesh contains invalid (non-finite) coordinates".into()));
    }
    m.compact();
    Ok(m)
}

/// Merges bit-identical positions of a triangle soup (3 verts per triangle).
pub fn weld_exact(soup: Vec<Vec3>) -> Mesh {
    let key = |v: &Vec3| (v.x.to_bits(), v.y.to_bits(), v.z.to_bits());
    let mut order: Vec<u32> = (0..soup.len() as u32).collect();
    order.par_sort_unstable_by_key(|&i| key(&soup[i as usize]));
    let mut remap = vec![0u32; soup.len()];
    let mut verts = Vec::with_capacity(soup.len() / 5);
    let mut prev = None;
    for &i in &order {
        let k = key(&soup[i as usize]);
        if prev != Some(k) {
            verts.push(soup[i as usize]);
            prev = Some(k);
        }
        remap[i as usize] = verts.len() as u32 - 1;
    }
    let tris = remap.par_chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    Mesh::new(verts, tris)
}
