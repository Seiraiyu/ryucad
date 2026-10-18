//! Sweeps from bytes: profiles of rectangles, polygons, circles, slots and
//! NURBS blobs, anywhere in a tilted frame (overlapping, nested, touching
//! or crossing a revolve's axis as the bytes fall), extruded or revolved by
//! any amount. Every result must be a structured error, or a body that
//! passes the checker and meshes with no faults at a chord well under the
//! profile's smallest feature (the mesher can't yet resolve a feature
//! thinner than its chord, such as a thin torus, whatever made it). Any
//! panic is a bug.

#![no_main]

use core::f64::consts::{PI, TAU};
use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::check;
use ryukei::geom::{
    Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Rigid, Vector2, Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use ryukei_testkit::Lcg;
use ryukei_testkit::euler_script::Bytes;
use ryukei_testkit::nurbs::blob2;
use std::sync::Arc;

fn line(key: u128, a: Point2, b: Point2) -> Option<Segment> {
    let curve = Curve2::Line2(Line2::through(a, b).ok()?);
    Some(Segment {
        key: SegmentKey(key),
        curve,
        t0: 0.0,
        t1: a.distance(b),
    })
}

fn arc(key: u128, c: Point2, r: f64, a0: f64, a1: f64) -> Option<Segment> {
    let f = Frame2::new(c, Vector2::new(1.0, 0.0), true).ok()?;
    Some(Segment {
        key: SegmentKey(key),
        curve: Curve2::Circle2(Circle2::new(f, r).ok()?),
        t0: a0,
        t1: a1,
    })
}

/// One loop from bytes, keyed from `key`, and its smallest dimension, or
/// `None` when its own geometry can't be built (a degenerate line or
/// circle).
fn shape(b: &mut Bytes, key: u128) -> Option<(Loop, f64)> {
    let c = Point2::new(b.range(-3.0, 3.0), b.range(-3.0, 3.0));
    let s = b.range(0.01, 2.0);
    let mut small = s;
    let k = key * 32;
    let segments = match b.pick(5) {
        0 => {
            let (w, h) = (s, b.range(0.01, 2.0));
            small = small.min(h);
            let p = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| c + Vector2::new(x, y));
            (0..4)
                .map(|i| line(k + i as u128, p[i], p[(i + 1) % 4]))
                .collect::<Option<Vec<_>>>()?
        }
        1 => {
            // A star-shaped polygon, perhaps self-crossing when the radii
            // jump about.
            let n = 3 + b.pick(6);
            let p: Vec<Point2> = (0..n)
                .map(|i| {
                    let a = TAU * i as f64 / n as f64;
                    let r = s * b.range(0.3, 1.0);
                    small = small.min(r);
                    c + Vector2::new(r * ryukei::math::cos(a), r * ryukei::math::sin(a))
                })
                .collect();
            (0..n)
                .map(|i| line(k + i as u128, p[i], p[(i + 1) % n]))
                .collect::<Option<Vec<_>>>()?
        }
        2 => {
            let start = b.range(-PI, PI);
            vec![arc(
                k,
                c,
                s,
                start,
                start + if b.byte() % 2 == 0 { TAU } else { -TAU },
            )?]
        }
        3 => {
            // A slot: two lines and two half circles.
            let (l, r) = (s, b.range(0.01, 1.0));
            small = small.min(r);
            let p = |x: f64, y: f64| c + Vector2::new(x, y);
            vec![
                line(k, p(0.0, -r), p(l, -r))?,
                arc(k + 1, p(l, 0.0), r, -PI / 2.0, PI / 2.0)?,
                line(k + 2, p(l, r), p(0.0, r))?,
                arc(k + 3, p(0.0, 0.0), r, PI / 2.0, 1.5 * PI)?,
            ]
        }
        _ => {
            let mut g = Lcg::new(u64::from(b.byte()));
            let n = blob2(&mut g, c, s);
            let [a, z] = n.domain();
            vec![Segment {
                key: SegmentKey(k),
                curve: Curve2::Nurbs2(Arc::new(n)),
                t0: a,
                t1: z,
            }]
        }
    };
    Some((
        Loop {
            key: LoopKey(key),
            segments,
        },
        small,
    ))
}

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes::new(data);
    let axis = Vector3::new(b.range(-1.0, 1.0), b.range(-1.0, 1.0), 1.0)
        .normalize()
        .expect("z is 1");
    let shift = Vector3::new(
        b.range(-20.0, 20.0),
        b.range(-20.0, 20.0),
        b.range(-20.0, 20.0),
    );
    let at = Rigid::translation(shift).then_after(&Rigid::rotation(axis, b.range(-3.2, 3.2)));
    let frame = Frame::new(
        at.apply_point(Point3::ORIGIN),
        at.apply_vector(Vector3::new(0.0, 0.0, 1.0)),
        at.apply_vector(Vector3::new(1.0, 0.0, 0.0)),
    )
    .expect("a rigid frame");
    let mut loops = Vec::new();
    let mut small = f64::INFINITY;
    for key in 1..=1 + b.pick(3) as u128 {
        let Some((lp, s)) = shape(&mut b, key) else {
            return;
        };
        loops.push(lp);
        small = small.min(s);
    }
    let profile = match ValidProfile::new(Profile { frame, loops }) {
        Ok(p) => p,
        Err(Error::InvalidProfile(_)) => return,
        Err(e) => panic!("unexpected {e:?}"),
    };
    let key = OpKey(7);
    let made = if b.byte() % 2 == 0 {
        let dir = frame.z().get()
            + frame.x().get() * b.range(-2.0, 2.0)
            + frame.y().get() * b.range(-2.0, 2.0);
        let start = b.range(-2.0, 2.0);
        extrude(key, &profile, dir, start, start + b.range(-0.1, 3.0))
    } else {
        // An axis in the plane (now and then a little out of it), through
        // a point that may sit inside the profile.
        let a = b.range(-PI, PI);
        let dir = frame.x().get() * ryukei::math::cos(a) + frame.y().get() * ryukei::math::sin(a);
        let lift = if b.byte() % 16 == 0 {
            b.range(-1e-6, 1e-6)
        } else {
            0.0
        };
        let through = frame.to_world(b.range(-4.0, 4.0), b.range(-4.0, 4.0), lift);
        let Ok(axis) = Axis::new(through, dir) else {
            return;
        };
        let start = b.range(-PI, PI);
        let span = if b.byte() % 4 == 0 {
            TAU
        } else {
            b.range(-0.1, 7.0)
        };
        revolve(key, &profile, axis, start, start + span)
    };
    let body = match made {
        Ok((body, _)) => body,
        Err(Error::InvalidSweep(_) | Error::Degenerate | Error::NonFinite) => return,
        Err(e) => panic!("unexpected {e:?}"),
    };
    assert_eq!(check(&body), []);
    let params = MeshParams {
        max_triangles: 20_000,
        ..MeshParams::new((small / 8.0).clamp(1e-3, 0.05), 0.5)
    };
    match tessellate(&body, &params) {
        Ok(m) => {
            assert_eq!(m.faults, []);
            assert_eq!(check_mesh(&body, &m), Ok(()));
        }
        Err(Error::MeshTooLarge { .. }) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
});
