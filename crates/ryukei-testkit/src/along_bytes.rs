//! Sweeps along paths and lofts from bytes, for the `sweep` fuzz target
//! and for replaying what it has found: a profile as [`super::sweep_bytes`]
//! makes them, swept along a smooth chain of lines, arcs and cubic splines
//! starting through it (by either frame law, twisted and scaled as the
//! bytes fall), or lofted through copies of it in frames moved along and
//! turned about its normal, now and then round to the first again.

use crate::euler_script::Bytes;
use crate::sweep_bytes::shape;
use core::f64::consts::PI;
use ryukei::Error;
use ryukei::geom::{
    Circle, Curve, Frame, FrameLaw, KnotVector, Line, NurbsCurve, Point3, Rigid, UnitVector3,
    Vector3,
};
use ryukei::name::OpKey;
use ryukei::sweep::{
    AlongOptions, LoftOptions, Path, PathSegment, Profile, SegmentKey, Tangent, ValidProfile,
    along, loft,
};
use ryukei::topo::Body;
use std::sync::Arc;

/// A unit vector across `t`, turned about it by `a`.
fn across(t: Vector3, a: f64) -> Vector3 {
    let up = if t.z.abs() < 0.9 {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        Vector3::new(1.0, 0.0, 0.0)
    };
    let n = t.cross(up).normalize().expect("across a unit vector").get();
    let m = t.cross(n);
    let (s, c) = ryukei::math::sin_cos(a);
    n * c + m * s
}

/// A smooth chain of one to three segments from `p` along `t`: lines,
/// arcs turning any way, and cubic splines leaving along `t`.
fn path(b: &mut Bytes, mut p: Point3, mut t: Vector3) -> Option<Path> {
    let mut segments = Vec::new();
    for k in 0..1 + b.pick(3) {
        let key = SegmentKey(100 + k as u128);
        let (curve, range) = match b.pick(3) {
            0 => {
                let len = b.range(0.1, 4.0);
                (Curve::Line(Line::new(p, t).ok()?), [0.0, len])
            }
            1 => {
                let r = b.range(0.2, 3.0);
                let n = across(t, b.range(-PI, PI));
                let f = Frame::new(p + n * r, (-n).cross(t), -n).ok()?;
                (
                    Curve::Circle(Circle::new(f, r).ok()?),
                    [0.0, b.range(0.1, 3.0)],
                )
            }
            _ => {
                let a = b.range(0.3, 2.0);
                let mut q = |p: Point3| {
                    p + Vector3::new(b.range(-2.0, 2.0), b.range(-2.0, 2.0), b.range(-2.0, 2.0))
                };
                let p1 = p + t * a;
                let (p2, p3) = (q(p1 + t * a), q(p1 + t * (2.0 * a)));
                let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).ok()?;
                let n = NurbsCurve::new(k, vec![p, p1, p2, p3], None).ok()?;
                (Curve::Nurbs(Arc::new(n)), [0.0, 1.0])
            }
        };
        let end = curve.eval(range[1]);
        p = end.point;
        t = end.d1.normalize()?.get();
        segments.push(PathSegment { key, curve, range });
    }
    Some(Path { segments })
}

/// The body the bytes sweep along a path or loft, the profile's smallest
/// dimension, and whether it was swept along a path (always an open one:
/// these chains never close); `None` where the bytes make no valid
/// profile, or the operation refuses them with a structured error. Any
/// other error panics: it is a bug.
pub fn carried(data: &[u8]) -> Option<(Body, f64, bool)> {
    let (made, small, swept) = outcome(data)?;
    match made {
        Ok(body) => Some((body, small, swept)),
        Err(
            Error::InvalidSweep(_) | Error::InvalidLoft(_) | Error::Degenerate | Error::NonFinite,
        ) => None,
        Err(e) => panic!("unexpected {e:?}"),
    }
}

/// What the sweep or loft the bytes make gives, refusal and all, with the
/// profile's smallest dimension and whether it was swept along a path;
/// `None` where the bytes make no valid profile or path.
pub fn outcome(data: &[u8]) -> Option<(Result<Body, Error>, f64, bool)> {
    let mut b = Bytes::new(data);
    let tilt = Vector3::new(b.range(-0.5, 0.5), b.range(-0.5, 0.5), 1.0)
        .normalize()
        .expect("z is 1")
        .get();
    let frame = Frame::new(Point3::ORIGIN, tilt, Vector3::new(1.0, 0.0, 0.0)).ok()?;
    let mut loops = Vec::new();
    let mut small = f64::INFINITY;
    for key in 1..=1 + b.pick(2) as u128 {
        let (lp, s) = shape(&mut b, key)?;
        loops.push(lp);
        small = small.min(s);
    }
    let profile = match ValidProfile::new(Profile {
        frame,
        loops: loops.clone(),
    }) {
        Ok(p) => p,
        Err(Error::InvalidProfile(_)) => return None,
        Err(e) => panic!("unexpected {e:?}"),
    };
    let key = OpKey(8);
    let swept = b.byte().is_multiple_of(2);
    let made = if swept {
        let t = Vector3::new(b.range(-0.5, 0.5), b.range(-0.5, 0.5), 1.0)
            .normalize()?
            .get();
        let path = path(&mut b, Point3::ORIGIN, t)?;
        let law = if b.byte().is_multiple_of(2) {
            FrameLaw::Minimal
        } else {
            FrameLaw::Binormal(UnitVector3::exact(across(t, b.range(-PI, PI)))?)
        };
        let twist = if b.byte().is_multiple_of(2) {
            0.0
        } else {
            b.range(-PI, PI)
        };
        let scale = if b.byte().is_multiple_of(2) {
            1.0
        } else {
            b.range(0.3, 2.0)
        };
        along(key, &profile, &path, &AlongOptions { law, twist, scale })
    } else {
        // Copies of the profile in frames moved along its normal and
        // turned about it.
        let n = 2 + b.pick(3);
        let mut sections = vec![profile];
        let mut at = Rigid::IDENTITY;
        for _ in 1..n {
            let step = Rigid::translation(tilt * b.range(0.05, 3.0)).then_after(&Rigid::rotation(
                UnitVector3::exact(tilt)?,
                b.range(-0.6, 0.6),
            ));
            at = step.then_after(&at);
            let f = Frame::new(
                at.apply_point(Point3::ORIGIN),
                at.apply_vector(tilt),
                at.apply_vector(Vector3::new(1.0, 0.0, 0.0)),
            )
            .ok()?;
            match ValidProfile::new(Profile {
                frame: f,
                loops: loops.clone(),
            }) {
                Ok(p) => sections.push(p),
                Err(Error::InvalidProfile(_)) => return None,
                Err(e) => panic!("unexpected {e:?}"),
            }
        }
        let tangent = |b: &mut Bytes| {
            b.byte().is_multiple_of(3).then(|| Tangent {
                direction: UnitVector3::exact(tilt).expect("unit"),
                weight: b.range(0.1, 4.0),
            })
        };
        let opts = LoftOptions {
            closed: b.byte().is_multiple_of(8),
            start: tangent(&mut b),
            end: tangent(&mut b),
        };
        loft(key, &sections, &opts)
    };
    Some((made.map(|(body, _)| body), small, swept))
}
