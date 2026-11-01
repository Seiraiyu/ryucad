use super::*;
use crate::SheetError;
use crate::geom::{Axis, Circle2, Curve2, Frame2, Line2, Point2, Point3, Vector2, Vector3};
use crate::name::OpKey;
use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use crate::topo::Body;
use core::f64::consts::{FRAC_PI_2, FRAC_PI_4};

#[test]
fn allowances_give_developed_lengths() {
    let (r, t, theta) = (0.002, 0.001, FRAC_PI_2);
    assert_eq!(
        Allowance::KFactor(0.5).length(r, t, theta),
        Some(theta * (r + 0.5 * t))
    );
    assert_eq!(Allowance::Length(0.003).length(r, t, theta), Some(0.003));
    for bad in [
        Allowance::KFactor(-0.1),
        Allowance::KFactor(1.01),
        Allowance::KFactor(f64::NAN),
        Allowance::Length(0.0),
        Allowance::Length(-1.0),
        Allowance::Length(f64::INFINITY),
    ] {
        assert_eq!(bad.length(r, t, theta), None, "{bad:?}");
    }
    assert_eq!(SheetOptions::default().tolerance, DEFAULT_TOLERANCE);
}

const K: OpKey = OpKey(7);
const T: f64 = 0.001;
const R: f64 = 0.002;
const W: f64 = 0.01;

/// One stretch of a bent strip's path.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Leg {
    /// Straight on.
    Run(f64),
    /// A turn by `angle` (left when positive) with inner radius `radius`.
    Turn { angle: f64, radius: f64 },
}

/// The segments of a strip's path, from the origin along `x`, its
/// material on the left, and of the path `t` to its left, as
/// `(start, end, curve, t0, t1)` from start to end.
fn path(legs: &[Leg], t: f64, offset: f64) -> Vec<(Curve2, f64, f64)> {
    let mut p = Point2::new(0.0, 0.0);
    let mut h = Vector2::new(1.0, 0.0);
    let left = |h: Vector2| Vector2::new(-h.y, h.x);
    let at = |p: Point2, v: Vector2| Point2::new(p.x + v.x, p.y + v.y);
    let mut out = Vec::new();
    for leg in legs {
        match *leg {
            Leg::Run(l) => {
                let a = at(p, left(h) * offset);
                let line = Line2::new(a, h).unwrap();
                out.push((Curve2::Line2(line), 0.0, l));
                p = at(p, h * l);
            }
            Leg::Turn { angle, radius } => {
                let s = angle.signum();
                // The path is the outer side of a left turn, the inner of a
                // right one.
                let rp = if s > 0.0 { radius + t } else { radius };
                let c = at(p, left(h) * (s * rp));
                let x = Vector2::new(p.x - c.x, p.y - c.y) * (1.0 / rp);
                let r = rp - s * offset;
                let frame = Frame2::new(c, x, s > 0.0).unwrap();
                out.push((
                    Curve2::Circle2(Circle2::new(frame, r).unwrap()),
                    0.0,
                    angle.abs(),
                ));
                let (sn, cs) = crate::math::sin_cos(angle);
                let xr = Vector2::new(x.x * cs - x.y * sn, x.x * sn + x.y * cs);
                p = at(c, xr * rp);
                h = Vector2::new(h.x * cs - h.y * sn, h.x * sn + h.y * cs);
            }
        }
    }
    out
}

/// A strip of sheet `t` thick along `legs`, extruded `w` along `z`.
pub(crate) fn strip(legs: &[Leg], t: f64, w: f64) -> Body {
    strip_along(legs, t, w, Vector3::new(0.0, 0.0, 1.0))
}

fn strip_along(legs: &[Leg], t: f64, w: f64, dir: Vector3) -> Body {
    let bottom = path(legs, t, 0.0);
    let top = path(legs, t, t);
    let mut segments = Vec::new();
    let mut key = 1;
    let mut push = |segments: &mut Vec<Segment>, c: Curve2, t0: f64, t1: f64| {
        segments.push(Segment {
            key: SegmentKey(key),
            curve: c,
            t0,
            t1,
        });
        key += 1;
    };
    for (c, t0, t1) in &bottom {
        push(&mut segments, c.clone(), *t0, *t1);
    }
    let (c, _, e) = bottom.last().unwrap();
    let end = c.point(*e);
    let (c, _, e) = top.last().unwrap();
    let end_top = c.point(*e);
    push(
        &mut segments,
        Curve2::Line2(Line2::through(end, end_top).unwrap()),
        0.0,
        end.distance(end_top),
    );
    for (c, t0, t1) in top.iter().rev() {
        push(&mut segments, c.clone(), *t1, *t0);
    }
    let start_top = top[0].0.point(top[0].1);
    push(
        &mut segments,
        Curve2::Line2(Line2::through(start_top, Point2::new(0.0, 0.0)).unwrap()),
        0.0,
        t,
    );
    let profile = ValidProfile::new(Profile {
        frame: crate::geom::Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    extrude(K, &profile, dir, 0.0, w).unwrap().0
}

pub(crate) fn l_bracket() -> Body {
    strip(
        &[
            Leg::Run(0.02),
            Leg::Turn {
                angle: FRAC_PI_2,
                radius: R,
            },
            Leg::Run(0.015),
        ],
        T,
        W,
    )
}

fn polygon(pts: &[(f64, f64)], w: f64) -> Body {
    let segments = (0..pts.len())
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: crate::geom::Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, w)
        .unwrap()
        .0
}

/// A square tube with rounded corners: four flanges and four bends in a
/// loop.
fn tube() -> Body {
    let rounded = |key: u128, half: f64, r: f64| {
        let mut segments = Vec::new();
        let mut n = 0;
        let corners = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
        for (i, &(cx, cy)) in corners.iter().enumerate() {
            let c = Point2::new(cx * (half - r), cy * (half - r));
            let start = FRAC_PI_2 * i as f64;
            let x = Vector2::new(crate::math::cos(start), crate::math::sin(start));
            let frame = Frame2::new(c, x, true).unwrap();
            segments.push(Segment {
                key: SegmentKey(key * 100 + n),
                curve: Curve2::Circle2(Circle2::new(frame, r).unwrap()),
                t0: 0.0,
                t1: FRAC_PI_2,
            });
            n += 1;
            let end = start + FRAC_PI_2;
            let a = Point2::new(
                c.x + r * crate::math::cos(end),
                c.y + r * crate::math::sin(end),
            );
            let (nx, ny) = corners[(i + 1) % 4];
            let c2 = Point2::new(nx * (half - r), ny * (half - r));
            let b = Point2::new(
                c2.x + r * crate::math::cos(end),
                c2.y + r * crate::math::sin(end),
            );
            segments.push(Segment {
                key: SegmentKey(key * 100 + n),
                curve: Curve2::Line2(Line2::through(a, b).unwrap()),
                t0: 0.0,
                t1: a.distance(b),
            });
            n += 1;
        }
        Loop {
            key: LoopKey(key),
            segments,
        }
    };
    let profile = ValidProfile::new(Profile {
        frame: crate::geom::Frame::WORLD,
        loops: vec![rounded(1, 0.01, R + T), rounded(2, 0.01 - T, R)],
    })
    .unwrap();
    extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, W)
        .unwrap()
        .0
}

/// The L bracket less a wedge about the bend's axis from 30° to 60°, over
/// the top half of its width: a notch whose walls are planes through the
/// axis. With `spline`, one of those walls is written as a NURBS.
fn notched(spline: bool) -> Body {
    let b = l_bracket();
    let m = model(&b);
    let axis = m.bends[0].axis;
    let o = axis.origin();
    let reach = 4.0 * (R + T);
    let ray = |a: f64| {
        let d = axis.radial(a);
        Point2::new(o.x + reach * d.x, o.y + reach * d.y)
    };
    let tri = [
        Point2::new(o.x, o.y),
        ray(FRAC_PI_4 - 0.25),
        ray(FRAC_PI_4 + 0.25),
    ];
    let segments = (0..3)
        .map(|i| Segment {
            key: SegmentKey(50 + i as u128),
            curve: Curve2::Line2(Line2::through(tri[i], tri[(i + 1) % 3]).unwrap()),
            t0: 0.0,
            t1: tri[i].distance(tri[(i + 1) % 3]),
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: crate::geom::Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(5),
            segments,
        }],
    })
    .unwrap();
    let wedge = extrude(
        OpKey(8),
        &profile,
        Vector3::new(0.0, 0.0, 1.0),
        0.5 * W,
        2.0 * W,
    )
    .unwrap()
    .0;
    let tool = crate::boolean::Tool {
        key: crate::name::ToolKey(1),
        body: &wedge,
        place: crate::geom::Rigid::IDENTITY,
    };
    let (mut cut, _) =
        crate::boolean::boolean(OpKey(9), crate::boolean::BooleanOp::Subtract, &b, &[tool])
            .unwrap();
    if spline {
        let f = cut
            .faces()
            .find(|&f| {
                super::shape::flat(&cut, f).is_some_and(|p| {
                    p.normal.get().dot(Vector3::new(0.0, 0.0, 1.0)).abs() < 1e-9
                        && p.height(axis.origin()).abs() < 1e-12
                })
            })
            .expect("a wall through the axis");
        super::radial::tests::to_spline(&mut cut, f);
    }
    cut
}

fn model(b: &Body) -> SheetModel {
    recognize(b, &SheetOptions::default()).unwrap()
}

fn refused(b: &Body) -> SheetError {
    match recognize(b, &SheetOptions::default()) {
        Err(crate::Error::InvalidSheet(e)) => e,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12
}

#[test]
fn an_l_bracket_is_two_flanges_and_a_bend() {
    let b = l_bracket();
    let m = model(&b);
    assert!(close(m.thickness, T));
    assert_eq!(m.flanges.len(), 2);
    assert_eq!(m.bends.len(), 1);
    let bend = &m.bends[0];
    assert!(close(bend.radius, R));
    assert!(close(bend.angle, FRAC_PI_2));
    assert_eq!(bend.inner.len(), 1);
    assert_eq!(bend.outer.len(), 1);
    // The two caps square to the axis.
    assert_eq!(bend.walls.len(), 2);
    assert_eq!(bend.lines[0].len(), 2);
    assert_eq!(bend.lines[1].len(), 2);
    assert!(m.is_tree());
    assert_eq!(bend.flanges[0], m.fixed);
    // Seen from the fixed flange's outer side, its first bend folds away.
    assert_eq!(bend.direction, BendDirection::Down);
    for f in &m.flanges {
        assert_eq!(f.sides[0].faces.len(), 1);
        assert_eq!(f.sides[1].faces.len(), 1);
        // A side face, its end face and both caps' parts are its faces.
        assert_eq!(f.faces.len(), 3);
    }
}

#[test]
fn a_flat_plate_is_one_flange() {
    let b = strip(&[Leg::Run(0.02)], T, W);
    let m = model(&b);
    assert!(close(m.thickness, T));
    assert_eq!(m.flanges.len(), 1);
    assert!(m.bends.is_empty());
    assert_eq!(m.flanges[0].faces.len(), 6);
}

#[test]
fn a_sharp_corner_is_not_a_bend() {
    // An L of two plates meeting square, no radius.
    // Wide enough that its sides are the sheet's, not its ends.
    let pts = [
        (0.0, 0.0),
        (0.02, 0.0),
        (0.02, 0.015),
        (0.019, 0.015),
        (0.019, 0.001),
        (0.0, 0.001),
    ];
    let b = polygon(&pts, 0.05);
    assert!(matches!(refused(&b), SheetError::SharpBend { .. }));
}

#[test]
fn an_emboss_stands_out_of_its_flange() {
    // A plate with a bump on its top, extruded across.
    let pts = [
        (0.0, 0.0),
        (0.03, 0.0),
        (0.03, T),
        (0.016, T),
        (0.016, 3.0 * T),
        (0.014, 3.0 * T),
        (0.014, T),
        (0.0, T),
    ];
    let b = polygon(&pts, W);
    assert!(matches!(refused(&b), SheetError::OutOfSlab { .. }));
}

#[test]
fn a_cut_askew_across_a_bend_is_not_radial() {
    // The bracket's top cut off by a plane tilted about x, across its first
    // flange, its bend and part of its second.
    let place = crate::geom::Rigid::translation(Vector3::new(-0.05, -0.01, 0.00291)).then_after(
        &crate::geom::Rigid::rotation(crate::geom::UnitVector3::X, 0.3),
    );
    let block = crate::primitive::block(OpKey(3), 0.2, 0.2, 0.2, &place)
        .unwrap()
        .0;
    let tool = crate::boolean::Tool {
        key: crate::name::ToolKey(1),
        body: &block,
        place: crate::geom::Rigid::IDENTITY,
    };
    let (b, _) = crate::boolean::boolean(
        OpKey(4),
        crate::boolean::BooleanOp::Subtract,
        &l_bracket(),
        &[tool],
    )
    .unwrap();
    assert!(matches!(refused(&b), SheetError::NotRadial { .. }));
}

#[test]
fn a_conical_bend_is_refused() {
    // A slanted strip turned a quarter about z: two cones joined by planes
    // through the axis.
    let frame = crate::geom::Frame::new(
        Point3::ORIGIN,
        Vector3::new(0.0, -1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let pts = [(0.01, 0.0), (0.011, 0.0), (0.016, 0.01), (0.015, 0.01)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(20 + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let b = revolve(K, &profile, axis, 0.0, FRAC_PI_2).unwrap().0;
    assert!(matches!(refused(&b), SheetError::ConicalBend { .. }));
}

#[test]
fn two_plates_apart_are_disconnected() {
    let pts =
        |x: f64| [(x, 0.0), (x + 0.01, 0.0), (x + 0.01, T), (x, T)].map(|(a, b)| Point2::new(a, b));
    let lp = |key: u128, ps: [Point2; 4]| Loop {
        key: LoopKey(key),
        segments: (0..4)
            .map(|i| Segment {
                key: SegmentKey(key * 10 + i as u128),
                curve: Curve2::Line2(Line2::through(ps[i], ps[(i + 1) % 4]).unwrap()),
                t0: 0.0,
                t1: ps[i].distance(ps[(i + 1) % 4]),
            })
            .collect(),
    };
    let profile = ValidProfile::new(Profile {
        frame: crate::geom::Frame::WORLD,
        loops: vec![lp(1, pts(0.0)), lp(2, pts(0.02))],
    })
    .unwrap();
    let b = extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, W)
        .unwrap()
        .0;
    assert_eq!(refused(&b), SheetError::Disconnected);
}

#[test]
fn a_tube_of_flanges_is_a_loop() {
    let m = model(&tube());
    assert_eq!(m.flanges.len(), 4);
    assert_eq!(m.bends.len(), 4);
    assert!(!m.is_tree());
    let c = m.cycle.expect("a loop");
    assert_eq!(c.bends.len(), 4);
    assert_eq!(c.faces.len(), 2);
}

#[test]
fn walls_through_the_axis_are_radial_even_as_splines() {
    for spline in [false, true] {
        let m = model(&notched(spline));
        assert_eq!(m.flanges.len(), 2);
        assert_eq!(m.bends.len(), 1);
        // The bottom cap, the top cap in two, the notch's floor and its
        // two walls through the axis.
        assert_eq!(m.bends[0].walls.len(), 6, "spline {spline}");
    }
}
