//! 3MF reading: all build items (with components and transforms) merged into one mesh.

use std::collections::HashMap;

use glam::{Mat4, Vec3, Vec4};

use super::IoError;
use super::zip::ZipReader;
use crate::mesh::Mesh;

#[derive(Default)]
struct Object {
    mesh: Mesh,
    components: Vec<(String, u32, Mat4)>, // (model path, object id, transform)
}

#[derive(Default)]
struct Model {
    unit: f32,
    objects: HashMap<u32, Object>,
    build: Vec<(u32, Mat4, Option<String>)>,
}

/// One XML tag: local name (namespace prefix dropped), whether it closes
/// an element, and its raw attribute text.
struct Tag<'a> {
    name: &'a str,
    close: bool,
    attrs: &'a str,
}

/// Calls `f` for every tag of an XML document, skipping comments,
/// declarations and CDATA. Enough for 3MF model files.
fn scan(xml: &str, mut f: impl FnMut(Tag) -> Result<(), IoError>) -> Result<(), IoError> {
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let skip_to = |rest: &str, end: &str| rest.find(end).map(|j| j + end.len());
        let jump = if rest.starts_with("!--") {
            skip_to(rest, "-->")
        } else if rest.starts_with("![CDATA[") {
            skip_to(rest, "]]>")
        } else if rest.starts_with('!') || rest.starts_with('?') {
            skip_to(rest, ">")
        } else {
            None
        };
        if let Some(j) = jump {
            rest = &rest[j..];
            continue;
        }
        let end = rest.find('>').ok_or_else(|| IoError::Format("3MF XML: unclosed tag".into()))?;
        let body = &rest[..end];
        rest = &rest[end + 1..];
        let (close, body) = match body.strip_prefix('/') {
            Some(b) => (true, b),
            None => (false, body),
        };
        let n = body.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(body.len());
        let name = &body[..n];
        f(Tag { name: name.rsplit(':').next().unwrap_or(name), close, attrs: &body[n..] })?;
    }
    Ok(())
}

/// Value of attribute `key` (local name) in raw attribute text.
fn attr(attrs: &str, key: &str) -> Option<String> {
    let mut rest = attrs;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let q = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let close = after[1..].find(q)? + 1;
        if name.rsplit(':').next() == Some(key) {
            let v = &after[1..close];
            return Some(v.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"));
        }
        rest = &after[close + 1..];
    }
}

fn num<T: std::str::FromStr>(attrs: &str, name: &str) -> Result<T, IoError> {
    attr(attrs, name)
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| IoError::Format(format!("3MF: bad or missing attribute '{name}'")))
}

/// 3MF matrices are 4x3 row-vector form: p' = p * M.
fn parse_transform(s: Option<String>) -> Result<Mat4, IoError> {
    let Some(s) = s else { return Ok(Mat4::IDENTITY) };
    let m: Vec<f32> = s.split_ascii_whitespace().filter_map(|t| t.parse().ok()).collect();
    if m.len() != 12 {
        return Err(IoError::Format("3MF: bad transform".into()));
    }
    Ok(Mat4::from_cols(
        Vec4::new(m[0], m[1], m[2], 0.0),
        Vec4::new(m[3], m[4], m[5], 0.0),
        Vec4::new(m[6], m[7], m[8], 0.0),
        Vec4::new(m[9], m[10], m[11], 1.0),
    ))
}

fn parse_model(xml: &[u8], path: &str) -> Result<Model, IoError> {
    let xml = String::from_utf8_lossy(xml);
    let mut model = Model { unit: 1.0, ..Default::default() };
    let mut cur: Option<(u32, Object)> = None;
    scan(&xml, |t| {
        if t.close {
            if t.name == "object" {
                if let Some((id, o)) = cur.take() {
                    model.objects.insert(id, o);
                }
            }
            return Ok(());
        }
        let a = t.attrs;
        match t.name {
            "model" => {
                model.unit = match attr(a, "unit").as_deref() {
                    Some("micron") => 0.001,
                    Some("centimeter") => 10.0,
                    Some("inch") => 25.4,
                    Some("foot") => 304.8,
                    Some("meter") => 1000.0,
                    _ => 1.0,
                }
            }
            "object" => {
                let id = num(a, "id")?;
                // A self-closing object has no mesh or components.
                let o = (id, Object::default());
                if a.trim_end().ends_with('/') {
                    model.objects.insert(o.0, o.1);
                } else {
                    cur = Some(o);
                }
            }
            "vertex" => {
                if let Some((_, o)) = &mut cur {
                    o.mesh.verts.push(Vec3::new(num(a, "x")?, num(a, "y")?, num(a, "z")?));
                }
            }
            "triangle" => {
                if let Some((_, o)) = &mut cur {
                    o.mesh.tris.push([num(a, "v1")?, num(a, "v2")?, num(a, "v3")?]);
                }
            }
            "component" => {
                if let Some((_, o)) = &mut cur {
                    let p = attr(a, "path").unwrap_or_else(|| path.to_string());
                    o.components.push((p, num(a, "objectid")?, parse_transform(attr(a, "transform"))?));
                }
            }
            "item" => model.build.push((num(a, "objectid")?, parse_transform(attr(a, "transform"))?, attr(a, "path"))),
            _ => {}
        }
        Ok(())
    })?;
    Ok(model)
}

pub fn read(bytes: &[u8]) -> Result<Mesh, IoError> {
    let zip = ZipReader::new(bytes)?;
    let mut models: HashMap<String, Model> = HashMap::new();
    for e in zip.entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".model")) {
        let key = format!("/{}", e.name.trim_start_matches('/')).to_ascii_lowercase();
        models.insert(key.clone(), parse_model(&zip.read(e)?, &key)?);
    }
    // The root model is referenced from _rels/.rels; fall back to the usual name.
    let root = zip
        .read_name("_rels/.rels")
        .ok()
        .and_then(|rels| {
            let s = String::from_utf8_lossy(&rels).into_owned();
            let i = s.find("3dmodel")?;
            let start = s[..i].rfind("Target=\"")? + 8;
            let end = start + s[start..].find('"')?;
            Some(format!("/{}", s[start..end].trim_start_matches('/')).to_ascii_lowercase())
        })
        .filter(|k| models.contains_key(k))
        .or_else(|| models.keys().find(|k| k.ends_with("3dmodel.model")).cloned())
        .or_else(|| models.keys().next().cloned())
        .ok_or_else(|| IoError::Format("3MF contains no model".into()))?;

    let mut out = Mesh::default();
    let unit = models[&root].unit;
    fn emit(models: &HashMap<String, Model>, path: &str, id: u32, xf: Mat4, out: &mut Mesh, depth: u32) -> Result<(), IoError> {
        if depth > 32 {
            return Err(IoError::Format("3MF component nesting too deep".into()));
        }
        let path = path.to_ascii_lowercase();
        let obj = models.get(&path).and_then(|m| m.objects.get(&id)).ok_or_else(|| IoError::Format(format!("3MF: missing object {id}")))?;
        let nv = obj.mesh.verts.len() as u32;
        if obj.mesh.tris.iter().flatten().any(|&i| i >= nv) {
            return Err(IoError::Format(format!("3MF object {id} references a missing vertex")));
        }
        if !obj.mesh.tris.is_empty() {
            out.append(&obj.mesh.transformed(&xf));
        }
        for (p, cid, cxf) in &obj.components {
            emit(models, p, *cid, xf * *cxf, out, depth + 1)?;
        }
        Ok(())
    }
    for (id, xf, path) in &models[&root].build {
        let p = path.clone().unwrap_or_else(|| root.clone());
        emit(&models, &p, *id, Mat4::from_scale(Vec3::splat(unit)) * *xf, &mut out, 0)?;
    }
    if out.tris.is_empty() {
        return Err(IoError::Format("3MF build contains no triangles".into()));
    }
    let n = out.verts.len() as u32;
    if out.tris.iter().flatten().any(|&i| i >= n) {
        return Err(IoError::Format("3MF triangle references a missing vertex".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_components_units_and_odd_markup() {
        let xml = br#"<?xml version="1.0"?>
<!-- a comment with <tags> -->
<model unit="centimeter" xmlns:p="urn:x">
 <resources>
  <object id="1" type="model"><mesh>
   <vertices><vertex x="0" y="0" z="0"/><vertex x='1' y="0" z="0" /><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/></vertices>
   <triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/></triangles>
  </mesh></object>
  <object id="2"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 5 0 0"/></components></object>
 </resources>
 <build><item objectid="2"/><item p:objectid="1"/></build>
</model>"#;
        let m = parse_model(xml, "/3d/3dmodel.model").unwrap();
        assert_eq!(m.unit, 10.0);
        assert_eq!(m.objects[&1].mesh.tris.len(), 4);
        assert_eq!(m.objects[&2].components.len(), 1);
        assert_eq!(m.objects[&2].components[0].2.w_axis.x, 5.0);
        assert_eq!(m.build.len(), 2);
        assert_eq!(attr(r#" name="a &amp; b" "#, "name").as_deref(), Some("a & b"));
    }
}
