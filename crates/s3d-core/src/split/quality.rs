//! Quality checks on cut pieces, measured on the real geometry:
//!
//! * printability: fits, needs no support, not tall (see `orient`);
//! * sharp edges: every edge where a cut face meets another cut face or the
//!   model surface must keep at least the minimum angle inside the piece, so
//!   there are no feather edges from cuts nearly tangent to the shell;
//! * sharp corners: where two cut lines meet on the surface, the corner of
//!   the piece's outline must keep the minimum angle too (no pointed tips);
//! * thin strips: rays shot from each cut face into the piece must travel at
//!   least the minimum thickness before leaving it;
//! * aligned seams: neighbouring seams on (nearly) the same plane form one
//!   long straight line unless they all border a single piece.

use std::collections::HashMap;

use glam::{DVec2, DVec3, Vec3};
use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::mesh::Mesh;
use crate::orient::{self, OrientParams};
use crate::partition::{Flag, ON_PLANE, Orientation, Partition, PieceId, SeamId, TriGrid, caps_on};
use crate::plane::Plane;
use crate::zones::Zones;

#[derive(Clone, Debug)]
pub struct QualityParams {
    pub orient: OrientParams,
    /// Smallest allowed angle inside a piece at a cut edge (radians).
    pub min_angle: f64,
    /// Narrowest allowed strip (mm).
    pub min_thickness: f64,
    pub min_piece: f64,
    /// Straight seam runs longer than this (about one piece) that cross
    /// several pieces count as aligned (mm).
    pub align_len: f64,
    /// Painted no-cut zones (prepared coordinates) that cut faces must avoid.
    pub zones: std::sync::Arc<Zones>,
}

impl QualityParams {
    pub fn from_settings(s: &crate::settings::Settings) -> QualityParams {
        let bed = s.printer.usable();
        let cell = s.split.target_size.unwrap_or(bed[0].min(bed[1])) as f64;
        QualityParams {
            orient: super::orient_params(s),
            min_angle: (s.split.min_angle as f64).clamp(5.0, 60.0).to_radians(),
            min_thickness: s.split.min_thickness.max(0.0) as f64,
            min_piece: s.split.min_piece as f64,
            align_len: cell * 1.2,
            zones: Default::default(),
        }
    }

    pub fn with_zones(mut self, zones: &Zones) -> QualityParams {
        self.zones = std::sync::Arc::new(zones.clone());
        self
    }
}

/// Angles closer than this to a straight continuation count as aligned.
const ALIGN_ANGLE_DEG: f64 = 15.0;
/// Sharp edges shorter than this in total are tessellation noise (mm).
const SHARP_NOISE: f64 = 2.0;
/// Share of a cut face that may sit over a thin strip: acute corners taper
/// for a little way, longer tapers are slivers.
const THIN_SHARE: f64 = 0.2;
/// Cut edges meeting the surface further than this from square are skived.
const SKIVE_DEG: f64 = 45.0;

#[derive(Clone, Debug)]
pub struct PieceReport {
    pub orient: Orientation,
    /// Printability and shape flags (TooBig, NeedsSupport, Tall, Sharp, Thin).
    pub flags: Vec<Flag>,
    /// Total length of edges sharper than the minimum angle (mm).
    pub sharp_len: f64,
    /// Pointed corners of the outline on the surface.
    pub corners: u32,
    /// Length-weighted mean of cos² of the cut-to-surface angle (0 = square).
    pub skive: f64,
    /// Length of cut edges meeting the surface more than 45° off square (mm).
    pub skive_len: f64,
    /// Total length of cut edges on the surface (mm).
    pub cut_len: f64,
    /// Largest share of one cut face lying over a strip thinner than allowed.
    pub thin: f64,
    /// Height over bed contact in print pose.
    pub ratio: f32,
    /// Painted no-cut spots a cut face of this piece runs through.
    pub zone: u32,
}

impl PieceReport {

    pub fn support(&self) -> f32 {
        self.flags.iter().map(|f| if let Flag::NeedsSupport(a) = f { *a } else { 0.0 }).sum()
    }
}

/// Analyses one piece. `cuts` are the piece's cut planes with normals
/// pointing out of the piece.
pub fn analyze_piece(mesh: &Mesh, cuts: &[Plane], q: &QualityParams) -> PieceReport {
    let normals: Vec<DVec3> = cuts.iter().map(|c| c.normal).collect();
    let (ev, o, mut flags) = orient::best_eval(mesh, &normals, &q.orient);
    let ratio = ev.ratio;
    let e = edges(mesh, cuts, q.min_angle);
    let thin = if q.min_thickness > 0.0 && !cuts.is_empty() { thin_share(mesh, cuts, q.min_thickness) } else { 0.0 };
    if e.sharp > SHARP_NOISE || e.corners > 0 {
        flags.push(Flag::Sharp);
    }
    if thin > THIN_SHARE {
        flags.push(Flag::Thin);
    }
    let zone = zone_hits(mesh, cuts, &q.zones);
    if zone > 0 {
        flags.push(Flag::InZone);
    }
    PieceReport {
        orient: o,
        flags,
        sharp_len: e.sharp,
        corners: e.corners,
        skive: if e.cut_len > 0.0 { e.skive_sum / e.cut_len } else { 0.0 },
        skive_len: e.skive_len,
        cut_len: e.cut_len,
        thin,
        ratio,
        zone,
    }
}

/// Number of painted spots that a cut face of the piece passes through.
pub fn zone_hits(mesh: &Mesh, cuts: &[Plane], zones: &Zones) -> u32 {
    if zones.is_empty() || cuts.is_empty() {
        return 0;
    }
    let bb = mesh.bbox();
    let near: Vec<&crate::zones::Dab> = zones.dabs.iter().filter(|d| (d.p - d.p.clamp(bb.min, bb.max)).length() <= d.radius).collect();
    if near.is_empty() {
        return 0;
    }
    let cap = cap_index(mesh, cuts);
    let caps: Vec<[Vec3; 3]> = (0..mesh.tris.len()).filter(|&t| cap[t] != u32::MAX).map(|t| mesh.tri(t)).collect();
    near.iter().filter(|d| caps.iter().any(|t| d.touches(t))).count() as u32
}

/// Which cut plane (index) each triangle lies on, if any.
pub fn cap_index(mesh: &Mesh, cuts: &[Plane]) -> Vec<u32> {
    let tol = ON_PLANE.max(mesh.bbox().diagonal() as f64 * 1e-7) * 4.0;
    (0..mesh.tris.len())
        .map(|t| {
            let p = mesh.tri(t);
            let n = mesh.tri_cross(t).as_dvec3();
            let l = n.length();
            if l < 1e-12 {
                return u32::MAX;
            }
            for (k, c) in cuts.iter().enumerate() {
                if n.dot(c.normal) > 0.99 * l && p.iter().all(|v| c.dist(*v).abs() < tol) {
                    return k as u32;
                }
            }
            u32::MAX
        })
        .collect()
}

struct EdgeStats {
    sharp: f64,
    corners: u32,
    skive_sum: f64,
    skive_len: f64,
    cut_len: f64,
}

fn edges(mesh: &Mesh, cuts: &[Plane], min_angle: f64) -> EdgeStats {
    let mut st = EdgeStats { sharp: 0.0, corners: 0, skive_sum: 0.0, skive_len: 0.0, cut_len: 0.0 };
    if cuts.is_empty() {
        return st;
    }
    let cap = cap_index(mesh, cuts);
    let nrm: Vec<DVec3> = (0..mesh.tris.len()).map(|t| mesh.tri_cross(t).as_dvec3()).collect();
    let skive_cos = SKIVE_DEG.to_radians().cos();
    let pairs = mesh.edge_pairs();
    st.corners = corners(mesh, &cap, &nrm, min_angle).len() as u32;
    for &(a, b, t1, t2) in &pairs {
        let (c1, c2) = (cap[t1 as usize], cap[t2 as usize]);
        if c1 == c2 {
            continue;
        }
        let (n1, n2) = (nrm[t1 as usize], nrm[t2 as usize]);
        let (l1, l2) = (n1.length(), n2.length());
        // Slivers from the boolean have unreliable normals.
        if l1 < 1e-6 || l2 < 1e-6 {
            continue;
        }
        let len = (mesh.verts[a as usize] - mesh.verts[b as usize]).length() as f64;
        let cos = (n1.dot(n2) / (l1 * l2)).clamp(-1.0, 1.0);
        // Angle inside the piece between the two faces.
        let inside = std::f64::consts::PI - cos.acos();
        if inside < min_angle {
            st.sharp += len;
        }
        if c1 == u32::MAX || c2 == u32::MAX {
            st.cut_len += len;
            st.skive_sum += len * cos * cos;
            if cos.abs() > skive_cos {
                st.skive_len += len;
            }
        }
    }
    st
}

/// Faces meeting at more than this are separate parts of the surface (60°).
const CREASE_COS: f64 = 0.5;

/// Vertices on a cut line where the piece's outline on the surface comes to
/// a point sharper than `min_angle`: where two cuts meet, or a cut meets an
/// edge of the model (such as the rim of a shell), at a narrow angle. The
/// angle is the sum of the surface triangles' angles at the vertex within
/// one smooth patch of surface.
fn corners(mesh: &Mesh, cap: &[u32], nrm: &[DVec3], min_angle: f64) -> Vec<u32> {
    let mut on_cut = vec![false; mesh.verts.len()];
    for (t, tri) in mesh.tris.iter().enumerate() {
        if cap[t] != u32::MAX {
            for &v in tri {
                on_cut[v as usize] = true;
            }
        }
    }
    // Triangles around each vertex on a cut line: (surface, cut).
    let mut fans: HashMap<u32, (Vec<u32>, Vec<u32>)> = HashMap::new();
    for (t, tri) in mesh.tris.iter().enumerate() {
        for &v in tri {
            if on_cut[v as usize] {
                let f = fans.entry(v).or_default();
                if cap[t] == u32::MAX { f.0.push(t as u32) } else { f.1.push(t as u32) }
            }
        }
    }
    let vert = |i: u32| mesh.verts[i as usize].as_dvec3();
    let mut out = Vec::new();
    for (v, (surf, cut)) in fans {
        if surf.is_empty() {
            continue;
        }
        let others = |t: u32| -> [u32; 2] {
            let tri = mesh.tris[t as usize];
            let k = tri.iter().position(|&x| x == v).unwrap();
            [tri[(k + 1) % 3], tri[(k + 2) % 3]]
        };
        let p = vert(v);
        // Slivers have no reliable normal: they join any neighbour.
        let sliver = |t: u32| {
            let [a, b] = others(t);
            let l = (vert(a) - p).length().max((vert(b) - p).length()).max((vert(a) - vert(b)).length());
            nrm[t as usize].length() < 1e-2 * l * l
        };
        // Smooth patches of surface around the vertex (union-find).
        let n = surf.len();
        let mut root: Vec<usize> = (0..n).collect();
        fn find(r: &mut [usize], mut x: usize) -> usize {
            while r[x] != x {
                r[x] = r[r[x]];
                x = r[x];
            }
            x
        }
        for i in 0..n {
            for j in i + 1..n {
                let (ti, tj) = (surf[i], surf[j]);
                let shared = others(ti).iter().any(|w| others(tj).contains(w));
                let (ni, nj) = (nrm[ti as usize], nrm[tj as usize]);
                let smooth = sliver(ti) || sliver(tj) || ni.dot(nj) > CREASE_COS * ni.length() * nj.length();
                if shared && smooth {
                    let (a, b) = (find(&mut root, i), find(&mut root, j));
                    root[a] = b;
                }
            }
        }
        // Per patch: angle at the vertex, bordered by a cut, has a proper
        // triangle (not only boolean slivers), how far it reaches (mm).
        let mut sums: HashMap<usize, (f64, bool, bool, f64)> = HashMap::new();
        for i in 0..n {
            let t = surf[i];
            let [a, b] = others(t);
            let (da, db) = (vert(a) - p, vert(b) - p);
            let ang = if da.length() < 1e-9 || db.length() < 1e-9 { 0.0 } else { da.normalize().dot(db.normalize()).clamp(-1.0, 1.0).acos() };
            // Does a cut face border this patch here?
            let by_cut = [a, b].iter().any(|w| cut.iter().any(|&c| mesh.tris[c as usize].contains(w)));
            let r = find(&mut root, i);
            let e = sums.entry(r).or_insert((0.0, false, false, 0.0));
            e.0 += ang;
            e.1 |= by_cut;
            e.2 |= !sliver(t);
            e.3 = e.3.max(da.length()).max(db.length());
        }
        if sums.values().any(|&(sum, by_cut, real, reach)| by_cut && real && reach >= 1.5 && sum < min_angle) {
            out.push(v);
        }
    }
    out.sort_unstable();
    out
}

/// Largest share of a cut face's area that sits over material thinner than
/// `min`. Rays start on the cut face and run into the piece along the
/// surface (the face normal turned parallel to the nearby skin), so on thin
/// shells they measure the width of the panel, not its wall. A ray that
/// leaves through the skin at a glancing angle ran off along the panel and
/// counts as wide; a ray that hits another cut or an edge of the surface
/// head-on within `min` marks a thin spot.
fn thin_share(mesh: &Mesh, cuts: &[Plane], min: f64) -> f64 {
    let cap = cap_index(mesh, cuts);
    let area: Vec<f64> = (0..mesh.tris.len()).map(|t| mesh.tri_cross(t).length() as f64 * 0.5).collect();
    let total: f64 = cap.iter().zip(&area).filter(|(c, _)| **c != u32::MAX).map(|(_, a)| *a).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let bvh = Bvh::new(mesh);
    let tol = ON_PLANE.max(mesh.bbox().diagonal() as f64 * 1e-7) * 4.0;
    let on_cut = |t: &[Vec3; 3]| cuts.iter().any(|c| t.iter().all(|v| c.dist(*v).abs() < tol));
    // About 2000 rays per piece at most.
    let spacing = (min / 3.0).max((total / 2000.0).sqrt());
    let mut thin = vec![0.0f64; cuts.len()];
    let mut all = vec![0.0f64; cuts.len()];
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let glancing = 0.35f32;
    for t in 0..mesh.tris.len() {
        let k = cap[t];
        if k == u32::MAX {
            continue;
        }
        let k = k as usize;
        let tri = mesh.tri(t);
        let n = (spacing * spacing).recip() * area[t];
        let count = n.floor() as usize + usize::from(crate::split::rand01(&mut rng) < n.fract());
        let into = -cuts[k].normal.as_vec3();
        let w = area[t] / count.max(1) as f64;
        all[k] += area[t];
        for _ in 0..count {
            let (mut u, mut v) = (crate::split::rand01(&mut rng), crate::split::rand01(&mut rng));
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            let p = tri[0] + (tri[1] - tri[0]) * u as f32 + (tri[2] - tri[0]) * v as f32 + into * 0.02;
            // Skin crossed by this cut near the sample.
            let skin = bvh.closest_where(p, min as f32, |tr| {
                let nn = (tr[1] - tr[0]).cross(tr[2] - tr[0]).normalize_or_zero();
                nn.dot(into).abs() < 0.7 && !on_cut(tr)
            });
            let dir = match skin {
                Some((_, _, st)) => {
                    let tr = bvh.triangle(st);
                    let m = (tr[1] - tr[0]).cross(tr[2] - tr[0]).normalize_or_zero();
                    (into - m * m.dot(into)).normalize_or(into)
                }
                None => into,
            };
            if let Some(h) = bvh.ray(p, dir, min as f32) {
                let hn = h.normal.normalize_or_zero();
                if hn.dot(dir).abs() >= glancing && (h.t as f64) < min - 0.02 {
                    thin[k] += w;
                }
            }
        }
    }
    let floor = min * min * 2.0;
    (0..cuts.len()).filter(|&k| all[k] > floor && thin[k] > floor * 0.5).map(|k| thin[k] / all[k]).fold(0.0, f64::max)
}

/// Outward cut planes of a piece in a partition.
pub fn piece_cuts(p: &Partition, id: PieceId) -> Vec<Plane> {
    crate::partition::dedup_planes(p.seams_of(id).map(|s| if s.a == id { s.plane.flipped() } else { s.plane }))
        .into_iter()
        .map(|pl| {
            // dedup may keep either orientation: re-orient outward.
            let out = p.seams_of(id).find(|s| s.plane.coincident(&pl, 1e-9, 1e-6)).map(|s| if s.a == id { -s.plane.normal } else { s.plane.normal }).unwrap_or(pl.normal);
            if pl.normal.dot(out) < 0.0 { pl.flipped() } else { pl }
        })
        .collect()
}

/// Reports for the given pieces (in parallel).
pub fn analyze(p: &Partition, ids: &[PieceId], q: &QualityParams) -> HashMap<PieceId, PieceReport> {
    ids.par_iter()
        .filter_map(|id| {
            let piece = p.piece(*id)?;
            Some((*id, analyze_piece(&piece.mesh, &piece_cuts(p, *id), q)))
        })
        .collect()
}

/// Writes reports into the pieces' orientation and flags.
pub fn apply_reports(p: &mut Partition, reports: &HashMap<PieceId, PieceReport>) {
    for q in p.pieces.iter_mut() {
        if let Some(r) = reports.get(&q.id) {
            q.orient = Some(r.orient);
            q.flags.retain(|f| !matches!(f, Flag::TooBig | Flag::NeedsSupport(_) | Flag::Tall | Flag::Sharp | Flag::Thin | Flag::InZone | Flag::NoAssemblyPath));
            q.flags.extend(r.flags.iter().cloned());
        }
    }
}

// ---------------------------------------------------------------------------
// Aligned seams

/// Points of a seam's contact region (3D), for adjacency tests: the cap
/// vertices of either piece that lie within the other piece's cap.
pub fn seam_points(p: &Partition, sid: SeamId) -> Vec<Vec3> {
    let Some(s) = p.seam(sid) else { return Vec::new() };
    let (Some(a), Some(b)) = (p.piece(s.a), p.piece(s.b)) else { return Vec::new() };
    let ca = caps_on(&a.mesh, &s.plane, -1.0);
    let cb = caps_on(&b.mesh, &s.plane, 1.0);
    if ca.is_empty() || cb.is_empty() {
        return Vec::new();
    }
    let (u, v) = s.plane.basis();
    let base = s.plane.normal * s.plane.offset;
    let mut seen: HashMap<(i64, i64), ()> = HashMap::new();
    let mut out = Vec::new();
    let (ga, gb) = (TriGrid::new(ca.clone()), TriGrid::new(cb.clone()));
    // Inside, allowing for vertices exactly on the other cap's border.
    let near = |g: &TriGrid, q: DVec2| [DVec2::ZERO, DVec2::new(0.3, 0.0), DVec2::new(-0.3, 0.0), DVec2::new(0.0, 0.3), DVec2::new(0.0, -0.3)].iter().any(|d| g.contains(q + *d));
    for (caps, other) in [(&ca, &gb), (&cb, &ga)] {
        for t in caps.iter() {
            for q in t {
                let key = ((q.x * 4.0).round() as i64, (q.y * 4.0).round() as i64);
                if seen.contains_key(&key) || !near(other, *q) {
                    continue;
                }
                seen.insert(key, ());
                out.push((base + u * q.x + v * q.y).as_vec3());
            }
        }
    }
    out
}

/// A run of seams on (nearly) one plane that touch end to end.
#[derive(Clone, Debug)]
pub struct Chain {
    pub seams: Vec<SeamId>,
    /// Length of the straight run (mm).
    pub length: f64,
    /// Pairs that continue straight across a junction where the pieces on
    /// both sides change (an X or a straight line through a corner).
    pub crossings: Vec<(SeamId, SeamId)>,
    /// All seams border one piece (a single straight edge with T-junctions).
    pub one_piece: bool,
}

impl Chain {
    pub fn is_violation(&self, q: &QualityParams) -> bool {
        !self.crossings.is_empty() || (!self.one_piece && self.length > q.align_len)
    }
}

/// Cache of seam contact points keyed by seam id (seams are immutable once
/// created; new seams get new ids).
#[derive(Default)]
pub struct SeamCache {
    pts: HashMap<SeamId, std::sync::Arc<Vec<Vec3>>>,
}

impl SeamCache {
    pub fn update(&mut self, p: &Partition) {
        let live: std::collections::HashSet<SeamId> = p.seams.iter().map(|s| s.id).collect();
        self.pts.retain(|k, _| live.contains(k));
        let missing: Vec<SeamId> = p.seams.iter().map(|s| s.id).filter(|id| !self.pts.contains_key(id)).collect();
        let fresh: Vec<(SeamId, Vec<Vec3>)> = missing.par_iter().map(|id| (*id, seam_points(p, *id))).collect();
        for (id, v) in fresh {
            self.pts.insert(id, std::sync::Arc::new(v));
        }
    }

    pub fn get(&self, id: SeamId) -> Option<&std::sync::Arc<Vec<Vec3>>> {
        self.pts.get(&id)
    }
}

/// Finds straight runs of touching seams.
pub fn chains(p: &Partition, cache: &SeamCache) -> Vec<Chain> {
    let tol = 1.5f32;
    let cos_align = ALIGN_ANGLE_DEG.to_radians().cos();
    let idx: HashMap<SeamId, usize> = p.seams.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    // Spatial hash of contact points: cell -> seam indices.
    let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    let key = |v: Vec3| ((v.x / tol).floor() as i32, (v.y / tol).floor() as i32, (v.z / tol).floor() as i32);
    for (i, s) in p.seams.iter().enumerate() {
        let Some(pts) = cache.get(s.id) else { continue };
        for &v in pts.iter() {
            let e = grid.entry(key(v)).or_default();
            if e.last() != Some(&(i as u32)) {
                e.push(i as u32);
            }
        }
    }
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for (k, list) in &grid {
        let mut near: Vec<u32> = list.clone();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if (dx, dy, dz) == (0, 0, 0) {
                        continue;
                    }
                    if let Some(o) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                        near.extend_from_slice(o);
                    }
                }
            }
        }
        near.sort_unstable();
        near.dedup();
        for &i in list {
            for &j in &near {
                if j <= i {
                    continue;
                }
                pairs.push((i, j));
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    // Pieces that touch (share a seam).
    let touching: std::collections::HashSet<(PieceId, PieceId)> = p.seams.iter().map(|s| (s.a.min(s.b), s.a.max(s.b))).collect();
    let adjacent = |x: PieceId, y: PieceId| x == y || touching.contains(&(x.min(y), x.max(y)));
    // Keep pairs on nearly the same plane.
    let aligned: Vec<(u32, u32)> = pairs
        .into_par_iter()
        .filter(|&(i, j)| {
            let (a, b) = (&p.seams[i as usize], &p.seams[j as usize]);
            if a.plane.normal.dot(b.plane.normal).abs() < cos_align {
                return false;
            }
            // Seams meeting at a junction have pieces that meet there too;
            // separate layers lying close together do not.
            if ![a.a, a.b].iter().any(|x| [b.a, b.b].iter().any(|y| adjacent(*x, *y))) {
                return false;
            }
            // The touching point must lie on both planes.
            let (Some(pa), Some(pb)) = (cache.get(a.id), cache.get(b.id)) else { return false };
            pb.iter().any(|v| a.plane.dist(*v).abs() < 2.0 && pa.iter().any(|w| w.distance(*v) < tol * 1.5))
        })
        .collect();
    // Union-find into chains.
    let n = p.seams.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for &(i, j) in &aligned {
        let (a, b) = (find(&mut parent, i as usize), find(&mut parent, j as usize));
        if a != b {
            parent[a.max(b)] = a.min(b);
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(i, j) in &aligned {
        let r = find(&mut parent, i as usize);
        groups.entry(r).or_default().extend([i as usize, j as usize]);
    }
    let _ = idx;
    let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
    for g in &mut groups {
        g.sort_unstable();
        g.dedup();
    }
    groups.sort();
    groups
        .into_iter()
        .map(|mut members| {
            members.sort_unstable();
            members.dedup();
            let seams: Vec<SeamId> = members.iter().map(|&i| p.seams[i].id).collect();
            let crossings: Vec<(SeamId, SeamId)> = aligned
                .iter()
                .filter(|(i, _)| members.binary_search(&(*i as usize)).is_ok())
                .filter(|&&(i, j)| {
                    let (a, b) = (&p.seams[i as usize], &p.seams[j as usize]);
                    !(a.touches(b.a) || a.touches(b.b))
                })
                .map(|&(i, j)| (p.seams[i as usize].id, p.seams[j as usize].id))
                .collect();
            let first = &p.seams[members[0]];
            let one_piece = [first.a, first.b].iter().any(|pc| members.iter().all(|&i| p.seams[i].touches(*pc)));
            // Length of the run along its main direction in the plane.
            let pts: Vec<Vec3> = members.iter().filter_map(|&i| cache.get(p.seams[i].id)).flat_map(|v| v.iter().copied()).collect();
            let length = run_length(&pts, &first.plane);
            Chain { seams, length, crossings, one_piece }
        })
        .collect()
}

/// Extent of points along their principal direction within a plane.
pub fn run_length(pts: &[Vec3], plane: &Plane) -> f64 {
    if pts.len() < 2 {
        return 0.0;
    }
    let (u, v) = plane.basis();
    let q: Vec<DVec2> = pts.iter().map(|p| DVec2::new(p.as_dvec3().dot(u), p.as_dvec3().dot(v))).collect();
    let m = q.iter().fold(DVec2::ZERO, |a, b| a + *b) / q.len() as f64;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in &q {
        let d = *p - m;
        sxx += d.x * d.x;
        sxy += d.x * d.y;
        syy += d.y * d.y;
    }
    let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let dir = DVec2::new(ang.cos(), ang.sin());
    let (lo, hi) = q.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
        let t = (*p - m).dot(dir);
        (lo.min(t), hi.max(t))
    });
    hi - lo
}

/// Pieces bordering violating chains.
pub fn aligned_pieces(p: &Partition, chains: &[Chain], q: &QualityParams) -> Vec<PieceId> {
    let mut out: Vec<PieceId> = chains
        .iter()
        .filter(|c| c.is_violation(q))
        .flat_map(|c| c.seams.iter().filter_map(|id| p.seam(*id)).flat_map(|s| [s.a, s.b]))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::Solid;

    fn params() -> QualityParams {
        QualityParams {
            orient: OrientParams { bed: [250.0, 250.0, 250.0], overhang_deg: 45.0 },
            min_angle: 25f64.to_radians(),
            min_thickness: 10.0,
            min_piece: 15.0,
            align_len: 120.0,
            zones: Default::default(),
        }
    }

    #[test]
    fn wedge_cut_is_sharp_and_square_cut_is_not() {
        let m = Solid::cube(DVec3::new(100.0, 100.0, 40.0), false).unwrap();
        // Square cut.
        let pl = Plane::axis(0, 50.0);
        let (a, _) = m.split_by_plane(pl.normal, pl.offset).unwrap();
        let r = analyze_piece(&a.to_mesh(), &[pl.flipped()], &params());
        assert!(r.sharp_len < 1.0, "{r:?}");
        assert!(r.skive < 0.01, "{r:?}");
        assert!(!r.flags.contains(&Flag::Thin), "{r:?}");
        // A cut entering the top face at 10° leaves a feather edge.
        let n = DVec3::new(10f64.to_radians().sin(), 0.0, 10f64.to_radians().cos());
        let pl2 = Plane::through(DVec3::new(20.0, 50.0, 40.0), n);
        let (a2, b2) = m.split_by_plane(pl2.normal, pl2.offset).unwrap();
        let ra = analyze_piece(&a2.to_mesh(), &[pl2.flipped()], &params());
        let rb = analyze_piece(&b2.to_mesh(), &[pl2], &params());
        assert!(ra.flags.contains(&Flag::Sharp) || rb.flags.contains(&Flag::Sharp), "{ra:?} {rb:?}");
    }

    #[test]
    fn pointed_corner_of_a_panel_is_sharp() {
        // A 4 mm panel cut by two vertical planes meeting at 20°: the tip is
        // sharp. Cut square, it is not.
        let panel = Solid::cube(DVec3::new(200.0, 200.0, 4.0), false).unwrap();
        let a = Plane::through(DVec3::new(100.0, 100.0, 0.0), DVec3::Y);
        let n2 = DVec3::new(-(20f64.to_radians().sin()), 20f64.to_radians().cos(), 0.0);
        let b = Plane::through(DVec3::new(100.0, 100.0, 0.0), -n2);
        let wedge = panel.split_by_plane(a.normal, a.offset).unwrap().0.split_by_plane(b.normal, b.offset).unwrap().0;
        let cuts = [a.flipped(), b.flipped()];
        let r = analyze_piece(&wedge.to_mesh(), &cuts, &params());
        assert!(r.corners >= 2, "{r:?}");
        assert!(r.flags.contains(&Flag::Sharp), "{r:?}");
        let c = Plane::through(DVec3::new(100.0, 100.0, 0.0), -DVec3::X);
        let square = panel.split_by_plane(a.normal, a.offset).unwrap().0.split_by_plane(c.normal, c.offset).unwrap().0;
        let r = analyze_piece(&square.to_mesh(), &[a.flipped(), c.flipped()], &params());
        assert_eq!(r.corners, 0, "{r:?}");
    }

    #[test]
    fn thin_strip_is_flagged() {
        let m = Solid::cube(DVec3::new(100.0, 100.0, 20.0), false).unwrap();
        let pl = Plane::axis(0, 94.0);
        let (strip, _) = m.split_by_plane(pl.normal, pl.offset).unwrap();
        let r = analyze_piece(&strip.to_mesh(), &[pl.flipped()], &params());
        assert!(r.flags.contains(&Flag::Thin), "{r:?}");
    }

    #[test]
    fn crossing_cuts_are_aligned_and_staggered_are_not() {
        // Four quarters (X crossing): aligned.
        let mut p = Partition::from_solids(vec![Solid::cube(DVec3::new(200.0, 200.0, 20.0), false).unwrap()]);
        p.split_piece(0, Plane::axis(0, 100.0)).unwrap();
        let ids: Vec<PieceId> = p.pieces.iter().map(|q| q.id).collect();
        for id in ids {
            p.split_piece(id, Plane::axis(1, 100.0)).unwrap();
        }
        let mut cache = SeamCache::default();
        cache.update(&p);
        let ch = chains(&p, &cache);
        assert!(ch.iter().any(|c| c.is_violation(&params())), "{ch:?}");
        // Staggered (brick) T-junctions with short runs: fine.
        let mut p = Partition::from_solids(vec![Solid::cube(DVec3::new(200.0, 100.0, 20.0), false).unwrap()]);
        p.split_piece(0, Plane::axis(1, 50.0)).unwrap();
        let lo = p.pieces.iter().find(|q| q.centroid.y < 50.0).unwrap().id;
        let hi = p.pieces.iter().find(|q| q.centroid.y > 50.0).unwrap().id;
        p.split_piece(lo, Plane::axis(0, 70.0)).unwrap();
        p.split_piece(hi, Plane::axis(0, 130.0)).unwrap();
        let mut cache = SeamCache::default();
        cache.update(&p);
        let ch = chains(&p, &cache);
        assert!(ch.iter().all(|c| c.crossings.is_empty()), "{ch:?}");
    }
}
