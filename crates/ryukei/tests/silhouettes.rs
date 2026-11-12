//! Silhouettes of every surface (design 326, plan 10.A): closed forms
//! against their geometry, traced ones against the surface's normal.
//! The traced cases run in release only:
//! `cargo test -p ryukei --release --test silhouettes`.

use core::f64::consts::{FRAC_PI_2, TAU};
use ryukei::geom::{
    Bent, Circle, Curve, Cylinder, Ellipse, Extrusion, Frame, FrameLaw, KnotVector, Line,
    NurbsCurve, Offset, Pipe, Plane, Point3, Revolution, Rigid, Surface, Sweep, Vector3,
};
use ryukei::name::OpKey;
use ryukei::project::{Silhouettes, View, silhouettes};
use ryukei::topo::Body;
use ryukei::{primitive, sheet};
use std::sync::Arc;

fn view(t: Vector3) -> View {
    let up = if t.cross(Vector3::new(0.0, 0.0, 1.0)).length() > 0.1 {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    View::new(t, up).unwrap()
}

fn one_face(s: &Surface, rect: [[f64; 2]; 2]) -> Body {
    sheet::from_surface(OpKey(9), s, rect).unwrap().0
}

/// Every stretch on its face's surface, square to the view.
fn sound(body: &Body, v: &View, s: &Silhouettes) {
    let t = v.towards().get();
    for b in &s.branches {
        let sid = body.face(b.face).unwrap().surface;
        let surface = body.surface(sid).unwrap();
        for i in 0..=32 {
            let x = b.range[0] + (b.range[1] - b.range[0]) * i as f64 / 32.0;
            let (c2, map) = &b.pcurve;
            let q = c2.point(map[0] + map[1] * x);
            let p = b.curve.point(x);
            assert!(
                surface.point(q.x, q.y).distance(p) <= b.tolerance,
                "{b:?} at {x}"
            );
            if let Some(n) = surface.normal(q.x, q.y) {
                assert!(n.get().dot(t).abs() <= 1e-6, "{b:?} at {x}");
            }
        }
    }
}

/// The stretches' total length, certified to a part in 10⁹.
fn length(s: &Silhouettes) -> f64 {
    s.branches
        .iter()
        .map(|b| {
            ryukei::measure::curve_length(&b.curve, b.range, 1e-9)
                .unwrap()
                .value()
        })
        .sum()
}

#[test]
fn a_cylinder_side_on_has_two_rulings_and_along_its_axis_none() {
    let (c, _) = primitive::cylinder(OpKey(1), 0.01, 0.03, &Rigid::IDENTITY).unwrap();
    let front = view(Vector3::new(0.0, -1.0, 0.0));
    let s = silhouettes(&c, &front).unwrap();
    assert_eq!(s.branches.len(), 2);
    sound(&c, &front, &s);
    assert!((length(&s) - 0.06).abs() < 1e-12);
    let top = view(Vector3::new(0.0, 0.0, 1.0));
    let s = silhouettes(&c, &top).unwrap();
    assert!(s.branches.is_empty() && s.edge_on.len() == 1);
}

#[test]
fn a_spheres_silhouette_is_a_circle_of_its_radius_from_any_side() {
    let (b, _) = primitive::sphere(OpKey(2), 0.02, &Rigid::IDENTITY).unwrap();
    for t in [
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(1.0, -2.0, 3.0),
    ] {
        let v = view(t);
        let s = silhouettes(&b, &v).unwrap();
        sound(&b, &v, &s);
        assert!(
            (length(&s) - TAU * 0.02).abs() < 1e-6,
            "{t:?}: {}",
            length(&s)
        );
    }
}

#[test]
fn a_cone_side_on_has_two_rulings() {
    let (b, _) = primitive::cone(OpKey(3), 0.01, 0.02, &Rigid::IDENTITY).unwrap();
    let v = view(Vector3::new(0.0, -1.0, 0.0));
    let s = silhouettes(&b, &v).unwrap();
    assert_eq!(s.branches.len(), 2);
    sound(&b, &v, &s);
}

#[test]
fn a_torus_along_its_axis_has_two_circles() {
    let (b, _) = primitive::torus(OpKey(4), 0.03, 0.01, &Rigid::IDENTITY).unwrap();
    let v = view(Vector3::new(0.0, 0.0, 1.0));
    let s = silhouettes(&b, &v).unwrap();
    assert_eq!(s.branches.len(), 2);
    sound(&b, &v, &s);
    assert!((length(&s) - TAU * 0.06).abs() < 1e-6);
}

#[test]
fn a_half_cylinders_boundary_rulings_are_its_silhouette_on_its_edges() {
    let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 0.01).unwrap());
    let half = one_face(&cyl, [[0.0, core::f64::consts::PI], [0.0, 0.02]]);
    // Seen along y the rulings at u = 0 and π are the boundary's.
    let v = view(Vector3::new(0.0, 1.0, 0.0));
    let s = silhouettes(&half, &v).unwrap();
    assert!(s.branches.is_empty(), "{s:?}");
    assert_eq!(s.on_edges.len(), 2);
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn a_torus_askew_is_traced() {
    let (b, _) = primitive::torus(OpKey(5), 0.03, 0.01, &Rigid::IDENTITY).unwrap();
    let v = view(Vector3::new(1.0, 0.2, 1.0));
    let s = silhouettes(&b, &v).unwrap();
    assert!(!s.branches.is_empty());
    sound(&b, &v, &s);
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn a_spline_cylinder_has_the_cylinders_rulings() {
    let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 0.01).unwrap());
    let (n, _) = cyl.to_nurbs([[0.0, TAU], [0.0, 0.03]]).unwrap();
    let spline = Surface::Nurbs(Arc::new(n));
    let b = one_face(&spline, spline.domain());
    let v = view(Vector3::new(0.0, -1.0, 0.0));
    let s = silhouettes(&b, &v).unwrap();
    sound(&b, &v, &s);
    assert!((length(&s) - 0.06).abs() < 1e-6, "{}", length(&s));
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn an_extrusion_a_revolution_and_a_pipe_are_traced() {
    let c = Curve::Circle(Circle::new(Frame::WORLD, 0.01).unwrap());
    let ex = Surface::Extrusion(Arc::new(
        Extrusion::new(c.clone(), Vector3::new(0.2, 0.0, 1.0)).unwrap(),
    ));
    let knob = Curve::Circle(
        Circle::new(
            Frame::new(
                Point3::new(0.02, 0.0, 0.0),
                Vector3::new(0.0, -1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
            0.005,
        )
        .unwrap(),
    );
    let rev = Surface::Revolution(Arc::new(Revolution::new(Frame::WORLD, knob).unwrap()));
    // A pipe on a circle is a torus, so the spine is an ellipse, a
    // centimetre above its anchor plane.
    let lifted = Frame::new(
        Point3::new(0.0, 0.0, 0.01),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let spine = Curve::Ellipse(Ellipse::new(lifted, 0.06, 0.04).unwrap());
    let pipe = Surface::Pipe(Arc::new(
        Pipe::new(
            spine.clone(),
            0.01,
            Surface::Plane(Plane::new(Frame::WORLD)),
        )
        .unwrap(),
    ));
    let cases = [
        (ex, [[0.0, TAU], [0.0, 0.02]]),
        (rev, [[0.0, TAU], [0.0, TAU]]),
        (
            pipe,
            [
                [0.0, FRAC_PI_2],
                [-core::f64::consts::PI, core::f64::consts::PI],
            ],
        ),
    ];
    for (surface, rect) in cases {
        let b = one_face(&surface, rect);
        let v = view(Vector3::new(0.3, -1.0, 0.4));
        let s = silhouettes(&b, &v).unwrap_or_else(|e| panic!("{surface:?}: {e}"));
        assert!(!s.branches.is_empty(), "{surface:?}");
        sound(&b, &v, &s);
    }
    let _ = (Line::new, NurbsCurve::new, KnotVector::clamped);
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn a_sweep_an_offset_and_a_bent_surface_are_traced() {
    let profile = Curve::Circle(Circle::new(Frame::WORLD, 0.005).unwrap());
    let path = Curve::Circle(
        Circle::new(
            Frame::new(
                Point3::new(-0.05, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
            0.05,
        )
        .unwrap(),
    );
    let sweep = Surface::Sweep(Arc::new(
        Sweep::new(
            (profile, [0.0, TAU]),
            (path, [0.0, 1.0]),
            Frame::WORLD,
            FrameLaw::Minimal,
            0.0,
            1.0,
        )
        .unwrap(),
    ));
    let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 0.01).unwrap());
    let (n, _) = cyl.to_nurbs([[0.0, TAU], [0.0, 0.03]]).unwrap();
    let offset = Surface::Offset(Arc::new(
        Offset::new(
            Surface::Nurbs(Arc::new(n)),
            0.002,
            [[0.0, TAU], [0.0, 0.03]],
        )
        .unwrap(),
    ));
    let bent = Surface::Bent(Arc::new(
        Bent::new(
            Surface::Plane(Plane::new(Frame::WORLD)),
            Frame::WORLD,
            0.01,
            100.0,
            [[0.0, 0.01], [0.0, 0.02]],
        )
        .unwrap(),
    ));
    let rects = [
        sweep.domain(),
        [[0.0, TAU], [0.0, 0.03]],
        [[0.0, 0.01], [0.0, 0.02]],
    ];
    for (surface, rect) in [sweep, offset, bent].into_iter().zip(rects) {
        let b = one_face(&surface, rect);
        // The bent plane is a quarter of a radian of cylinder about z: seen
        // square to its middle's radial direction, its silhouette is the
        // ruling at u = 0.005.
        let v = view(Vector3::new(
            -0.479_425_538_604_203,
            0.877_582_561_890_372_8,
            0.0,
        ));
        let s = silhouettes(&b, &v).unwrap_or_else(|e| panic!("{surface:?}: {e}"));
        sound(&b, &v, &s);
    }
}

/// Seen from the front, a torus about `z` has its two meridian circles
/// and two parallels (design 484, plan 17.B): `4π (R + r)` long.
#[test]
fn a_torus_from_the_front_has_two_meridians_and_two_parallels() {
    let (b, _) = primitive::torus(OpKey(6), 0.03, 0.01, &Rigid::IDENTITY).unwrap();
    for t in [Vector3::new(0.0, -1.0, 0.0), Vector3::new(1.0, 1.0, 0.0)] {
        let v = view(t);
        let s = silhouettes(&b, &v).unwrap_or_else(|e| panic!("{t:?}: {e}"));
        sound(&b, &v, &s);
        assert!(
            (length(&s) - 2.0 * TAU * 0.04).abs() < 1e-9,
            "{}",
            length(&s)
        );
    }
}

/// Tilted off square by 1e-9 to 1e-3 rad, the contour pinches but is
/// regular: traced, it certifies or refuses `Uncertain` within the
/// trace's cap, never anything else, and what it gives is sound.
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn a_torus_nearly_square_to_the_view_certifies_or_refuses_within_the_cap() {
    let (b, _) = primitive::torus(OpKey(7), 0.03, 0.01, &Rigid::IDENTITY).unwrap();
    for tilt in [1e-9, 1e-7, 1e-5, 1e-3] {
        let v = view(Vector3::new(0.0, -1.0, tilt));
        let start = std::time::Instant::now();
        match silhouettes(&b, &v) {
            Ok(s) => {
                sound(&b, &v, &s);
                eprintln!(
                    "tilt {tilt}: certified in {:.2} s",
                    start.elapsed().as_secs_f64()
                );
            }
            Err(ryukei::Error::InvalidView(ryukei::ViewError::Uncertain { .. })) => {
                eprintln!(
                    "tilt {tilt}: refused in {:.2} s",
                    start.elapsed().as_secs_f64()
                );
            }
            Err(e) => panic!("tilt {tilt}: {e:?}"),
        }
    }
}
