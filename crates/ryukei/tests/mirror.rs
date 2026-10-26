//! Mirror images: every body ryukei builds, reflected in a plane, passes
//! the checker and the self-intersection check, meshes without faults,
//! encloses the volume it did and keeps every name; reflected twice, it
//! is back where it was.

use core::f64::consts::{PI, TAU};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{Fault, check, check_intersections};
use ryukei::geom::{
    Axis, Circle2, Curve2, Ellipse2, Frame, Frame2, Line2, Plane, Point2, Point3, Rigid, Surface,
    Vector2, Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use ryukei::topo::{Body, EntityId};

/// The volume a sound body's mesh encloses, at a chord of `CHORD`.
fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    let m = tessellate(b, &MeshParams::new(CHORD, 0.3)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

/// The meshes' chord. A body and its image are meshed in their own
/// parameter planes, so their volumes agree only to the meshes' error.
const CHORD: f64 = 1e-3;

/// An oblique plane through the point `(0.3, -0.2, 0.1)`.
fn oblique() -> Plane {
    Plane::new(
        Frame::new(
            Point3::new(0.3, -0.2, 0.1),
            Vector3::new(1.0, 0.4, -0.7),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    )
}

/// The world's yz plane.
fn yz() -> Plane {
    Plane::new(
        Frame::new(
            Point3::ORIGIN,
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    )
}

/// The faults of the self-intersection check.
fn crossings(b: &Body) -> Vec<Fault> {
    check_intersections(b)
}

/// Mirrors `b` in `plane` and checks the image: sound, the same volume
/// and names, every vertex at its reflection, and back where it was when
/// mirrored again.
fn mirrors(b: &Body, plane: &Plane) {
    let m = b.mirrored(plane).unwrap();
    let (before, after) = (volume(b), volume(&m));
    assert!(
        (before - after).abs() <= 1e-3 * before.abs().max(1.0),
        "{before} {after}"
    );
    assert!(after > 0.0);
    assert_eq!(crossings(&m), crossings(b));
    assert_eq!(m.counts(), b.counts());
    let names = |x: &Body| {
        let faces = x.faces().map(EntityId::Face);
        let edges = x.edges().map(EntityId::Edge);
        let vertices = x.vertices().map(EntityId::Vertex);
        faces
            .chain(edges)
            .chain(vertices)
            .map(|e| x.name_of(e).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&m), names(b));
    let f = plane.frame();
    let reflect = |p: Point3| p - f.z().get() * (2.0 * (p - f.origin()).dot(f.z().get()));
    for v in b.vertices() {
        let (p, q) = (b.vertex(v).unwrap().point, m.vertex(v).unwrap().point);
        assert!(q.distance(reflect(p)) < 1e-12, "{p:?} {q:?}");
    }
    let back = m.mirrored(plane).unwrap();
    assert_eq!(check(&back), []);
    for v in b.vertices() {
        let (p, q) = (b.vertex(v).unwrap().point, back.vertex(v).unwrap().point);
        assert!(q.distance(p) < 1e-12);
    }
}

#[test]
fn primitives() {
    let at = Rigid::translation(Vector3::new(0.5, 1.0, -0.3)).then_after(&Rigid::rotation(
        Vector3::new(1.0, -2.0, 0.5).normalize().unwrap(),
        1.1,
    ));
    let k = OpKey(1);
    for b in [
        primitive::block(k, 1.0, 2.0, 3.0, &at).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &at).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &at).unwrap().0,
        primitive::frustum(k, 1.0, 0.4, 2.0, &at).unwrap().0,
        primitive::sphere(k, 1.0, &at).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &at).unwrap().0,
    ] {
        mirrors(&b, &oblique());
    }
    let b = primitive::block(k, 1.0, 2.0, 3.0, &at).unwrap().0;
    mirrors(&b, &yz());
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

fn closed(key: u128, parts: Vec<(Curve2, f64, f64)>) -> Loop {
    Loop {
        key: LoopKey(key),
        segments: parts
            .into_iter()
            .enumerate()
            .map(|(i, (curve, t0, t1))| Segment {
                key: SegmentKey(key * 100 + i as u128),
                curve,
                t0,
                t1,
            })
            .collect(),
    }
}

fn profile(frame: Frame, loops: Vec<Loop>) -> ValidProfile {
    ValidProfile::new(Profile { frame, loops }).unwrap()
}

#[test]
fn sweeps() {
    let frame = Frame::new(
        Point3::new(1.0, -2.0, 0.5),
        Vector3::new(0.1, 0.2, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    // A slot with a round hole, extruded.
    let slot = closed(
        1,
        vec![
            line((0.0, 0.0), (2.0, 0.0)),
            arc((2.0, 0.5), 0.5, -PI / 2.0, PI / 2.0),
            line((2.0, 1.0), (0.0, 1.0)),
            arc((0.0, 0.5), 0.5, PI / 2.0, 1.5 * PI),
        ],
    );
    let hole = closed(2, vec![arc((1.0, 0.5), 0.25, 0.0, TAU)]);
    let p = profile(frame, vec![slot, hole]);
    let n = frame.z().get();
    let b = extrude(OpKey(2), &p, n, 0.0, 0.7).unwrap().0;
    mirrors(&b, &oblique());
    // A drilled hole's tool: a bore and a drill point, turned all round.
    let bore = closed(
        3,
        vec![
            line((0.0, 0.0), (0.3, 0.0)),
            line((0.3, 0.0), (0.3, 1.0)),
            line((0.3, 1.0), (0.0, 1.2)),
            line((0.0, 1.2), (0.0, 0.0)),
        ],
    );
    let p = profile(frame, vec![bore]);
    let axis = Axis::new(frame.origin(), frame.y().get()).unwrap();
    let b = revolve(OpKey(3), &p, axis, 0.0, TAU).unwrap().0;
    mirrors(&b, &oblique());
    // A partial turn of a half disc and a ring: sphere, torus and caps.
    let half = closed(
        4,
        vec![
            arc((0.0, 0.0), 0.5, -PI / 2.0, PI / 2.0),
            line((0.0, 0.5), (0.0, -0.5)),
        ],
    );
    let ring = closed(5, vec![arc((1.5, 0.0), 0.3, 0.0, TAU)]);
    let p = profile(frame, vec![half, ring]);
    let b = revolve(OpKey(4), &p, axis, 0.0, 2.0).unwrap().0;
    mirrors(&b, &oblique());
}

#[test]
fn an_extruded_ellipse() {
    // An ellipse along its plane's normal is an extrusion: a surface that
    // keeps its parameters, so its face flips its sense.
    let f = Frame2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.3), true).unwrap();
    let e = Curve2::Ellipse2(Ellipse2::new(f, 1.0, 0.4).unwrap());
    let p = profile(Frame::WORLD, vec![closed(6, vec![(e, 0.0, TAU)])]);
    let b = extrude(OpKey(5), &p, Vector3::new(0.0, 0.3, 1.0), 0.0, 1.0)
        .unwrap()
        .0;
    assert!(b.faces().any(|x| matches!(
        b.surface(b.face(x).unwrap().surface),
        Some(Surface::Extrusion(_))
    )));
    mirrors(&b, &oblique());
}

#[test]
fn booleans_with_traced_curves() {
    // Two crossed cylinders of different radii meet in traced curves,
    // fitted as NURBS with NURBS pcurves on both.
    let main = primitive::cylinder(OpKey(6), 1.0, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let bore = primitive::cylinder(OpKey(7), 0.4, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let x = Vector3::new(1.0, 0.0, 0.0).normalize().unwrap();
    let place =
        Rigid::translation(Vector3::new(0.0, 2.0, 2.0)).then_after(&Rigid::rotation(x, PI / 2.0));
    let tool = Tool {
        key: ToolKey(1),
        body: &bore,
        place,
    };
    let b = boolean(OpKey(8), BooleanOp::Subtract, &main, &[tool])
        .unwrap()
        .0;
    assert!(b.edges().any(|e| matches!(
        b.curve(b.edge(e).unwrap().curve),
        Some(ryukei::geom::Curve::Nurbs(_))
    )));
    assert_eq!(crossings(&b), []);
    mirrors(&b, &oblique());
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_pipe_fillet() {
    use ryukei::blend::{BlendKind, EdgeBlend, blend};
    use ryukei::geom::Curve;
    // A cylinder cut off askew has an elliptic rim; its fillet is a pipe.
    let cyl = primitive::cylinder(OpKey(1), 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let cutter = primitive::block(OpKey(2), 6.0, 6.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let x = Vector3::new(1.0, 0.0, 0.0).normalize().unwrap();
    let place = Rigid::translation(Vector3::new(0.0, 0.0, 1.5))
        .then_after(&Rigid::rotation(x, 0.3))
        .then_after(&Rigid::translation(Vector3::new(-3.0, -3.0, 0.0)));
    let tool = Tool {
        key: ToolKey(1),
        body: &cutter,
        place,
    };
    let b = boolean(OpKey(3), BooleanOp::Subtract, &cyl, &[tool])
        .unwrap()
        .0;
    let rim = b
        .edges()
        .find(|&e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Ellipse(_))))
        .unwrap();
    let edge = EdgeBlend {
        edge: b.edge(rim).unwrap().name,
        kind: BlendKind::Fillet { radius: 0.2 },
    };
    let (r, _) = blend(OpKey(4), &b, &[edge]).unwrap();
    assert!(r.faces().any(|f| matches!(
        r.surface(r.face(f).unwrap().surface),
        Some(Surface::Pipe(_))
    )));
    mirrors(&r, &yz());
}

#[test]
fn leaving_the_size_box_is_refused() {
    let at = Rigid::translation(Vector3::new(495.0, 0.0, 0.0));
    let b = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &at).unwrap().0;
    let far = Plane::new(
        Frame::new(
            Point3::new(-3.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    );
    // Reflected about x = -3 it would sit at x = -501.
    assert_eq!(b.mirrored(&far).err(), Some(ryukei::Error::OutsideSizeBox));
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn seeded_sweeps_booleans_and_blends() {
    use ryukei::blend::blend;
    use ryukei_testkit::Lcg;
    use ryukei_testkit::blend_bytes::blend_case;
    use ryukei_testkit::boolean_bytes::boolean_case;
    use ryukei_testkit::sweep_bytes::sweep;
    let bytes = |seed: u64| {
        let mut g = Lcg::new(seed);
        (0..96)
            .map(|_| (g.next_u64() >> 56) as u8)
            .collect::<Vec<u8>>()
    };
    let mut n = 0;
    for seed in 0..24 {
        let mut bodies = Vec::new();
        if let Some((b, _)) = sweep(&bytes(seed)) {
            bodies.push(b);
        }
        if let Some(case) = boolean_case(&bytes(seed))
            && let Ok((b, _)) = boolean(OpKey(9), case.op, &case.target, &case.tools())
            && b.counts().faces > 0
        {
            bodies.push(b);
        }
        if let Some(case) = blend_case(&bytes(seed))
            && let Ok((b, _)) = blend(OpKey(9), &case.body, &case.blends)
        {
            bodies.push(b);
        }
        for b in &bodies {
            mirrors(b, &oblique());
            n += 1;
        }
    }
    assert!(n >= 30, "{n} bodies");
}
