//! Small software rasteriser for guide images (z-buffer, flat shading,
//! outlines at depth/part edges). Keeps export independent of the GPU.

use glam::{Mat4, Vec3, Vec4};
use rayon::prelude::*;

use crate::mesh::{Aabb, Mesh};

pub struct Image {
    pub w: usize,
    pub h: usize,
    /// RGB, row-major from the top.
    pub rgb: Vec<u8>,
}

pub struct DrawMesh<'a> {
    pub mesh: &'a Mesh,
    pub model: Mat4,
    pub color: [f32; 3],
    /// Outlines drawn where this id meets another (part boundaries).
    pub id: u32,
}

/// Isometric-ish camera looking at `bb` from the front-right-above.
pub fn iso_camera(bb: &Aabb, aspect: f32) -> Mat4 {
    let c = bb.center();
    let r = (bb.diagonal() * 0.5).max(1.0);
    let dir = Vec3::new(0.62, -0.78, 0.55).normalize();
    let eye = c + dir * r * 3.2;
    let view = glam::camera::rh::view::look_at_mat4(eye, c, Vec3::Z);
    let fov = 2.0 * (0.8 / 3.2f32).atan();
    let fov = if aspect < 1.0 { 2.0 * ((fov * 0.5).tan() / aspect).atan() } else { fov };
    glam::camera::rh::proj::directx::perspective(fov, aspect, r * 1.0, r * 6.0) * view
}

pub fn render(items: &[DrawMesh], view_proj: Mat4, w: usize, h: usize, bg: [u8; 3]) -> Image {
    let light = Vec3::new(0.4, -0.5, 0.75).normalize();
    // Transform and shade triangles in parallel, then rasterise per band.
    struct T {
        p: [Vec3; 3],
        rgb: [u8; 3],
        id: u32,
    }
    let tris: Vec<T> = items
        .par_iter()
        .flat_map_iter(|it| {
            let m = it.mesh;
            let mvp = view_proj * it.model;
            (0..m.tris.len()).filter_map(move |t| {
                let wp = m.tri(t).map(|v| it.model.transform_point3(v));
                let n = (wp[1] - wp[0]).cross(wp[2] - wp[0]).normalize_or_zero();
                let lit = (0.35 + 0.65 * n.dot(light).abs()).min(1.0);
                let clip = m.tri(t).map(|v| mvp * Vec4::new(v.x, v.y, v.z, 1.0));
                if clip.iter().any(|c| c.w <= 1e-6) {
                    return None;
                }
                let p = clip.map(|c| Vec3::new((c.x / c.w * 0.5 + 0.5) * w as f32, (0.5 - c.y / c.w * 0.5) * h as f32, c.z / c.w));
                let rgb = it.color.map(|c| (c * lit * 255.0).clamp(0.0, 255.0) as u8);
                Some(T { p, rgb, id: it.id })
            })
        })
        .collect();
    let mut rgb = vec![0u8; w * h * 3];
    let mut depth = vec![f32::INFINITY; w * h];
    let mut ids = vec![u32::MAX; w * h];
    let band = 32;
    rgb.par_chunks_mut(w * 3 * band)
        .zip(depth.par_chunks_mut(w * band))
        .zip(ids.par_chunks_mut(w * band))
        .enumerate()
        .for_each(|(bi, ((rgb, depth), ids))| {
            let y0 = bi * band;
            let y1 = (y0 + band).min(h);
            for i in 0..rgb.len() / 3 {
                rgb[i * 3..i * 3 + 3].copy_from_slice(&bg);
            }
            for t in &tris {
                let [a, b, c] = t.p;
                let miny = a.y.min(b.y).min(c.y).floor().max(y0 as f32) as usize;
                let maxy = (a.y.max(b.y).max(c.y).ceil() as isize).min(y1 as isize - 1);
                if maxy < miny as isize {
                    continue;
                }
                let minx = a.x.min(b.x).min(c.x).floor().max(0.0) as usize;
                let maxx = (a.x.max(b.x).max(c.x).ceil() as isize).min(w as isize - 1);
                if maxx < minx as isize {
                    continue;
                }
                let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if area.abs() < 1e-9 {
                    continue;
                }
                for y in miny..=maxy as usize {
                    let py = y as f32 + 0.5;
                    for x in minx..=maxx as usize {
                        let px = x as f32 + 0.5;
                        let w0 = ((b.x - px) * (c.y - py) - (b.y - py) * (c.x - px)) / area;
                        let w1 = ((c.x - px) * (a.y - py) - (c.y - py) * (a.x - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let z = a.z * w0 + b.z * w1 + c.z * w2;
                        let i = (y - y0) * w + x;
                        if z < depth[i] {
                            depth[i] = z;
                            ids[i] = t.id;
                            rgb[i * 3..i * 3 + 3].copy_from_slice(&t.rgb);
                        }
                    }
                }
            }
        });
    // Outlines: where neighbouring pixels belong to different parts or jump in depth.
    let mut out = rgb.clone();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let edge = [i - 1, i + 1, i - w, i + w].iter().any(|&j| ids[j] != ids[i] || (depth[j] - depth[i]).abs() > 0.004);
            if edge && (ids[i] != u32::MAX || [i - 1, i + 1, i - w, i + w].iter().any(|&j| ids[j] != u32::MAX)) {
                out[i * 3..i * 3 + 3].copy_from_slice(&[40, 44, 52]);
            }
        }
    }
    Image { w, h, rgb: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;

    #[test]
    fn renders_something() {
        let m = shapes::cube(Vec3::splat(10.0));
        let vp = iso_camera(&m.bbox(), 1.5);
        let img = render(&[DrawMesh { mesh: &m, model: Mat4::IDENTITY, color: [1.0, 0.5, 0.2], id: 0 }], vp, 120, 80, [255, 255, 255]);
        let non_bg = img.rgb.chunks(3).filter(|p| p != &[255, 255, 255]).count();
        assert!(non_bg > 1000, "{non_bg}");
    }
}
