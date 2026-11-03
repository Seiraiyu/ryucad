//! Fillets and chamfers on sheets (plan 12.F).

use super::tests::{K, block, edge_of, face_name};
use super::*;
use crate::check::{check, check_intersections};
use crate::geom::{Frame, Plane, Surface, Vector3};
use crate::measure::{Entity, Properties, properties};
use crate::name::{Event, FaceRole};
use crate::topo::BodyKind;
use core::f64::consts::{FRAC_PI_2, PI};

fn fillet(edge: Name, radius: f64) -> EdgeBlend {
    EdgeBlend {
        edge,
        kind: BlendKind::Fillet { radius },
    }
}

fn chamfer(edge: Name, distance: f64) -> EdgeBlend {
    EdgeBlend {
        edge,
        kind: BlendKind::Chamfer(Chamfer::Equal { distance }),
    }
}

fn area(b: &Body) -> f64 {
    match properties(b, Entity::Body).unwrap() {
        Properties::Sheet { area } => area.value(),
        p => panic!("not a sheet: {p:?}"),
    }
}

/// A sheet that checks clean with `laminar` laminar edges.
fn sound_sheet(b: &Body, laminar: usize) {
    assert_eq!(b.kind(), BodyKind::Sheet);
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    assert_eq!(b.edges().filter(|&e| b.is_laminar(e)).count(), laminar);
    let mesh = crate::mesh::tessellate(b, &crate::mesh::MeshParams::new(1e-3, 0.3)).unwrap();
    assert!(mesh.faults.is_empty(), "{:?}", mesh.faults);
}

/// The unit cube's top and `+x` faces: an L whose inner edge is convex
/// seen from the front, six laminar edges.
fn l_sheet() -> Body {
    let names = [face_name(FaceRole::PlusZ), face_name(FaceRole::PlusX)];
    crate::sheet::extract(OpKey(0x5e1), &block(), &names)
        .unwrap()
        .0
}

/// The unit cube without its top: an open box, its rim four laminar
/// edges in `z = 1`.
fn open_box() -> Body {
    crate::sheet::remove(OpKey(0x5e2), &block(), &[face_name(FaceRole::PlusZ)])
        .unwrap()
        .0
}

/// The same L as [`l_sheet`] from two bounded planes sewn together,
/// facing `+z` and `+x`, or `-z` and `-x` when `back` (concave seen from
/// the front).
fn sewn_l(back: bool) -> Body {
    let s = if back { -1.0 } else { 1.0 };
    let top = Frame::new(
        crate::geom::Point3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, 0.0, s),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let side = Frame::new(
        crate::geom::Point3::new(1.0, 0.0, 0.0),
        Vector3::new(s, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    // Each plane's box in its own (x, y): the top over x in [0, 1] and
    // y in [0, 1] (y runs against world y when facing down); the side
    // over world y in [0, 1] and z in [0, 1].
    let ty = if back { [-1.0, 0.0] } else { [0.0, 1.0] };
    let sz = if back { [-1.0, 0.0] } else { [0.0, 1.0] };
    let a =
        crate::sheet::from_surface(OpKey(1), &Surface::Plane(Plane::new(top)), [[0.0, 1.0], ty])
            .unwrap()
            .0;
    let b = crate::sheet::from_surface(
        OpKey(2),
        &Surface::Plane(Plane::new(side)),
        [[0.0, 1.0], sz],
    )
    .unwrap()
    .0;
    let opts = crate::heal::SewOptions {
        tolerance: 1e-6,
        solid: false,
    };
    let (mut out, _) = crate::heal::sew(OpKey(3), &[a, b], &opts).unwrap();
    assert_eq!(out.len(), 1);
    out.remove(0)
}

/// The inner edge of a sheet with two faces.
fn inner_edge(b: &Body) -> Name {
    let inner: Vec<_> = b.edges().filter(|&e| !b.is_laminar(e)).collect();
    assert_eq!(inner.len(), 1);
    b.edges[inner[0]].name
}

#[test]
fn an_l_sheets_edge_is_filleted_to_its_open_ends() {
    let s = l_sheet();
    let e = edge_of(&block(), FaceRole::PlusZ, FaceRole::PlusX);
    let r = 0.2;
    let (out, journal) = blend(OpKey(7), &s, &[fillet(e, r)]).unwrap();
    // Six laminar edges, cut short, and an open end at each end.
    sound_sheet(&out, 8);
    assert_eq!(out.faces().count(), 3);
    let want = 2.0 - 2.0 * r + FRAC_PI_2 * r;
    assert!((area(&out) - want).abs() < 1e-7, "{} {want}", area(&out));
    // Both vertices the blend took away are replaced by its face.
    let replaced = journal
        .events()
        .iter()
        .filter(|ev| matches!(ev, Event::Replaced { .. }))
        .count();
    assert_eq!(replaced, 3);
}

#[test]
fn an_l_sheets_edge_is_chamfered_to_its_open_ends() {
    let s = l_sheet();
    let e = edge_of(&block(), FaceRole::PlusZ, FaceRole::PlusX);
    let d = 0.25;
    let (out, _) = blend(OpKey(7), &s, &[chamfer(e, d)]).unwrap();
    sound_sheet(&out, 8);
    let want = 2.0 - 2.0 * d + d * 2f64.sqrt();
    assert!((area(&out) - want).abs() < 1e-9, "{} {want}", area(&out));
}

#[test]
fn open_ends_are_named_from_the_blend_face_and_the_vertex() {
    let s = l_sheet();
    let e = edge_of(&block(), FaceRole::PlusZ, FaceRole::PlusX);
    let (a, _) = blend(OpKey(7), &s, &[fillet(e, 0.2)]).unwrap();
    let (b, _) = blend(OpKey(7), &s, &[fillet(e, 0.3)]).unwrap();
    let face = Name::derive(OpKey(7), Role::Blend(BlendRole::Face), &[e]);
    let ends = |x: &Body| -> Vec<Name> {
        let mut v: Vec<Name> = s
            .vertices()
            .filter(|&v| {
                s.vertex_edges(v).contains(&match s.by_name(e) {
                    Some(crate::topo::EntityId::Edge(g)) => g,
                    _ => unreachable!(),
                })
            })
            .map(|v| {
                Name::derive(
                    OpKey(7),
                    Role::Blend(BlendRole::Open),
                    &[face, s.vertices[v].name],
                )
            })
            .collect();
        v.sort();
        for n in &v {
            assert!(x.by_name(*n).is_some(), "no open end {n:?}");
        }
        v
    };
    // A new radius renames nothing.
    assert_eq!(ends(&a), ends(&b));
    let names = |x: &Body| -> Vec<Name> {
        let mut v: Vec<Name> = x.edges().map(|g| x.edges[g].name).collect();
        v.sort();
        v
    };
    assert_eq!(names(&a), names(&b));
}

#[test]
fn a_sheet_has_one_fillet_whichever_way_it_faces() {
    let r = 0.2;
    let mut areas = Vec::new();
    let mut middles = Vec::new();
    for back in [false, true] {
        let s = sewn_l(back);
        let (out, _) = blend(OpKey(7), &s, &[fillet(inner_edge(&s), r)]).unwrap();
        sound_sheet(&out, 8);
        areas.push(area(&out));
        // The blend face: the one on a cylinder, its axis in the wedge.
        let f = out
            .faces()
            .find(|&f| matches!(out.surfaces[out.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        let frame = out.surfaces[out.faces[f].surface].frame().unwrap();
        middles.push(frame.origin());
    }
    assert!((areas[0] - areas[1]).abs() < 1e-9);
    assert!((areas[0] - (2.0 - 2.0 * r + FRAC_PI_2 * r)).abs() < 1e-7);
    for m in middles {
        assert!(
            (m.x - (1.0 - r)).abs() < 1e-9 && (m.z - (1.0 - r)).abs() < 1e-9,
            "{m:?}"
        );
    }
}

#[test]
fn an_open_box_is_rounded_inside_with_balls_and_open_ends() {
    let s = open_box();
    let b = block();
    use FaceRole::*;
    let r = 0.1;
    let mut blends = Vec::new();
    for (x, y) in [
        (MinusZ, PlusX),
        (MinusZ, MinusX),
        (MinusZ, PlusY),
        (MinusZ, MinusY),
        (PlusX, PlusY),
        (PlusY, MinusX),
        (MinusX, MinusY),
        (MinusY, PlusX),
    ] {
        blends.push(fillet(edge_of(&b, x, y), r));
    }
    let (out, _) = blend(OpKey(7), &s, &blends).unwrap();
    // The rim's four edges, each cut at both ends, and four open ends.
    sound_sheet(&out, 8);
    let (a, h) = (1.0 - 2.0 * r, 1.0 - r);
    let want =
        a * a + 4.0 * a * h + 4.0 * FRAC_PI_2 * r * h + 4.0 * FRAC_PI_2 * r * a + 2.0 * PI * r * r;
    assert!((area(&out) - want).abs() < 1e-6, "{} {want}", area(&out));
}

#[test]
fn laminar_edges_are_not_blended() {
    let s = l_sheet();
    let lam = s.edges().find(|&e| s.is_laminar(e)).unwrap();
    let name = s.edges[lam].name;
    assert_eq!(
        blend(OpKey(7), &s, &[fillet(name, 0.1)]).unwrap_err(),
        BlendError::Unsupported { at: name }.into()
    );
}

#[test]
fn a_fillet_too_large_for_the_sheet_overflows() {
    let s = l_sheet();
    let e = edge_of(&block(), FaceRole::PlusZ, FaceRole::PlusX);
    assert!(matches!(
        blend(OpKey(7), &s, &[fillet(e, 1.5)]).unwrap_err(),
        Error::InvalidBlend(BlendError::Overflow { .. } | BlendError::RadiusTooLarge { .. })
    ));
    let _ = K;
}

/// An open end whose laminar edges don't lie in one plane: a top face
/// whose boundary leaves the vertex along an arc, sewn to a wall.
#[test]
fn an_open_end_off_one_plane_is_unsupported() {
    use crate::geom::{Circle2, Curve2, Frame2, Line2, Point2, Point3, Vector2};
    use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, planar};
    let line = |key: u128, a: (f64, f64), b: (f64, f64)| {
        let (a, b) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
        Segment {
            key: SegmentKey(key),
            curve: Curve2::Line2(Line2::through(a, b).unwrap()),
            t0: 0.0,
            t1: a.distance(b),
        }
    };
    // From (0.5, 0.5) round a circle about (1, 0.5) to (1, 0), leaving
    // it along -x, so its plane with the wall's edge (along -z) is y = 0
    // only at the vertex.
    let f = Frame2::new(Point2::new(1.0, 0.5), Vector2::new(1.0, 0.0), true).unwrap();
    let arc = Segment {
        key: SegmentKey(4),
        curve: Curve2::Circle2(Circle2::new(f, 0.5).unwrap()),
        t0: PI,
        t1: 1.5 * PI,
    };
    let frame = Frame::new(
        Point3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let profile = ValidProfile::new(Profile {
        frame,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![
                line(1, (1.0, 0.0), (1.0, 1.0)),
                line(2, (1.0, 1.0), (0.5, 1.0)),
                line(3, (0.5, 1.0), (0.5, 0.5)),
                arc,
            ],
        }],
    })
    .unwrap();
    let top = planar(OpKey(1), &profile).unwrap().0;
    let wall = Frame::new(
        Point3::new(1.0, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let wall = crate::sheet::from_surface(
        OpKey(2),
        &Surface::Plane(Plane::new(wall)),
        [[0.0, 1.0], [0.0, 1.0]],
    )
    .unwrap()
    .0;
    let opts = crate::heal::SewOptions {
        tolerance: 1e-6,
        solid: false,
    };
    let (mut out, _) = crate::heal::sew(OpKey(3), &[top, wall], &opts).unwrap();
    assert_eq!(out.len(), 1);
    let s = out.remove(0);
    let e = inner_edge(&s);
    // The vertex at (1, 0, 1), where the arc leaves.
    let v = s
        .vertices()
        .find(|&v| s.vertices[v].point.distance(Point3::new(1.0, 0.0, 1.0)) < 1e-9)
        .unwrap();
    assert_eq!(
        blend(OpKey(7), &s, &[fillet(e, 0.1)]).unwrap_err(),
        BlendError::Unsupported {
            at: s.vertices[v].name
        }
        .into()
    );
}

/// A cup, a cylinder without its top, rounded inside round its bottom
/// rim: a closed chain with no ends, a quarter torus between the floor
/// and the wall.
#[test]
fn a_cups_bottom_rim_is_rounded() {
    let (rc, hc, r) = (0.3, 0.5, 0.05);
    let (cyl, _) = crate::primitive::cylinder(K, rc, hc, &crate::geom::Rigid::IDENTITY).unwrap();
    let cup = crate::sheet::remove(OpKey(0x5e3), &cyl, &[face_name(FaceRole::Top)])
        .unwrap()
        .0;
    let rim: Vec<EdgeBlend> = cup
        .edges()
        .filter(|&e| !cup.is_laminar(e))
        .map(|e| fillet(cup.edges[e].name, r))
        .collect();
    assert_eq!(rim.len(), 1);
    let (out, _) = blend(OpKey(7), &cup, &rim).unwrap();
    sound_sheet(&out, 1);
    let want = PI * (rc - r) * (rc - r)
        + 2.0 * PI * rc * (hc - r)
        + 2.0 * PI * r * ((rc - r) * FRAC_PI_2 + r);
    assert!((area(&out) - want).abs() < 1e-7, "{} {want}", area(&out));
}
