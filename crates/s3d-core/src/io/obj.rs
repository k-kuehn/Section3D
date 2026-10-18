//! Wavefront OBJ reading (positions and faces only).

use glam::Vec3;

use super::IoError;
use crate::mesh::Mesh;

pub fn read(bytes: &[u8]) -> Result<Mesh, IoError> {
    let text = String::from_utf8_lossy(bytes);
    let mut verts = Vec::new();
    let mut tris = Vec::new();
    let mut face = Vec::with_capacity(8);
    for (ln, line) in text.lines().enumerate() {
        let mut it = line.split_ascii_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.take(3).filter_map(|s| s.parse().ok()).collect();
                if c.len() != 3 {
                    return Err(IoError::Format(format!("OBJ line {}: bad vertex", ln + 1)));
                }
                verts.push(Vec3::new(c[0], c[1], c[2]));
            }
            Some("f") => {
                face.clear();
                for tok in it {
                    let idx = tok.split('/').next().unwrap_or("");
                    let i: i64 = idx.parse().map_err(|_| IoError::Format(format!("OBJ line {}: bad face index", ln + 1)))?;
                    let i = if i < 0 { verts.len() as i64 + i } else { i - 1 };
                    if i < 0 || i as usize >= verts.len() {
                        return Err(IoError::Format(format!("OBJ line {}: face index out of range", ln + 1)));
                    }
                    face.push(i as u32);
                }
                for k in 1..face.len().saturating_sub(1) {
                    tris.push([face[0], face[k], face[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if tris.is_empty() {
        return Err(IoError::Format("OBJ file contains no faces".into()));
    }
    Ok(Mesh::new(verts, tris))
}

pub fn write(mesh: &Mesh) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(mesh.verts.len() * 40 + mesh.tris.len() * 30);
    for v in &mesh.verts {
        let _ = writeln!(s, "v {} {} {}", v.x, v.y, v.z);
    }
    for t in &mesh.tris {
        let _ = writeln!(s, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1);
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn quads_and_negative_indices() {
        let src = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf -4/1/1 -3/2/1 -2/3/1 -1/4/1\n";
        let m = super::read(src.as_bytes()).unwrap();
        assert_eq!(m.tris, vec![[0, 1, 2], [0, 2, 3]]);
    }
}
