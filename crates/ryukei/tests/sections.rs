//! Sections (design 326, plan 10.C): bodies cut by planes against closed
//! forms, a sheet trimmed, a section view, one thread and eight.

use core::f64::consts::PI;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Frame, Plane, Point3, Rigid, Surface, Vector3};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::name::OpKey;
use ryukei::project::{Placed, Source, View, ViewOptions, Visibility, section, view};
use ryukei::topo::{Body, FaceId};
use ryukei::{check, primitive, sheet};

fn plane(origin: Point3, normal: Vector3) -> Plane {
    let hint = if normal.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    Plane::new(Frame::new(origin, normal, hint).unwrap())
}

fn area(b: &Body, f: FaceId) -> f64 {
    match properties(b, Entity::Face(f)).unwrap() {
        Properties::Face { area, .. } => area.value(),
        p => panic!("{p:?}"),
    }
}

/// A 40 × 30 × 10 mm plate with a 4 mm bore through its middle.
fn drilled() -> Body {
    let plate = primitive::block(OpKey(1), 0.04, 0.03, 0.01, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let pin = primitive::cylinder(OpKey(2), 0.004, 0.02, &Rigid::IDENTITY)
        .unwrap()
        .0;
    boolean(
        OpKey(3),
        BooleanOp::Subtract,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &pin,
            place: Rigid::translation(Vector3::new(0.02, 0.015, -0.005)),
        }],
    )
    .unwrap()
    .0
}

#[test]
fn a_drilled_plates_cap_is_its_section_less_the_bore() {
    let b = drilled();
    let s = section(
        OpKey(4),
        &b,
        &plane(Point3::new(0.02, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)),
    )
    .unwrap();
    assert_eq!(check::check(&s.body), []);
    assert_eq!(s.faces.len(), 2, "the bore parts the cap in two");
    let caps: f64 = s.faces.iter().map(|&f| area(&s.body, f)).sum();
    assert!((caps - (0.03 - 0.008) * 0.01).abs() < 1e-15, "{caps}");
}

#[test]
fn a_balls_cap_through_its_middle_is_a_disc() {
    let (ball, _) = primitive::sphere(OpKey(5), 0.02, &Rigid::IDENTITY).unwrap();
    let s = section(
        OpKey(6),
        &ball,
        &plane(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)),
    )
    .unwrap();
    assert_eq!(check::check(&s.body), []);
    assert_eq!(s.faces.len(), 1);
    let a = match properties(&s.body, Entity::Face(s.faces[0])).unwrap() {
        Properties::Face { area, .. } => area,
        p => panic!("{p:?}"),
    };
    assert!(a.holds(PI * 0.02 * 0.02, 1e-12), "{a:?}");
}

#[test]
fn a_cylinder_cut_along_its_axis_caps_a_rectangle() {
    let (c, _) = primitive::cylinder(OpKey(7), 0.01, 0.03, &Rigid::IDENTITY).unwrap();
    let s = section(
        OpKey(8),
        &c,
        &plane(Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0)),
    )
    .unwrap();
    assert_eq!(check::check(&s.body), []);
    assert_eq!(s.faces.len(), 1);
    assert!((area(&s.body, s.faces[0]) - 0.02 * 0.03).abs() < 1e-15);
}

#[test]
fn a_sheet_is_trimmed_and_caps_nothing() {
    let patch = Surface::Plane(Plane::new(Frame::WORLD));
    let (sh, _) = sheet::from_surface(OpKey(9), &patch, [[0.0, 1.0], [0.0, 1.0]]).unwrap();
    let s = section(
        OpKey(10),
        &sh,
        &plane(Point3::new(0.5, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)),
    )
    .unwrap();
    assert!(s.faces.is_empty());
    match properties(&s.body, Entity::Body).unwrap() {
        Properties::Sheet { area } => assert!(area.holds(0.5, 1e-12), "{area:?}"),
        p => panic!("{p:?}"),
    }
}

#[test]
fn a_section_view_sees_every_edge_of_its_cap() {
    let b = drilled();
    let s = section(
        OpKey(4),
        &b,
        &plane(Point3::new(0.02, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)),
    )
    .unwrap();
    // Looking at the cap: from the side cut away.
    let v = View::new(Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let p = view(
        &[Placed {
            body: &s.body,
            place: Rigid::IDENTITY,
        }],
        &v,
        &ViewOptions::default(),
    )
    .unwrap();
    let cap_edges: Vec<_> = s
        .faces
        .iter()
        .flat_map(|&f| {
            s.body
                .face_loops(f)
                .flat_map(|l| s.body.loop_fins(l).map(|fin| s.body.fin(fin).unwrap().edge))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(cap_edges.len(), 8);
    for q in &p.pieces {
        if let Source::Edge { edge, .. } = q.source
            && cap_edges.contains(&edge)
        {
            assert_eq!(q.visibility, Visibility::Visible, "{q:?}");
        }
    }
}

#[test]
fn one_thread_and_eight_cut_alike() {
    let b = drilled();
    let p = plane(Point3::new(0.013, 0.0, 0.0), Vector3::new(1.0, 0.2, 0.1));
    let cut = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| section(OpKey(11), &b, &p).unwrap())
    };
    let (one, eight) = (cut(1), cut(8));
    assert_eq!(one.body.dump(), eight.body.dump());
    assert_eq!(one.journal, eight.journal);
    assert_eq!(one.faces, eight.faces);
}
