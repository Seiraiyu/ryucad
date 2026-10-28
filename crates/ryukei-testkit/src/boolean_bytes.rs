//! Booleans from bytes, for the `boolean` fuzz target and for replaying
//! what it has found: a target and one or two tools, each a block,
//! cylinder, cone, frustum, sphere or torus placed on a coarse grid of
//! positions, axes and sizes (so that coaxial, flush, tangent and
//! coincident placements come up often), and an operation. A general case
//! ([`general_case`]) also takes those primitives in NURBS form, lofted
//! frustums, sweeps round an arc and spline revolutions.

use crate::euler_script::Bytes;
use core::f64::consts::{FRAC_PI_2, TAU};
use ryukei::boolean::{BooleanOp, Tool, ToolKey};
use ryukei::geom::{
    Axis, Circle, Circle2, Curve, Curve2, Frame, Frame2, KnotVector, Line2, NurbsCurve2, Point2,
    Point3, Rigid, Vector2, Vector3,
};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{
    AlongOptions, LoftOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey,
    ValidProfile, along, loft, revolve,
};
use ryukei::topo::Body;
use std::sync::Arc;

/// A grid value: one of `n` steps across `[lo, hi]`.
fn grid(b: &mut Bytes, lo: f64, hi: f64, n: usize) -> f64 {
    lo + (hi - lo) * b.pick(n + 1) as f64 / n as f64
}

/// A direction on a coarse grid of the sphere, mostly along an axis.
fn direction(b: &mut Bytes) -> Vector3 {
    match b.pick(4) {
        0 => Vector3::new(0.0, 0.0, 1.0),
        1 => Vector3::new(1.0, 0.0, 0.0),
        2 => Vector3::new(0.0, 1.0, 0.0),
        _ => {
            let (lon, lat) = (grid(b, 0.0, TAU, 8), grid(b, -FRAC_PI_2, FRAC_PI_2, 4));
            let (sl, cl) = ryukei::math::sin_cos(lon);
            let (sa, ca) = ryukei::math::sin_cos(lat);
            Vector3::new(ca * cl, ca * sl, sa)
        }
    }
}

/// A placement: an origin on a grid of half steps, and grid axes.
fn place(b: &mut Bytes) -> Option<Rigid> {
    let o = Point3::new(
        grid(b, -1.0, 1.0, 4),
        grid(b, -1.0, 1.0, 4),
        grid(b, -1.0, 1.0, 4),
    );
    let z = direction(b);
    let f = Frame::new(o, z, direction(b))
        .or_else(|_| Frame::new(o, z, Vector3::new(1.0, 2.0, 3.0)))
        .ok()?;
    Some(Rigid::from_frame(&f))
}

/// A placed primitive, or `None` when the bytes give one that can't be
/// built.
pub fn solid(b: &mut Bytes) -> Option<Body> {
    let at = place(b)?;
    let (s, t) = (grid(b, 0.5, 2.0, 3), grid(b, 0.5, 2.0, 3));
    let k = OpKey(1);
    let made = match b.pick(6) {
        0 => primitive::block(k, s, t, grid(b, 0.5, 2.0, 3), &at),
        1 => primitive::cylinder(k, s / 2.0, t, &at),
        2 => primitive::cone(k, s / 2.0, t, &at),
        3 => primitive::frustum(k, s / 2.0, s / 4.0, t, &at),
        4 => primitive::sphere(k, s / 2.0, &at),
        _ => primitive::torus(k, s / 2.0 + 0.25, 0.25, &at),
    };
    made.ok().map(|(body, _)| body)
}

/// A boolean to try: the operation, the target, and its tools.
pub struct Case {
    pub op: BooleanOp,
    pub target: Body,
    pub tools: Vec<Body>,
}

impl Case {
    /// The tools as the boolean takes them, keyed from 1, in place.
    pub fn tools(&self) -> Vec<Tool<'_>> {
        self.tools
            .iter()
            .enumerate()
            .map(|(i, body)| Tool {
                key: ToolKey(i as u128 + 1),
                body,
                place: Rigid::IDENTITY,
            })
            .collect()
    }
}

/// A boolean from bytes, or `None` when they give no buildable operands.
pub fn boolean_case(data: &[u8]) -> Option<Case> {
    let mut b = Bytes::new(data);
    let op = match b.pick(3) {
        0 => BooleanOp::Unite,
        1 => BooleanOp::Intersect,
        _ => BooleanOp::Subtract,
    };
    let target = solid(&mut b)?;
    let mut tools = vec![solid(&mut b)?];
    if b.pick(3) == 0 {
        tools.push(solid(&mut b)?);
    }
    Some(Case { op, target, tools })
}

/// A general boolean from bytes, or `None` when they give no buildable
/// operands: like [`boolean_case`], each operand any of [`general_solid`]'s.
pub fn general_case(data: &[u8]) -> Option<Case> {
    let mut b = Bytes::new(data);
    let op = match b.pick(3) {
        0 => BooleanOp::Unite,
        1 => BooleanOp::Intersect,
        _ => BooleanOp::Subtract,
    };
    let target = general_solid(&mut b)?;
    let mut tools = vec![general_solid(&mut b)?];
    if b.pick(3) == 0 {
        tools.push(general_solid(&mut b)?);
    }
    Some(Case { op, target, tools })
}

fn seg(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(key),
        curve,
        t0,
        t1,
    }
}

/// A square of side `s`, or a circle of diameter `s`, about the origin.
fn section_loop(square: bool, first: u128, s: f64) -> Loop {
    if !square {
        let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap();
        return Loop {
            key: LoopKey(1),
            segments: vec![seg(
                first,
                Curve2::Circle2(Circle2::new(f, s / 2.0).unwrap()),
                0.0,
                TAU,
            )],
        };
    }
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            seg(
                first + i as u128,
                Curve2::Line2(Line2::through(p, q).unwrap()),
                0.0,
                p.distance(q),
            )
        })
        .collect();
    Loop {
        key: LoopKey(1),
        segments,
    }
}

/// A placed body on any surface: one of [`solid`]'s primitives, as it is
/// or with every curve and surface its exact NURBS form; a lofted frustum
/// of squares or circles; a square or circle swept round a quarter turn;
/// or a spline meridian turned about its axis. Sizes on the same grid.
pub fn general_solid(b: &mut Bytes) -> Option<Body> {
    let kind = b.pick(5);
    if kind == 0 {
        return solid(b);
    }
    if kind == 1 {
        let mut body = solid(b)?;
        crate::nurbs::convert_body(&mut body).ok()?;
        return Some(body);
    }
    let at = place(b)?;
    let (s, t) = (grid(b, 0.5, 2.0, 3), grid(b, 0.5, 2.0, 3));
    let square = b.pick(2) == 0;
    let made = match kind {
        2 => {
            let section = |h: f64, first: u128, size: f64, turn: f64| {
                let (sn, cs) = ryukei::math::sin_cos(turn);
                ValidProfile::new(Profile {
                    frame: Frame::new(
                        Point3::new(0.0, 0.0, h),
                        Vector3::new(0.0, 0.0, 1.0),
                        Vector3::new(cs, sn, 0.0),
                    )
                    .ok()?,
                    loops: vec![section_loop(square, first, size)],
                })
                .ok()
            };
            let turn = grid(b, 0.0, 0.6, 2);
            let sections = [section(0.0, 10, s, 0.0)?, section(t, 20, s / 2.0, turn)?];
            loft(OpKey(1), &sections, &LoftOptions::default()).ok()?.0
        }
        3 => {
            // Round a quarter turn of radius `s` about z, from the x axis.
            let frame = Frame::new(
                Point3::new(s, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .ok()?;
            let profile = ValidProfile::new(Profile {
                frame,
                loops: vec![section_loop(square, 1, (s * 0.4).min(t * 0.4))],
            })
            .ok()?;
            let arc = Circle::new(Frame::WORLD, s).ok()?;
            let path = Path {
                segments: vec![PathSegment {
                    key: SegmentKey(9),
                    curve: Curve::Circle(arc),
                    range: [0.0, FRAC_PI_2],
                }],
            };
            let opts = AlongOptions {
                twist: grid(b, 0.0, 1.0, 2),
                ..AlongOptions::default()
            };
            along(OpKey(1), &profile, &path, &opts).ok()?.0
        }
        _ => {
            let kv = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).ok()?;
            let pts = vec![
                Point2::new(0.0, -t / 2.0),
                Point2::new(s * 0.65, -t / 2.0),
                Point2::new(s * 0.45, t / 2.0),
                Point2::new(0.0, t / 2.0),
            ];
            let meridian = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(kv, pts, None).ok()?));
            let back = Curve2::Line2(
                Line2::through(Point2::new(0.0, t / 2.0), Point2::new(0.0, -t / 2.0)).ok()?,
            );
            let profile = ValidProfile::new(Profile {
                frame: Frame::new(
                    Point3::ORIGIN,
                    Vector3::new(0.0, -1.0, 0.0),
                    Vector3::new(1.0, 0.0, 0.0),
                )
                .ok()?,
                loops: vec![Loop {
                    key: LoopKey(1),
                    segments: vec![seg(1, meridian, 0.0, 1.0), seg(2, back, 0.0, t)],
                }],
            })
            .ok()?;
            let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).ok()?;
            revolve(OpKey(1), &profile, axis, 0.0, TAU).ok()?.0
        }
    };
    made.transformed(&at).ok()
}
