//! Minimal PDF writer: A4 pages with Helvetica text, lines, rectangles and
//! RGB images. Streams are deflate-compressed.

use crate::render::Image;

pub const A4: (f32, f32) = (595.0, 842.0);

#[derive(Default)]
pub struct Page {
    ops: String,
    images: Vec<usize>,
}

impl Page {
    /// Text at (x, y) from the bottom-left, size in points.
    pub fn text(&mut self, x: f32, y: f32, size: f32, bold: bool, s: &str) {
        let font = if bold { "F2" } else { "F1" };
        self.ops.push_str(&format!("BT /{font} {size} Tf {x:.2} {y:.2} Td ({}) Tj ET\n", escape(s)));
    }

    pub fn color(&mut self, r: f32, g: f32, b: f32) {
        self.ops.push_str(&format!("{r:.3} {g:.3} {b:.3} rg {r:.3} {g:.3} {b:.3} RG\n"));
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: bool) {
        self.ops.push_str(&format!("{x:.2} {y:.2} {w:.2} {h:.2} re {}\n", if fill { "f" } else { "S" }));
    }

    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32) {
        self.ops.push_str(&format!("{width:.2} w {x0:.2} {y0:.2} m {x1:.2} {y1:.2} l S\n"));
    }

    /// Places image `idx` (from `Pdf::add_image`) in the given box.
    pub fn image(&mut self, idx: usize, x: f32, y: f32, w: f32, h: f32) {
        self.ops.push_str(&format!("q {w:.2} 0 0 {h:.2} {x:.2} {y:.2} cm /Im{idx} Do Q\n"));
        self.images.push(idx);
    }
}

/// Encodes text for a WinAnsi Type1 font string.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '×' => out.push_str("\\327"),
            '°' => out.push_str("\\260"),
            '·' => out.push_str("\\267"),
            '—' | '–' => out.push('-'),
            '²' => out.push_str("\\262"),
            '³' => out.push_str("\\263"),
            c if c.is_ascii() => out.push(c),
            _ => out.push('?'),
        }
    }
    out
}

/// Approximate Helvetica width (points) for layout.
pub fn text_width(s: &str, size: f32) -> f32 {
    s.chars().count() as f32 * size * 0.52
}

#[derive(Default)]
pub struct Pdf {
    pub pages: Vec<Page>,
    images: Vec<(usize, usize, Vec<u8>)>,
}

impl Pdf {
    pub fn add_image(&mut self, img: &Image) -> usize {
        let data = miniz_oxide::deflate::compress_to_vec_zlib(&img.rgb, 6);
        self.images.push((img.w, img.h, data));
        self.images.len() - 1
    }

    pub fn finish(self) -> Vec<u8> {
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets: Vec<usize> = Vec::new();
        // Object numbers: 1 catalog, 2 pages, 3 F1, 4 F2, then images, then page+content pairs.
        let n_img = self.images.len();
        let first_img = 5;
        let first_page = first_img + n_img;
        let total = first_page + self.pages.len() * 2;
        let mut objs: Vec<Vec<u8>> = vec![Vec::new(); total];
        let kids: Vec<String> = (0..self.pages.len()).map(|i| format!("{} 0 R", first_page + i * 2)).collect();
        objs[1] = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
        objs[2] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), self.pages.len()).into_bytes();
        objs[3] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec();
        objs[4] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec();
        for (i, (w, h, data)) in self.images.iter().enumerate() {
            let mut o = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n", data.len()).into_bytes();
            o.extend_from_slice(data);
            o.extend_from_slice(b"\nendstream");
            objs[first_img + i] = o;
        }
        for (i, p) in self.pages.iter().enumerate() {
            let page_no = first_page + i * 2;
            let content_no = page_no + 1;
            let mut imgs: Vec<usize> = p.images.clone();
            imgs.sort_unstable();
            imgs.dedup();
            let xobj: String = imgs.iter().map(|k| format!("/Im{k} {} 0 R", first_img + k)).collect::<Vec<_>>().join(" ");
            objs[page_no] = format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> /XObject << {xobj} >> >> /Contents {content_no} 0 R >>",
                A4.0, A4.1
            )
            .into_bytes();
            let z = miniz_oxide::deflate::compress_to_vec_zlib(p.ops.as_bytes(), 6);
            let mut o = format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", z.len()).into_bytes();
            o.extend_from_slice(&z);
            o.extend_from_slice(b"\nendstream");
            objs[content_no] = o;
        }
        offsets.push(0);
        for (i, o) in objs.iter().enumerate().skip(1) {
            offsets.push(out.len());
            out.extend_from_slice(format!("{i} 0 obj\n").as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {total}\n0000000000 65535 f \n").as_bytes());
        for off in offsets.iter().skip(1) {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {total} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_structure() {
        let mut pdf = Pdf::default();
        let img = Image { w: 2, h: 2, rgb: vec![255; 12] };
        let k = pdf.add_image(&img);
        let mut p = Page::default();
        p.text(50.0, 800.0, 14.0, true, "Hello (world) 12 × 3");
        p.image(k, 50.0, 500.0, 100.0, 100.0);
        pdf.pages.push(p);
        let b = pdf.finish();
        let s = String::from_utf8_lossy(&b);
        assert!(s.starts_with("%PDF-1.4"));
        assert!(s.contains("/Count 1"));
        assert!(s.trim_end().ends_with("%%EOF"));
        // startxref holds the byte offset of the xref table.
        let tail = &s[s.rfind("startxref\n").unwrap() + 10..];
        let xref_pos: usize = tail.lines().next().unwrap().parse().unwrap();
        assert!(b[xref_pos..].starts_with(b"xref"));
    }
}
