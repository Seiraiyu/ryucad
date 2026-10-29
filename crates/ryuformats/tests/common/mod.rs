//! Test-only readers and checks for what the writers write: never part
//! of the crate, which writes these formats and never reads them.

#![allow(dead_code)]

pub mod brep;
pub mod usda;

use ryuformats::scene::{Placed, Scene, Unit, placed_point};
use ryuformats::{Child, Product, Solid};
use ryukei::geom::{Rigid, UnitVector3, Vector3};
use ryukei::mesh::MeshParams;
use ryukei::name::OpKey;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A triangle as three vertices.
pub type Triangle = [[f32; 3]; 3];

/// Reads a binary STL file: its header and triangles. Panics on anything
/// malformed.
pub fn read_binary_stl(bytes: &[u8]) -> (Vec<u8>, Vec<Triangle>) {
    assert!(bytes.len() >= 84, "shorter than a header");
    let n = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    assert_eq!(
        bytes.len(),
        84 + 50 * n,
        "the count and the length disagree"
    );
    let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let triangles = (0..n)
        .map(|t| {
            let at = 84 + 50 * t + 12;
            [0, 1, 2].map(|v| [0, 1, 2].map(|k| f(at + 12 * v + 4 * k)))
        })
        .collect();
    (bytes[..80].to_vec(), triangles)
}

/// Reads an ASCII STL file: each solid's name and triangles. Panics on
/// anything malformed.
pub fn read_ascii_stl(text: &str) -> Vec<(String, Vec<Triangle>)> {
    let mut words = text.split_whitespace().peekable();
    let number = |w: Option<&str>| {
        let w = w.expect("the file ends early");
        w.parse::<f32>()
            .unwrap_or_else(|_| panic!("not a number: {w}"))
    };
    let mut solids = Vec::new();
    while words.peek().is_some() {
        assert_eq!(words.next(), Some("solid"));
        let name = words.next().expect("a solid's name").to_owned();
        let mut triangles = Vec::new();
        loop {
            match words.next() {
                Some("endsolid") => {
                    assert_eq!(words.next(), Some(name.as_str()));
                    break;
                }
                Some("facet") => assert_eq!(words.next(), Some("normal")),
                w => panic!("expected a facet, got {w:?}"),
            }
            let _normal = [0; 3].map(|_| number(words.next()));
            assert_eq!((words.next(), words.next()), (Some("outer"), Some("loop")));
            let t = [0; 3].map(|_| {
                assert_eq!(words.next(), Some("vertex"));
                [0; 3].map(|_| number(words.next()))
            });
            assert_eq!(
                (words.next(), words.next()),
                (Some("endloop"), Some("endfacet"))
            );
            triangles.push(t);
        }
        solids.push((name, triangles));
    }
    solids
}

/// Whether `triangles` close up: welded by their vertices' bits, every
/// edge used as often one way as the other, and never by one triangle
/// twice.
pub fn watertight(triangles: &[Triangle]) -> bool {
    let key = |v: [f32; 3]| v.map(f32::to_bits);
    let mut uses: BTreeMap<([u32; 3], [u32; 3]), i64> = BTreeMap::new();
    for t in triangles {
        let k = t.map(key);
        if k[0] == k[1] || k[1] == k[2] || k[2] == k[0] {
            return false;
        }
        for (a, b) in [(k[0], k[1]), (k[1], k[2]), (k[2], k[0])] {
            *uses.entry((a, b)).or_default() += 1;
        }
    }
    uses.iter()
        .all(|(&(a, b), &n)| uses.get(&(b, a)).copied().unwrap_or(0) == n)
}

/// The volume `triangles` enclose, in f64.
pub fn volume(triangles: &[Triangle]) -> f64 {
    triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| v.map(f64::from));
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum()
}

/// The volume a scene's placed meshes enclose, in `unit`'s cube, from
/// their f64 positions.
pub fn scene_volume(scene: &Scene, unit: Unit) -> f64 {
    scene.placed().iter().map(|p| placed_volume(p, unit)).sum()
}

fn placed_volume(p: &Placed<'_>, unit: Unit) -> f64 {
    let at: Vec<[f64; 3]> = p
        .mesh
        .positions
        .iter()
        .map(|&q| placed_point(&p.place, unit, q))
        .collect();
    p.mesh
        .welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| at[i as usize]);
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum()
}

/// A 3MF package read back: its entries' names in order, the model's
/// unit and metadata, each object by ID, and the build's items.
pub struct ThreeMfFile {
    pub entries: Vec<String>,
    pub content_types: String,
    pub rels: String,
    pub unit: String,
    pub metadata: Vec<(String, String)>,
    pub objects: BTreeMap<u32, Object>,
    pub items: Vec<u32>,
}

/// A mesh object's vertices and triangles.
pub type Mesh = (Vec<[f64; 3]>, Vec<[usize; 3]>);

/// An object: its name and part number, and its mesh or its components.
pub struct Object {
    pub name: String,
    pub partnumber: Option<String>,
    pub mesh: Option<Mesh>,
    pub components: Vec<(u32, [f64; 12])>,
}

const IDENTITY: [f64; 12] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

/// An element's children named `tag`.
fn children<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Vec<roxmltree::Node<'a, 'i>> {
    n.children()
        .filter(|c| c.is_element() && c.tag_name().name() == tag)
        .collect()
}

/// An attribute as a number.
fn num(n: &roxmltree::Node<'_, '_>, a: &str) -> f64 {
    n.attribute(a).unwrap().parse().unwrap()
}

/// Reads a 3MF package with the `zip` crate and its model with
/// `roxmltree`. Panics on anything malformed.
pub fn read_3mf(bytes: &[u8]) -> ThreeMfFile {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut text = |name: &str| {
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    };
    let (content_types, rels, model) = (
        text("[Content_Types].xml"),
        text("_rels/.rels"),
        text("3D/3dmodel.model"),
    );
    let entries = (0..z.len())
        .map(|k| z.by_index(k).unwrap().name().to_owned())
        .collect();
    let doc = roxmltree::Document::parse(&model).unwrap();
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "model");
    let core = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
    assert_eq!(root.tag_name().namespace(), Some(core));
    let metadata = children(root, "metadata")
        .iter()
        .map(|m| {
            (
                m.attribute("name").unwrap().to_owned(),
                m.text().unwrap_or("").to_owned(),
            )
        })
        .collect();
    let mut objects = BTreeMap::new();
    let resources = children(root, "resources")[0];
    for o in children(resources, "object") {
        assert_eq!(o.attribute("type"), Some("model"));
        let id: u32 = o.attribute("id").unwrap().parse().unwrap();
        let mesh = children(o, "mesh").first().map(|m| {
            let vertices = children(children(*m, "vertices")[0], "vertex")
                .iter()
                .map(|v| [num(v, "x"), num(v, "y"), num(v, "z")])
                .collect();
            let triangles = children(children(*m, "triangles")[0], "triangle")
                .iter()
                .map(|t| ["v1", "v2", "v3"].map(|a| num(t, a) as usize))
                .collect();
            (vertices, triangles)
        });
        let components: Vec<(u32, [f64; 12])> = children(o, "components")
            .first()
            .map(|c| {
                children(*c, "component")
                    .iter()
                    .map(|c| {
                        let t = c.attribute("transform").map_or(IDENTITY, |t| {
                            let v: Vec<f64> = t.split(' ').map(|x| x.parse().unwrap()).collect();
                            v.try_into().unwrap()
                        });
                        (num(c, "objectid") as u32, t)
                    })
                    .collect()
            })
            .unwrap_or_default();
        // An object is defined before anything uses it.
        for (c, _) in &components {
            assert!(
                objects.contains_key(c),
                "object {id} uses {c} before it is defined"
            );
        }
        objects.insert(
            id,
            Object {
                name: o.attribute("name").unwrap_or("").to_owned(),
                partnumber: o.attribute("partnumber").map(str::to_owned),
                mesh,
                components,
            },
        );
    }
    let items = children(children(root, "build")[0], "item")
        .iter()
        .map(|i| num(i, "objectid") as u32)
        .collect();
    ThreeMfFile {
        entries,
        content_types,
        rels,
        unit: root.attribute("unit").unwrap().to_owned(),
        metadata,
        objects,
        items,
    }
}

/// `p` through a 3MF transform (row vectors: `p M`).
fn apply(m: &[f64; 12], p: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|c| p[0] * m[c] + p[1] * m[3 + c] + p[2] * m[6 + c] + m[9 + c])
}

/// `first` then `then`.
fn compose(first: &[f64; 12], then: &[f64; 12]) -> [f64; 12] {
    let mut m = [0.0; 12];
    for r in 0..4 {
        for c in 0..3 {
            let mut v = if r == 3 { then[9 + c] } else { 0.0 };
            for k in 0..3 {
                v += first[3 * r + k] * then[3 * k + c];
            }
            m[3 * r + c] = v;
        }
    }
    m
}

impl ThreeMfFile {
    /// The volume the build encloses, every component placed.
    pub fn volume(&self) -> f64 {
        self.items
            .iter()
            .map(|&i| self.object_volume(i, &IDENTITY))
            .sum()
    }

    fn object_volume(&self, id: u32, m: &[f64; 12]) -> f64 {
        let o = &self.objects[&id];
        let own = o.mesh.as_ref().map_or(0.0, |(v, t)| {
            t.iter()
                .map(|t| {
                    let [a, b, c] = t.map(|k| apply(m, v[k]));
                    (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                        + a[2] * (b[0] * c[1] - b[1] * c[0]))
                        / 6.0
                })
                .sum()
        });
        own + o
            .components
            .iter()
            .map(|(c, t)| self.object_volume(*c, &compose(t, m)))
            .sum::<f64>()
    }

    /// Whether every mesh closes up by index: each edge used once each way.
    pub fn meshes_watertight(&self) -> bool {
        self.objects
            .values()
            .filter_map(|o| o.mesh.as_ref())
            .all(|(_, t)| {
                let mut uses: BTreeMap<(usize, usize), i64> = BTreeMap::new();
                for t in t {
                    if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
                        return false;
                    }
                    for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                        *uses.entry((a, b)).or_default() += 1;
                    }
                }
                uses.iter()
                    .all(|(&(a, b), &n)| n == 1 && uses.get(&(b, a)) == Some(&1))
            })
    }
}

pub fn meshed(b: ryukei::topo::Body) -> Arc<ryukei::mesh::BodyMesh> {
    Arc::new(ryukei::mesh::tessellate(&b, &MeshParams::new(1e-5, 0.26)).unwrap())
}

/// A plate with a ball on it, and a cylinder placed twice, the second
/// turned: curved faces, seams, poles.
pub fn scene() -> Scene {
    let k = OpKey(7);
    let plate = ryukei::primitive::block(k, 0.08, 0.05, 0.004, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let ball = ryukei::primitive::sphere(
        k,
        0.006,
        &Rigid::translation(Vector3::new(0.02, 0.02, 0.02)),
    )
    .unwrap()
    .0;
    let rod = ryukei::primitive::cylinder(k, 0.003, 0.03, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let solid = |name: &str, b| Solid {
        name: name.into(),
        mesh: meshed(b),
    };
    let turned = Rigid::translation(Vector3::new(0.06, 0.03, 0.01))
        .then_after(&Rigid::rotation(UnitVector3::X, 1.0));
    Scene {
        products: vec![
            Product {
                id: "A-1".into(),
                name: "Base".into(),
                part: "base".into(),
                solids: vec![solid("plate", plate), solid("ball", ball)],
                children: vec![
                    Child {
                        product: 1,
                        place: Rigid::translation(Vector3::new(0.01, 0.01, 0.004)),
                        name: "r1".into(),
                    },
                    Child {
                        product: 1,
                        place: turned,
                        name: "r2".into(),
                    },
                ],
            },
            Product {
                id: "R-1".into(),
                name: "Rod".into(),
                part: "rod".into(),
                solids: vec![solid("rod", rod)],
                children: vec![],
            },
        ],
    }
}
