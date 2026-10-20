//! Bodies with pipe faces: a closed tube and a bent cylinder pass the
//! checker, mesh without faults, and enclose the volume and area Pappus's
//! theorems give: `π r² L` and `2 π r L` for a spine of length `L`.

use ryukei::check::{Fault, check, check_intersections};
use ryukei::euler::{Corner, kvr, mef, mvfs, mvr, set_surface};
use ryukei::geom::{
    Circle, Curve, Cylinder, Ellipse, Frame, KnotVector, NurbsCurve, Pipe, Plane, Point3, Surface,
    Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::{Body, Sense};
use ryukei_testkit::mesh::volume_and_area;
use std::f64::consts::{PI, TAU};
use std::sync::Arc;

const R: f64 = 0.3;

/// The length of a curve over its domain, by chords: far finer than the
/// tests' tolerances.
fn length(c: &Curve) -> f64 {
    let [a, b] = c.domain();
    let n = 200_000;
    (0..n)
        .map(|i| {
            let t = |k: u32| a + (b - a) * f64::from(k) / f64::from(n);
            c.point(t(i)).distance(c.point(t(i + 1)))
        })
        .sum()
}

/// Checks a body, meshes it, and compares its volume and area with
/// Pappus's.
fn pappus(b: &Body, spine: &Curve) {
    assert_eq!(check(b), []);
    // Pipe faces join the self-intersection check with booleans (plan
    // 6.D): until then a pipe face and its neighbours can't be checked.
    for fault in check_intersections(b) {
        assert!(matches!(fault, Fault::CannotCheck { .. }), "{fault:?}");
    }
    let m = tessellate(b, &MeshParams::new(2e-4, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    let (vol, area) = volume_and_area(&m);
    let l = length(spine);
    let (want_vol, want_area) = (PI * R * R * l, TAU * R * l);
    assert!(
        (vol - want_vol).abs() <= 1e-3 * want_vol,
        "volume {vol} against {want_vol}"
    );
    // A bent cylinder's caps add two disks.
    let caps = if spine.period().is_some() {
        0.0
    } else {
        2.0 * PI * R * R
    };
    assert!(
        (area - want_area - caps).abs() <= 1e-3 * want_area,
        "area {area} against {}",
        want_area + caps
    );
}

/// A closed tube: one face with no loops on a pipe about a closed spine.
fn tube(spine: Curve, anchor: Surface) -> Body {
    let pipe = Pipe::new(spine, R, anchor).unwrap();
    let on = Surface::Pipe(Arc::new(pipe.clone())).point(0.0, 0.0);
    let mut b = Body::new();
    let s = b.add_surface(Surface::Pipe(Arc::new(pipe)));
    let (m, _) = mvfs(&mut b, OpKey(1), on, s).unwrap();
    kvr(&mut b, m.loop_).unwrap();
    b
}

#[test]
fn a_closed_tube_on_a_plane() {
    let frame = Frame::new(
        Point3::new(0.5, -1.0, 1.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 1.0, 0.0),
    )
    .unwrap();
    let spine = Curve::Ellipse(Ellipse::new(frame, 3.0, 2.0).unwrap());
    let b = tube(spine.clone(), Surface::Plane(Plane::new(Frame::WORLD)));
    pappus(&b, &spine);
    let dump = b.dump();
    assert!(dump.contains("pipe r="), "{dump}");
}

#[test]
fn a_closed_tube_round_a_cylinder() {
    // The ellipse where a tilted plane meets the cylinder of radius 3.3:
    // a ball on it rolls round the cylinder of radius 3.
    let (s, c) = (0.5, 0.75f64.sqrt());
    let frame = Frame::new(
        Point3::ORIGIN,
        Vector3::new(0.0, -s, c),
        Vector3::new(0.0, c, s),
    )
    .unwrap();
    let spine = Curve::Ellipse(Ellipse::new(frame, 3.3 / c, 3.3).unwrap());
    let anchor = Surface::Cylinder(Cylinder::new(Frame::WORLD, 3.0).unwrap());
    pappus(&tube(spine.clone(), anchor), &spine);
}

#[test]
fn a_bent_cylinder() {
    // A wavy cubic spine at height R over the plane z = 0, with disks
    // normal to it at both ends.
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, R),
        Point3::new(1.0, 1.0, R),
        Point3::new(2.0, -0.5, R),
        Point3::new(3.0, 0.5, R),
        Point3::new(4.0, 0.0, R),
    ];
    let spine = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    let pipe = Surface::Pipe(Arc::new(
        Pipe::new(spine.clone(), R, Surface::Plane(Plane::new(Frame::WORLD))).unwrap(),
    ));
    // Each end's disk: its frame's x towards the seam (θ = π), so the
    // circle's vertex sits on it.
    let end = |s: f64, outward: f64| {
        let c = spine.point(s);
        let t = spine.eval(s).d1.normalize().unwrap().get() * outward;
        let seam = pipe.point(s, PI) - c;
        Frame::new(c, t, seam).unwrap()
    };
    let (base_frame, top_frame) = (end(0.0, -1.0), end(2.0, 1.0));
    let mut b = Body::new();
    let side = b.add_surface(pipe.clone());
    let base_plane = b.add_surface(Surface::Plane(Plane::new(base_frame)));
    let top_plane = b.add_surface(Surface::Plane(Plane::new(top_frame)));
    let base_circle = b.add_curve(Curve::Circle(Circle::new(base_frame, R).unwrap()));
    let top_circle = b.add_curve(Curve::Circle(Circle::new(top_frame, R).unwrap()));
    let key = OpKey(1);
    let (m, _) = mvfs(&mut b, key, pipe.point(0.0, PI), side).unwrap();
    let lone = Corner::Lone(m.loop_);
    let (base, _) = mef(&mut b, key, lone, lone, base_circle, [0.0, TAU]).unwrap();
    set_surface(&mut b, base.faces[0], base_plane, Sense::Forward).unwrap();
    let (ring, _) = mvr(&mut b, key, m.face, pipe.point(2.0, PI)).unwrap();
    let lone = Corner::Lone(ring.loop_);
    let (top, _) = mef(&mut b, key, lone, lone, top_circle, [0.0, TAU]).unwrap();
    set_surface(&mut b, top.faces[0], top_plane, Sense::Forward).unwrap();
    ryukei::pcurve::fill(&mut b).unwrap();
    pappus(&b, &spine);
}

#[test]
fn a_pipe_face_box_holds_its_face() {
    // The bent cylinder's pipe face: its box from the spine over the
    // stretch its loops span holds sampled points of the face, and is
    // smaller than the whole pipe's box would be on a longer spine.
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, R),
        Point3::new(1.0, 1.0, R),
        Point3::new(2.0, -0.5, R),
        Point3::new(3.0, 0.5, R),
        Point3::new(4.0, 0.0, R),
    ];
    let spine = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    let pipe = Pipe::new(spine.clone(), R, Surface::Plane(Plane::new(Frame::WORLD))).unwrap();
    let surface = Surface::Pipe(Arc::new(pipe));
    // A band of the pipe between s = 0.2 and s = 0.7, within the spine's
    // first Bézier piece, as two section circles.
    let section = |s: f64| {
        let c = spine.point(s);
        let t = spine.eval(s).d1.normalize().unwrap().get();
        Frame::new(c, t, surface.point(s, PI) - c).unwrap()
    };
    let (f0, f1) = (section(0.2), section(0.7));
    let mut b = Body::new();
    let side = b.add_surface(surface.clone());
    let c0 = b.add_curve(Curve::Circle(Circle::new(f0, R).unwrap()));
    let c1 = b.add_curve(Curve::Circle(Circle::new(f1, R).unwrap()));
    let p0 = b.add_surface(Surface::Plane(Plane::new(f0)));
    let p1 = b.add_surface(Surface::Plane(Plane::new(f1)));
    let key = OpKey(1);
    let (m, _) = mvfs(&mut b, key, surface.point(0.2, PI), side).unwrap();
    let lone = Corner::Lone(m.loop_);
    let (cap, _) = mef(&mut b, key, lone, lone, c0, [0.0, TAU]).unwrap();
    set_surface(&mut b, cap.faces[0], p0, Sense::Forward).unwrap();
    let (ring, _) = mvr(&mut b, key, m.face, surface.point(0.7, PI)).unwrap();
    let lone = Corner::Lone(ring.loop_);
    let (cap, _) = mef(&mut b, key, lone, lone, c1, [0.0, TAU]).unwrap();
    set_surface(&mut b, cap.faces[0], p1, Sense::Forward).unwrap();
    ryukei::pcurve::fill(&mut b).unwrap();
    let fb = ryukei::intersect::face_box(&b, m.face);
    for i in 0..=20 {
        for j in 0..=20 {
            let s = 0.2 + 0.5 * f64::from(i) / 20.0;
            let p = surface.point(s, -PI + TAU * f64::from(j) / 20.0);
            let (lo, hi) = (fb.min(), fb.max());
            assert!(p.x >= lo.x && p.y >= lo.y && p.z >= lo.z, "{p:?}");
            assert!(p.x <= hi.x && p.y <= hi.y && p.z <= hi.z, "{p:?}");
        }
    }
    let whole = surface.bounds();
    assert!(fb.max().x - fb.min().x < 0.7 * (whole.max().x - whole.min().x));
}
