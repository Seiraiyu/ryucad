//! The stage: a model of products as one USD layer.
//!
//! A part exported alone is a root `Xform` (kind `component`) holding an
//! `Xform` per solid, each with its `brep` (a `BrepArray`) and its
//! `mesh`. An assembly defines each part it places at each revision once,
//! as a `class Xform` (kind `component`) under `class "Prototypes"`, and
//! places it with an `Xform` per occurrence, `instanceable` and
//! referencing it; subassemblies are `Xform`s of their own (kind
//! `subassembly`) holding their occurrences, never instances, since an
//! instance can't hold further instances that differ. The root is kind
//! `assembly`. Each occurrence's pose is a `matrix4d xformOp:transform` in
//! the stage's unit. Prim names come from IDs ([`super::names`]); each
//! prim keeps what it stands for in `customData`'s `ryucad` dictionary:
//! part, revision, product and name for a product, the instance's ID for
//! an occurrence, `<part>/<solid>` for a solid. The layer is Z up, in the
//! export's unit, and keeps the export's settings (and the omniSolid
//! schema's commit, with B-rep) in `customLayerData`.

use super::brep::{BrepError, SCHEMA_COMMIT, brep_array};
use super::layer::{BadName, Dict, Layer, Prim, Specifier, UpAxis, Value};
use super::mesh::mesh_prim;
use super::names::Siblings;
use crate::scene::{Child, Unit};
use rayon::prelude::*;
use ryukei::geom::Rigid;
use ryukei::mesh::BodyMesh;
use ryukei::topo::Body;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The products to write, the root first, as [`crate::Scene`] has them,
/// with each solid's body as well as (or instead of) its mesh.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub products: Vec<Product>,
}

/// A part at a revision: its identifier, name, part ID, release label
/// (none at the working files), solids and what it places.
#[derive(Clone, Debug, Default)]
pub struct Product {
    pub id: String,
    pub name: String,
    pub part: String,
    pub label: Option<String>,
    pub solids: Vec<Solid>,
    pub children: Vec<Child>,
}

/// A solid: its founder's ID, its body, and its mesh in its product's
/// frame (none when no mesh is written).
#[derive(Clone, Debug)]
pub struct Solid {
    pub name: String,
    pub body: Arc<Body>,
    pub mesh: Option<Arc<BodyMesh>>,
}

/// How to write a stage: its unit, whether solids get a BrepArray and a
/// mesh (at least one), the tolerance pipes and sweeps are approximated
/// within, and the export's settings for `customLayerData`, by name in
/// order.
#[derive(Clone, Debug)]
pub struct Options {
    pub unit: Unit,
    pub brep: bool,
    pub mesh: bool,
    pub pipe_tolerance: f64,
    pub layer_data: Vec<(String, String)>,
}

/// Why a model can't be written.
#[derive(Clone, Debug, PartialEq)]
pub enum StageError {
    /// A solid's body has no BrepArray: the product and solid (by index)
    /// and why.
    Brep {
        product: usize,
        solid: usize,
        error: BrepError,
    },
    /// Neither a BrepArray nor a mesh asked for.
    Nothing,
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StageError::Brep { error, .. } => write!(f, "{error}"),
            StageError::Nothing => write!(f, "a stage needs a BrepArray or a mesh"),
        }
    }
}

impl std::error::Error for StageError {}

fn names(e: BadName) -> StageError {
    unreachable!("prim names are made identifiers: {e}")
}

/// `model` as a layer.
pub fn layer(model: &Model, opts: &Options) -> Result<Layer, StageError> {
    if !opts.brep && !opts.mesh {
        return Err(StageError::Nothing);
    }
    // Every solid's BrepArray, side by side; the first that fails, in
    // model order, refuses the stage.
    let solids: Vec<(usize, usize)> = model
        .products
        .iter()
        .enumerate()
        .flat_map(|(k, p)| (0..p.solids.len()).map(move |s| (k, s)))
        .collect();
    let breps: Vec<Result<Option<Prim>, StageError>> = solids
        .par_iter()
        .map(|&(k, s)| {
            if !opts.brep {
                return Ok(None);
            }
            brep_array(
                &model.products[k].solids[s].body,
                "brep",
                opts.unit,
                opts.pipe_tolerance,
            )
            .map(Some)
            .map_err(|error| StageError::Brep {
                product: k,
                solid: s,
                error,
            })
        })
        .collect();
    let mut brep: BTreeMap<(usize, usize), Prim> = BTreeMap::new();
    for (at, b) in solids.iter().zip(breps) {
        if let Some(p) = b? {
            brep.insert(*at, p);
        }
    }
    let mut w = Writer {
        model,
        opts,
        brep,
        prototypes: Vec::new(),
        proto_names: Siblings::default(),
        proto_of: BTreeMap::new(),
    };
    let root = &model.products[0];
    let mut top = Siblings::reserving(&["Prototypes"]);
    let root_name = top.name(&root.part);
    let mut r = Prim::new(Specifier::Def, Some("Xform"), &root_name).map_err(names)?;
    r.meta.custom_data = product_data(root);
    if root.children.is_empty() {
        r.meta.kind = Some("component".into());
        w.solids(&mut r, 0)?;
    } else {
        r.meta.kind = Some("assembly".into());
        w.contents(&mut r, 0)?;
    }
    let mut prims = Vec::new();
    if !w.prototypes.is_empty() {
        let mut protos = Prim::new(Specifier::Class, None, "Prototypes").map_err(names)?;
        protos.children = std::mem::take(&mut w.prototypes);
        prims.push(protos);
    }
    prims.push(r);
    let mut ryucad = Dict::default();
    for (k, v) in &opts.layer_data {
        ryucad.string(k, v);
    }
    if opts.brep {
        ryucad.string("omni_solid", SCHEMA_COMMIT);
    }
    let mut data = Dict::default();
    data.dict("ryucad", ryucad);
    Ok(Layer {
        default_prim: Some(root_name),
        meters_per_unit: 1.0 / opts.unit.per_meter(),
        up_axis: UpAxis::Z,
        custom_layer_data: data,
        prims,
    })
}

/// `model` as usda.
pub fn write_usda(
    model: &Model,
    opts: &Options,
    out: &mut dyn std::io::Write,
) -> Result<(), std::io::Error> {
    let l =
        layer(model, opts).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    super::usda::write(&l, out)
}

/// A product's `customData`.
fn product_data(p: &Product) -> Dict {
    let mut d = Dict::default();
    d.string("part", &p.part)
        .string("rev", p.label.as_deref().unwrap_or("working"))
        .string("product", &p.id)
        .string("name", &p.name);
    let mut out = Dict::default();
    out.dict("ryucad", d);
    out
}

fn one(key: &str, value: &str) -> Dict {
    let mut d = Dict::default();
    d.string(key, value);
    let mut out = Dict::default();
    out.dict("ryucad", d);
    out
}

struct Writer<'a> {
    model: &'a Model,
    opts: &'a Options,
    brep: BTreeMap<(usize, usize), Prim>,
    prototypes: Vec<Prim>,
    proto_names: Siblings,
    /// Each leaf product's prototype's path.
    proto_of: BTreeMap<usize, String>,
}

impl Writer<'_> {
    /// Product `k`'s solids as children of `prim`.
    fn solids(&mut self, prim: &mut Prim, k: usize) -> Result<(), StageError> {
        let p = &self.model.products[k];
        let mut names = Siblings::default();
        for c in &prim.children {
            names.name(&c.name);
        }
        for (s, solid) in p.solids.iter().enumerate() {
            let mut x = Prim::new(Specifier::Def, Some("Xform"), &names.name(&solid.name))
                .map_err(names_error)?;
            x.meta.custom_data = one("reference", &format!("{}/{}", p.part, solid.name));
            if let Some(b) = self.brep.get(&(k, s)) {
                x.children.push(b.clone());
            }
            if self.opts.mesh
                && let Some(m) = &solid.mesh
            {
                x.children.push(
                    mesh_prim("mesh", &solid.body, m, self.opts.unit, !self.opts.brep)
                        .map_err(names_error)?,
                );
            }
            prim.children.push(x);
        }
        Ok(())
    }

    /// Product `k`'s own solids, then an occurrence of each child, under
    /// `prim`.
    fn contents(&mut self, prim: &mut Prim, k: usize) -> Result<(), StageError> {
        self.solids(prim, k)?;
        let p = &self.model.products[k];
        let mut names = Siblings::default();
        for c in &prim.children {
            names.name(&c.name);
        }
        for c in &p.children {
            let child = &self.model.products[c.product];
            let mut x = Prim::new(Specifier::Def, Some("Xform"), &names.name(&c.name))
                .map_err(names_error)?;
            x.varying(
                "xformOp:transform",
                Value::Matrix4d(matrix(&c.place, self.opts.unit)),
            )
            .map_err(names_error)?;
            x.uniform(
                "xformOpOrder",
                Value::TokenArray(vec!["xformOp:transform".into()]),
            )
            .map_err(names_error)?;
            if child.children.is_empty() {
                x.meta.custom_data = one("instance", &c.name);
                x.meta.instanceable = Some(true);
                x.meta.references = vec![self.prototype(c.product)?];
            } else {
                let mut d = product_data(child);
                if let Some((_, super::layer::DictValue::Dict(r))) = d.0.first_mut() {
                    r.0.insert(
                        0,
                        (
                            "instance".into(),
                            super::layer::DictValue::String(c.name.clone()),
                        ),
                    );
                }
                x.meta.custom_data = d;
                x.meta.kind = Some("subassembly".into());
                self.contents(&mut x, c.product)?;
            }
            prim.children.push(x);
        }
        Ok(())
    }

    /// The path of leaf product `k`'s prototype, defined the first time.
    fn prototype(&mut self, k: usize) -> Result<String, StageError> {
        if let Some(path) = self.proto_of.get(&k) {
            return Ok(path.clone());
        }
        let p = &self.model.products[k];
        let base = match &p.label {
            Some(l) => format!("{}_{l}", p.part),
            None => p.part.clone(),
        };
        let name = self.proto_names.name(&base);
        let mut x = Prim::new(Specifier::Class, Some("Xform"), &name).map_err(names_error)?;
        x.meta.custom_data = product_data(p);
        x.meta.kind = Some("component".into());
        self.solids(&mut x, k)?;
        self.prototypes.push(x);
        let path = format!("/Prototypes/{name}");
        self.proto_of.insert(k, path.clone());
        Ok(path)
    }
}

fn names_error(e: BadName) -> StageError {
    names(e)
}

/// A placing as USD's row-vector matrix: each row the image of an axis,
/// the translation (in `unit`) last.
fn matrix(place: &Rigid, unit: Unit) -> [[f64; 4]; 4] {
    let r = place.rotation_matrix();
    let t = place.translation_part();
    let k = unit.per_meter();
    [
        [r[0][0], r[1][0], r[2][0], 0.0],
        [r[0][1], r[1][1], r[2][1], 0.0],
        [r[0][2], r[1][2], r[2][2], 0.0],
        [t.x * k, t.y * k, t.z * k, 1.0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usd::usda::to_string;
    use ryukei::geom::Vector3;
    use ryukei::mesh::{MeshParams, tessellate};
    use ryukei::name::OpKey;

    fn solid(name: &str, x: f64) -> Solid {
        let b = ryukei::primitive::block(OpKey(1), x, x, x, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let m = tessellate(&b, &MeshParams::new(1e-5, 0.26)).unwrap();
        Solid {
            name: name.into(),
            body: Arc::new(b),
            mesh: Some(Arc::new(m)),
        }
    }

    fn opts() -> Options {
        Options {
            unit: Unit::Millimeter,
            brep: true,
            mesh: true,
            pipe_tolerance: 1e-6,
            layer_data: vec![("version".into(), "0.0.0".into())],
        }
    }

    /// A frame placing a pin twice, at the working files and at release
    /// B, and a subassembly placing the pin once more.
    pub(crate) fn frame() -> Model {
        let at = |x: f64| Rigid::translation(Vector3::new(x, 0.0, 0.01));
        let product = |part: &str, label: Option<&str>, solids, children| Product {
            id: part.to_uppercase(),
            name: part.into(),
            part: part.into(),
            label: label.map(str::to_owned),
            solids,
            children,
        };
        let child = |product, x, name: &str| Child {
            product,
            place: at(x),
            name: name.into(),
        };
        Model {
            products: vec![
                product(
                    "frame",
                    None,
                    vec![solid("plate", 0.1)],
                    vec![
                        child(1, 0.0, "a"),
                        child(2, 0.05, "b-1"),
                        child(3, 0.08, "sub"),
                    ],
                ),
                product("pin", None, vec![solid("shank", 0.01)], vec![]),
                product("pin", Some("B"), vec![solid("shank", 0.01)], vec![]),
                product("hold", None, vec![], vec![child(1, 0.0, "holes:1")]),
            ],
        }
    }

    #[test]
    fn an_assembly_is_instanced_prototypes() {
        let l = layer(&frame(), &opts()).unwrap();
        assert_eq!(l.default_prim.as_deref(), Some("frame"));
        assert_eq!(l.meters_per_unit, 0.001);
        let names = |p: &Prim| {
            p.children
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };
        let [protos, root] = l.prims.as_slice() else {
            panic!()
        };
        assert_eq!(protos.specifier, Specifier::Class);
        assert_eq!(names(protos), ["pin", "pin_B"]);
        assert_eq!(names(root), ["plate", "a", "b_1", "sub"]);
        let a = &root.children[1];
        assert_eq!(a.meta.instanceable, Some(true));
        assert_eq!(a.meta.references, ["/Prototypes/pin"]);
        assert_eq!(root.children[2].meta.references, ["/Prototypes/pin_B"]);
        let sub = &root.children[3];
        assert_eq!(sub.meta.kind.as_deref(), Some("subassembly"));
        assert_eq!(names(sub), ["holes_1"]);
        assert_eq!(sub.children[0].meta.references, ["/Prototypes/pin"]);
        assert_eq!(names(&protos.children[0]), ["shank"]);
        assert_eq!(names(&protos.children[0].children[0]), ["brep", "mesh"]);
        let text = to_string(&l).unwrap();
        assert!(text.contains("matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (50, 0, 10, 1) )"));
        assert!(text.contains(&format!("string omni_solid = \"{SCHEMA_COMMIT}\"")));
        assert!(text.contains("string reference = \"pin/shank\""));
        assert!(text.contains("string instance = \"b-1\""));
    }

    #[test]
    fn a_part_alone_has_no_prototypes() {
        let mut m = frame();
        m.products.truncate(1);
        m.products[0].children.clear();
        let l = layer(&m, &opts()).unwrap();
        assert_eq!(l.prims.len(), 1);
        assert_eq!(l.prims[0].meta.kind.as_deref(), Some("component"));
        assert_eq!(l.prims[0].children[0].name, "plate");
    }

    #[test]
    fn breps_or_meshes_or_both() {
        let mut o = opts();
        o.mesh = false;
        let l = layer(&frame(), &o).unwrap();
        let shank = &l.prims[0].children[0].children[0];
        assert_eq!(shank.children.len(), 1);
        assert_eq!(shank.children[0].type_name.as_deref(), Some("BrepArray"));
        o.mesh = true;
        o.brep = false;
        let l = layer(&frame(), &o).unwrap();
        let mesh = &l.prims[0].children[0].children[0].children[0];
        assert_eq!(mesh.type_name.as_deref(), Some("Mesh"));
        assert!(mesh.attributes.iter().any(|a| a.name == "ryucad:faceName"));
        assert!(!to_string(&l).unwrap().contains("omni_solid"));
        o.mesh = false;
        assert_eq!(layer(&frame(), &o).unwrap_err(), StageError::Nothing);
    }

    #[test]
    fn transforms_are_rows() {
        let q = Rigid::translation(Vector3::new(0.001, 0.002, 0.003)).then_after(&Rigid::rotation(
            Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(),
            core::f64::consts::FRAC_PI_2,
        ));
        let m = matrix(&q, Unit::Meter);
        // x goes to y.
        assert!((m[0][1] - 1.0).abs() < 1e-15 && m[0][0].abs() < 1e-15);
        assert_eq!(m[3], [0.001, 0.002, 0.003, 1.0]);
    }
}
