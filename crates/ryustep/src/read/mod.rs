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
//! number of threads.
//!
//! One bad body never fails the file: each representation's result
//! stands alone.
//! Only a file that isn't Part 21, has no data section, or follows a
//! schema this crate doesn't read fails as a whole.

mod geometry;
mod spline;
mod topology;

use crate::part21::{self, Header};
use crate::products::Products;
use crate::schema::{Entities, Id, Schema};
use crate::{ImportError, StepError};
use rayon::prelude::*;
use ryukei::heal::{HealOptions, HealReport, heal};
use ryukei::name::{Journal, OpKey};
use ryukei::topo::Body;
use std::collections::BTreeMap;

/// The name a writer gives a `VERTEX_POINT` it added only to end a seam
/// (see [`ryukei::heal::Vertex::seam`]).
pub const SEAM_VERTEX: &str = "ryukei:seam";

/// How to read a file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadOptions {
    /// The caller's key: every import name is derived from it.
    pub key: OpKey,
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
}

/// A representation's context, the product definition whose shape it
/// is, and its B-rep items.
type Holder = (Id, Option<usize>, Vec<Id>);

/// Reads every body and the product tree of the STEP file `bytes`.
pub fn read(bytes: &[u8], opts: &ReadOptions) -> Result<Model, StepError> {
    let file = part21::parse(bytes)?;
    let schema = Schema::of(&file.header)?;
    let e = Entities::decode(&file);
    let products = Products::build(&e);
    // Every representation holding B-rep items, once, with its context,
    // definition and items.
    let mut reps: BTreeMap<Id, Holder> = BTreeMap::new();
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
        let entry = reps
            .entry(s.representation)
            .or_insert((s.context, definition, Vec::new()));
        entry.2.extend(&s.breps);
    }
    let list: Vec<(Id, Holder)> = reps
        .into_iter()
        .map(|(r, (c, d, mut items))| {
            items.sort_unstable();
            items.dedup();
            (r, (c, d, items))
        })
        .collect();
    let bodies = list
        .par_iter()
        .map(|(representation, (context, definition, items))| Imported {
            representation: *representation,
            items: items.clone(),
            definition: *definition,
            result: body(&e, items, *representation, *context, opts),
        })
        .collect();
    Ok(Model {
        header: file.header,
        schema,
        products,
        bodies,
    })
}

/// One representation's body.
fn body(
    e: &Entities,
    items: &[Id],
    representation: Id,
    context: Id,
    opts: &ReadOptions,
) -> Result<(Body, Journal, HealReport), ImportError> {
    let units = e.units(representation, context)?;
    let g = geometry::Geo {
        e,
        length: units.length,
        angle: units.angle.unwrap_or(1.0),
    };
    let shape = topology::shape(&g, items)?;
    heal(opts.key, &shape, &opts.heal).map_err(|err| match err {
        ryukei::Error::InvalidHeal(h) => ImportError::Heal(h),
        ryukei::Error::OutsideSizeBox => ImportError::OutsideSizeBox {
            instance: representation,
        },
        error => ImportError::InvalidGeometry {
            instance: representation,
            error,
        },
    })
}
