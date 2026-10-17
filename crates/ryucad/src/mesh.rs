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
        use core::f64::consts::PI;
        let (fraction, angle) = match self {
            Quality::Coarse => (2e-3, PI / 6.0),
            Quality::Normal => (5e-4, PI / 12.0),
            Quality::Fine => (1.25e-4, PI * 8.0 / 180.0),
        };
        let floor = 10.0 * ryukei::tol::LINEAR_RESOLUTION;
        MeshParams::new((diagonal * fraction).max(floor), angle)
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
        self.clock += 1;
        let key = (built.hash.clone(), quality);
        if let Some((used, m)) = self.entries.get_mut(&key) {
            *used = self.clock;
            return Arc::clone(m);
        }
        let m = Arc::new(make(f, &built.body, quality));
        self.bytes += m.bytes();
        self.entries.insert(key, (self.clock, Arc::clone(&m)));
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
        m
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The diagonal of the feature's bounding box.
fn diagonal(f: &Feature) -> f64 {
    match crate::inspect::extent(f) {
        Some((lo, hi)) => {
            let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        }
        None => 1.0,
    }
}

fn make(f: &Feature, body: &Body, quality: Quality) -> DisplayMesh {
    let params = quality.params(diagonal(f));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{FeatureType, Placement};
    use crate::doc::id::Id;
    use crate::rebuild::build_feature;
    use crate::units::{Quantity, Unit};

    fn sphere(r: &str) -> (Feature, Built) {
        let f = Feature {
            ty: FeatureType::Sphere,
            params: [("radius".to_owned(), Quantity::parse(r, Unit::Mm).unwrap())].into(),
            placement: Placement::identity(Unit::Mm),
        };
        let id = Id::parse("s", "feature").unwrap();
        let b = build_feature(&id, &id, &f).unwrap();
        (f, b)
    }

    #[test]
    fn quality_scales_with_size() {
        let p = Quality::Normal.params(2.0);
        assert_eq!(p.chord, 1e-3);
        assert_eq!(Quality::Coarse.params(2.0).chord, 4e-3);
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
        // Over budget, the least recently used goes; the newest stays.
        let (f2, b2) = sphere("20 mm");
        cache.get(&f2, &b2, Quality::Normal, 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn json_names_every_face_edge_and_vertex() {
        let f = Feature {
            ty: FeatureType::Cylinder,
            params: [
                ("radius".to_owned(), Quantity::parse("5", Unit::Mm).unwrap()),
                (
                    "height".to_owned(),
                    Quantity::parse("10", Unit::Mm).unwrap(),
                ),
            ]
            .into(),
            placement: Placement::identity(Unit::Mm),
        };
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
