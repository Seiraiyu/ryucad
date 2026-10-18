//! Bodies hold swept surfaces like any other: primitives whose faces are
//! swapped for equivalent extrusions and revolutions, and a hand-built
//! elliptic cylinder, pass the checker, mesh with no faults, and enclose
//! the right volume and area.

use core::f64::consts::{PI, TAU};
use proptest::prelude::*;
use ryukei::check::check;
use ryukei::euler;
use ryukei::geom::{
    Circle, Curve, Ellipse, Extrusion, Frame, KnotVector, NurbsCurve, Point3, Revolution, Rigid,
    Surface, Vector3,
};
use ryukei::mesh::{BodyMesh, MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::predicates::{Sign, volume_sign};
use ryukei::primitive;
use ryukei::topo::{Body, FaceId, Sense};
use ryukei_testkit::mesh::{deviation, volume_and_area};
use std::sync::Arc;

const Z: Vector3 = Vector3::new(0.0, 0.0, 1.0);

fn placement() -> impl Strategy<Value = Rigid> {
    (-1.0f64..1.0, -1.0f64..1.0, -3.0f64..3.0, -20.0f64..20.0).prop_map(|(x, y, angle, shift)| {
        let axis = Vector3::new(x, y, 1.0).normalize().unwrap();
        Rigid::translation(Vector3::new(shift, -shift / 2.0, shift / 3.0))
            .then_after(&Rigid::rotation(axis, angle))
    })
}

/// The face of `b` called `side` by the primitives: the one that isn't on
/// a plane.
fn side(b: &Body) -> FaceId {
    b.faces()
        .find(|&f| {
            !matches!(
                b.surface(b.face(f).unwrap().surface),
                Some(Surface::Plane(_))
            )
        })
        .unwrap()
}

/// Puts face `f` on `s` (given in the primitive's local frame), with the
/// sense that keeps the face's normal: the new surface's natural normal is
/// compared with the old face's at a point of the face.
fn swap(b: &mut Body, f: FaceId, s: Surface, at: &Rigid) {
    let face = b.face(f).unwrap();
    let old = b.surface(face.surface).unwrap().clone();
    let corner = b
        .face_loops(f)
        .flat_map(|l| b.loop_fins(l).collect::<Vec<_>>())
        .next()
        .map(|fin| b.vertex(b.fin_start(fin).unwrap()).unwrap().point);
    let p = match corner {
        Some(p) => p,
        None => old.point(0.5, 0.3),
    };
    let (u, v) = old.project(p).param;
    let n = old.normal(u, v).unwrap().get();
    let n = if face.sense == Sense::Forward { n } else { -n };
    let s = s.transformed(at);
    let (u, v) = s.project(p).param;
    let m = s.normal(u, v).unwrap().get();
    let sense = if m.dot(n) > 0.0 {
        Sense::Forward
    } else {
        Sense::Reversed
    };
    let id = b.add_surface(s);
    euler::set_surface(b, f, id, sense).unwrap();
}

/// A mesh that passes every check, with its volume and area.
fn sound(b: &Body, chord: f64) -> (BodyMesh, f64, f64) {
    let p = MeshParams::new(chord, 0.5);
    let m = tessellate(b, &p).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(b, &m), Ok(()));
    let tris: Vec<_> = m
        .welded_triangles()
        .map(|t| t.map(|i| m.positions[i as usize]))
        .collect();
    assert_eq!(volume_sign(&tris), Sign::Positive);
    let d = deviation(b, &m, 2, 3);
    assert!(d <= chord, "deviation {d} over {chord}");
    let (v, a) = volume_and_area(&m);
    (m, v, a)
}

/// The mesh's volume and area match the exact ones: the volume within
/// the chord over the area, the area within a few chords over the
/// smallest radius of curvature.
fn measures(b: &Body, chord: f64, volume: f64, area: f64, radius: f64) {
    assert_eq!(check(b), []);
    let (_, v, a) = sound(b, chord);
    assert!((v - volume).abs() <= chord * area, "volume {v} vs {volume}");
    let slack = 3.0 * chord / radius * area;
    assert!((a - area).abs() <= slack, "area {a} vs {area}");
}

/// A cylinder whose side is the extrusion of its base circle along its
/// axis.
fn extruded_cylinder(r: f64, h: f64, at: &Rigid) -> Body {
    let (mut b, _) = primitive::cylinder(OpKey(1), r, h, at).unwrap();
    let f = side(&b);
    let circle = Curve::Circle(Circle::new(Frame::WORLD, r).unwrap());
    let e = Extrusion::new(circle, Z).unwrap();
    swap(&mut b, f, Surface::Extrusion(Arc::new(e)), at);
    b
}

/// The perimeter of an ellipse, by Simpson's rule over 4000 steps.
fn perimeter(a: f64, b: f64) -> f64 {
    let n = 4000;
    let speed = |t: f64| {
        let (s, c) = ryukei::math::sin_cos(t);
        ryukei::math::hypot(a * s, b * c)
    };
    let h = TAU / f64::from(n);
    let mut sum = speed(0.0) + speed(TAU);
    for i in 1..n {
        sum += speed(h * f64::from(i)) * if i % 2 == 1 { 4.0 } else { 2.0 };
    }
    sum * h / 3.0
}

/// A unit cylinder with its circles swapped for ellipses with semi-axes
/// `a` along x and `b` along y, and its side for their extrusion, moved
/// by `at`.
fn elliptic_cylinder(a: f64, b: f64, h: f64, at: &Rigid) -> Body {
    let (mut body, _) = primitive::cylinder(OpKey(2), 1.0, h, at).unwrap();
    let ex = Vector3::new(1.0, 0.0, 0.0);
    let base = Frame::new(Point3::ORIGIN, -Z, ex).unwrap();
    let top = Frame::new(Point3::new(0.0, 0.0, h), Z, ex).unwrap();
    let edges: Vec<_> = body.edges().collect();
    for e in edges {
        let start = body.edge(e).unwrap().start;
        let p = at.inverse().apply_point(body.vertex(start).unwrap().point);
        let frame = if p.z < h / 2.0 { base } else { top };
        let ellipse = Curve::Ellipse(Ellipse::new(frame, a, b).unwrap()).transformed(at);
        let ellipse = body.add_curve(ellipse);
        euler::set_curve(&mut body, e, ellipse, [0.0, TAU]).unwrap();
        let corner = at.apply_point(Point3::new(a, 0.0, frame.origin().z));
        euler::set_point(&mut body, start, corner).unwrap();
    }
    let f = side(&body);
    let ellipse = Curve::Ellipse(Ellipse::new(Frame::WORLD, a, b).unwrap());
    let e = Extrusion::new(ellipse, Z).unwrap();
    swap(&mut body, f, Surface::Extrusion(Arc::new(e)), at);
    body
}

#[test]
fn an_extruded_cylinder() {
    let at = Rigid::translation(Vector3::new(1.0, -2.0, 3.0)).then_after(&Rigid::rotation(
        Vector3::new(1.0, 1.0, 0.0).normalize().unwrap(),
        0.7,
    ));
    let (r, h) = (0.8, 1.7);
    let b = extruded_cylinder(r, h, &at);
    measures(&b, 2e-3, PI * r * r * h, 2.0 * PI * r * (r + h), r);
}

#[test]
fn an_elliptic_cylinder() {
    let at = Rigid::rotation(Vector3::new(0.2, -1.0, 0.4).normalize().unwrap(), 1.1);
    let (a, b, h) = (1.5, 0.6, 2.0);
    let body = elliptic_cylinder(a, b, h, &at);
    let area = 2.0 * PI * a * b + perimeter(a, b) * h;
    // The tightest bend of the ellipse is b² / a.
    measures(&body, 2e-3, PI * a * b * h, area, b * b / a);
}

/// A curve in the world xz half-plane through `(ρ, h)` control values.
fn meridian(knots: KnotVector, local: &[(f64, f64)], weights: Vec<f64>) -> Curve {
    let points = local.iter().map(|&(r, h)| Point3::new(r, 0.0, h)).collect();
    Curve::Nurbs(Arc::new(
        NurbsCurve::new(knots, points, Some(weights)).unwrap(),
    ))
}

fn revolved(curve: Curve) -> Surface {
    Surface::Revolution(Arc::new(Revolution::new(Frame::WORLD, curve).unwrap()))
}

/// A degree 1 segment from `a` to `b` over `[0, 1]`.
fn segment(a: (f64, f64), b: (f64, f64)) -> Curve {
    meridian(
        KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap(),
        &[a, b],
        vec![1.0; 2],
    )
}

/// A primitive with its side swapped for a revolution about its axis.
fn with_revolved_side(b: (Body, ryukei::name::Journal), side_curve: Curve, at: &Rigid) -> Body {
    let mut b = b.0;
    let f = side(&b);
    swap(&mut b, f, revolved(side_curve), at);
    b
}

/// A cylinder whose side is a line parallel to the axis, turned; the line
/// reaches past both ends.
fn revolved_cylinder(r: f64, h: f64, at: &Rigid) -> Body {
    let b = primitive::cylinder(OpKey(3), r, h, at).unwrap();
    with_revolved_side(b, segment((r, -h / 2.0), (r, 1.5 * h)), at)
}

/// A cone whose side is a line from its apex (a pole) turned.
fn revolved_cone(r: f64, h: f64, at: &Rigid) -> Body {
    let b = primitive::cone(OpKey(4), r, h, at).unwrap();
    with_revolved_side(b, segment((0.0, h), (1.5 * r, -h / 2.0)), at)
}

/// A sphere whose face is a NURBS semicircle from pole to pole turned: a
/// loopless face on a surface of closed genus 0.
fn revolved_sphere(r: f64, at: &Rigid) -> Body {
    let k = KnotVector::clamped(2, &[-1.0, 0.0, 1.0], &[3, 2, 3]).unwrap();
    let w = core::f64::consts::FRAC_1_SQRT_2;
    let local = [(0.0, -r), (r, -r), (r, 0.0), (r, r), (0.0, r)];
    let curve = meridian(k, &local, vec![1.0, w, 1.0, w, 1.0]);
    let b = primitive::sphere(OpKey(5), r, at).unwrap();
    with_revolved_side(b, curve, at)
}

/// A torus whose face is its tube circle turned: genus 1.
fn revolved_torus(major: f64, minor: f64, at: &Rigid) -> Body {
    let tube = Frame::new(
        Point3::new(major, 0.0, 0.0),
        Vector3::new(0.0, -1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let curve = Curve::Circle(Circle::new(tube, minor).unwrap());
    let b = primitive::torus(OpKey(6), major, minor, at).unwrap();
    with_revolved_side(b, curve, at)
}

/// The surface of the first face of `b`.
fn first_surface(b: &Body) -> &Surface {
    let f = b.faces().next().unwrap();
    b.surface(b.face(f).unwrap().surface).unwrap()
}

#[test]
fn revolved_primitives() {
    let at = Rigid::translation(Vector3::new(-1.0, 2.0, 0.5)).then_after(&Rigid::rotation(
        Vector3::new(0.3, -0.5, 1.0).normalize().unwrap(),
        2.1,
    ));
    let (r, h) = (0.9, 1.6);
    let chord = 2e-3;
    let b = revolved_cylinder(r, h, &at);
    measures(&b, chord, PI * r * r * h, 2.0 * PI * r * (r + h), r);
    let b = revolved_cone(r, h, &at);
    let slant = (r * r + h * h).sqrt();
    measures(&b, chord, PI * r * r * h / 3.0, PI * r * (r + slant), r);
    let b = revolved_sphere(r, &at);
    assert_eq!(first_surface(&b).closed_genus(), Some(0));
    measures(&b, chord, 4.0 * PI * r * r * r / 3.0, 4.0 * PI * r * r, r);
    let (big, small) = (1.5, 0.5);
    let b = revolved_torus(big, small, &at);
    assert_eq!(first_surface(&b).closed_genus(), Some(1));
    let (volume, area) = (
        2.0 * PI * PI * big * small * small,
        4.0 * PI * PI * big * small,
    );
    measures(&b, chord, volume, area, small);
}

#[test]
fn open_revolutions_cannot_be_loopless() {
    let (mut b, _) = primitive::sphere(OpKey(8), 1.0, &Rigid::IDENTITY).unwrap();
    let f = side(&b);
    // A cap from the north pole down to the equator: one pole only.
    let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
    let w = core::f64::consts::FRAC_1_SQRT_2;
    let cap = meridian(k, &[(1.0, 0.0), (1.0, 1.0), (0.0, 1.0)], vec![1.0, w, 1.0]);
    let id = b.add_surface(revolved(cap));
    euler::set_surface(&mut b, f, id, Sense::Forward).unwrap();
    assert_eq!(check(&b), [ryukei::check::Fault::Loopless { face: f }]);
}

/// A NURBS revolution that is not a sphere, from pole to pole: an egg.
#[test]
fn an_egg() {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let local = [(0.0, 0.0), (1.2, 0.0), (1.0, 1.5), (0.6, 2.6), (0.0, 2.6)];
    let curve = meridian(k, &local, vec![1.0, 0.8, 1.3, 1.0, 1.0]);
    let at = Rigid::rotation(Vector3::new(1.0, 0.2, 0.0).normalize().unwrap(), 0.8);
    let (mut b, _) = primitive::sphere(OpKey(7), 1.0, &at).unwrap();
    let f = side(&b);
    // The sphere's face moves onto the egg: no vertices to keep on it.
    swap(&mut b, f, revolved(curve.clone()), &at);
    assert_eq!(check(&b), []);
    let (_, v, a) = sound(&b, 2e-3);
    // Pappus, by Simpson's rule along the curve: the volume is
    // ∫ π ρ² h' and the area ∫ 2π ρ |C'|.
    let n = 2000;
    let (mut vol, mut area) = (0.0, 0.0);
    for i in 0..=n {
        let t = 2.0 * f64::from(i) / f64::from(n);
        let wt = if i == 0 || i == n {
            1.0
        } else if i % 2 == 1 {
            4.0
        } else {
            2.0
        };
        let e = curve.eval(t);
        vol += wt * PI * e.point.x * e.point.x * e.d1.z;
        area += wt * 2.0 * PI * e.point.x * e.d1.length();
    }
    let step = 2.0 / f64::from(n) / 3.0;
    let (vol, area) = (vol * step, area * step);
    assert!((v - vol).abs() <= 2e-3 * area, "volume {v} vs {vol}");
    assert!((a - area).abs() <= 0.05 * area, "area {a} vs {area}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(6))]

    #[test]
    fn extruded_cylinders(r in 0.2f64..2.0, h in 0.2f64..2.0, at in placement()) {
        let b = extruded_cylinder(r, h, &at);
        measures(&b, 1e-2, PI * r * r * h, 2.0 * PI * r * (r + h), r);
    }

    #[test]
    fn revolved_bodies(
        kind in 0u8..4,
        a in 0.3f64..2.0,
        b in 0.3f64..2.0,
        at in placement(),
    ) {
        let chord = 1e-2;
        let slant = (a * a + b * b).sqrt();
        let (body, volume, area, radius) = match kind {
            0 => (revolved_cylinder(a, b, &at), PI * a * a * b, 2.0 * PI * a * (a + b), a),
            1 => (revolved_cone(a, b, &at), PI * a * a * b / 3.0, PI * a * (a + slant), a.min(b)),
            2 => (revolved_sphere(a, &at), 4.0 * PI * a * a * a / 3.0, 4.0 * PI * a * a, a),
            _ => {
                let (big, small) = (a + b, b);
                let v = 2.0 * PI * PI * big * small * small;
                (revolved_torus(big, small, &at), v, 4.0 * PI * PI * big * small, small)
            }
        };
        measures(&body, chord, volume, area, radius);
    }
}
