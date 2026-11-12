//! Reading bodies: a file's solids as ryukei bodies, and its product tree.
//!
//! [`read`] parses and decodes the file (see [`crate::part21`] and
//! [`crate::schema`]), reads its product tree (see [`crate::products`]),
//! and makes a body of every B-rep item a shape reaches: its context's
//! units (lengths to meters by one multiplication, angles to radians),
//! its geometry as ryukei's ([`geometry`]), its topology as a
//! [`ryukei::heal::Shape`] ([`topology`]), healed into a body
//! ([`ryukei::heal::heal`]). The B-rep items of one representation are
//! one body, a solid region each: a part with two lumps stays one body,
//! as ryukei made it. Representations are read and healed in parallel
//! and returned in instance order, so the result is the same on any
//! number of threads; several holding the same items in one context are
//! read once.
//!
//! Surface models (`SHELL_BASED_SURFACE_MODEL`, `FACE_BASED_SURFACE_MODEL`)
//! are read as sheets when the caller asks for them
//! (`opts.heal.open` is [`ryukei::heal::Open::Sheet`]): their shells go
//! to [`ryukei::heal::Shape::sheets`], and healing makes the
//! representation a sheet body (see [`Imported::kind`]). By default they
//! are refused as open shells, as they always were. A representation
//! holding solids and surface models together heals with `Sheet` to
//! [`ryukei::HealError::MixedShape`]; a manifold solid brep that doesn't
//! close is an open shell whatever the option.
//!
//! One bad body never fails the file: each representation's result
//! stands alone. Healing names entities by their import names; each
//! body says which instance every name it read came from, so a refusal
//! can be found in the file.
//! Only a file that isn't Part 21, has no data section, or follows a
//! schema this crate doesn't read fails as a whole.

mod geometry;
mod spline;
mod topology;

use crate::part21::{self, Header};
use crate::products::{Products, Transform};
use crate::schema::{Entities, Entity, Id, Schema};
use crate::{ImportError, StepError};
use rayon::prelude::*;
use ryukei::geom::{Frame, Rigid, UnitVector3};
use ryukei::heal::{
    HealOptions, HealReport, ReferenceOptions, ReferenceReport, heal_with, import_names,
    reference_with,
};
use ryukei::name::{Journal, Name, OpKey};
use ryukei::topo::Body;
use ryukei::work::Work;
use std::collections::BTreeMap;

/// The name a writer gives a `VERTEX_POINT` it added only to end a seam
/// (see [`ryukei::heal::Vertex::seam`]).
pub const SEAM_VERTEX: &str = "ryukei:seam";

/// How to read a file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadOptions {
    /// The caller's key: every import name is derived from it.
    pub key: OpKey,
    /// How to heal; its `open` says whether surface models are read as
    /// sheets ([`ryukei::heal::Open::Sheet`]) or refused, the default.
    pub heal: HealOptions,
}

/// A file read.
#[derive(Clone, Debug)]
pub struct Model {
    pub header: Header,
    pub schema: Schema,
    pub products: Products,
    /// One per representation holding B-rep items, in instance order.
    pub bodies: Vec<Imported>,
}

/// One representation's B-rep items and what they became.
#[derive(Clone, Debug)]
pub struct Imported {
    /// The representation's instance number.
    pub representation: Id,
    /// Its B-rep items' instance numbers, ascending: a solid region each.
    pub items: Vec<Id>,
    /// The product definition whose shape that is, as an index into
    /// `products.definitions`; `None` for a representation no product
    /// reaches.
    pub definition: Option<usize>,
    pub result: Result<(Body, Journal, HealReport), ImportError>,
    /// The instance every import name of its vertices, edges and faces
    /// came from: a `VERTEX_POINT`, an `EDGE_CURVE` or a face (a poly
    /// loop's `CARTESIAN_POINT`s and the `POLY_LOOP` that first gave a
    /// line edge). Empty when the shape couldn't be read; names healing
    /// made (sewn edges, joined vertices) are not in it.
    pub instances: BTreeMap<Name, Id>,
    /// The B-rep item (a solid of `items`) each face's import name came
    /// from, so a refusal naming two faces can say whether they are in one
    /// solid of the file or two. Empty when the shape couldn't be read.
    pub solids: BTreeMap<Name, Id>,
}

/// One representation's B-rep items read as a reference (design 364): a
/// sheet with the file's topology, never healed, for display and reading
/// geometry. Named as [`Imported`]'s body would be under the same key.
#[derive(Clone, Debug)]
pub struct Referenced {
    /// The representation's instance number.
    pub representation: Id,
    /// Its B-rep items' instance numbers, ascending.
    pub items: Vec<Id>,
    /// The product definition whose shape it is, as [`Imported`] says.
    pub definition: Option<usize>,
    pub result: Result<(Body, ReferenceReport), ImportError>,
    /// The instance each import name came from, as [`Imported`] says.
    pub instances: BTreeMap<Name, Id>,
    /// The B-rep item each face's import name came from.
    pub solids: BTreeMap<Name, Id>,
}

/// A representation holding B-rep items: what [`Opened::body`] reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// The representation's instance number.
    pub representation: Id,
    /// Its context, which gives its units.
    pub context: Id,
    /// The product definition whose shape it is, as an index into
    /// `products.definitions`; `None` for a representation no product
    /// reaches.
    pub definition: Option<usize>,
    /// Its B-rep items' instance numbers, ascending.
    pub items: Vec<Id>,
    /// The name of its first solid or surface model item that has one
    /// (empty for none):
    /// ryucad writes each solid's ID there (#412).
    pub name: String,
}

/// A file parsed and decoded, with its product tree and every
/// representation holding B-rep items, each to be read on its own under
/// a key of the caller's: what [`read`] does for every body at once.
pub struct Opened {
    pub header: Header,
    pub schema: Schema,
    pub products: Products,
    /// One per representation holding B-rep items, in instance order.
    pub held: Vec<Held>,
    /// For each of `held`, the first that holds the same items in the
    /// same context: the one whose body it is.
    first: Vec<usize>,
    entities: Entities,
}

/// Parses and decodes the STEP file `bytes` and reads its product tree,
/// reading no bodies yet.
pub fn open(bytes: &[u8]) -> Result<Opened, StepError> {
    let file = part21::parse(bytes)?;
    let schema = Schema::of(&file.header)?;
    let entities = Entities::decode(&file);
    let products = Products::build(&entities);
    // Every representation holding B-rep items, once, with its context,
    // definition and items.
    let mut reps: BTreeMap<Id, Held> = BTreeMap::new();
    let shapes = products
        .definitions
        .iter()
        .enumerate()
        .flat_map(|(k, d)| d.shapes.iter().map(move |s| (s, Some(k))))
        .chain(products.unplaced.iter().map(|s| (s, None)));
    for (s, definition) in shapes {
        if s.breps.is_empty() {
            continue;
        }
        let entry = reps.entry(s.representation).or_insert(Held {
            representation: s.representation,
            context: s.context,
            definition,
            items: Vec::new(),
            name: String::new(),
        });
        entry.items.extend(&s.breps);
    }
    let held: Vec<Held> = reps
        .into_values()
        .map(|mut h| {
            h.items.sort_unstable();
            h.items.dedup();
            h.name = h
                .items
                .iter()
                .find_map(|&i| match entities.get(i, i) {
                    Ok(crate::schema::Entity::Brep(
                        crate::schema::Brep::Solid { name, .. }
                        | crate::schema::Brep::Shells { name, .. },
                    )) if !name.is_empty() => Some(name.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            h
        })
        .collect();
    // Representations holding the same items in the same context (a
    // part's shape and its copies under other representations) are one
    // body, read once, under the first of them.
    let mut firsts: BTreeMap<(Id, &[Id]), usize> = BTreeMap::new();
    let first = held
        .iter()
        .enumerate()
        .map(|(k, h)| *firsts.entry((h.context, h.items.as_slice())).or_insert(k))
        .collect();
    Ok(Opened {
        header: file.header,
        schema,
        products,
        held,
        first,
        entities,
    })
}

impl Imported {
    /// The kind of body read: a solid or a sheet; `None` where it was
    /// refused.
    pub fn kind(&self) -> Option<ryukei::topo::BodyKind> {
        self.result.as_ref().ok().map(|(b, _, _)| b.kind())
    }
}

impl Opened {
    /// The body of `held[k]`, read under `opts`: the same as [`read`]
    /// gives it.
    pub fn body(&self, k: usize, opts: &ReadOptions) -> Imported {
        self.body_with(k, opts, &Work::unlimited())
    }

    /// [`Opened::body`] healing under `work`: a stop is
    /// [`ImportError::Work`].
    pub fn body_with(&self, k: usize, opts: &ReadOptions, work: &Work) -> Imported {
        let (result, instances, solids) = self.read(self.first[k], opts, work);
        self.imported(k, result, instances, solids)
    }

    /// The reference of `held[k]`, read under `opts`' key: the shape as
    /// [`Opened::body`] reads it, built by [`ryukei::heal::reference`]
    /// with `opts.heal.tolerance_cap` as its cap. Faces are named as the
    /// healed body names them. A file whose solids touch or cross isn't
    /// united, and a face split at a revolution's axis isn't repaired:
    /// both are what a reference leaves as the file has it.
    pub fn reference(&self, k: usize, opts: &ReadOptions) -> Referenced {
        self.reference_with(k, opts, &Work::unlimited())
    }

    /// [`Opened::reference`] under `work`: a stop is [`ImportError::Work`].
    pub fn reference_with(&self, k: usize, opts: &ReadOptions, work: &Work) -> Referenced {
        let first = &self.held[self.first[k]];
        let h = &self.held[k];
        let (result, instances, solids) = match shape_of(
            &self.entities,
            &first.items,
            first.representation,
            first.context,
            opts.key,
        ) {
            Ok((_, shape, read)) => {
                let options = ReferenceOptions {
                    tolerance_cap: opts.heal.tolerance_cap,
                };
                let result = reference_with(opts.key, &shape, &options, work)
                    .map_err(|e| failed(e, first.representation));
                (result, read.instances, read.solids)
            }
            Err(e) => (Err(e), BTreeMap::new(), BTreeMap::new()),
        };
        Referenced {
            representation: h.representation,
            items: h.items.clone(),
            definition: h.definition,
            result,
            instances,
            solids,
        }
    }

    /// Where `products.definitions[parent].children[k]` puts its child:
    /// the rigid motion taking the child's coordinates into its parent's,
    /// in meters, each placement read in its own representation's units.
    /// The identity where the file says nothing. An operator's `axis2` is
    /// ignored (its frame is right-handed from `axis3` and `axis1`), and
    /// a scale other than 1 is `Malformed`: an assembly places its parts
    /// rigidly.
    pub fn placement(&self, parent: usize, k: usize) -> Result<Rigid, ImportError> {
        let o = &self.products.definitions[parent].children[k];
        let (Some(t), Some([child, onto])) = (o.transform, o.representations) else {
            return Ok(Rigid::IDENTITY);
        };
        let geo = |rep: Id| -> Result<geometry::Geo<'_>, ImportError> {
            let context = match self.entities.get(o.instance, rep)? {
                Entity::Representation { context, .. } => *context,
                _ => return Err(crate::schema::wrong(rep, "a representation")),
            };
            let units = self.entities.units(rep, context)?;
            Ok(geometry::Geo {
                e: &self.entities,
                length: units.length,
                angle: units.angle.unwrap_or(1.0),
            })
        };
        match t {
            Transform::Items {
                child: c,
                parent: p,
            } => {
                let from = Rigid::from_frame(&geo(child)?.frame(c)?);
                let to = Rigid::from_frame(&geo(onto)?.frame(p)?);
                Ok(to.then_after(&from.inverse()))
            }
            Transform::Operator { operator, inverse } => {
                let Entity::Operator {
                    axis1,
                    origin,
                    scale,
                    axis3,
                    ..
                } = *self.entities.get(o.instance, operator)?
                else {
                    return Err(crate::schema::wrong(operator, "a transformation operator"));
                };
                if scale.is_some_and(|s| s != 1.0) {
                    return Err(ImportError::Malformed {
                        instance: operator,
                        attribute: 4,
                        what: "a scale other than 1: an assembly places its parts rigidly",
                    });
                }
                // The operator's origin is in the units of the space it
                // maps into.
                let g = geo(if inverse { child } else { onto })?;
                let z = match axis3 {
                    Some(a) => g.unit(a)?,
                    None => UnitVector3::Z,
                };
                let x = match axis1 {
                    Some(a) => g.unit(a)?,
                    None if z.get().y == 0.0 && z.get().z == 0.0 => UnitVector3::Y,
                    None => UnitVector3::X,
                };
                let at = g.point(origin)?;
                let frame = Frame::from_axes(at, z, x)
                    .or_else(|_| Frame::new(at, z.get(), x.get()))
                    .map_err(|e| geometry::invalid(operator, e))?;
                let r = Rigid::from_frame(&frame);
                Ok(if inverse { r.inverse() } else { r })
            }
        }
    }

    /// What reading `held[k]` gives, before it is labelled.
    fn read(&self, k: usize, opts: &ReadOptions, work: &Work) -> Read {
        let h = &self.held[k];
        body(
            &self.entities,
            &h.items,
            h.representation,
            h.context,
            opts,
            work,
        )
    }

    fn imported(
        &self,
        k: usize,
        result: Result<(Body, Journal, HealReport), ImportError>,
        instances: BTreeMap<Name, Id>,
        solids: BTreeMap<Name, Id>,
    ) -> Imported {
        let h = &self.held[k];
        Imported {
            representation: h.representation,
            items: h.items.clone(),
            definition: h.definition,
            result,
            instances,
            solids,
        }
    }
}

/// Reads every body and the product tree of the STEP file `bytes`.
pub fn read(bytes: &[u8], opts: &ReadOptions) -> Result<Model, StepError> {
    let opened = open(bytes)?;
    let mut firsts: Vec<usize> = opened.first.clone();
    firsts.sort_unstable();
    firsts.dedup();
    let read: BTreeMap<usize, Read> = firsts
        .par_iter()
        .map(|&k| (k, opened.read(k, opts, &Work::unlimited())))
        .collect();
    let bodies = (0..opened.held.len())
        .map(|k| {
            let (result, instances, solids) = read[&opened.first[k]].clone();
            opened.imported(k, result, instances, solids)
        })
        .collect();
    Ok(Model {
        header: opened.header,
        schema: opened.schema,
        products: opened.products,
        bodies,
    })
}

/// A body read, the instance each import name came from, and the B-rep
/// item each face's came from.
type Read = (
    Result<(Body, Journal, HealReport), ImportError>,
    BTreeMap<Name, Id>,
    BTreeMap<Name, Id>,
);

/// One representation's body, and the instance each import name came
/// from.
fn body(
    e: &Entities,
    items: &[Id],
    representation: Id,
    context: Id,
    opts: &ReadOptions,
    work: &Work,
) -> Read {
    let (g, shape, read) = match shape_of(e, items, representation, context, opts.key) {
        Ok(x) => x,
        Err(err) => return (Err(err), BTreeMap::new(), BTreeMap::new()),
    };
    let ShapeRead {
        instances,
        solids,
        split,
    } = read;
    let result =
        heal_with(opts.key, &shape, &opts.heal, work).map_err(|err| failed(err, representation));
    // A face split at a revolution's axis (#316) leaves the halves of a
    // sweep that folds through it crossing the faces beside them (a lead
    // bent tighter than its radius): the body is united with itself, its
    // winding number's region, as healing's repair unites one. A union
    // refused keeps the healed body, which the self-intersection check
    // then reports; a stop is the import's.
    let result = match result {
        Ok((body, mut journal, mut report)) if split => {
            let key = OpKey::from_bytes(
                &[b"ryustep.split\0".as_slice(), &opts.key.0.to_le_bytes()].concat(),
            );
            match ryukei::repair::self_union_with(key, &body, work) {
                Ok((united, j)) => {
                    let faces: std::collections::BTreeSet<Name> = body
                        .faces()
                        .filter_map(|f| body.face(f).map(|x| x.name))
                        .collect();
                    for e in j.events() {
                        if let ryukei::name::Event::Split { parent, .. }
                        | ryukei::name::Event::Deleted(parent)
                        | ryukei::name::Event::Modified(parent) = e
                            && faces.contains(parent)
                            && !report.repaired.contains(parent)
                        {
                            report.repaired.push(*parent);
                        }
                    }
                    report.repaired.sort();
                    journal.append(j);
                    Ok((united, journal, report))
                }
                Err(e @ (ryukei::Error::Cancelled | ryukei::Error::BudgetExceeded { .. })) => {
                    Err(failed(e, representation))
                }
                Err(_) => Ok((body, journal, report)),
            }
        }
        r => r,
    };
    // Solids of one representation that touch (a part's body and its pins,
    // sharing faces) overlap when healed as one body, ones that cross (a
    // pin pushed into the body) have shells that cross, and ones that
    // nearly touch can leave which encloses which uncertain: each is healed
    // alone and they are united (issues #315, #208, #317 and #319). Only
    // what is refused now takes this way, so nothing that heals changes.
    let apart = match &result {
        Err(ImportError::Heal(
            ryukei::HealError::FacesOverlap { a, b } | ryukei::HealError::ShellsCross { a, b },
        )) => matches!((solids.get(a), solids.get(b)), (Some(x), Some(y)) if x != y),
        Err(ImportError::Heal(ryukei::HealError::Uncertain { .. })) => true,
        _ => false,
    };
    if apart
        && items.len() > 1
        && let Some(united) = united(&g, items, opts, work)
    {
        return united;
    }
    (result, instances, solids)
}

/// What reading a representation's shape says beyond the shape.
struct ShapeRead {
    /// The instance each import name came from.
    instances: BTreeMap<Name, Id>,
    /// The B-rep item each face's import name came from.
    solids: BTreeMap<Name, Id>,
    /// Whether a face was split at a revolution's axis (#316).
    split: bool,
}

/// A representation's shape as read, in meters and radians, with where
/// its import names under `key` came from.
fn shape_of<'e>(
    e: &'e Entities,
    items: &[Id],
    representation: Id,
    context: Id,
    key: OpKey,
) -> Result<(geometry::Geo<'e>, ryukei::heal::Shape, ShapeRead), ImportError> {
    let units = e.units(representation, context)?;
    let g = geometry::Geo {
        e,
        length: units.length,
        angle: units.angle.unwrap_or(1.0),
    };
    let (shape, sources) = topology::shape(&g, items)?;
    let names = import_names(key, &shape);
    let instances = [
        (&names.vertices, &sources.vertices),
        (&names.edges, &sources.edges),
        (&names.faces, &sources.faces),
    ]
    .into_iter()
    .flat_map(|(n, s)| n.iter().copied().zip(s.iter().copied()))
    .collect();
    let solids = names
        .faces
        .iter()
        .copied()
        .zip(sources.face_items.iter().copied())
        .collect();
    Ok((
        g,
        shape,
        ShapeRead {
            instances,
            solids,
            split: sources.split,
        },
    ))
}

/// A heal or kernel refusal as an import error of `representation`.
fn failed(err: ryukei::Error, representation: Id) -> ImportError {
    match err {
        ryukei::Error::InvalidHeal(h) => ImportError::Heal(h),
        e @ (ryukei::Error::Cancelled | ryukei::Error::BudgetExceeded { .. }) => {
            ImportError::Work(e)
        }
        ryukei::Error::OutsideSizeBox => ImportError::OutsideSizeBox {
            instance: representation,
        },
        error => ImportError::InvalidGeometry {
            instance: representation,
            error,
        },
    }
}

/// The B-rep items each healed alone, under a key derived from the
/// caller's and the item's instance number, and united into one body:
/// the journal is each heal's then the union's, the report every heal's,
/// and an import name the union renamed (a tool's entities, kept whole)
/// still finds its instance. `None` where any step fails: the caller keeps
/// its refusal.
fn united(g: &geometry::Geo<'_>, items: &[Id], opts: &ReadOptions, work: &Work) -> Option<Read> {
    use ryukei::boolean::{BooleanOp, Tool, boolean_with};
    use ryukei::name::{Event, ToolKey};
    let key = |what: &[u8], id: Id| {
        OpKey::from_bytes(&[what, &opts.key.0.to_le_bytes(), &id.to_le_bytes()].concat())
    };
    let mut bodies = Vec::new();
    let (mut instances, mut solids) = (BTreeMap::new(), BTreeMap::new());
    let mut journal = Journal::new();
    let mut report = HealReport::default();
    for &item in items {
        let k = key(b"ryustep.item\0", item);
        let (shape, sources) = topology::shape(g, &[item]).ok()?;
        let names = import_names(k, &shape);
        for (n, s) in [
            (&names.vertices, &sources.vertices),
            (&names.edges, &sources.edges),
            (&names.faces, &sources.faces),
        ] {
            instances.extend(n.iter().copied().zip(s.iter().copied()));
        }
        solids.extend(
            names
                .faces
                .iter()
                .copied()
                .zip(sources.face_items.iter().copied()),
        );
        let (body, j, r) = heal_with(k, &shape, &opts.heal, work).ok()?;
        journal.append(j);
        merge(&mut report, r);
        bodies.push(body);
    }
    let (first, rest) = bodies.split_first()?;
    let tools: Vec<Tool> = rest
        .iter()
        .zip(&items[1..])
        .map(|(body, &item)| Tool {
            key: ToolKey(u128::from(item)),
            body,
            place: Rigid::IDENTITY,
        })
        .collect();
    let (body, j) = boolean_with(
        key(b"ryustep.unite\0", items[0]),
        BooleanOp::Unite,
        first,
        &tools,
        work,
    )
    .ok()?;
    for e in j.events() {
        if let Event::Placed { from, to, .. } = e {
            if let Some(&i) = instances.get(from) {
                instances.insert(*to, i);
            }
            if let Some(&i) = solids.get(from) {
                solids.insert(*to, i);
            }
        }
    }
    journal.append(j);
    Some((Ok((body, journal, report)), instances, solids))
}

/// `b`'s report added to `a`'s.
fn merge(a: &mut HealReport, b: HealReport) {
    a.sewn.extend(b.sewn);
    a.merged.extend(b.merged);
    a.collapsed.extend(b.collapsed);
    a.split.extend(b.split);
    a.seams.extend(b.seams);
    a.reversed.extend(b.reversed);
    a.turned.extend(b.turned);
    a.tightened.extend(b.tightened);
    a.poles.extend(b.poles);
    a.joined.extend(b.joined);
    a.tolerances.extend(b.tolerances);
    a.approximated.extend(b.approximated);
    a.repaired.extend(b.repaired);
}
