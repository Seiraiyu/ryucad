//! Booleans from bytes, for the `boolean` fuzz target and for replaying
//! what it has found: a target and one or two tools, each a block,
//! cylinder, cone, frustum, sphere or torus placed on a coarse grid of
//! positions, axes and sizes (so that coaxial, flush, tangent and
//! coincident placements come up often), and an operation.

use crate::euler_script::Bytes;
use core::f64::consts::{FRAC_PI_2, TAU};
use ryukei::boolean::{BooleanOp, Tool, ToolKey};
use ryukei::geom::{Frame, Point3, Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;

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
