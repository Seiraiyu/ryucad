//! Extrude and revolve end to end: profiles to solids that pass the
//! checker, mesh cleanly and enclose the volume they should, with names
//! that survive edits.

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use ryukei::check::check;
use ryukei::geom::{
    Axis, Circle2, Curve2, Ellipse2, Frame, Frame2, Line2, Point2, Point3, Surface, Vector2,
    Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Event, OpKey};
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use ryukei::topo::Body;
use ryukei::{Error, SweepError};
use ryukei_testkit::mesh::volume_and_area;
use std::sync::Arc;

const K: OpKey = OpKey(0x51);

/// Segments with keys counted from `first`.
fn keyed(first: u128, parts: Vec<(Curve2, f64, f64)>) -> Vec<Segment> {
    parts
        .into_iter()
        .enumerate()
        .map(|(i, (curve, t0, t1))| Segment {
            key: SegmentKey(first + i as u128),
            curve,
            t0,
            t1,
        })
        .collect()
}

fn line(a: (f64, f64), b: (f64, f64)) -> (Curve2, f64, f64) {
    let (p, q) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
    (
        Curve2::Line2(Line2::through(p, q).unwrap()),
        0.0,
        p.distance(q),
    )
}

fn arc(c: (f64, f64), r: f64, a0: f64, a1: f64) -> (Curve2, f64, f64) {
    let f = Frame2::new(Point2::new(c.0, c.1), Vector2::new(1.0, 0.0), true).unwrap();
    (Curve2::Circle2(Circle2::new(f, r).unwrap()), a0, a1)
}

/// A closed polygon through `pts`, keyed from `first`.
fn polygon(key: u128, first: u128, pts: &[(f64, f64)]) -> Loop {
    let n = pts.len();
    let parts = (0..n).map(|i| line(pts[i], pts[(i + 1) % n])).collect();
    Loop {
        key: LoopKey(key),
        segments: keyed(first, parts),
    }
}

fn rect(key: u128, first: u128, x: f64, y: f64, w: f64, h: f64) -> Loop {
    polygon(
        key,
        first,
        &[(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
    )
}

fn circle(key: u128, first: u128, c: (f64, f64), r: f64) -> Loop {
    Loop {
        key: LoopKey(key),
        segments: keyed(first, vec![arc(c, r, 0.0, TAU)]),
    }
}

fn frame() -> Frame {
    Frame::new(
        Point3::new(1.0, -2.0, 0.5),
        Vector3::new(0.1, 0.2, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap()
}

fn profile(loops: Vec<Loop>) -> ValidProfile {
    ValidProfile::new(Profile {
        frame: frame(),
        loops,
    })
    .unwrap()
}

/// Checks a body and its mesh, and returns the mesh's volume.
fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    let m = tessellate(b, &MeshParams::new(1e-3, 0.5)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    volume_and_area(&m).0
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

fn surfaces(b: &Body) -> Vec<&'static str> {
    let mut out: Vec<&str> = b
        .faces()
        .map(|f| match b.surface(b.face(f).unwrap().surface).unwrap() {
            Surface::Plane(_) => "plane",
            Surface::Cylinder(_) => "cylinder",
            Surface::Cone(_) => "cone",
            Surface::Sphere(_) => "sphere",
            Surface::Torus(_) => "torus",
            Surface::Extrusion(_) => "extrusion",
            Surface::Revolution(_) => "revolution",
            _ => "nurbs",
        })
        .collect();
    out.sort();
    out
}

fn counts(b: &Body) -> (usize, usize, usize) {
    let c = b.counts();
    (c.vertices, c.edges, c.faces)
}

#[test]
fn a_block_and_a_cylinder_by_extrude() {
    let n = frame().z().get();
    let p = profile(vec![rect(1, 10, 0.0, 0.0, 2.0, 3.0)]);
    let (b, j) = extrude(K, &p, n, 0.0, 1.5).unwrap();
    assert_eq!(counts(&b), (8, 12, 6));
    assert_eq!(surfaces(&b), ["plane"; 6]);
    assert!(close(volume(&b), 9.0, 1e-9));
    assert!(j.events().iter().all(|e| matches!(e, Event::Created(_))));
    assert_eq!(j.events().len(), 26);
    let p = profile(vec![circle(1, 10, (0.0, 0.0), 1.0)]);
    let (b, _) = extrude(K, &p, n, 0.0, 2.0).unwrap();
    // As the cylinder primitive: two circles, no seam.
    assert_eq!(counts(&b), (2, 2, 3));
    assert_eq!(surfaces(&b), ["cylinder", "plane", "plane"]);
    assert!(close(volume(&b), 2.0 * PI, 2e-3));
}

#[test]
fn extrudes_either_way_obliquely_and_from_behind() {
    let n = frame().z().get();
    let p = profile(vec![rect(1, 10, 0.0, 0.0, 2.0, 1.0)]);
    for (dir, start, end) in [
        (n, -1.0, 1.0),
        (-n, 0.0, 1.0),
        (n + frame().x().get(), 0.0, 1.0),
        (-n - frame().y().get() * 0.5, 0.5, 2.0),
    ] {
        let (b, _) = extrude(K, &p, dir, start, end).unwrap();
        let along = dir.dot(n).abs() / dir.length();
        let want = 2.0 * (end - start) * along;
        assert!(close(volume(&b), want, 1e-9), "{dir:?}");
    }
    // An oblique circle is an elliptic cylinder: an extrusion.
    let p = profile(vec![circle(1, 10, (0.0, 0.0), 1.0)]);
    let (b, _) = extrude(K, &p, n + frame().x().get(), 0.0, 1.0).unwrap();
    assert_eq!(surfaces(&b), ["extrusion", "plane", "plane"]);
    assert!(close(
        volume(&b),
        PI * core::f64::consts::FRAC_1_SQRT_2,
        3e-3
    ));
}

#[test]
fn holes_and_lumps() {
    let n = frame().z().get();
    let p = profile(vec![
        rect(1, 10, 0.0, 0.0, 6.0, 4.0),
        circle(2, 20, (1.5, 2.0), 1.0),
        rect(3, 30, 3.5, 1.0, 2.0, 2.0),
        // An island inside the square hole, and a lump of its own.
        rect(4, 40, 4.0, 1.5, 1.0, 1.0),
        circle(5, 50, (10.0, 0.0), 1.0),
    ]);
    let (b, _) = extrude(K, &p, n, 0.0, 1.0).unwrap();
    assert_eq!(b.regions().count(), 4);
    let want = 24.0 - PI - 4.0 + 1.0 + PI;
    assert!(close(volume(&b), want, 3e-3));
}

#[test]
fn a_slot_an_ellipse_and_a_nurbs_blob() {
    let n = frame().z().get();
    let slot = Loop {
        key: LoopKey(1),
        segments: keyed(
            10,
            vec![
                line((0.0, 0.0), (2.0, 0.0)),
                arc((2.0, 1.0), 1.0, -FRAC_PI_2, FRAC_PI_2),
                line((2.0, 2.0), (0.0, 2.0)),
                arc((0.0, 1.0), 1.0, FRAC_PI_2, 3.0 * FRAC_PI_2),
            ],
        ),
    };
    let (b, _) = extrude(K, &profile(vec![slot]), n, 0.0, 1.0).unwrap();
    assert_eq!(counts(&b), (8, 12, 6));
    assert!(close(volume(&b), 4.0 + PI, 3e-3));
    let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 1.0), false).unwrap();
    let ellipse = Curve2::Ellipse2(Ellipse2::new(f, 2.0, 1.0).unwrap());
    let lp = Loop {
        key: LoopKey(1),
        segments: keyed(10, vec![(ellipse, TAU, 0.0)]),
    };
    let (b, _) = extrude(K, &profile(vec![lp]), n, 0.0, 1.0).unwrap();
    assert_eq!(surfaces(&b), ["extrusion", "plane", "plane"]);
    assert!(close(volume(&b), 2.0 * PI, 3e-3));
    let mut g = ryukei_testkit::Lcg::new(7);
    let blob = ryukei_testkit::nurbs::blob2(&mut g, Point2::ORIGIN, 1.0);
    let [a, z] = blob.domain();
    let lp = Loop {
        key: LoopKey(1),
        segments: keyed(10, vec![(Curve2::Nurbs2(Arc::new(blob)), a, z)]),
    };
    let p = profile(vec![lp]);
    let area = p.lumps()[0].outer.area;
    let (b, _) = extrude(K, &p, n, 0.0, 0.5).unwrap();
    assert!(close(volume(&b), area * 0.5, 3e-3));
}

#[test]
fn extrude_refuses_bad_parameters() {
    let p = profile(vec![rect(1, 10, 0.0, 0.0, 1.0, 1.0)]);
    let (x, n) = (frame().x().get(), frame().z().get());
    let sweep = |e| Err(Error::InvalidSweep(e));
    assert_eq!(
        extrude(K, &p, x, 0.0, 1.0).map(|_| ()),
        sweep(SweepError::DirectionInPlane)
    );
    assert_eq!(
        extrude(K, &p, n, 0.0, 5e-8).map(|_| ()),
        sweep(SweepError::ShortSpan)
    );
    assert_eq!(
        extrude(K, &p, n, 1.0, 0.0).map(|_| ()),
        sweep(SweepError::ShortSpan)
    );
    assert_eq!(
        extrude(K, &p, n, 0.0, 600.0).map(|_| ()),
        sweep(SweepError::OutsideSizeBox)
    );
    assert_eq!(
        extrude(K, &p, Vector3::ZERO, 0.0, 1.0).map(|_| ()),
        Err(Error::Degenerate)
    );
    assert_eq!(
        extrude(K, &p, n, f64::NAN, 1.0).map(|_| ()),
        Err(Error::NonFinite)
    );
}

/// The profile frame's y axis through its origin: the revolve axis used
/// below, so profile x is the distance from it.
fn y_axis() -> Axis {
    Axis::new(frame().origin(), frame().y().get()).unwrap()
}

#[test]
fn the_turned_primitives_by_revolve() {
    // A cylinder: a rectangle against the axis.
    let p = profile(vec![rect(1, 10, 0.0, 0.0, 1.0, 2.0)]);
    let (b, _) = revolve(K, &p, y_axis(), 0.0, TAU).unwrap();
    assert_eq!(counts(&b), (2, 2, 3));
    assert_eq!(surfaces(&b), ["cylinder", "plane", "plane"]);
    assert!(close(volume(&b), 2.0 * PI, 2e-3));
    // A cone: a right triangle with a leg on the axis.
    let p = profile(vec![polygon(1, 10, &[(0.0, 0.0), (1.0, 0.0), (0.0, 2.0)])]);
    let (b, _) = revolve(K, &p, y_axis(), 0.0, TAU).unwrap();
    assert_eq!(counts(&b), (2, 1, 2));
    assert_eq!(surfaces(&b), ["cone", "plane"]);
    assert!(close(volume(&b), 2.0 * PI / 3.0, 2e-3));
    // A frustum.
    let p = profile(vec![polygon(
        1,
        10,
        &[(0.0, 0.0), (1.0, 0.0), (0.5, 2.0), (0.0, 2.0)],
    )]);
    let (b, _) = revolve(K, &p, y_axis(), 0.0, TAU).unwrap();
    assert_eq!(counts(&b), (2, 2, 3));
    assert!(close(volume(&b), PI * 2.0 / 3.0 * (1.0 + 0.5 + 0.25), 2e-3));
    // A sphere: a half disc, one face with no vertices.
    let half = Loop {
        key: LoopKey(1),
        segments: keyed(
            10,
            vec![
                arc((0.0, 0.0), 1.0, -FRAC_PI_2, FRAC_PI_2),
                line((0.0, 1.0), (0.0, -1.0)),
            ],
        ),
    };
    let (b, _) = revolve(K, &profile(vec![half]), y_axis(), 0.0, TAU).unwrap();
    assert_eq!(counts(&b), (0, 0, 1));
    assert_eq!(surfaces(&b), ["sphere"]);
    assert!(close(volume(&b), 4.0 / 3.0 * PI, 2e-3));
    // A torus: a circle off the axis.
    let p = profile(vec![circle(1, 10, (3.0, 0.0), 1.0)]);
    let (b, _) = revolve(K, &p, y_axis(), 0.0, TAU).unwrap();
    assert_eq!(counts(&b), (0, 0, 1));
    assert_eq!(surfaces(&b), ["torus"]);
    assert!(close(volume(&b), 6.0 * PI * PI, 2e-3));
}

#[test]
fn partial_revolves() {
    // A quarter turn of a rectangle off the axis.
    let p = profile(vec![rect(1, 10, 1.0, 0.0, 1.0, 2.0)]);
    let (b, _) = revolve(K, &p, y_axis(), 0.0, FRAC_PI_2).unwrap();
    assert_eq!(counts(&b), (8, 12, 6));
    assert!(close(volume(&b), FRAC_PI_2 / 2.0 * 3.0 * 2.0, 2e-3));
    // Against the axis: the axis edge is shared by the caps.
    let p = profile(vec![rect(1, 10, 0.0, 0.0, 1.0, 2.0)]);
    let (b, _) = revolve(K, &p, y_axis(), -0.5, 1.0).unwrap();
    assert_eq!(counts(&b), (6, 9, 5));
    assert!(close(volume(&b), 1.5 / 2.0 * 2.0, 2e-3));
}

fn half_disc() -> ValidProfile {
    profile(vec![Loop {
        key: LoopKey(1),
        segments: keyed(
            10,
            vec![
                arc((0.0, 0.0), 1.0, -FRAC_PI_2, FRAC_PI_2),
                line((0.0, 1.0), (0.0, -1.0)),
            ],
        ),
    }])
}

#[test]
fn a_wedge_of_a_sphere() {
    // Three quarters of a turn: the sphere's face runs between two
    // meridians that meet at the poles, on the axis edge.
    let (b, _) = revolve(K, &half_disc(), y_axis(), 0.0, 1.5 * PI).unwrap();
    assert_eq!(counts(&b), (2, 3, 3));
    assert!(close(volume(&b), 4.0 / 3.0 * PI * 0.75, 2e-3));
}

#[test]
fn half_turns_and_more() {
    // Lateral arcs of half a turn and more, round cylinders and planes.
    let p = profile(vec![rect(1, 10, 1.0, 0.0, 2.0, 2.0)]);
    for span in [PI, 3.5, 5.0] {
        let (b, _) = revolve(K, &p, y_axis(), 0.0, span).unwrap();
        let want = span / 2.0 * (9.0 - 1.0) * 2.0;
        assert!(close(volume(&b), want, 2e-3), "{span}");
    }
}

#[test]
fn torus_bands() {
    // A circular hole through a part turn: a band of a torus between two
    // circles, open across the tube's turn.
    let p = profile(vec![
        rect(1, 10, 1.0, 0.0, 2.0, 2.0),
        circle(2, 20, (2.0, 1.0), 0.5),
    ]);
    for span in [1.0, PI, 5.0] {
        let (b, _) = revolve(K, &p, y_axis(), 0.0, span).unwrap();
        let want = span / 2.0 * (9.0 - 1.0) * 2.0 - PI * 0.25 * 2.0 * span;
        assert!(close(volume(&b), want, 2e-3), "{span}");
    }
    // A slot turned whole: bands open along the tube, between circles.
    let (big, r, l) = (1.5, 0.25, 0.5);
    let slot = Loop {
        key: LoopKey(1),
        segments: keyed(
            10,
            vec![
                line((big + r, 0.0), (big + r, l)),
                arc((big, l), r, 0.0, PI),
                line((big - r, l), (big - r, 0.0)),
                arc((big, 0.0), r, PI, TAU),
            ],
        ),
    };
    let (b, _) = revolve(K, &profile(vec![slot]), y_axis(), 0.0, TAU).unwrap();
    let area = 2.0 * r * l + PI * r * r;
    assert!(close(volume(&b), area * TAU * big, 2e-3));
}

#[test]
fn thin_tubes_and_tiny_blobs() {
    // A slot turned whole: tori whose tubes are not much thicker than the
    // chord, whose bands ask for their circles again and again.
    let (big, r, l) = (1.5, 0.08, 0.5);
    let slot = Loop {
        key: LoopKey(1),
        segments: keyed(
            10,
            vec![
                line((big + r, 0.0), (big + r, l)),
                arc((big, l), r, 0.0, PI),
                line((big - r, l), (big - r, 0.0)),
                arc((big, 0.0), r, PI, TAU),
            ],
        ),
    };
    let p = profile(vec![slot]);
    let (b, _) = revolve(K, &p, y_axis(), 0.0, TAU).unwrap();
    let m = tessellate(&b, &MeshParams::new(0.05, 0.5)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    let area = 2.0 * r * l + PI * r * r;
    assert!(close(volume_and_area(&m).0, area * TAU * big, 0.05));
    // A small NURBS blob extruded obliquely: its side asks for boundary
    // samples somewhere new every round until its edges are halved.
    let mut g = ryukei_testkit::Lcg::new(28);
    let blob = ryukei_testkit::nurbs::blob2(&mut g, Point2::new(-1.6, -2.8), 0.02);
    let [a, z] = blob.domain();
    let lp = Loop {
        key: LoopKey(1),
        segments: keyed(10, vec![(Curve2::Nurbs2(Arc::new(blob)), a, z)]),
    };
    let p = profile(vec![lp]);
    let dir = frame().z().get() + frame().x().get() * 0.7;
    let (b, _) = extrude(K, &p, dir, 0.0, 1.0).unwrap();
    let m = tessellate(&b, &MeshParams::new(0.0045, 0.5)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    let along = 1.0 / dir.length();
    let want = p.lumps()[0].outer.area * along;
    assert!(close(volume_and_area(&m).0, want, 5e-3));
}

#[test]
fn revolve_refuses_bad_profiles_and_parameters() {
    let sweep = |e| Err(Error::InvalidSweep(e));
    let crossing = profile(vec![rect(1, 10, -1.0, 0.0, 2.0, 1.0)]);
    assert!(matches!(
        revolve(K, &crossing, y_axis(), 0.0, 1.0).map(|_| ()),
        Err(Error::InvalidSweep(SweepError::CrossesAxis { .. }))
    ));
    // A circle touching the axis in its middle.
    let touching = profile(vec![circle(1, 10, (1.0, 0.0), 1.0)]);
    assert_eq!(
        revolve(K, &touching, y_axis(), 0.0, 1.0).map(|_| ()),
        sweep(SweepError::TouchesAxisInside {
            segment: SegmentKey(10)
        })
    );
    // A triangle touching the axis at a corner, turned whole.
    let pinched = profile(vec![polygon(1, 10, &[(0.0, 0.0), (1.0, -1.0), (1.0, 1.0)])]);
    assert_eq!(
        revolve(K, &pinched, y_axis(), 0.0, TAU).map(|_| ()),
        sweep(SweepError::PinchedOnAxis {
            segment: SegmentKey(10)
        })
    );
    // ...but a part turn of it is a solid.
    let (b, _) = revolve(K, &pinched, y_axis(), 0.0, 1.0).unwrap();
    assert!(volume(&b) > 0.0);

    let p = profile(vec![rect(1, 10, 1.0, 0.0, 1.0, 1.0)]);
    let off = Axis::new(frame().origin() + frame().z().get(), frame().y().get()).unwrap();
    assert_eq!(
        revolve(K, &p, off, 0.0, 1.0).map(|_| ()),
        sweep(SweepError::AxisNotInPlane)
    );
    let tilted = Axis::new(frame().origin(), frame().y().get() + frame().z().get()).unwrap();
    assert_eq!(
        revolve(K, &p, tilted, 0.0, 1.0).map(|_| ()),
        sweep(SweepError::AxisNotInPlane)
    );
    assert_eq!(
        revolve(K, &p, y_axis(), 1.0, 1.0).map(|_| ()),
        sweep(SweepError::BadAngle)
    );
    assert_eq!(
        revolve(K, &p, y_axis(), 0.0, 7.0).map(|_| ()),
        sweep(SweepError::BadAngle)
    );
    assert_eq!(
        revolve(K, &p, y_axis(), 0.0, 1e-9).map(|_| ()),
        sweep(SweepError::ShortSpan)
    );
}

#[test]
fn revolutions_where_no_analytic_surface_fits() {
    // A full ellipse off the axis: a loopless, doubly periodic face.
    let f = Frame2::new(Point2::new(3.0, 0.0), Vector2::new(0.0, 1.0), true).unwrap();
    let ellipse = Curve2::Ellipse2(Ellipse2::new(f, 2.0, 1.0).unwrap());
    let lp = Loop {
        key: LoopKey(1),
        segments: keyed(10, vec![(ellipse.clone(), 0.0, TAU)]),
    };
    let (b, _) = revolve(K, &profile(vec![lp]), y_axis(), 0.0, TAU).unwrap();
    assert_eq!((counts(&b), surfaces(&b)), ((0, 0, 1), vec!["revolution"]));
    assert!(close(volume(&b), 2.0 * PI * 3.0 * 2.0 * PI, 3e-3));
    // Half an ellipse against the axis: poles at both ends.
    let f = Frame2::new(Point2::ORIGIN, Vector2::new(0.0, 1.0), true).unwrap();
    let half = Curve2::Ellipse2(Ellipse2::new(f, 2.0, 1.0).unwrap());
    let lp = Loop {
        key: LoopKey(1),
        segments: keyed(10, vec![(half, PI, TAU), line((0.0, 2.0), (0.0, -2.0))]),
    };
    let (b, _) = revolve(K, &profile(vec![lp]), y_axis(), 0.0, TAU).unwrap();
    assert_eq!(surfaces(&b), ["revolution"]);
    assert!(close(volume(&b), 4.0 / 3.0 * PI * 2.0, 3e-3));
}

/// The names of a body's faces, edges and vertices, sorted.
fn names(b: &Body) -> Vec<u128> {
    let mut out: Vec<u128> = b
        .faces()
        .map(|f| b.face(f).unwrap().name)
        .chain(b.edges().map(|e| b.edge(e).unwrap().name))
        .chain(b.vertices().map(|v| b.vertex(v).unwrap().name))
        .map(|n| n.to_u128())
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn names_survive_edits() {
    let n = frame().z().get();
    let build = |w: f64, h: f64, r: f64, depth: f64| {
        let p = profile(vec![
            rect(1, 10, 0.0, 0.0, w, h),
            circle(2, 20, (w / 2.0, h / 2.0), r),
        ]);
        extrude(K, &p, n, 0.0, depth).unwrap().0
    };
    let base = names(&build(4.0, 3.0, 1.0, 1.0));
    assert_eq!(names(&build(5.0, 2.5, 0.5, 2.0)), base);
    // Starting the loop at another segment renames nothing.
    let mut lp = rect(1, 10, 0.0, 0.0, 4.0, 3.0);
    lp.segments.rotate_left(2);
    let p = profile(vec![lp, circle(2, 20, (2.0, 1.5), 1.0)]);
    assert_eq!(names(&extrude(K, &p, n, 0.0, 1.0).unwrap().0), base);
    // Another key renames everything.
    let p = profile(vec![
        rect(1, 10, 0.0, 0.0, 4.0, 3.0),
        circle(2, 20, (2.0, 1.5), 1.0),
    ]);
    let other = names(&extrude(OpKey(9), &p, n, 0.0, 1.0).unwrap().0);
    assert!(other.iter().all(|x| !base.contains(x)));
    // Splitting one edge of the rectangle in two renames only what was at
    // the new vertex's place: the lateral there and its two vertices, which
    // now sit between the new segment and the next.
    let split = Loop {
        key: LoopKey(1),
        segments: keyed(
            10,
            vec![
                line((0.0, 0.0), (4.0, 0.0)),
                line((4.0, 0.0), (4.0, 3.0)),
                line((4.0, 3.0), (2.0, 3.0)),
                line((0.0, 3.0), (0.0, 0.0)),
            ],
        ),
    };
    let mut split = split;
    split
        .segments
        .insert(3, keyed(99, vec![line((2.0, 3.0), (0.0, 3.0))]).remove(0));
    let p = profile(vec![split, circle(2, 20, (2.0, 1.5), 1.0)]);
    let after = names(&extrude(K, &p, n, 0.0, 1.0).unwrap().0);
    let kept = base.iter().filter(|x| after.contains(x)).count();
    assert_eq!(kept, base.len() - 3, "{kept} of {}", base.len());
}

#[test]
fn revolve_names_survive_edits() {
    let build = |r: f64, h: f64, angle: f64| {
        let p = profile(vec![rect(1, 10, 0.0, 0.0, r, h)]);
        revolve(K, &p, y_axis(), 0.0, angle).unwrap().0
    };
    assert_eq!(names(&build(1.0, 2.0, TAU)), names(&build(2.0, 0.5, TAU)));
    assert_eq!(names(&build(1.0, 2.0, 1.0)), names(&build(2.0, 0.5, 2.5)));
}

#[test]
fn full_turns_that_enclose_a_void_hollow_it_out() {
    // A ring with a round hole: a torus-shaped cavity, by Pappus 16π less
    // π².
    let ring = profile(vec![
        rect(1, 10, 1.0, 0.0, 2.0, 2.0),
        circle(2, 20, (2.0, 1.0), 0.5),
    ]);
    let (b, j) = revolve(K, &ring, y_axis(), 0.0, TAU).unwrap();
    let solid = b.regions().nth(1).unwrap();
    assert_eq!(b.region_shells(solid).count(), 2);
    assert_eq!(b.regions().count(), 2);
    let v = volume(&b);
    assert!(close(v, 16.0 * PI - PI * PI, 1e-3), "{v}");
    assert!(j.events().iter().all(|e| matches!(e, Event::Created(_))));
    // A notch in a profile against the axis: a cylinder of radius 2 and
    // height 3 with a cylindrical cavity of radius 1 and height 1.
    let notch = profile(vec![polygon(
        1,
        10,
        &[
            (0.0, 0.0),
            (2.0, 0.0),
            (2.0, 3.0),
            (0.0, 3.0),
            (0.0, 2.0),
            (1.0, 2.0),
            (1.0, 1.0),
            (0.0, 1.0),
        ],
    )]);
    let (b, _) = revolve(K, &notch, y_axis(), 0.0, TAU).unwrap();
    let solid = b.regions().nth(1).unwrap();
    assert_eq!(b.region_shells(solid).count(), 2);
    let v = volume(&b);
    assert!(close(v, 11.0 * PI, 1e-3), "{v}");
    // Turned the other way round the axis: the same solid.
    let back = Axis::new(frame().origin(), -frame().y().get()).unwrap();
    let (b, _) = revolve(K, &notch, back, 0.0, TAU).unwrap();
    assert!(close(volume(&b), 11.0 * PI, 1e-3));
}
