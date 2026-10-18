//! 3MF writing: one object per part, positions baked in (units: mm).

use std::fmt::Write;

use crate::io::zip::ZipWriter;
use crate::mesh::Mesh;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Writes a 3MF with the given named meshes, already placed on the bed.
pub fn write(objects: &[(String, &Mesh)], title: &str) -> Vec<u8> {
    let mut x = String::with_capacity(objects.iter().map(|o| o.1.tris.len() * 60 + o.1.verts.len() * 50).sum::<usize>() + 1024);
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.push_str("<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n");
    let _ = writeln!(x, " <metadata name=\"Title\">{}</metadata>", xml_escape(title));
    x.push_str(" <metadata name=\"Application\">Section3D</metadata>\n <resources>\n");
    for (i, (name, m)) in objects.iter().enumerate() {
        let _ = writeln!(x, "  <object id=\"{}\" type=\"model\" name=\"{}\">\n   <mesh>\n    <vertices>", i + 1, xml_escape(name));
        for v in &m.verts {
            let _ = writeln!(x, "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", v.x, v.y, v.z);
        }
        x.push_str("    </vertices>\n    <triangles>\n");
        for t in &m.tris {
            let _ = writeln!(x, "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", t[0], t[1], t[2]);
        }
        x.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    x.push_str(" </resources>\n <build>\n");
    for i in 0..objects.len() {
        let _ = writeln!(x, "  <item objectid=\"{}\"/>", i + 1);
    }
    x.push_str(" </build>\n</model>\n");
    let mut z = ZipWriter::new();
    z.add("[Content_Types].xml", CONTENT_TYPES.as_bytes(), true);
    z.add("_rels/.rels", RELS.as_bytes(), true);
    z.add_level("3D/3dmodel.model", x.as_bytes(), 6);
    z.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::shapes;
    use glam::Vec3;

    #[test]
    fn roundtrip_through_reader() {
        let a = shapes::cube(Vec3::splat(10.0));
        let mut b = shapes::cube(Vec3::splat(5.0));
        b.translate(Vec3::new(20.0, 0.0, 0.0));
        let bytes = write(&[("a".into(), &a), ("b & c".into(), &b)], "t");
        let back = crate::io::threemf::read(&bytes).unwrap();
        assert_eq!(back.tris.len(), 24);
        assert!((back.volume() - 1125.0).abs() < 1e-3);
    }
}
