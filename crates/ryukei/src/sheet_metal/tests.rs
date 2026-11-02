use super::*;
use crate::SheetMetalError;
use crate::geom::{Axis, Circle2, Curve2, Frame2, Line2, Point2, Point3, Vector2, Vector3};
use crate::name::{Event, OpKey};
use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use crate::topo::Body;
use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

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

fn refused(b: &Body) -> SheetMetalError {
    match recognize(b, &SheetOptions::default()) {
        Err(crate::Error::InvalidSheetMetal(e)) => e,
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
    assert!(matches!(refused(&b), SheetMetalError::SharpBend { .. }));
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
    assert!(matches!(refused(&b), SheetMetalError::OutOfSlab { .. }));
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
    assert!(matches!(refused(&b), SheetMetalError::NotRadial { .. }));
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
    assert!(matches!(refused(&b), SheetMetalError::ConicalBend { .. }));
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
    assert_eq!(refused(&b), SheetMetalError::Disconnected);
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

/// The sorted extents of a body's vertices' box.
fn extents(b: &Body) -> [f64; 3] {
    let pts: Vec<Point3> = b.vertices().map(|v| b.vertices[v].point).collect();
    let bx = crate::geom::Box3::from_points(pts).unwrap();
    let d = bx.max() - bx.min();
    let mut e = [d.x, d.y, d.z];
    e.sort_by(f64::total_cmp);
    e
}

fn developed(theta: f64, k: f64) -> f64 {
    theta * (R + k * T)
}

#[test]
fn an_l_bracket_unfolds_to_its_developed_length() {
    let b = l_bracket();
    let m = model(&b);
    for k in [0.0, 0.33, 0.5, 1.0] {
        let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(k))).unwrap();
        let want = 0.02 + 0.015 + developed(FRAC_PI_2, k);
        let e = extents(&u.body);
        assert!(
            close(e[0], T) && close(e[1], W) && close(e[2], want),
            "{e:?} for k {k}"
        );
    }
    // The same by length.
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::Length(0.004))).unwrap();
    assert!(close(extents(&u.body)[2], 0.039));
}

#[test]
fn unfolding_keeps_every_name_and_journals_it() {
    let b = l_bracket();
    let m = model(&b);
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.4))).unwrap();
    let c = b.counts();
    assert_eq!(u.body.counts(), c);
    assert_eq!(u.journal.events().len(), c.faces + c.edges + c.vertices);
    for f in b.faces() {
        let n = b.faces[f].name;
        assert_eq!(u.body.faces[f].name, n);
        assert!(u.journal.events().contains(&Event::Modified(n)));
    }
}

#[test]
fn half_a_k_factor_keeps_the_volume() {
    let b = l_bracket();
    let m = model(&b);
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap();
    let e = extents(&u.body);
    let flat = e[0] * e[1] * e[2];
    let folded = T * W * 0.035 + FRAC_PI_2 / 2.0 * ((R + T) * (R + T) - R * R) * W;
    assert!((flat - folded).abs() <= 1e-15);
}

#[test]
fn channels_jogs_and_hems_unfold() {
    let turn = |angle| Leg::Turn { angle, radius: R };
    let cases: [(&[Leg], f64); 3] = [
        (
            &[
                Leg::Run(0.01),
                turn(FRAC_PI_2),
                Leg::Run(0.02),
                turn(FRAC_PI_2),
                Leg::Run(0.01),
            ],
            0.04 + 2.0 * developed(FRAC_PI_2, 0.4),
        ),
        (
            &[
                Leg::Run(0.01),
                turn(FRAC_PI_2),
                Leg::Run(0.02),
                turn(-FRAC_PI_2),
                Leg::Run(0.01),
            ],
            0.04 + 2.0 * developed(FRAC_PI_2, 0.4),
        ),
        (
            &[Leg::Run(0.02), turn(PI), Leg::Run(0.01)],
            0.03 + developed(PI, 0.4),
        ),
    ];
    for (legs, want) in cases {
        let b = strip(legs, T, W);
        let m = model(&b);
        let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.4))).unwrap();
        let e = extents(&u.body);
        assert!(close(e[2], want), "{e:?} against {want}");
    }
}

#[test]
fn a_partial_unfold_carries_folded_bends_with_their_flanges() {
    let turn = |angle| Leg::Turn { angle, radius: R };
    let b = strip(
        &[
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.02),
            turn(FRAC_PI_2),
            Leg::Run(0.01),
        ],
        T,
        W,
    );
    let m = model(&b);
    let one = m.bends[0].name;
    let spec = UnfoldSpec {
        bends: Some(vec![one]),
        ..UnfoldSpec::all(Allowance::KFactor(0.5))
    };
    let u = unfold(K, &b, &m, &spec).unwrap();
    assert_eq!(u.map.strips.len(), 1);
    assert_eq!(u.map.strips[0].bend, one);
    assert_eq!(u.pattern.bend_lines.len(), 1);
    // The bend left folded keeps its cylinders.
    let other = m.bends[1].inner[0];
    let Some(crate::topo::EntityId::Face(f)) = u.body.by_name(other) else {
        panic!("the face");
    };
    assert!(matches!(
        u.body.surfaces[u.body.faces[f].surface],
        crate::geom::Surface::Cylinder(_)
    ));
}

#[test]
fn the_flat_map_names_its_strip_and_motions() {
    let b = l_bracket();
    let m = model(&b);
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap();
    assert_eq!(u.map.flanges.len(), 2);
    let s = &u.map.strips[0];
    assert_eq!(s.near, m.flanges[m.fixed].name);
    assert!(close(s.length, developed(FRAC_PI_2, 0.5)));
    // The fixed flange doesn't move.
    let (_, fixed) = u.map.flanges.iter().find(|(n, _)| *n == s.near).unwrap();
    assert_eq!(*fixed, crate::geom::Rigid::IDENTITY);
}

#[test]
fn either_flange_can_be_fixed() {
    let b = l_bracket();
    let m = model(&b);
    let other = &m.flanges[1 - m.fixed];
    let spec = UnfoldSpec {
        fixed: Some(other.sides[0].faces[0]),
        ..UnfoldSpec::all(Allowance::KFactor(0.5))
    };
    let u = unfold(K, &b, &m, &spec).unwrap();
    let e = extents(&u.body);
    assert!(close(e[2], 0.035 + developed(FRAC_PI_2, 0.5)));
    assert_eq!(u.map.strips[0].near, other.name);
}

#[test]
fn one_thread_and_many_unfold_alike() {
    let b = l_bracket();
    let m = model(&b);
    let spec = UnfoldSpec::all(Allowance::KFactor(0.45));
    let run = |n| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| crate::codec::encode_body(&unfold(K, &b, &m, &spec).unwrap().body))
    };
    assert_eq!(run(1), run(4));
}

#[test]
fn a_loop_wont_unfold() {
    let b = tube();
    let m = model(&b);
    let c = m.cycle.clone().expect("a loop");
    let e = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap_err();
    assert_eq!(
        e,
        crate::Error::InvalidSheetMetal(SheetMetalError::BendLoop {
            bends: c.bends,
            faces: c.faces
        })
    );
}

#[test]
fn allowances_and_names_out_of_range_are_refused() {
    let b = l_bracket();
    let m = model(&b);
    let bend = m.bends[0].name;
    let sheet = |spec: &UnfoldSpec| match unfold(K, &b, &m, spec) {
        Err(crate::Error::InvalidSheetMetal(e)) => e,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(
        sheet(&UnfoldSpec::all(Allowance::KFactor(1.5))),
        SheetMetalError::BadAllowance { bend }
    );
    assert_eq!(
        sheet(&UnfoldSpec::all(Allowance::Length(-1.0))),
        SheetMetalError::BadAllowance { bend }
    );
    let nobody = crate::name::Name::from_u128(42);
    let spec = UnfoldSpec {
        bends: Some(vec![nobody]),
        ..UnfoldSpec::all(Allowance::KFactor(0.5))
    };
    assert_eq!(sheet(&spec), SheetMetalError::NoSuchBend { name: nobody });
    let spec = UnfoldSpec {
        fixed: Some(nobody),
        ..UnfoldSpec::all(Allowance::KFactor(0.5))
    };
    assert_eq!(sheet(&spec), SheetMetalError::NoSuchFace { name: nobody });
    let bad = SheetOptions {
        tolerance: f64::NAN,
    };
    assert_eq!(
        recognize(&b, &bad),
        Err(crate::Error::InvalidSheetMetal(
            SheetMetalError::BadTolerance
        ))
    );
}

#[test]
fn a_bend_with_a_notch_through_its_axis_unfolds() {
    let b = notched(false);
    let m = model(&b);
    assert_eq!(m.bends.len(), 1);
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap();
    let e = extents(&u.body);
    assert!(close(e[2], 0.035 + developed(FRAC_PI_2, 0.5)));
}

#[test]
fn a_spline_wall_through_the_axis_unfolds_as_its_plane_would() {
    let plain = notched(false);
    let spline = notched(true);
    let m = model(&spline);
    let spec = UnfoldSpec::all(Allowance::KFactor(0.5));
    let a = unfold(K, &plain, &model(&plain), &spec).unwrap().body;
    let c = unfold(K, &spline, &m, &spec).unwrap().body;
    for v in a.vertices() {
        assert!(a.vertices[v].point.distance(c.vertices[v].point) < 1e-12);
    }
}

#[test]
fn the_pattern_draws_the_outline_and_the_bend_line() {
    let b = l_bracket();
    let m = model(&b);
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap();
    let p = &u.pattern;
    assert_eq!(p.loops.len(), 1);
    assert!(p.loops[0].outer);
    // Two runs and the strip along the long sides, one end each.
    assert_eq!(p.loops[0].segments.len(), 8);
    let length = 0.035 + developed(FRAC_PI_2, 0.5);
    let area = p.loops[0].area();
    assert!((area - length * W).abs() <= 1e-15, "{area}");
    assert_eq!(p.bend_lines.len(), 1);
    let line = &p.bend_lines[0];
    assert_eq!(line.bend, m.bends[0].name);
    assert!(close(line.start.distance(line.end), W));
    assert!(close(line.length, developed(FRAC_PI_2, 0.5)));
    assert_eq!(line.edges.len(), 2);
    assert_eq!(line.direction, BendDirection::Down);
    // Consecutive segments meet.
    let segs = &p.loops[0].segments;
    for (i, s) in segs.iter().enumerate() {
        let n = &segs[(i + 1) % segs.len()];
        assert!(s.curve.point(s.t1).distance(n.curve.point(n.t0)) < 1e-12);
    }
}

/// `a` with `tool` united into it, or taken from it.
fn combined(a: &Body, tool: &Body, op: crate::boolean::BooleanOp) -> Body {
    let tool = crate::boolean::Tool {
        key: crate::name::ToolKey(1),
        body: tool,
        place: crate::geom::Rigid::IDENTITY,
    };
    crate::boolean::boolean(OpKey(11), op, a, &[tool])
        .unwrap()
        .0
}

/// A block from `lo` to `hi`.
fn block(key: u128, lo: [f64; 3], hi: [f64; 3]) -> Body {
    let place = crate::geom::Rigid::translation(Vector3::new(lo[0], lo[1], lo[2]));
    crate::primitive::block(
        OpKey(key),
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2],
        &place,
    )
    .unwrap()
    .0
}

/// The names of a body's cylinder faces, inner (concave) or outer, sorted.
fn cylinders(b: &Body, inner: bool) -> Vec<crate::name::Name> {
    let mut out: Vec<_> = b
        .faces()
        .filter(|&f| {
            matches!(
                b.surfaces[b.faces[f].surface],
                crate::geom::Surface::Cylinder(_)
            ) && (b.faces[f].sense == crate::topo::Sense::Reversed) == inner
        })
        .map(|f| b.faces[f].name)
        .collect();
    out.sort_unstable();
    out
}

/// The L bracket with a block beside its bend from `z0` to `z1`, on the
/// first flange's plane and running on past the bend line under it: the
/// first flange's inner side runs round the bend's end and meets its outer
/// cylinder where that rises through it.
fn blocked(b: &Body, z0: f64, z1: f64) -> Body {
    let tab = block(12, [0.0, 0.0, z0], [0.04, T, z1]);
    combined(b, &tab, crate::boolean::BooleanOp::Unite)
}

#[test]
fn material_beside_a_bend_blocks_it() {
    let b = blocked(&l_bracket(), 0.5 * W, 2.0 * W);
    let SheetMetalError::BendBlocked { bend, face } = refused(&b) else {
        panic!("expected BendBlocked, got {:?}", refused(&b));
    };
    let all: Vec<_> = cylinders(&b, true)
        .into_iter()
        .chain(cylinders(&b, false))
        .collect();
    assert_eq!(Some(bend), all.iter().copied().min());
    // The face is the first flange's inner side, on y = T facing +y.
    let f = b.faces().find(|&f| b.faces[f].name == face).unwrap();
    let p = super::shape::flat(&b, f).expect("a planar face");
    assert!(p.normal.get().y > 1.0 - 1e-12 && close(p.origin.y, T));
}

/// A bend in two pieces either side of a relief slot, blocked beside each
/// piece in turn: the refusal names the bend (its least face over both
/// pieces) whichever piece is blocked. Both pieces share the outer faces,
/// so either block blocks both, and the first piece found is the one
/// named: the slot's key is chosen for a bend named by the other piece's
/// inner face.
#[test]
fn a_blocked_piece_is_named_as_its_bend() {
    let (relieved, bend) = (13..64)
        .find_map(|key| {
            let slot = block(key, [0.015, -0.01, 0.4 * W], [0.03, 0.008, 0.6 * W]);
            let b = combined(&l_bracket(), &slot, crate::boolean::BooleanOp::Subtract);
            let m = model(&b);
            assert_eq!(m.bends.len(), 1);
            assert_eq!(m.bends[0].inner.len(), 2);
            let bend = m.bends[0].name;
            // Pieces are found, and so judged blocked, in face order.
            let last = b
                .faces()
                .filter(|&f| {
                    matches!(
                        b.surfaces[b.faces[f].surface],
                        crate::geom::Surface::Cylinder(_)
                    ) && b.faces[f].sense == crate::topo::Sense::Reversed
                })
                .last()
                .map(|f| b.faces[f].name);
            (last == Some(bend)).then_some((b, bend))
        })
        .expect("a slot whose bend is named by its last inner face");
    for (z0, z1) in [(0.8 * W, 2.0 * W), (-W, 0.2 * W)] {
        let b = blocked(&relieved, z0, z1);
        match refused(&b) {
            SheetMetalError::BendBlocked { bend: named, .. } => {
                assert_eq!(named, bend, "blocked from {z0} to {z1}");
            }
            other => panic!("expected BendBlocked, got {other:?}"),
        }
    }
}

#[test]
fn bends_of_different_thickness_are_mixed() {
    // Two brackets apart, one twice as thick: thickness is judged before
    // whether the flanges join up.
    let thick = strip(
        &[
            Leg::Run(0.02),
            Leg::Turn {
                angle: FRAC_PI_2,
                radius: R,
            },
            Leg::Run(0.015),
        ],
        2.0 * T,
        W,
    );
    let apart = thick
        .transformed(&crate::geom::Rigid::translation(Vector3::new(
            0.0,
            0.0,
            2.0 * W,
        )))
        .unwrap();
    let b = combined(&l_bracket(), &apart, crate::boolean::BooleanOp::Unite);
    assert_eq!(
        refused(&b),
        SheetMetalError::MixedThickness {
            faces: cylinders(&b, true).try_into().unwrap()
        }
    );
}

#[test]
fn a_bend_past_half_a_turn_is_not_sheet() {
    let b = strip(
        &[
            Leg::Run(0.02),
            Leg::Turn {
                angle: PI * 200.0 / 180.0,
                radius: R,
            },
            Leg::Run(0.005),
        ],
        T,
        W,
    );
    assert_eq!(
        refused(&b),
        SheetMetalError::NotSheet {
            face: cylinders(&b, true)[0]
        }
    );
}

#[test]
fn a_body_with_no_opposite_planes_is_not_sheet() {
    let ball = crate::primitive::sphere(OpKey(14), 0.01, &crate::geom::Rigid::IDENTITY)
        .unwrap()
        .0;
    let face = ball.faces().map(|f| ball.faces[f].name).min().unwrap();
    assert_eq!(refused(&ball), SheetMetalError::NotSheet { face });
}

/// Each group of a flat moves back to where its faces were folded, and
/// each strip's faces stay in its strip.
#[test]
fn a_flat_splits_into_strips_and_groups_that_move_back() {
    let turn = |angle| Leg::Turn { angle, radius: R };
    for legs in [
        &[Leg::Run(0.02), turn(FRAC_PI_2), Leg::Run(0.015)][..],
        &[
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.02),
            turn(-FRAC_PI_2),
            Leg::Run(0.01),
        ][..],
    ] {
        let b = strip(legs, T, W);
        let m = model(&b);
        let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap();
        let folds = vec![true; u.map.strips.len()];
        let r = regions::classify(&u.body, &u.map, &folds, DEFAULT_TOLERANCE).unwrap();
        assert_eq!(r.motions.len(), m.flanges.len());
        for (k, bend) in m.bends.iter().enumerate() {
            for n in bend.inner.iter().chain(&bend.outer) {
                let Some(crate::topo::EntityId::Face(f)) = u.body.by_name(*n) else {
                    panic!("a bend face");
                };
                let s = u
                    .map
                    .strips
                    .iter()
                    .position(|s| s.bend == bend.name)
                    .unwrap();
                assert_eq!(r.faces[&f], regions::Region::Strip(s), "bend {k}");
            }
        }
        // The two end faces run across every strip.
        let across = r
            .faces
            .values()
            .filter(|&&x| x == regions::Region::Across)
            .count();
        assert_eq!(across, 2);
        for v in u.body.vertices() {
            if let regions::Region::Group(g) = r.vertices[&v] {
                let name = u.body.vertices[v].name;
                let Some(crate::topo::EntityId::Vertex(w)) = b.by_name(name) else {
                    panic!("the folded vertex");
                };
                let back = r.motions[g].apply_point(u.body.vertices[v].point);
                assert!(back.distance(b.vertices[w].point) < 1e-12);
            }
        }
    }
}

/// Folding one strip of two moves the flange between them back, and
/// carries the other strip with the flange beyond it.
#[test]
fn folding_one_strip_carries_the_rest_rigidly() {
    let turn = |angle| Leg::Turn { angle, radius: R };
    let b = strip(
        &[
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.02),
            turn(FRAC_PI_2),
            Leg::Run(0.01),
        ],
        T,
        W,
    );
    let m = model(&b);
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap();
    let r = regions::classify(&u.body, &u.map, &[true, false], DEFAULT_TOLERANCE).unwrap();
    assert_eq!(r.motions.len(), 2);
    assert!(r.axes[0].is_some() && r.axes[1].is_none());
    let second = &u.map.strips[1];
    for n in m.bend(second.bend).unwrap().inner.iter() {
        let Some(crate::topo::EntityId::Face(f)) = u.body.by_name(*n) else {
            panic!("a bend face");
        };
        assert!(matches!(r.faces[&f], regions::Region::Group(_)));
    }
    // The middle flange's vertices go back where they were folded.
    let middle = m
        .flanges
        .iter()
        .position(|f| f.name == u.map.strips[0].far)
        .unwrap();
    for n in &m.flanges[middle].sides[0].faces {
        let Some(crate::topo::EntityId::Face(f)) = u.body.by_name(*n) else {
            panic!("a side");
        };
        let regions::Region::Group(g) = r.faces[&f] else {
            panic!("in a group")
        };
        for e in shape::edges_of(&u.body, f) {
            let v = u.body.edges[e].start;
            let Some(crate::topo::EntityId::Vertex(w)) = b.by_name(u.body.vertices[v].name) else {
                panic!("the folded vertex");
            };
            if !matches!(r.vertices[&v], regions::Region::Group(_)) {
                continue;
            }
            let back = r.motions[g].apply_point(u.body.vertices[v].point);
            assert!(back.distance(b.vertices[w].point) < 1e-12);
        }
    }
}

/// A bracket's flat, unfolded with a K-factor of a half, cut by `tool`
/// placed in its strip's frame at `(x, y)` of the strip, turned by `turn`
/// about the strip's `z`.
pub(crate) fn cut_flat(
    tool: &Body,
    x: f64,
    y: f64,
    turn: f64,
) -> (Body, FlatMap, SheetModel, Body) {
    use crate::boolean::{BooleanOp, Tool, boolean};
    use crate::geom::{Frame, Rigid};
    let b = l_bracket();
    let m = model(&b);
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap();
    let s = &u.map.strips[0];
    let at = s.frame.to_world(x, s.span[0] + y, -0.001);
    let along =
        s.frame.x().get() * crate::math::cos(turn) + s.frame.y().get() * crate::math::sin(turn);
    let frame = Frame::new(at, s.frame.z().get(), along).unwrap();
    let (cut, _) = boolean(
        OpKey(98),
        BooleanOp::Subtract,
        &u.body,
        &[Tool {
            key: crate::name::ToolKey(1),
            body: tool,
            place: Rigid::from_frame(&frame),
        }],
    )
    .unwrap();
    (cut, u.map, m, b)
}

/// A round hole on the near bend line, and a slot at thirty degrees
/// across it, imprinted: every face then lies in the strip or beside it.
#[test]
fn imprinting_cuts_walls_at_the_bend_lines() {
    let hole = crate::primitive::cylinder(OpKey(99), 0.0008, 0.003, &crate::geom::Rigid::IDENTITY)
        .unwrap()
        .0;
    let slot = crate::primitive::block(
        OpKey(99),
        0.004,
        0.0012,
        0.003,
        &crate::geom::Rigid::translation(Vector3::new(-0.002, -0.0006, 0.0)),
    )
    .unwrap()
    .0;
    // The hole's wall is cut twice, joining its loops and then splitting
    // it; the slot's two long walls are split once each, its far end
    // lying in the strip.
    for (tool, turn, splits, in_strip) in [(hole, 0.0, 1, 3), (slot, PI / 6.0, 2, 5)] {
        let (cut, map, _, _) = cut_flat(&tool, 0.0, 0.005, turn);
        assert!(regions::classify(&cut, &map, &[true], DEFAULT_TOLERANCE).is_err());
        let mut b = cut.clone();
        let mut journal = crate::name::Journal::new();
        imprint::imprint(
            &mut b,
            K,
            &map.strips,
            &[true],
            map.thickness,
            DEFAULT_TOLERANCE,
            &mut journal,
        )
        .unwrap();
        assert!(crate::check::check_topology(&b).is_empty());
        let r = regions::classify(&b, &map, &[true], DEFAULT_TOLERANCE).unwrap();
        let split = journal
            .events()
            .iter()
            .filter(|e| matches!(e, Event::Split { .. }))
            .count();
        assert_eq!(split, splits);
        let faces = b
            .faces()
            .filter(|f| r.faces[f] == regions::Region::Strip(0))
            .count();
        assert_eq!(faces, in_strip);
    }
}
