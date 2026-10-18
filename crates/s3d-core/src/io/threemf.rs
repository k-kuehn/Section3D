//! 3MF reading: all build items (with components and transforms) merged into one mesh.

use std::collections::HashMap;

use glam::{Mat4, Vec3, Vec4};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

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

fn attr<'a>(e: &'a BytesStart, name: &str) -> Option<std::borrow::Cow<'a, str>> {
    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == name).map(|a| a.value)
}

fn attr_str(e: &BytesStart, name: &str) -> Option<String> {
    attr(e, name).map(|v| v.into_owned())
}

fn num<T: std::str::FromStr>(e: &BytesStart, name: &str) -> Result<T, IoError> {
    attr(e, name)
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
    let mut r = Reader::from_reader(xml);
    let mut model = Model { unit: 1.0, ..Default::default() };
    let mut cur: Option<(u32, Object)> = None;
    let mut buf = Vec::new();
    let bad = |e: quick_xml::Error| IoError::Format(format!("3MF XML error: {e}"));
    loop {
        let ev = r.read_event_into(&mut buf).map_err(bad)?;
        match &ev {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "model" => {
                    model.unit = match attr_str(e, "unit").as_deref() {
                        Some("micron") => 0.001,
                        Some("centimeter") => 10.0,
                        Some("inch") => 25.4,
                        Some("foot") => 304.8,
                        Some("meter") => 1000.0,
                        _ => 1.0,
                    }
                }
                "object" => cur = Some((num(e, "id")?, Object::default())),
                "vertex" => {
                    if let Some((_, o)) = &mut cur {
                        o.mesh.verts.push(Vec3::new(num(e, "x")?, num(e, "y")?, num(e, "z")?));
                    }
                }
                "triangle" => {
                    if let Some((_, o)) = &mut cur {
                        o.mesh.tris.push([num(e, "v1")?, num(e, "v2")?, num(e, "v3")?]);
                    }
                }
                "component" => {
                    if let Some((_, o)) = &mut cur {
                        let p = attr_str(e, "path").unwrap_or_else(|| path.to_string());
                        o.components.push((p, num(e, "objectid")?, parse_transform(attr_str(e, "transform"))?));
                    }
                }
                "item" => model.build.push((num(e, "objectid")?, parse_transform(attr_str(e, "transform"))?, attr_str(e, "path"))),
                _ => {}
            },
            Event::End(e) if e.local_name().as_ref() == "object" => {
                if let Some((id, o)) = cur.take() {
                    model.objects.insert(id, o);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
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
