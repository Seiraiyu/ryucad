//! A fill's boundary read into curves: each side's exact NURBS form,
//! turned to run round the loop, its corners joined, and where a tangent
//! side's face normal is read from.

use super::{FillOptions, FillSide};
use crate::blend::convex;
use crate::error::{Error, FillError};
use crate::geom::nurbs::Reparam;
use crate::geom::{Curve, NurbsCurve, Point3, Vector3};
use crate::name::Name;
use crate::math;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::EdgeId;

/// The least angle, in radians, the sides at a corner may turn by, and
/// the most short of a half turn: a corner turning less is a tangent
/// joint, more a cusp, and the patch's normal vanishes at either.
pub(crate) const MIN_TURN: f64 = 1e-2;

/// One side of the loop, running in the loop's order.
pub(crate) struct Side<'a> {
    /// The exact NURBS form, clamped, its ends on the loop's corners.
    pub(crate) curve: NurbsCurve,
    /// The boundary edge's name, for a side that is an edge.
    pub(crate) name: Option<Name>,
    /// How far joining the corners moved the side, at most.
    pub(crate) deviation: f64,
    /// For a tangent side: the face it meets.
    pub(crate) trace: Option<Trace<'a>>,
}

/// Where a tangent side reads its face's normal: the edge's one fin, and
/// how the side's own parameter maps onto the edge's.
pub(crate) struct Trace<'a> {
    pub(crate) face: convex::Side<'a>,
    /// The NURBS form's parameter for the edge's analytic one.
    pub(crate) reparam: Reparam,
    /// `a + b` of the NURBS form's domain where the side runs against the
    /// edge, so the edge's NURBS parameter is `flip - t`.
    pub(crate) flip: Option<f64>,
}

impl Trace<'_> {
    /// The edge's parameter at the side's NURBS parameter `t`.
    pub(crate) fn edge_param(&self, t: f64) -> f64 {
        let s = self.flip.map_or(t, |f| f - t);
        self.reparam.from_nurbs(s)
    }

    /// The face's unit normal (its outward one) at the side's parameter
    /// `t`; zero where the surface has none.
    pub(crate) fn normal(&self, t: f64) -> Vector3 {
        self.face.normal(self.edge_param(t))
    }
}

/// A side before it is turned: its NURBS form over the edge's interval,
/// and its tangent face, if asked for.
struct Read<'a> {
    curve: NurbsCurve,
    name: Option<Name>,
    face: Option<(convex::Side<'a>, Reparam)>,
}

/// The sides read, turned to run round the loop from side 0, each start
/// joined to the end before it at their midpoint (`NotClosed` where
/// they are farther apart than the tolerance), and checked
/// (`Degenerate`).
pub(crate) fn read<'a>(sides: &[FillSide<'a>], options: &FillOptions) -> Result<Vec<Side<'a>>, Error> {
    if sides.len() < 3 {
        return Err(FillError::TooFewSides.into());
    }
    let mut read: Vec<Read<'a>> = Vec::with_capacity(sides.len());
    for (i, s) in sides.iter().enumerate() {
        read.push(read_one(i, s)?);
    }
    // Side 0 runs whichever way ends nearer either end of side 1; each
    // later side starts at the end nearer where the one before ended.
    let ends = |c: &NurbsCurve| {
        let [a, b] = c.domain();
        (c.point(a), c.point(b))
    };
    let (a0, b0) = ends(&read[0].curve);
    let (a1, b1) = ends(&read[1].curve);
    let forward0 = b0.distance(a1).min(b0.distance(b1)) <= a0.distance(a1).min(a0.distance(b1));
    let mut turned: Vec<(NurbsCurve, bool)> = Vec::with_capacity(read.len());
    let mut at = if forward0 { b0 } else { a0 };
    turned.push(turn(&read[0].curve, !forward0));
    for r in &read[1..] {
        let (a, b) = ends(&r.curve);
        let back = b.distance(at) < a.distance(at);
        at = if back { a } else { b };
        turned.push(turn(&r.curve, back));
    }
    // Gaps and corners.
    let n = turned.len();
    let mut corners = Vec::with_capacity(n);
    for k in 0..n {
        let prev = &turned[(k + n - 1) % n].0;
        let this = &turned[k].0;
        let (end, start) = (ends(prev).1, ends(this).0);
        let gap = end.distance(start);
        if gap > options.tolerance {
            return Err(FillError::NotClosed { side: k, gap }.into());
        }
        corners.push(Point3::ORIGIN + (end.to_vector() + start.to_vector()) * 0.5);
    }
    let mut out = Vec::with_capacity(n);
    for (k, (r, (c, back))) in read.into_iter().zip(turned).enumerate() {
        let (curve, deviation) = joined(&c, corners[k], corners[(k + 1) % n])?;
        if length_bound(&curve) < 10.0 * LINEAR_RESOLUTION {
            return Err(FillError::Degenerate { side: Some(k) }.into());
        }
        let trace = r.face.map(|(face, reparam)| {
            let [a, b] = r.curve.domain();
            Trace {
                face,
                reparam,
                flip: back.then_some(a + b),
            }
        });
        out.push(Side {
            curve,
            name: r.name,
            deviation,
            trace,
        });
    }
    // Corners must turn, and not back.
    for k in 0..n {
        let prev = &out[(k + n - 1) % n].curve;
        let this = &out[k].curve;
        let t_in = prev.tangent(prev.domain()[1]);
        let t_out = this.tangent(this.domain()[0]);
        let (Some(t_in), Some(t_out)) = (t_in, t_out) else {
            return Err(FillError::Degenerate { side: Some(k) }.into());
        };
        let (a, b) = (t_in.get(), t_out.get());
        let turn = math::atan2(a.cross(b).length(), a.dot(b));
        if !(MIN_TURN..=core::f64::consts::PI - MIN_TURN).contains(&turn) {
            return Err(FillError::Degenerate { side: Some(k) }.into());
        }
    }
    Ok(out)
}

/// One side's exact NURBS form over its range, and its face where it is
/// tangent.
fn read_one<'a>(i: usize, side: &FillSide<'a>) -> Result<Read<'a>, Error> {
    let degenerate = |_| Error::from(FillError::Degenerate { side: Some(i) });
    match side {
        FillSide::Curve { curve, range } => {
            let curve = nurbs_over(curve, *range).map_err(degenerate)?;
            Ok(Read {
                curve: curve.0,
                name: None,
                face: None,
            })
        }
        FillSide::Edge {
            body,
            edge,
            tangent,
        } => {
            let e: EdgeId = body
                .edges()
                .find(|&e| body.edges[e].name == *edge)
                .ok_or(FillError::NoSuchEdge { side: i })?;
            let data = &body.edges[e];
            let (curve, reparam) =
                nurbs_over(&body.curves[data.curve], data.interval).map_err(degenerate)?;
            let face = if *tangent {
                let mut fins = body.edge_fins(e);
                let fin = fins.next().ok_or(FillError::NoSuchEdge { side: i })?;
                if fins.next().is_some() {
                    return Err(FillError::NotLaminar { side: i }.into());
                }
                if body.fins[fin].pcurve.is_none() {
                    return Err(FillError::Degenerate { side: Some(i) }.into());
                }
                Some((convex::Side::of(body, fin), reparam))
            } else {
                None
            };
            Ok(Read {
                curve,
                name: Some(*edge),
                face,
            })
        }
    }
}

/// `curve` over `range` as an exact clamped NURBS, and how its parameter
/// relates to the curve's.
fn nurbs_over(curve: &Curve, [t0, t1]: [f64; 2]) -> Result<(NurbsCurve, Reparam), Error> {
    if !(t0.is_finite() && t1.is_finite() && t0 < t1) {
        return Err(Error::Degenerate);
    }
    let (n, reparam) = curve.to_nurbs([t0, t1])?;
    let range = [reparam.to_nurbs(t0), reparam.to_nurbs(t1)];
    let part = if n.domain() == range && n.period().is_none() {
        n
    } else {
        n.part(range)?
    };
    Ok((part, reparam))
}

/// The curve run the other way where `back`, and whether it was.
fn turn(c: &NurbsCurve, back: bool) -> (NurbsCurve, bool) {
    if back {
        (c.reversed(), true)
    } else {
        (c.clone(), false)
    }
}

/// The curve with its first control point moved onto `start` and its last
/// onto `end`, and how far that moved it at most: an end control point's
/// basis function times its weight over the curve's weight is at most 1,
/// and the two ends' together are too, so no point moves farther than the
/// larger of the two moves.
fn joined(c: &NurbsCurve, start: Point3, end: Point3) -> Result<(NurbsCurve, f64), Error> {
    let mut points: Vec<Point3> = c.control_points().collect();
    let n = points.len() - 1;
    let moved = points[0].distance(start).max(points[n].distance(end));
    points[0] = start;
    points[n] = end;
    let curve = NurbsCurve::new(c.knots().clone(), points, Some(c.weights().to_vec()))?;
    Ok((curve, moved))
}

/// The length of the control polygon: at least the curve's length.
fn length_bound(c: &NurbsCurve) -> f64 {
    let pts: Vec<Point3> = c.control_points().collect();
    pts.windows(2).map(|w| w[0].distance(w[1])).sum()
}
