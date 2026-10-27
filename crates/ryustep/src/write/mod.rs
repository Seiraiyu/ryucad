//! Writing bodies: ryukei bodies and a product tree as a STEP file.
//!
//! [`write`] gives each body as a file must describe it
//! ([`ryukei::heal::seamed`]: periodic faces cut open along seams, poles
//! ended, pipes as NURBS within a tolerance), its entities in AP242's or
//! AP214's B-rep forms ([`body`]), and the product tree as products,
//! definitions, shape representations and assembly usages placed by item
//! defined transformations, as OCCT and other writers give them.
//!
//! The file is the same bytes for the same input on every platform:
//! instances are numbered by a fixed walk (the contexts, then each
//! product with its bodies, then each usage), reals are written as their
//! shortest round trip ([`crate::part21::write`]), lengths in the chosen
//! unit by one division, and the header's time stamp is the caller's.

mod body;

use crate::WriteError;
use crate::part21::{self, File, Header, Instance, Param, Part, Record};
use crate::schema::Schema;
use rayon::prelude::*;
use ryukei::geom::{Point3, Rigid, Vector3};
use ryukei::heal::seamed;
use ryukei::name::Name;
use ryukei::topo::Body;

/// A product to write: its identifier, name and description, its
/// revision (the `PRODUCT_DEFINITION_FORMATION`'s identifier; empty for
/// none), its bodies, and where it uses other products.
#[derive(Clone, Debug, Default)]
pub struct Product {
    pub id: String,
    pub name: String,
    pub description: String,
    pub revision: String,
    pub bodies: Vec<Body>,
    pub children: Vec<Child>,
}

/// One use of a product in another, placed by a rigid motion of its
/// frame into its parent's. `id` is the `NEXT_ASSEMBLY_USAGE_OCCURRENCE`'s
/// identifier; empty, it is the use's place among its parent's, from 1.
#[derive(Clone, Debug)]
pub struct Child {
    /// Index into the model's products.
    pub product: usize,
    pub place: Rigid,
    pub id: String,
}

/// What to write.
#[derive(Clone, Debug, Default)]
pub struct WriteModel {
    pub products: Vec<Product>,
}

/// The length unit a file is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthUnit {
    Meter,
    Millimeter,
}

impl LengthUnit {
    /// Its size in meters.
    fn size(self) -> f64 {
        match self {
            LengthUnit::Meter => 1.0,
            LengthUnit::Millimeter => 1e-3,
        }
    }
}

/// How to write.
#[derive(Clone, Debug)]
pub struct WriteOptions {
    /// AP242 or AP214; AP203 is read only.
    pub schema: Schema,
    pub unit: LengthUnit,
    /// How far a pipe's NURBS may stray from it, in meters.
    pub pipe_tolerance: f64,
    /// The header's file name and time stamp, as the caller gives them.
    pub file_name: String,
    pub time_stamp: String,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            schema: Schema::Ap242,
            unit: LengthUnit::Meter,
            pipe_tolerance: 1e-7,
            file_name: String::new(),
            time_stamp: String::new(),
        }
    }
}

/// The instances being written, numbered from 1 as they are added.
#[derive(Default)]
pub(crate) struct Out {
    instances: Vec<Instance>,
}

impl Out {
    fn next(&self) -> u64 {
        self.instances.len() as u64 + 1
    }

    /// Adds a simple record; returns its number.
    pub(crate) fn add(&mut self, name: &str, params: Vec<Param>) -> u64 {
        self.record(Record::Simple(Part {
            name: name.into(),
            params,
        }))
    }

    /// Adds a complex record, its parts in the order given (Part 21 wants
    /// them alphabetical).
    pub(crate) fn complex(&mut self, parts: Vec<(&str, Vec<Param>)>) -> u64 {
        self.record(Record::Complex(
            parts
                .into_iter()
                .map(|(name, params)| Part {
                    name: name.into(),
                    params,
                })
                .collect(),
        ))
    }

    fn record(&mut self, record: Record) -> u64 {
        let id = self.next();
        self.instances.push(Instance { id, record });
        id
    }

    /// A number for a record added later by [`Out::fill`].
    fn reserve(&mut self) -> u64 {
        self.add("", Vec::new())
    }

    fn fill(&mut self, id: u64, name: &str, params: Vec<Param>) {
        self.instances[id as usize - 1].record = Record::Simple(Part {
            name: name.into(),
            params,
        });
    }
}

pub(crate) fn s(x: &str) -> Param {
    Param::Str(x.into())
}

pub(crate) fn r(id: u64) -> Param {
    Param::Ref(id)
}

pub(crate) fn f(x: f64) -> Param {
    Param::Real(x)
}

pub(crate) fn e(x: &str) -> Param {
    Param::Enum(x.into())
}

pub(crate) fn b(x: bool) -> Param {
    e(if x { "T" } else { "F" })
}

pub(crate) fn refs(ids: impl IntoIterator<Item = u64>) -> Param {
    Param::List(ids.into_iter().map(Param::Ref).collect())
}

/// The shared contexts and units.
pub(crate) struct Contexts {
    pub(crate) product: u64,
    pub(crate) definition: u64,
    /// The 3D geometric context with units and uncertainty.
    pub(crate) geometric: u64,
    /// The 2D parameter space pcurves are drawn in.
    pub(crate) parametric: u64,
    /// Meters per file length unit.
    pub(crate) unit: f64,
}

/// Writes `model` as a STEP file. Fails with
/// [`WriteError::NotRepresentable`], naming the face, for a face that
/// can't be cut open into a region of its surface or a pipe that can't
/// be approximated within the tolerance.
pub fn write(model: &WriteModel, opts: &WriteOptions) -> Result<Vec<u8>, WriteError> {
    // Each body as a file must describe it, in parallel; the first body
    // that can't be fails, whatever the number of threads.
    let all: Vec<&Body> = model.products.iter().flat_map(|p| &p.bodies).collect();
    let mut seamed_all = all
        .par_iter()
        .map(|b| seamed(b, opts.pipe_tolerance).map_err(|err| not_representable(b, err)))
        .collect::<Vec<_>>()
        .into_iter();
    let shapes: Vec<Vec<ryukei::heal::Shape>> = model
        .products
        .iter()
        .map(|p| {
            seamed_all
                .by_ref()
                .take(p.bodies.len())
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<_, _>>()?;
    let mut out = Out::default();
    let uncertainty = model
        .products
        .iter()
        .flat_map(|p| &p.bodies)
        .map(largest_tolerance)
        .chain(
            shapes
                .iter()
                .flatten()
                .flat_map(|s| s.faces.iter().map(|f| f.tolerance)),
        )
        .fold(ryukei::tol::LINEAR_RESOLUTION, f64::max);
    let cx = contexts(&mut out, opts, uncertainty);
    // Products: each one's definition and representation, then its
    // bodies; the usages after, once every product has its number.
    let mut defs = Vec::with_capacity(model.products.len());
    for (k, p) in model.products.iter().enumerate() {
        let product = out.add(
            "PRODUCT",
            vec![s(&p.id), s(&p.name), s(&p.description), refs([cx.product])],
        );
        let formation = out.add(
            "PRODUCT_DEFINITION_FORMATION",
            vec![s(&p.revision), s(""), r(product)],
        );
        let definition = out.add(
            "PRODUCT_DEFINITION",
            vec![s("design"), s(""), r(formation), r(cx.definition)],
        );
        let shape = out.add(
            "PRODUCT_DEFINITION_SHAPE",
            vec![s(""), s(""), r(definition)],
        );
        let rep = out.reserve();
        out.add("SHAPE_DEFINITION_REPRESENTATION", vec![r(shape), r(rep)]);
        let origin = placement(
            &mut out,
            &cx,
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        );
        let mut items = vec![origin];
        let mut child_axes = Vec::with_capacity(p.children.len());
        for c in &p.children {
            let o = c.place.apply_point(Point3::ORIGIN);
            let z = c.place.apply_vector(Vector3::new(0.0, 0.0, 1.0));
            let x = c.place.apply_vector(Vector3::new(1.0, 0.0, 0.0));
            let a = placement(&mut out, &cx, o, z, x);
            items.push(a);
            child_axes.push(a);
        }
        // One body's solids go in the product's own representation;
        // several bodies each have a representation of their own,
        // related to it, so each reads back as the body it was.
        let kind = match shapes[k].len() {
            0 => "SHAPE_REPRESENTATION",
            1 => {
                items.extend(body::solid_items(&mut out, &cx, &shapes[k][0]));
                "ADVANCED_BREP_SHAPE_REPRESENTATION"
            }
            _ => {
                for sh in &shapes[k] {
                    let own = placement(
                        &mut out,
                        &cx,
                        Point3::ORIGIN,
                        Vector3::new(0.0, 0.0, 1.0),
                        Vector3::new(1.0, 0.0, 0.0),
                    );
                    let mut body_items = vec![own];
                    body_items.extend(body::solid_items(&mut out, &cx, sh));
                    let body_rep = out.add(
                        "ADVANCED_BREP_SHAPE_REPRESENTATION",
                        vec![s(""), refs(body_items), r(cx.geometric)],
                    );
                    out.add(
                        "SHAPE_REPRESENTATION_RELATIONSHIP",
                        vec![s(""), s(""), r(body_rep), r(rep)],
                    );
                }
                "SHAPE_REPRESENTATION"
            }
        };
        out.fill(rep, kind, vec![s(""), refs(items), r(cx.geometric)]);
        defs.push((definition, rep, origin, child_axes));
    }
    for (k, p) in model.products.iter().enumerate() {
        for (n, c) in p.children.iter().enumerate() {
            let (parent_def, parent_rep, _, ref axes) = defs[k];
            let (child_def, child_rep, child_origin, _) = defs[c.product];
            let id = match c.id.as_str() {
                "" => format!("{}", n + 1),
                id => id.to_owned(),
            };
            let usage = out.add(
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
                vec![
                    s(&id),
                    s(""),
                    s(""),
                    r(parent_def),
                    r(child_def),
                    Param::Unset,
                ],
            );
            let shape = out.add(
                "PRODUCT_DEFINITION_SHAPE",
                vec![s("Placement"), s("Placement of an item"), r(usage)],
            );
            let transform = out.add(
                "ITEM_DEFINED_TRANSFORMATION",
                vec![s(""), s(""), r(child_origin), r(axes[n])],
            );
            let relationship = out.complex(vec![
                (
                    "REPRESENTATION_RELATIONSHIP",
                    vec![s(""), s(""), r(child_rep), r(parent_rep)],
                ),
                (
                    "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION",
                    vec![r(transform)],
                ),
                ("SHAPE_REPRESENTATION_RELATIONSHIP", vec![]),
            ]);
            out.add(
                "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
                vec![r(relationship), r(shape)],
            );
        }
    }
    let file = File {
        header: header(opts),
        instances: out.instances,
    };
    part21::write(&file)
}

/// The refusal for a body `seamed` couldn't write.
fn not_representable(b: &Body, err: ryukei::Error) -> WriteError {
    let face = match err {
        ryukei::Error::InvalidHeal(ryukei::HealError::BadDomain { face }) => face,
        _ => b
            .faces()
            .map(|f| b.face(f).expect("live").name)
            .min()
            .unwrap_or(Name::from_u128(0)),
    };
    WriteError::NotRepresentable { face }
}

/// The largest tolerance of any edge or vertex of a body.
fn largest_tolerance(b: &Body) -> f64 {
    let edges = b.edges().map(|e| b.edge(e).expect("live").tolerance.get());
    let vertices = b
        .vertices()
        .map(|v| b.vertex(v).expect("live").tolerance.get());
    edges.chain(vertices).fold(0.0, f64::max)
}

/// The application, product and geometric contexts, with the units.
fn contexts(out: &mut Out, opts: &WriteOptions, uncertainty: f64) -> Contexts {
    let (description, protocol, year) = match opts.schema {
        Schema::Ap214 => (
            "core data for automotive mechanical design processes",
            "automotive_design",
            2001,
        ),
        _ => (
            "Managed model based 3d engineering",
            "ap242_managed_model_based_3d_engineering",
            2014,
        ),
    };
    let application = out.add("APPLICATION_CONTEXT", vec![s(description)]);
    out.add(
        "APPLICATION_PROTOCOL_DEFINITION",
        vec![
            s("international standard"),
            s(protocol),
            Param::Int(year),
            r(application),
        ],
    );
    let product = out.add(
        "PRODUCT_CONTEXT",
        vec![s(""), r(application), s("mechanical")],
    );
    let definition = out.add(
        "PRODUCT_DEFINITION_CONTEXT",
        vec![s("part definition"), r(application), s("design")],
    );
    let prefix = match opts.unit {
        LengthUnit::Meter => Param::Unset,
        LengthUnit::Millimeter => e("MILLI"),
    };
    let length = out.complex(vec![
        ("LENGTH_UNIT", vec![]),
        ("NAMED_UNIT", vec![Param::Derived]),
        ("SI_UNIT", vec![prefix, e("METRE")]),
    ]);
    let angle = out.complex(vec![
        ("NAMED_UNIT", vec![Param::Derived]),
        ("PLANE_ANGLE_UNIT", vec![]),
        ("SI_UNIT", vec![Param::Unset, e("RADIAN")]),
    ]);
    let solid = out.complex(vec![
        ("NAMED_UNIT", vec![Param::Derived]),
        ("SI_UNIT", vec![Param::Unset, e("STERADIAN")]),
        ("SOLID_ANGLE_UNIT", vec![]),
    ]);
    let unit = opts.unit.size();
    let accuracy = out.add(
        "UNCERTAINTY_MEASURE_WITH_UNIT",
        vec![
            Param::Typed("LENGTH_MEASURE".into(), Box::new(f(uncertainty / unit))),
            r(length),
            s("distance_accuracy_value"),
            s("the largest tolerance of any entity"),
        ],
    );
    let geometric = out.complex(vec![
        ("GEOMETRIC_REPRESENTATION_CONTEXT", vec![Param::Int(3)]),
        (
            "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
            vec![refs([accuracy])],
        ),
        (
            "GLOBAL_UNIT_ASSIGNED_CONTEXT",
            vec![refs([length, angle, solid])],
        ),
        (
            "REPRESENTATION_CONTEXT",
            vec![s("Context #1"), s("3D Context with UNIT and UNCERTAINTY")],
        ),
    ]);
    let parametric = out.complex(vec![
        ("GEOMETRIC_REPRESENTATION_CONTEXT", vec![Param::Int(2)]),
        ("PARAMETRIC_REPRESENTATION_CONTEXT", vec![]),
        ("REPRESENTATION_CONTEXT", vec![s("2D SPACE"), s("")]),
    ]);
    Contexts {
        product,
        definition,
        geometric,
        parametric,
        unit,
    }
}

/// An `AXIS2_PLACEMENT_3D` at `o` with axis `z` and reference `x`.
pub(crate) fn placement(out: &mut Out, cx: &Contexts, o: Point3, z: Vector3, x: Vector3) -> u64 {
    let p = point(out, cx, o);
    let z = direction(out, z);
    let x = direction(out, x);
    out.add("AXIS2_PLACEMENT_3D", vec![s(""), r(p), r(z), r(x)])
}

/// A `CARTESIAN_POINT` in the file's length unit.
pub(crate) fn point(out: &mut Out, cx: &Contexts, p: Point3) -> u64 {
    let u = cx.unit;
    out.add(
        "CARTESIAN_POINT",
        vec![s(""), Param::List(vec![f(p.x / u), f(p.y / u), f(p.z / u)])],
    )
}

/// A `DIRECTION`, as it is.
pub(crate) fn direction(out: &mut Out, d: Vector3) -> u64 {
    out.add(
        "DIRECTION",
        vec![s(""), Param::List(vec![f(d.x), f(d.y), f(d.z)])],
    )
}

/// The header: a description, the file's name and time stamp as the
/// caller gives them, and the schema.
fn header(opts: &WriteOptions) -> Header {
    let schema = match opts.schema {
        Schema::Ap214 => "AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }",
        _ => "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 1 1 4 }",
    };
    let version = format!("ryustep {}", ryukei::VERSION);
    Header {
        entities: vec![
            Part {
                name: "FILE_DESCRIPTION".into(),
                params: vec![Param::List(vec![s("ryukei model")]), s("2;1")],
            },
            Part {
                name: "FILE_NAME".into(),
                params: vec![
                    s(&opts.file_name),
                    s(&opts.time_stamp),
                    Param::List(vec![s("")]),
                    Param::List(vec![s("")]),
                    s(&version),
                    s("ryukei"),
                    s(""),
                ],
            },
            Part {
                name: "FILE_SCHEMA".into(),
                params: vec![Param::List(vec![s(schema)])],
            },
        ],
    }
}
