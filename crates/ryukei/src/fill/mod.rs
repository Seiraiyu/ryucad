//! Fills (design 295, plan 12.G): a sheet spanning a closed loop of
//! boundary pieces.
//!
//! Each side becomes its exact NURBS form, turned to run round the loop,
//! and the sides' ends are joined at each corner's midpoint
//! ([`sides::read`]). A boundary in one plane is one plane face bounded
//! by its sides ([`planar`]). Otherwise four sides are one Coons patch on the sides' own
//! knots ([`coons`]). Three, or five or more, are split at a centre into one
//! four-sided patch per corner, bounded by two half sides and two inner
//! cubics from the sides' middles to the centre ([`split`]). A tangent
//! side's cross-boundary derivative is moved into its face's tangent
//! plane, and every inner edge's into a plane field both its patches
//! share, each angle certified within [`FillOptions::angle`]
//! ([`tangent`]).

mod build;
mod coons;
mod planar;
mod sides;
mod split;
mod tangent;
#[cfg(test)]
mod tests;

use crate::error::{Error, FillError};
use crate::geom::{Curve, NurbsCurve, Point3};
use crate::name::{FillRole, Journal, Name, OpKey};
use crate::tol::{LINEAR_RESOLUTION, TOLERANCE_CAP};
use crate::topo::Body;
use crate::work::{Meter, Work};
use build::{EdgeSpec, Namer, Piece};
use coons::{Quad, U0, U1, V0, V1};
use tangent::{PatchError, Target};

/// One side of a fill's boundary.
#[derive(Clone, Debug)]
pub enum FillSide<'a> {
    /// An edge of a body, named. With `tangent`, the fill meets the
    /// edge's face tangentially: the edge must be laminar (one face).
    Edge {
        body: &'a Body,
        edge: Name,
        tangent: bool,
    },
    /// A curve over an increasing range of its parameter.
    Curve { curve: Curve, range: [f64; 2] },
}

/// The loop a fill spans: its sides in order round it, each meeting the
/// next within the tolerance. Either end of a side may come first.
#[derive(Clone, Debug)]
pub struct FillBoundary<'a> {
    pub sides: Vec<FillSide<'a>>,
}

/// How closely a fill must meet its boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillOptions {
    /// The farthest a side's start may be from the end before it, in
    /// metres: from `LINEAR_RESOLUTION` to `TOLERANCE_CAP`.
    pub tolerance: f64,
    /// The largest angle, in radians, between the fill and a tangent
    /// side's face, or between its patches across an inner edge: in
    /// `(0, π/2)`.
    pub angle: f64,
}

/// What a fill measured.
#[derive(Clone, Debug, PartialEq)]
pub struct FillReport {
    /// How far each side's laminar edges are from the side given, at
    /// most: joining its ends to the corners moved it.
    pub deviations: Vec<f64>,
    /// Each tangent side's certified angle to its face, in radians.
    pub angles: Vec<Option<f64>>,
    /// The certified angle between patches across the inner edges, for a
    /// fill of several patches.
    pub inner: Option<f64>,
}

/// A sheet spanning `boundary`, its front the side the loop runs
/// counterclockwise about, with the journal (everything `Created`) and
/// what it measured. Refusals are `Error::InvalidFill`, made before
/// anything is built.
///
/// Names come from `key`, each side's place in the loop and its edge's
/// name ([`FillRole`]), so moving the boundary renames nothing.
pub fn fill(
    key: OpKey,
    boundary: &FillBoundary<'_>,
    options: &FillOptions,
) -> Result<(Body, Journal, FillReport), Error> {
    fill_with(key, boundary, options, &Work::unlimited())
}

/// [`fill`] under `work`: `Cancelled` or `BudgetExceeded` once it is
/// spent, building nothing.
pub fn fill_with(
    key: OpKey,
    boundary: &FillBoundary<'_>,
    options: &FillOptions,
    work: &Work,
) -> Result<(Body, Journal, FillReport), Error> {
    crate::work::run(work, |meter| fill_in(key, boundary, options, meter))
}

fn fill_in(
    key: OpKey,
    boundary: &FillBoundary<'_>,
    options: &FillOptions,
    meter: &Meter,
) -> Result<(Body, Journal, FillReport), Error> {
    let tol_ok = (LINEAR_RESOLUTION..=TOLERANCE_CAP).contains(&options.tolerance);
    let angle_ok = options.angle > 0.0 && options.angle < core::f64::consts::FRAC_PI_2;
    if !(tol_ok && angle_ok) {
        return Err(FillError::BadOptions.into());
    }
    let sides = sides::read(&boundary.sides, options)?;
    let mut namer = Namer::new(key);
    let stand_ins: Vec<Name> = sides
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut parents = vec![Name::from_u128(i as u128)];
            parents.extend(s.name);
            namer.name(FillRole::Side, &parents)
        })
        .collect();
    if let Some(plane) = planar::plane_of(&sides) {
        let halt = |_| Error::from(FillError::Uncertain { side: None });
        if let Some(angles) = planar::angles(&sides, &plane, options.angle, meter).map_err(halt)? {
            let (body, journal) = planar::sheet(&mut namer, &sides, &stand_ins, plane)?;
            let report = FillReport {
                deviations: sides.iter().map(|s| s.deviation).collect(),
                angles,
                inner: None,
            };
            return Ok((body, journal, report));
        }
    }
    let (pieces, edges, vertices, angles, inner) = if sides.len() == 4 {
        single(&sides, &stand_ins, &mut namer, options, meter)?
    } else {
        split::split(&sides, &stand_ins, &mut namer, options, meter)?
    };
    let (body, journal) = build::sheet(&namer, &pieces, &edges, &vertices)?;
    let report = FillReport {
        deviations: sides.iter().map(|s| s.deviation).collect(),
        angles,
        inner,
    };
    Ok((body, journal, report))
}

/// What a fill builds: its pieces, edges, vertices, each side's angle and
/// the inner edges'.
type Plan = (
    Vec<Piece>,
    Vec<EdgeSpec>,
    Vec<(Point3, Name)>,
    Vec<Option<f64>>,
    Option<f64>,
);

/// A patch's refusal as the fill's: `side` the fill side each tensor
/// place belongs to (`None` for an inner edge).
fn refused(e: PatchError, side: impl Fn(usize) -> Option<usize>) -> Error {
    match e {
        PatchError::Error(e) => e,
        PatchError::Halt => FillError::Uncertain { side: None }.into(),
        PatchError::Failed(f) if f.sampled_within => FillError::Uncertain {
            side: side(f.place),
        }
        .into(),
        PatchError::Failed(f) => FillError::AngleTooLarge {
            side: side(f.place),
            angle: f.angle,
        }
        .into(),
    }
}

/// `c` reversed, and `a + b` of its domain, the flip of its parameter.
fn reversed(c: &NurbsCurve) -> (NurbsCurve, f64) {
    let [a, b] = c.domain();
    (c.reversed(), a + b)
}

/// One patch: four sides in loop order, `v0` and `u1` as they run, `v1`
/// and `u0` reversed.
fn single(
    sides: &[sides::Side<'_>],
    stand_ins: &[Name],
    namer: &mut Namer,
    options: &FillOptions,
    meter: &Meter,
) -> Result<Plan, Error> {
    let corner = |k: usize| {
        let c = &sides[k].curve;
        c.point(c.domain()[0])
    };
    let (v1, f2) = reversed(&sides[2].curve);
    let (u0, f3) = reversed(&sides[3].curve);
    let tangent = sides.iter().any(|s| s.trace.is_some());
    let quad = Quad::new(
        [sides[0].curve.clone(), sides[1].curve.clone(), v1, u0],
        if tangent { 3 } else { 1 },
    )?;
    let flips = [None, None, Some(f2), Some(f3)];
    // The tensor places are the sides' own order: v0, u1, v1, u0.
    let targets: [Option<Target<'_, '_>>; 4] = core::array::from_fn(|place| {
        let trace = sides[place].trace.as_ref()?;
        Some(Target::Face {
            trace,
            map: quad.maps[place].clone(),
            flip: flips[place],
        })
    });
    let (surface, _, angles) =
        tangent::patch(quad, &targets, options.angle, meter).map_err(|e| refused(e, Some))?;
    let mut vertices = Vec::with_capacity(4);
    for k in 0..4 {
        let name = namer.name(FillRole::Corner, &[stand_ins[(k + 3) % 4], stand_ins[k]]);
        vertices.push((corner(k), name));
    }
    // Each side's edge runs with its patch parameter: (place, start, end).
    let mut edges = Vec::with_capacity(4);
    let mut piece_edges = [None; 4];
    for (place, start, end) in [(V0, 0, 1), (U1, 1, 2), (V1, 3, 2), (U0, 0, 3)] {
        piece_edges[place] = Some(edges.len());
        edges.push(EdgeSpec {
            piece: 0,
            place,
            start,
            end,
            name: stand_ins[place],
        });
    }
    let pieces = vec![Piece {
        surface,
        edges: piece_edges,
        name: namer.name(FillRole::Patch, &[]),
    }];
    Ok((pieces, edges, vertices, angles.to_vec(), None))
}

