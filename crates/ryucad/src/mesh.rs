//! Display meshes. The daemon tessellates each body with parameters
//! derived from its size and the display quality, and caches the result by
//! body hash, so editing one feature re-meshes one body; beneath that, the
//! disk cache keeps them by body hash and parameters ([`crate::cache`]).
//! Agents get JSON; the app can ask for a compact binary form. Exports to
//! mesh formats mesh at their own parameters through the same caches
//! ([`Cache::export_all`]).

use crate::doc::files::Feature;
use crate::error::{Code, Error, Result};
use crate::rebuild::Built;
use ryukei::mesh::{BodyMesh, MeshParams};
use ryukei::topo::Body;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    Coarse,
    #[default]
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

/// A body's mesh and the parameters it was made with.
#[derive(Debug)]
pub struct DisplayMesh {
    pub params: MeshParams,
    pub mesh: Result<Arc<BodyMesh>>,
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

/// What a mesh is kept under beside its body's hash: a display quality,
/// or an export's parameters by their bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Display(Quality),
    Export([u64; 4]),
}

impl Key {
    fn export(p: &MeshParams) -> Key {
        Key::Export([
            p.chord.to_bits(),
            p.angle.to_bits(),
            p.max_edge.map_or(u64::MAX, f64::to_bits),
            p.max_triangles,
        ])
    }
}

/// Meshes by body hash and quality (or an export's parameters), least
/// recently used first out when the cache passes its budget, and the disk
/// cache beneath them, if any.
#[derive(Debug, Default)]
pub struct Cache {
    entries: BTreeMap<(String, Key), (u64, Arc<DisplayMesh>)>,
    clock: u64,
    bytes: usize,
    pub disk: Option<Arc<crate::cache::Disk>>,
}

impl Cache {
    /// An empty cache over `disk`.
    pub fn with_disk(disk: Option<Arc<crate::cache::Disk>>) -> Cache {
        Cache {
            disk,
            ..Cache::default()
        }
    }

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
            .filter(|(_, b)| {
                !self
                    .entries
                    .contains_key(&(b.hash.clone(), Key::Display(quality)))
            })
            .collect();
        let disk = self.disk.as_deref();
        let made: Vec<(String, Arc<DisplayMesh>)> = missing
            .par_iter()
            .map(|(f, b)| (b.hash.clone(), Arc::new(make(disk, f, b, quality))))
            .collect();
        let mut fresh: BTreeMap<String, Arc<DisplayMesh>> = BTreeMap::new();
        for (hash, m) in made {
            self.add(&hash, Key::Display(quality), &m);
            fresh.insert(hash, m);
        }
        let out: Vec<Arc<DisplayMesh>> = items
            .iter()
            .map(|(_, b)| {
                self.clock += 1;
                match self
                    .entries
                    .get_mut(&(b.hash.clone(), Key::Display(quality)))
                {
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

    /// The meshes of `items` the cache doesn't hold, to be made away from
    /// the session ([`Pending::make`]) and put back with [`Cache::put`];
    /// `None` when it holds them all.
    pub fn pending(&self, items: &[(&Feature, &Built)], quality: Quality) -> Option<Pending> {
        let mut seen = std::collections::BTreeSet::new();
        let items: Vec<(Feature, Built)> = items
            .iter()
            .filter(|(_, b)| {
                !self
                    .entries
                    .contains_key(&(b.hash.clone(), Key::Display(quality)))
            })
            .filter(|(_, b)| seen.insert(b.hash.clone()))
            .map(|(f, b)| ((*f).clone(), (*b).clone()))
            .collect();
        (!items.is_empty()).then(|| Pending {
            disk: self.disk.clone(),
            quality,
            items,
        })
    }

    /// Takes in meshes [`Pending::make`] made, keeping those the cache
    /// already has (made meanwhile by another request).
    pub fn put(&mut self, made: Made, budget: usize) {
        for (hash, m) in made.meshes {
            if !self
                .entries
                .contains_key(&(hash.clone(), Key::Display(made.quality)))
            {
                self.add(&hash, Key::Display(made.quality), &m);
            }
        }
        self.evict(budget);
    }

    fn add(&mut self, hash: &str, key: Key, m: &Arc<DisplayMesh>) {
        self.bytes += m.bytes();
        self.entries
            .insert((hash.to_owned(), key), (self.clock, Arc::clone(m)));
    }

    /// The meshes of several bodies for an export at `params`, in order:
    /// each body (by hash) meshed once, those not cached in parallel.
    pub fn export_all(
        &mut self,
        items: &[&Built],
        params: &MeshParams,
        budget: usize,
    ) -> Vec<Arc<DisplayMesh>> {
        use rayon::prelude::*;
        let key = Key::export(params);
        let mut missing: BTreeMap<&str, &Built> = BTreeMap::new();
        for b in items {
            if !self.entries.contains_key(&(b.hash.clone(), key)) {
                missing.entry(&b.hash).or_insert(b);
            }
        }
        let disk = self.disk.as_deref();
        let made: Vec<(String, Arc<DisplayMesh>)> = missing
            .into_values()
            .collect::<Vec<_>>()
            .par_iter()
            .map(|b| (b.hash.clone(), Arc::new(make_with(disk, b, *params))))
            .collect();
        for (hash, m) in &made {
            self.add(hash, key, m);
        }
        let out = items
            .iter()
            .map(|b| {
                self.clock += 1;
                let (used, m) = self
                    .entries
                    .get_mut(&(b.hash.clone(), key))
                    .expect("made above");
                *used = self.clock;
                Arc::clone(m)
            })
            .collect();
        // What it made stays until the budget pushes it out, the meshes the
        // export uses last.
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

    /// Whether the mesh of the body with `hash` at `quality` is held.
    pub fn has(&self, hash: &str, quality: Quality) -> bool {
        self.entries.contains_key(&(hash.to_owned(), quality))
    }

    /// What the meshes held take, in bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Meshes a cache lacks, with what making them needs, so the daemon can
/// make them without holding its session (#91).
pub struct Pending {
    disk: Option<Arc<crate::cache::Disk>>,
    quality: Quality,
    items: Vec<(Feature, Built)>,
}

/// Meshes [`Pending::make`] made, for [`Cache::put`].
pub struct Made {
    quality: Quality,
    meshes: Vec<(String, Arc<DisplayMesh>)>,
}

impl Pending {
    /// Makes the meshes, in parallel, as [`Cache::get_all`] would.
    pub fn make(self) -> Made {
        use rayon::prelude::*;
        let (disk, quality) = (self.disk.as_deref(), self.quality);
        let meshes = self
            .items
            .par_iter()
            .map(|(f, b)| (b.hash.clone(), Arc::new(make(disk, f, b, quality))))
            .collect();
        Made { quality, meshes }
    }
}

/// The diagonal of the feature's bounding box.
fn diagonal(f: &Feature, body: &Body) -> f64 {
    match crate::inspect::extent(f).or_else(|| crate::inspect::loose_extent(body)) {
        Some((lo, hi)) => {
            let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        }
        None => 1.0,
    }
}

/// The disk cache's key for a mesh of the body hashed `hash` made with
/// `params`.
fn disk_input(hash: &str, params: &MeshParams) -> Vec<u8> {
    let mut v = hash.as_bytes().to_vec();
    v.push(0);
    for x in [params.chord, params.angle, params.max_edge.unwrap_or(-1.0)] {
        v.extend_from_slice(&x.to_bits().to_le_bytes());
    }
    v.extend_from_slice(&params.max_triangles.to_le_bytes());
    v
}

/// The display mesh of `built` (feature `f`) at `quality`.
fn make(
    disk: Option<&crate::cache::Disk>,
    f: &Feature,
    built: &Built,
    quality: Quality,
) -> DisplayMesh {
    make_with(disk, built, quality.params(diagonal(f, &built.body)))
}

/// The mesh of `built` at `params`: from `disk` if it holds it, else
/// made, and written to `disk` when it meshed. A mesh past ryukei's budget
/// says by how much (`triangles`).
fn make_with(disk: Option<&crate::cache::Disk>, built: &Built, params: MeshParams) -> DisplayMesh {
    use crate::cache::Kind;
    let input = disk_input(&built.hash, &params);
    if let Some(d) = disk
        && let Some(bytes) = d.get(Kind::Mesh, &input)
    {
        match ryukei::codec::decode_mesh(&bytes) {
            Ok(m) => {
                return DisplayMesh {
                    params,
                    mesh: Ok(Arc::new(m)),
                };
            }
            Err(_) => d.discard(Kind::Mesh, &input),
        }
    }
    let mesh = ryukei::mesh::tessellate(&built.body, &params).map_err(|e| {
        let err = Error::new(Code::MeshFailed, format!("can't mesh the body: {e}"))
            .with("kernel", format!("{e:?}"));
        match e {
            ryukei::Error::MeshTooLarge { triangles } => err.with("triangles", triangles),
            _ => err,
        }
    });
    if let (Some(d), Ok(m)) = (disk, &mesh) {
        d.put(Kind::Mesh, &input, &ryukei::codec::encode_mesh(m));
    }
    DisplayMesh {
        params,
        mesh: mesh.map(Arc::new),
    }
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
/// The compact encoding: `ryucad.mesh.1`'s layout with indices in two
/// bytes where they fit and normals in four ([`encode_compact`]).
pub const COMPACT: &str = "ryucad.mesh.2";
const MAGIC: &[u8; 4] = b"RYMS";
const VERSION: u32 = 1;
const COMPACT_VERSION: u32 = 2;

/// Whether indices into `n` items need four bytes: two hold up to 65,536.
fn wide(n: usize) -> bool {
    n > 1 << 16
}

/// `v` in `[-1, 1]` as an `i16`, to the nearest of 65,535 steps.
fn snorm(v: f64) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn sign(v: f64) -> f64 {
    if v < 0.0 { -1.0 } else { 1.0 }
}

/// A unit normal in two `i16`s, octahedrally: projected onto the
/// octahedron `|x| + |y| + |z| = 1`, the lower half folded out over the
/// upper. Decoded ([`unoct`]) it turns by at most about 1e-4 rad.
fn oct(n: [f64; 3]) -> [i16; 2] {
    let s = n[0].abs() + n[1].abs() + n[2].abs();
    let (x, y) = (n[0] / s, n[1] / s);
    if n[2] < 0.0 {
        [
            snorm((1.0 - y.abs()) * sign(x)),
            snorm((1.0 - x.abs()) * sign(y)),
        ]
    } else {
        [snorm(x), snorm(y)]
    }
}

/// The unit normal [`oct`] encoded.
fn unoct(e: [i16; 2]) -> [f32; 3] {
    let (x, y) = (f32::from(e[0]) / 32767.0, f32::from(e[1]) / 32767.0);
    let z = 1.0 - x.abs() - y.abs();
    let (x, y) = if z < 0.0 {
        let s = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
        ((1.0 - y.abs()) * s(x), (1.0 - x.abs()) * s(y))
    } else {
        (x, y)
    };
    let l = (x * x + y * y + z * z).sqrt();
    [x / l, y / l, z / l]
}

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
    /// An index, in two bytes unless `wide`.
    fn index(&mut self, i: u32, wide: bool) {
        if wide {
            self.u32(i);
        } else {
            self.u16(u16::try_from(i).expect("an index that fits"));
        }
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
pub fn encode<K: AsRef<str>>(bodies: &[(K, Result<Meshed>)]) -> Vec<u8> {
    encode_as(bodies, false)
}

/// Encodes a part's display meshes as `ryucad.mesh.2`: `ryucad.mesh.1`
/// (version 2), but for these, about half its size:
///
/// ```text
/// u32 n, n × (index position, i16×2 normal) corners
/// u32 n, n × index×3 triangles
/// edges' and vertices' positions as indices
/// index = u16 where what it indexes numbers at most 65,536, else u32;
/// normal = octahedral, each i16 a coordinate times 32,767
/// ```
///
/// The app reads it; `ryucad.mesh.1` stays what `binary` sends, which
/// other readers (the hosted viewer) decode.
pub fn encode_compact<K: AsRef<str>>(bodies: &[(K, Result<Meshed>)]) -> Vec<u8> {
    encode_as(bodies, true)
}

fn encode_as<K: AsRef<str>>(bodies: &[(K, Result<Meshed>)], compact: bool) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.0.extend_from_slice(MAGIC);
    w.u32(if compact { COMPACT_VERSION } else { VERSION });
    w.len(bodies.len());
    for (id, r) in bodies {
        w.str16(id.as_ref());
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
        // Indices are four bytes in `ryucad.mesh.1`, as wide as needed in
        // the compact encoding.
        let (wp, wc) = (
            !compact || wide(mesh.positions.len()),
            !compact || wide(mesh.corners.len()),
        );
        w.len(mesh.corners.len());
        for c in &mesh.corners {
            w.index(c.position, wp);
            let n = c.normal.get();
            if compact {
                oct([n.x, n.y, n.z])
                    .into_iter()
                    .for_each(|x| w.u16(x as u16));
            } else {
                [n.x, n.y, n.z].into_iter().for_each(|x| w.f32(x as f32));
            }
        }
        w.len(mesh.triangles.len());
        mesh.triangles
            .iter()
            .flatten()
            .for_each(|i| w.index(*i, wc));
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
            e.positions.iter().for_each(|p| w.index(*p, wp));
        }
        let vertices: Vec<_> = body.vertices().collect();
        w.len(vertices.len());
        for (i, v) in vertices.into_iter().enumerate() {
            named(
                &mut w,
                body.vertex(v).expect("live vertex").name.to_string(),
            );
            w.index(i as u32, wp);
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

/// One body of a decoded `ryucad.mesh.1` or `ryucad.mesh.2`.
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
    /// An index, two bytes unless `wide`.
    fn index(&mut self, wide: bool) -> Result<u32> {
        if wide {
            self.u32()
        } else {
            Ok(u32::from(self.u16()?))
        }
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

/// Decodes `ryucad.mesh.1` or `ryucad.mesh.2`, checking every index.
pub fn decode(bytes: &[u8]) -> Result<Vec<WireBody>> {
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC {
        return Err(corrupt("not mesh data"));
    }
    let version = r.u32()?;
    if version != VERSION && version != COMPACT_VERSION {
        return Err(corrupt(&format!(
            "version {version}, expected {VERSION} or {COMPACT_VERSION}"
        )));
    }
    let compact = version == COMPACT_VERSION;
    let n = r.count(5)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let feature = r.str16()?;
        let hash = r.str16()?;
        let mesh = match r.u8()? {
            0 => Ok(decode_mesh(&mut r, compact)?),
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

fn decode_mesh(r: &mut Reader, compact: bool) -> Result<WireMesh> {
    let origin = [r.f64()?, r.f64()?, r.f64()?];
    let (chord, angle) = (r.f64()?, r.f64()?);
    let mut positions = Vec::new();
    for _ in 0..r.count(12)? {
        positions.push([r.f32()?, r.f32()?, r.f32()?]);
    }
    let np = positions.len() as u32;
    let wp = !compact || wide(positions.len());
    let ip = if wp { 4 } else { 2 };
    let mut corners = Vec::new();
    for _ in 0..r.count(ip + if compact { 4 } else { 12 })? {
        let p = r.index(wp)?;
        if p >= np {
            return Err(corrupt("a corner past the positions"));
        }
        let n = if compact {
            unoct([r.u16()? as i16, r.u16()? as i16])
        } else {
            [r.f32()?, r.f32()?, r.f32()?]
        };
        corners.push((p, n));
    }
    let nc = corners.len() as u32;
    let wc = !compact || wide(corners.len());
    let ic = if wc { 4 } else { 2 };
    let mut triangles = Vec::new();
    for _ in 0..r.count(3 * ic)? {
        let t = [r.index(wc)?, r.index(wc)?, r.index(wc)?];
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
        for _ in 0..r.count(ip)? {
            let p = r.index(wp)?;
            if p >= np {
                return Err(corrupt("an edge point past the positions"));
            }
            ps.push(p);
        }
        edges.push((name, ps));
    }
    let mut vertices = Vec::new();
    for _ in 0..r.count(16 + ip)? {
        let name = r.name()?;
        let p = r.index(wp)?;
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

    /// `m`'s wire form both ways: `ryucad.mesh.1` and `ryucad.mesh.2`.
    fn both_ways(m: &Meshed) -> (Vec<u8>, WireMesh, Vec<u8>, WireMesh) {
        let id = Id::parse("s", "feature").unwrap();
        let bodies = [(id, Ok(m.clone()))];
        let (one, two) = (encode(&bodies), encode_compact(&bodies));
        let wire = |b: &[u8]| decode(b).unwrap().remove(0).mesh.unwrap();
        let (w1, w2) = (wire(&one), wire(&two));
        (one, w1, two, w2)
    }

    /// The compact encoding carries what `ryucad.mesh.1` does, bit for
    /// bit but for normals, which turn by at most 2e-4 rad.
    fn alike(w1: &WireMesh, w2: &WireMesh) {
        assert_eq!(w1.positions, w2.positions);
        assert_eq!(w1.triangles, w2.triangles);
        assert_eq!(w1.faces, w2.faces);
        assert_eq!(w1.edges, w2.edges);
        assert_eq!(w1.vertices, w2.vertices);
        assert_eq!(w1.faults, w2.faults);
        assert_eq!(
            (w1.origin, w1.chord, w1.angle),
            (w2.origin, w2.chord, w2.angle)
        );
        assert_eq!(w1.corners.len(), w2.corners.len());
        for ((p1, n1), (p2, n2)) in w1.corners.iter().zip(&w2.corners) {
            assert_eq!(p1, p2);
            let f = |n: &[f32; 3]| n.map(f64::from);
            assert!(turn(f(n1), f(n2)) < 2e-4, "{n1:?} {n2:?}");
            let len: f32 = n2.iter().map(|x| x * x).sum();
            assert!((len - 1.0).abs() < 1e-6);
        }
    }

    /// The angle between unit vectors, from their cross product: precise
    /// where a dot product's cosine isn't.
    fn turn(a: [f64; 3], b: [f64; 3]) -> f64 {
        let c = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
    }

    #[test]
    fn the_compact_encoding_carries_the_same_mesh_in_about_half() {
        let (f, b) = sphere("10 mm");
        let m = Meshed {
            body: b.body.clone(),
            hash: b.hash.clone(),
            mesh: Cache::default().get(&f, &b, Quality::Normal, usize::MAX),
        };
        let (one, w1, two, w2) = both_ways(&m);
        alike(&w1, &w2);
        assert!(
            two.len() * 10 <= one.len() * 6,
            "{} bytes against {}",
            two.len(),
            one.len()
        );
        let step = (two.len() / 300).max(1);
        for n in (0..two.len()).step_by(step) {
            assert!(decode(&two[..n]).is_err(), "{n}");
        }
    }

    #[test]
    fn indices_widen_past_65536() {
        let (_, b) = sphere("10 mm");
        let params = MeshParams::new(2e-7, Quality::Normal.angle());
        let mesh = ryukei::mesh::tessellate(&b.body, &params).unwrap();
        assert!(wide(mesh.positions.len()) && wide(mesh.corners.len()));
        let m = Meshed {
            body: b.body.clone(),
            hash: b.hash.clone(),
            mesh: Arc::new(DisplayMesh {
                params,
                mesh: Ok(mesh),
            }),
        };
        let (_, w1, _, w2) = both_ways(&m);
        alike(&w1, &w2);
        assert!(!wide(1 << 16) && wide((1 << 16) + 1));
    }

    #[test]
    fn octahedral_normals_round_trip() {
        let s = 1.0 / 3f64.sqrt();
        let mut normals = vec![
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [s, -s, -s],
            [-s, s, -s],
        ];
        // A spiral over the sphere.
        for i in 0..2000 {
            let z = 1.0 - (2 * i + 1) as f64 / 2000.0;
            let r = (1.0 - z * z).sqrt();
            let (sn, cs) = ryukei::math::sin_cos(i as f64 * 2.399_963);
            normals.push([r * cs, r * sn, z]);
        }
        for n in normals {
            let m = unoct(oct(n));
            let t = turn(n, m.map(f64::from));
            assert!(t < 1e-4, "{n:?} came back as {m:?}, {t:e} rad off");
        }
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
    fn meshes_come_from_the_disk_cache() {
        let dir = tempfile::tempdir().unwrap();
        let disk = Arc::new(crate::cache::Disk::open(dir.path(), [5; 32]));
        let (f, b) = sphere("10 mm");
        let mut first = Cache::with_disk(Some(Arc::clone(&disk)));
        let made = first.get(&f, &b, Quality::Normal, usize::MAX);
        assert_eq!(disk.stats().kinds[2].1, 1, "one mesh written");
        let mut second = Cache::with_disk(Some(Arc::clone(&disk)));
        let read = second.get(&f, &b, Quality::Normal, usize::MAX);
        assert_eq!(read.mesh.as_ref().unwrap(), made.mesh.as_ref().unwrap());
        assert_eq!(read.params, made.params);
        // Another quality is another entry; a cleared cache makes it again.
        second.get(&f, &b, Quality::Coarse, usize::MAX);
        assert_eq!(disk.stats().kinds[2].1, 2);
        disk.clear();
        let again = Cache::default().get(&f, &b, Quality::Normal, usize::MAX);
        assert_eq!(again.mesh.as_ref().unwrap(), made.mesh.as_ref().unwrap());
    }

    #[test]
    fn export_meshes_are_kept_by_their_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let disk = Arc::new(crate::cache::Disk::open(dir.path(), [6; 32]));
        let (_, b) = sphere("10 mm");
        let (_, other) = sphere("20 mm");
        let params = MeshParams::new(1e-5, 0.26);
        let mut cache = Cache::with_disk(Some(Arc::clone(&disk)));
        // One body twice, one other: two meshes made, the first shared.
        let m = cache.export_all(&[&b, &other, &b], &params, usize::MAX);
        assert!(Arc::ptr_eq(&m[0], &m[2]));
        assert!(!Arc::ptr_eq(&m[0], &m[1]));
        assert_eq!(m[0].params, params);
        assert_eq!(disk.stats().kinds[2].1, 2, "both written to disk");
        // The same parameters again: from memory.
        let again = cache.export_all(&[&b], &params, usize::MAX);
        assert!(Arc::ptr_eq(&again[0], &m[0]));
        assert_eq!(cache.len(), 2);
        // Another chord is another mesh, finer here.
        let fine = cache.export_all(&[&b], &MeshParams::new(2.5e-6, 0.26), usize::MAX);
        let count = |d: &DisplayMesh| d.mesh.as_ref().unwrap().triangles.len();
        assert!(count(&fine[0]) > count(&m[0]));
        // A fresh cache over the disk reads it back.
        let read = Cache::with_disk(Some(disk)).export_all(&[&b], &params, usize::MAX);
        assert_eq!(read[0].mesh.as_ref().unwrap(), m[0].mesh.as_ref().unwrap());
        // Past the budget, the refusal says how many triangles it would take.
        let small = MeshParams {
            max_triangles: 10,
            ..params
        };
        let e = cache.export_all(&[&b], &small, usize::MAX)[0]
            .mesh
            .clone()
            .unwrap_err();
        assert_eq!(e.code, Code::MeshFailed);
        assert!(e.details["triangles"].as_u64().unwrap() > 10, "{e:?}");
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
        let m = make(None, &f, &b, Quality::Coarse);
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
