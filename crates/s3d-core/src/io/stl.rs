//! STL reading (binary and ASCII) and binary writing.

use glam::Vec3;
use rayon::prelude::*;

use super::{IoError, weld_exact};
use crate::mesh::Mesh;

pub fn read(bytes: &[u8]) -> Result<Mesh, IoError> {
    if is_binary(bytes) { read_binary(bytes) } else { read_ascii(bytes) }
}

fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let n = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    // Many binary files start with "solid" in the header, so trust the size check first.
    if 84 + n * 50 == bytes.len() {
        return true;
    }
    !bytes[..bytes.len().min(512)].trim_ascii_start().starts_with(b"solid")
}

fn read_binary(bytes: &[u8]) -> Result<Mesh, IoError> {
    if bytes.len() < 84 {
        return Err(IoError::Format("STL file is truncated".into()));
    }
    let n = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    let avail = (bytes.len() - 84) / 50;
    let n = n.min(avail);
    if n == 0 {
        return Err(IoError::Format("STL file contains no triangles".into()));
    }
    let body = &bytes[84..84 + n * 50];
    let f = |b: &[u8], o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let soup: Vec<Vec3> = body
        .par_chunks_exact(50)
        .flat_map_iter(|r| (0..3).map(move |k| Vec3::new(f(r, 12 + k * 12), f(r, 16 + k * 12), f(r, 20 + k * 12))))
        .collect();
    Ok(weld_exact(soup))
}

fn read_ascii(bytes: &[u8]) -> Result<Mesh, IoError> {
    let text = std::str::from_utf8(bytes).map_err(|_| IoError::Format("STL is neither binary nor valid ASCII".into()))?;
    let mut soup = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("vertex") {
            let mut it = rest.split_ascii_whitespace().map(|s| s.parse::<f32>());
            match (it.next(), it.next(), it.next()) {
                (Some(Ok(x)), Some(Ok(y)), Some(Ok(z))) => soup.push(Vec3::new(x, y, z)),
                _ => return Err(IoError::Format(format!("bad STL vertex line: {line}"))),
            }
        }
    }
    if soup.is_empty() || soup.len() % 3 != 0 {
        return Err(IoError::Format("ASCII STL has no complete triangles".into()));
    }
    Ok(weld_exact(soup))
}

pub fn write_binary(mesh: &Mesh, name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + mesh.tris.len() * 50);
    let mut header = [0u8; 80];
    let h = format!("Section3D {name}");
    let hb = h.as_bytes();
    header[..hb.len().min(80)].copy_from_slice(&hb[..hb.len().min(80)]);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(mesh.tris.len() as u32).to_le_bytes());
    let recs: Vec<[u8; 50]> = (0..mesh.tris.len())
        .into_par_iter()
        .map(|t| {
            let mut r = [0u8; 50];
            let n = mesh.tri_cross(t).normalize_or_zero();
            let pts = mesh.tri(t);
            for (i, v) in std::iter::once(n).chain(pts).enumerate() {
                for k in 0..3 {
                    r[i * 12 + k * 4..i * 12 + k * 4 + 4].copy_from_slice(&v[k].to_le_bytes());
                }
            }
            r
        })
        .collect();
    for r in recs {
        out.extend_from_slice(&r);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;

    #[test]
    fn binary_roundtrip_welds() {
        let m = shapes::cube(Vec3::new(1.0, 2.0, 3.0));
        let bytes = write_binary(&m, "cube");
        let back = read(&bytes).unwrap();
        assert_eq!(back.tris.len(), 12);
        assert_eq!(back.verts.len(), 8);
        assert!((back.volume() - 6.0).abs() < 1e-5);
    }

    #[test]
    fn ascii() {
        let s = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        let m = read(s.as_bytes()).unwrap();
        assert_eq!(m.tris.len(), 1);
    }
}
