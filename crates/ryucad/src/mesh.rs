//! Display meshes. The daemon tessellates each body with parameters
//! derived from its size and the display quality, and caches the result by
//! body hash, so editing one feature re-meshes one body. Agents get JSON;
//! the app can ask for a compact binary form.

use crate::doc::files::Feature;
use crate::error::{Code, Error, Result};
use crate::rebuild::Built;
use ryukei::mesh::{BodyMesh, MeshParams};
use ryukei::topo::Body;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    Coarse,
    Normal,
    Fine,
}

impl Quality {
    pub fn parse(s: &str) -> Result<Quality> {
        match s {
            "coarse" => Ok(Quality::Coarse),
            "normal" => Ok(Quality::Normal),
            "fine" => Ok(Quality::Fine),
            _ => Err(Error::new(
                Code::InvalidParam,
                format!("unknown quality {s:?}; use coarse, normal or fine"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Quality::Coarse => "coarse",
            Quality::Normal => "normal",
            Quality::Fine => "fine",
        }
    }

    /// The user's display quality.
    pub fn preferred() -> Quality {
        Quality::parse(
            crate::config::user("display.quality")
                .as_str()
                .unwrap_or("normal"),
        )
        .unwrap_or(Quality::Normal)
    }

    /// Tessellation parameters for a body whose bounding box has diagonal
    /// `diagonal` (meters): the chord is a fixed fraction of it, never
    /// below what ryukei accepts. The same body and quality give the same
    /// parameters on every platform.
    pub fn params(self, diagonal: f64) -> MeshParams {
        let fraction = match self {
            // Coarse is for size, not looks: visibly faceted.
            Quality::Coarse => 1e-2,
            Quality::Normal => 5e-4,
            Quality::Fine => 1.25e-4,
        };
        let floor = 10.0 * ryukei::tol::LINEAR_RESOLUTION;
        MeshParams::new((diagonal * fraction).max(floor), self.angle())
    }

    /// The most a mesh's normal turns across a triangle, and a sketch's
    /// arcs between the points they are drawn through.
    pub fn angle(self) -> f64 {
        use core::f64::consts::PI;
        match self {
            Quality::Coarse => PI / 4.0,
            Quality::Normal => PI / 12.0,
            Quality::Fine => PI * 8.0 / 180.0,
        }
    }
}

/// A body's display mesh and the parameters it was made with.
#[derive(Debug)]
pub struct DisplayMesh {
    pub params: MeshParams,
    pub mesh: Result<BodyMesh>,
}

impl DisplayMesh {
    fn bytes(&self) -> usize {
        self.mesh.as_ref().map_or(64, |m| {
            m.positions.len() * 24
                + m.corners.len() * 48
                + m.triangles.len() * 12
                + m.edges
                    .iter()
                    .map(|e| e.positions.len() * 12)
                    .sum::<usize>()
        })
    }
}

/// A built body with its hash and display mesh.
#[derive(Clone, Debug)]
pub struct Meshed {
    pub body: Body,
    pub hash: String,
    pub mesh: Arc<DisplayMesh>,
}

/// Meshes by body hash and quality, least recently used first out when
/// the cache passes its budget.
#[derive(Debug, Default)]
pub struct Cache {
    entries: BTreeMap<(String, Quality), (u64, Arc<DisplayMesh>)>,
    clock: u64,
    bytes: usize,
}

impl Cache {
    /// The mesh of `built` (feature `f`), made if needed. `budget` is in
    /// bytes.
    pub fn get(
        &mut self,
        f: &Feature,
        built: &Built,
        quality: Quality,
        budget: usize,
    ) -> Arc<DisplayMesh> {
        self.get_all(&[(f, built)], quality, budget).remove(0)
    }

    /// The meshes of several bodies, in order: the ones not cached are
    /// made in parallel.
    pub fn get_all(
        &mut self,
        items: &[(&Feature, &Built)],
        quality: Quality,
        budget: usize,
    ) -> Vec<Arc<DisplayMesh>> {
        use rayon::prelude::*;
        let missing: Vec<&(&Feature, &Built)> = items
            .iter()
            .filter(|(_, b)| !self.entries.contains_key(&(b.hash.clone(), quality)))
            .collect();
        let made: Vec<(String, Arc<DisplayMesh>)> = missing
            .par_iter()
            .map(|(f, b)| (b.hash.clone(), Arc::new(make(f, &b.body, quality))))
            .collect();
        let mut fresh: BTreeMap<String, Arc<DisplayMesh>> = BTreeMap::new();
        for (hash, m) in made {
            self.bytes += m.bytes();
            self.entries
                .insert((hash.clone(), quality), (self.clock, Arc::clone(&m)));
            fresh.insert(hash, m);
        }
        let out: Vec<Arc<DisplayMesh>> = items
            .iter()
            .map(|(_, b)| {
                self.clock += 1;
                match self.entries.get_mut(&(b.hash.clone(), quality)) {
                    Some((used, m)) => {
                        *used = self.clock;
                        Arc::clone(m)
                    }
                    None => Arc::clone(&fresh[&b.hash]),
                }
            })
            .collect();
        self.evict(budget);
        out
    }

    /// Drops the least recently used meshes until the cache fits `budget`,
    /// keeping at least one.
    fn evict(&mut self, budget: usize) {
        while self.bytes > budget && self.entries.len() > 1 {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(k, _)| k.clone())
                .expect("not empty");
            let (_, gone) = self.entries.remove(&oldest).expect("present");
            self.bytes -= gone.bytes();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The diagonal of the feature's bounding box.
fn diagonal(f: &Feature, body: &Body) -> f64 {
    match crate::inspect::extent(f).or_else(|| crate::inspect::body_extent(body)) {
        Some((lo, hi)) => {
            let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        }
        None => 1.0,
    }
}

fn make(f: &Feature, body: &Body, quality: Quality) -> DisplayMesh {
    let params = quality.params(diagonal(f, body));
    let mesh = ryukei::mesh::tessellate(body, &params).map_err(|e| {
        Error::new(Code::MeshFailed, format!("can't mesh the body: {e}"))
            .with("kernel", format!("{e:?}"))
    });
    DisplayMesh { params, mesh }
}

/// The mesh as JSON: welded positions (meters), corners (a position and a
/// normal each), triangles of corners counterclockwise from outside, and
/// faces, edges and vertices by stable name.
pub fn to_json(body: &Body, m: &DisplayMesh) -> Value {
    let params = json!({"chord": m.params.chord, "angle": m.params.angle});
    let mesh = match &m.mesh {
        Ok(mesh) => mesh,
        Err(e) => return json!({"ok": false, "params": params, "error": e}),
    };
    let face_name = |id| body.face(id).expect("live face").name.to_string();
    let positions: Vec<[f64; 3]> = mesh.positions.iter().map(|p| [p.x, p.y, p.z]).collect();
    let corners: Vec<Value> = mesh
        .corners
        .iter()
        .map(|c| {
            let n = c.normal.get();
            json!([c.position, n.x, n.y, n.z])
        })
        .collect();
    let faces: Vec<Value> = mesh
        .faces
        .iter()
        .map(|f| {
            json!({"name": face_name(f.face),
                   "corners": [f.corners.start, f.corners.end],
                   "triangles": [f.triangles.start, f.triangles.end]})
        })
        .collect();
    let edges: Vec<Value> = mesh
        .edges
        .iter()
        .map(|e| json!({"name": body.edge(e.edge).expect("live edge").name.to_string(), "positions": e.positions}))
        .collect();
    let vertices: Vec<Value> = body
        .vertices()
        .enumerate()
        .map(|(i, v)| json!({"name": body.vertex(v).expect("live vertex").name.to_string(), "position": i}))
        .collect();
    let faults: Vec<Value> = mesh
        .faults
        .iter()
        .map(|f| json!({"face": face_name(f.face), "kind": format!("{:?}", f.kind)}))
        .collect();
    json!({
        "ok": true,
        "params": params,
        "positions": positions,
        "corners": corners,
        "triangles": mesh.triangles,
        "faces": faces,
        "edges": edges,
        "vertices": vertices,
        "faults": faults,
    })
}

/// The binary encoding's name and magic.
pub const ENCODING: &str = "ryucad.mesh.1";
const MAGIC: &[u8; 4] = b"RYMS";
const VERSION: u32 = 1;

/// A stable name as 16 bytes.
fn name_bytes(hex: &str) -> [u8; 16] {
    u128::from_str_radix(hex, 16)
        .expect("names are hex")
        .to_be_bytes()
}

/// A body's centre: positions are sent as f32 offsets from it, which keeps
/// display error far below a pixel anywhere in the ±500 m size box.
fn centre(positions: &[ryukei::geom::Point3]) -> [f64; 3] {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in positions {
        for (k, x) in [p.x, p.y, p.z].into_iter().enumerate() {
            lo[k] = lo[k].min(x);
            hi[k] = hi[k].max(x);
        }
    }
    if positions.is_empty() {
        return [0.0; 3];
    }
    [0, 1, 2].map(|k| lo[k] + (hi[k] - lo[k]) / 2.0)
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, x: u8) {
        self.0.push(x);
    }
    fn u16(&mut self, x: u16) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn u32(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn f32(&mut self, x: f32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn f64(&mut self, x: f64) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn str16(&mut self, s: &str) {
        self.u16(s.len() as u16);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn str32(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("fewer than 2^32 items"));
    }
}

/// Encodes a part's display meshes as `ryucad.mesh.1` (little endian):
///
/// ```text
/// "RYMS" u32:version u32:bodies
/// per body: str16 feature, str16 hash, u8 status (0 ok, 1 error)
///   error: str32 JSON of the error
///   ok:    f64×3 origin, f64 chord, f64 angle
///          u32 n, n × f32×3 positions (offsets from origin)
///          u32 n, n × (u32 position, f32×3 normal) corners
///          u32 n, n × u32×3 triangles (of corners)
///          u32 n, n × (name, u32 corner start, end, u32 triangle start, end) faces
///          u32 n, n × (name, u32 k, k × u32 position) edges
///          u32 n, n × (name, u32 position) vertices
///          u32 n, n × (name, str16 kind) faults
/// name = 16 bytes; str16/str32 = u16/u32 length then UTF-8
/// ```
pub fn encode(bodies: &[(crate::doc::id::Id, Result<Meshed>)]) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.0.extend_from_slice(MAGIC);
    w.u32(VERSION);
    w.len(bodies.len());
    for (id, r) in bodies {
        w.str16(id.as_str());
        let m = match r {
            Ok(m) => m,
            Err(e) => {
                w.str16("");
                w.u8(1);
                w.str32(&json!(e).to_string());
                continue;
            }
        };
        w.str16(&m.hash);
        let mesh = match &m.mesh.mesh {
            Ok(mesh) => mesh,
            Err(e) => {
                w.u8(1);
                w.str32(&json!(e).to_string());
                continue;
            }
        };
        w.u8(0);
        let body = &m.body;
        let o = centre(&mesh.positions);
        o.iter().for_each(|x| w.f64(*x));
        w.f64(m.mesh.params.chord);
        w.f64(m.mesh.params.angle);
        w.len(mesh.positions.len());
        for p in &mesh.positions {
            for (k, x) in [p.x, p.y, p.z].into_iter().enumerate() {
                w.f32((x - o[k]) as f32);
            }
        }
        w.len(mesh.corners.len());
        for c in &mesh.corners {
            w.u32(c.position);
            let n = c.normal.get();
            [n.x, n.y, n.z].into_iter().for_each(|x| w.f32(x as f32));
        }
        w.len(mesh.triangles.len());
        mesh.triangles.iter().flatten().for_each(|i| w.u32(*i));
        let named = |w: &mut Writer, hex: String| w.0.extend_from_slice(&name_bytes(&hex));
        w.len(mesh.faces.len());
        for f in &mesh.faces {
            named(
                &mut w,
                body.face(f.face).expect("live face").name.to_string(),
            );
            [
                f.corners.start,
                f.corners.end,
                f.triangles.start,
                f.triangles.end,
            ]
            .into_iter()
            .for_each(|x| w.u32(x));
        }
        w.len(mesh.edges.len());
        for e in &mesh.edges {
            named(
                &mut w,
                body.edge(e.edge).expect("live edge").name.to_string(),
            );
            w.len(e.positions.len());
            e.positions.iter().for_each(|p| w.u32(*p));
        }
        let vertices: Vec<_> = body.vertices().collect();
        w.len(vertices.len());
        for (i, v) in vertices.into_iter().enumerate() {
            named(
                &mut w,
                body.vertex(v).expect("live vertex").name.to_string(),
            );
            w.u32(i as u32);
        }
        w.len(mesh.faults.len());
        for f in &mesh.faults {
            named(
                &mut w,
                body.face(f.face).expect("live face").name.to_string(),
            );
            w.str16(&format!("{:?}", f.kind));
        }
    }
    w.0
}

/// One body of a decoded `ryucad.mesh.1`.
#[derive(Clone, Debug, PartialEq)]
pub struct WireBody {
    pub feature: String,
    pub hash: String,
    /// The mesh, or the error (as JSON) why there is none.
    pub mesh: core::result::Result<WireMesh, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WireMesh {
    pub origin: [f64; 3],
    pub chord: f64,
    pub angle: f64,
    pub positions: Vec<[f32; 3]>,
    pub corners: Vec<(u32, [f32; 3])>,
    pub triangles: Vec<[u32; 3]>,
    pub faces: Vec<WireFace>,
    pub edges: Vec<([u8; 16], Vec<u32>)>,
    pub vertices: Vec<([u8; 16], u32)>,
    pub faults: Vec<([u8; 16], String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WireFace {
    pub name: [u8; 16],
    pub corners: (u32, u32),
    pub triangles: (u32, u32),
}

/// The 32 hex digits of a 16-byte name.
pub fn name_hex(name: &[u8; 16]) -> String {
    format!("{:032x}", u128::from_be_bytes(*name))
}

struct Reader<'a>(&'a [u8]);

fn corrupt(why: &str) -> Error {
    Error::new(Code::InvalidDocument, format!("bad {ENCODING} data: {why}"))
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(corrupt("truncated"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }
    fn name(&mut self) -> Result<[u8; 16]> {
        Ok(self.take(16)?.try_into().expect("16 bytes"))
    }
    fn string(&mut self, n: usize) -> Result<String> {
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| corrupt("text is not UTF-8"))
    }
    fn str16(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        self.string(n)
    }
    fn str32(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        self.string(n)
    }
    /// A count of items at least `min` bytes each, refused if the data
    /// can't hold them (so a corrupt count can't ask for a huge buffer).
    fn count(&mut self, min: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min) > self.0.len() {
            return Err(corrupt("a count larger than the data"));
        }
        Ok(n)
    }
}

/// Decodes `ryucad.mesh.1`, checking every index.
pub fn decode(bytes: &[u8]) -> Result<Vec<WireBody>> {
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC {
        return Err(corrupt("not mesh data"));
    }
    let version = r.u32()?;
    if version != VERSION {
        return Err(corrupt(&format!("version {version}, expected {VERSION}")));
    }
    let n = r.count(5)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let feature = r.str16()?;
        let hash = r.str16()?;
        let mesh = match r.u8()? {
            0 => Ok(decode_mesh(&mut r)?),
            1 => Err(serde_json::from_str(&r.str32()?).map_err(|_| corrupt("bad error JSON"))?),
            _ => return Err(corrupt("bad status")),
        };
        out.push(WireBody {
            feature,
            hash,
            mesh,
        });
    }
    if !r.0.is_empty() {
        return Err(corrupt("trailing bytes"));
    }
    Ok(out)
}

fn decode_mesh(r: &mut Reader) -> Result<WireMesh> {
    let origin = [r.f64()?, r.f64()?, r.f64()?];
    let (chord, angle) = (r.f64()?, r.f64()?);
    let mut positions = Vec::new();
    for _ in 0..r.count(12)? {
        positions.push([r.f32()?, r.f32()?, r.f32()?]);
    }
    let np = positions.len() as u32;
    let mut corners = Vec::new();
    for _ in 0..r.count(16)? {
        let p = r.u32()?;
        if p >= np {
            return Err(corrupt("a corner past the positions"));
        }
        corners.push((p, [r.f32()?, r.f32()?, r.f32()?]));
    }
    let nc = corners.len() as u32;
    let mut triangles = Vec::new();
    for _ in 0..r.count(12)? {
        let t = [r.u32()?, r.u32()?, r.u32()?];
        if t.iter().any(|i| *i >= nc) {
            return Err(corrupt("a triangle past the corners"));
        }
        triangles.push(t);
    }
    let nt = triangles.len() as u32;
    let mut faces = Vec::new();
    for _ in 0..r.count(32)? {
        let name = r.name()?;
        let (cs, ce, ts, te) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        if cs > ce || ce > nc || ts > te || te > nt {
            return Err(corrupt("a face range past the data"));
        }
        faces.push(WireFace {
            name,
            corners: (cs, ce),
            triangles: (ts, te),
        });
    }
    let mut edges = Vec::new();
    for _ in 0..r.count(20)? {
        let name = r.name()?;
        let mut ps = Vec::new();
        for _ in 0..r.count(4)? {
            let p = r.u32()?;
            if p >= np {
                return Err(corrupt("an edge point past the positions"));
            }
            ps.push(p);
        }
        edges.push((name, ps));
    }
    let mut vertices = Vec::new();
    for _ in 0..r.count(20)? {
        let name = r.name()?;
        let p = r.u32()?;
        if p >= np {
            return Err(corrupt("a vertex past the positions"));
        }
        vertices.push((name, p));
    }
    let mut faults = Vec::new();
    for _ in 0..r.count(18)? {
        faults.push((r.name()?, r.str16()?));
    }
    Ok(WireMesh {
        origin,
        chord,
        angle,
        positions,
        corners,
        triangles,
        faces,
        edges,
        vertices,
        faults,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{Placement, Primitive, PrimitiveType};
    use crate::doc::id::Id;
    use crate::rebuild::build_feature;
    use crate::units::{Quantity, Unit};

    fn sphere(r: &str) -> (Feature, Built) {
        let f = Feature::Primitive(Primitive {
            ty: PrimitiveType::Sphere,
            params: [("radius".to_owned(), Quantity::parse(r, Unit::Mm).unwrap())].into(),
            placement: Placement::identity(Unit::Mm),
            combine: crate::doc::solid::Combine::NEW,
        });
        let id = Id::parse("s", "feature").unwrap();
        let b = build_feature(&id, &id, &f).unwrap();
        (f, b)
    }

    #[test]
    fn binary_carries_what_json_does() {
        let (f, b) = sphere("10 mm");
        let mut cache = Cache::default();
        let m = Meshed {
            body: b.body.clone(),
            hash: b.hash.clone(),
            mesh: cache.get(&f, &b, Quality::Coarse, usize::MAX),
        };
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        let failed = Err(Error::new(Code::RebuildFailed, "no"));
        let bytes = encode(&[(id("s"), Ok(m.clone())), (id("t"), failed)]);
        let wire = decode(&bytes).unwrap();
        assert_eq!(wire.len(), 2);
        assert_eq!(
            (wire[0].feature.as_str(), wire[0].hash.as_str()),
            ("s", b.hash.as_str())
        );
        assert_eq!(wire[1].mesh.as_ref().unwrap_err()["code"], "rebuild_failed");
        let w = wire[0].mesh.as_ref().unwrap();
        let j = to_json(&m.body, &m.mesh);
        assert_eq!(w.triangles.len(), j["triangles"].as_array().unwrap().len());
        assert_eq!(json!(w.triangles), j["triangles"]);
        assert_eq!(name_hex(&w.faces[0].name), j["faces"][0]["name"]);
        // Positions agree to f32 precision around the body's centre.
        for (p, q) in w.positions.iter().zip(j["positions"].as_array().unwrap()) {
            for k in 0..3 {
                let exact = q[k].as_f64().unwrap();
                assert!((w.origin[k] + p[k] as f64 - exact).abs() < 1e-8);
            }
        }
        // Truncations are errors, never panics: the first and last bytes,
        // and cuts spread through the middle.
        let step = (bytes.len() / 300).max(1);
        let cuts = (0..64)
            .chain((0..bytes.len()).step_by(step))
            .chain(bytes.len() - 64..bytes.len());
        for n in cuts {
            assert!(decode(&bytes[..n]).is_err(), "{n}");
        }
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(decode(&bad).is_err());
    }

    #[test]
    fn quality_scales_with_size() {
        let p = Quality::Normal.params(2.0);
        assert_eq!(p.chord, 1e-3);
        assert_eq!(Quality::Coarse.params(2.0).chord, 2e-2);
        assert_eq!(Quality::Fine.params(2.0).chord, 2.5e-4);
        // Tiny bodies stop at ryukei's floor.
        assert_eq!(
            Quality::Fine.params(1e-9).chord,
            10.0 * ryukei::tol::LINEAR_RESOLUTION
        );
        assert!(Quality::Fine.params(1.0).angle < Quality::Coarse.params(1.0).angle);
        assert_eq!(Quality::parse("fine").unwrap(), Quality::Fine);
        assert_eq!(
            Quality::parse("ultra").unwrap_err().code,
            Code::InvalidParam
        );
    }

    #[test]
    fn meshes_are_cached_by_hash_and_quality() {
        let (f, b) = sphere("10 mm");
        let mut cache = Cache::default();
        let a = cache.get(&f, &b, Quality::Normal, usize::MAX);
        let again = cache.get(&f, &b, Quality::Normal, usize::MAX);
        assert!(Arc::ptr_eq(&a, &again));
        let fine = cache.get(&f, &b, Quality::Fine, usize::MAX);
        assert!(!Arc::ptr_eq(&a, &fine));
        let (n, nf) = (
            a.mesh.as_ref().unwrap().triangles.len(),
            fine.mesh.as_ref().unwrap().triangles.len(),
        );
        assert!(nf > n, "{nf} > {n}");
        // Coarse is visibly coarse: a fraction of fine's triangles.
        let coarse = cache.get(&f, &b, Quality::Coarse, usize::MAX);
        let nc = coarse.mesh.as_ref().unwrap().triangles.len();
        assert!(4 * nc <= n, "coarse {nc}, normal {n}");
        assert!(16 * nc <= nf, "coarse {nc}, fine {nf}");
        // Over budget, the least recently used goes; the newest stays.
        let (f2, b2) = sphere("20 mm");
        cache.get(&f2, &b2, Quality::Normal, 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn json_names_every_face_edge_and_vertex() {
        let f = Feature::Primitive(Primitive {
            ty: PrimitiveType::Cylinder,
            params: [
                ("radius".to_owned(), Quantity::parse("5", Unit::Mm).unwrap()),
                (
                    "height".to_owned(),
                    Quantity::parse("10", Unit::Mm).unwrap(),
                ),
            ]
            .into(),
            placement: Placement::identity(Unit::Mm),
            combine: crate::doc::solid::Combine::NEW,
        });
        let id = Id::parse("c", "feature").unwrap();
        let b = build_feature(&id, &id, &f).unwrap();
        let m = make(&f, &b.body, Quality::Coarse);
        let v = to_json(&b.body, &m);
        assert_eq!(v["ok"], true);
        let c = b.body.counts();
        assert_eq!(v["faces"].as_array().unwrap().len(), c.faces);
        assert_eq!(v["edges"].as_array().unwrap().len(), c.edges);
        assert_eq!(v["vertices"].as_array().unwrap().len(), c.vertices);
        assert_eq!(v["faults"], json!([]));
        let corners = v["corners"].as_array().unwrap().len() as u64;
        let max = v["triangles"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| t.as_array().unwrap().iter().map(|i| i.as_u64().unwrap()))
            .max()
            .unwrap();
        assert!(max < corners);
        assert_eq!(v["faces"][0]["name"].as_str().unwrap().len(), 32);
    }
}
