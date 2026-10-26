use super::*;
use crate::geom::pipe::tests::on_plane;
use crate::geom::{Rigid, Vector2};
use crate::name::{OpKey, Role};
use crate::primitive;
use crate::topo::fixture::{hollow_cube, tetrahedron};
use crate::topo::{CurveId, EntityId};

fn round_trip(b: &Body) -> Body {
    let bytes = encode_body(b);
    assert!(bytes.starts_with(BODY_MAGIC));
    let d = decode_body(&bytes).unwrap();
    assert_eq!(d.dump(), b.dump());
    assert_eq!(encode_body(&d), bytes, "a decoded body encodes alike");
    d
}

/// A tetrahedron carrying every kind of curve, surface and pcurve, some
/// NURBS shared by several entries and by a swept surface.
fn every_kind() -> Body {
    let mut b = tetrahedron();
    let f = Frame::new(
        Point3::new(0.1, 0.2, 0.3),
        Vector3::new(0.2, 0.3, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    b.add_curve(Curve::Circle(Circle::new(f, 2.0).unwrap()));
    b.add_curve(Curve::Ellipse(Ellipse::new(f, 3.0, 2.0).unwrap()));
    let (n, _) = Circle::new(f, 1.5).unwrap().to_nurbs().unwrap();
    let shared = Arc::new(n);
    b.add_curve(Curve::Nurbs(shared.clone()));
    b.add_curve(Curve::Nurbs(shared.clone()));
    b.add_surface(Surface::Cylinder(Cylinder::new(f, 1.0).unwrap()));
    b.add_surface(Surface::Cone(Cone::new(f, 1.0, 0.4).unwrap()));
    b.add_surface(Surface::Sphere(Sphere::new(f, 1.0).unwrap()));
    b.add_surface(Surface::Torus(Torus::new(f, 2.0, 0.5).unwrap()));
    let (s, _) = Surface::Plane(Plane::new(f))
        .to_nurbs([[0.0, 1.0], [0.0, 2.0]])
        .unwrap();
    b.add_surface(Surface::Nurbs(Arc::new(s)));
    let e = Extrusion::new(Curve::Nurbs(shared), Vector3::new(0.3, 0.1, 1.0)).unwrap();
    let e = Arc::new(e);
    b.add_surface(Surface::Extrusion(e.clone()));
    b.add_surface(Surface::Extrusion(e));
    let tube = Frame::new(
        Point3::new(3.0, 0.0, 0.0),
        Vector3::new(0.0, -1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let tube = Curve::Circle(Circle::new(tube, 1.0).unwrap());
    let r = Revolution::new(Frame::WORLD, tube).unwrap();
    b.add_surface(Surface::Revolution(Arc::new(r)));
    b.add_surface(Surface::Pipe(Arc::new(on_plane())));
    let f2 = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(1.0, 0.3), false).unwrap();
    b.add_curve2(Curve2::Line2(
        Line2::new(Point2::new(0.0, 0.5), Vector2::new(0.3, 1.0)).unwrap(),
    ));
    b.add_curve2(Curve2::Circle2(Circle2::new(f2, 3.0).unwrap()));
    b.add_curve2(Curve2::Ellipse2(Ellipse2::new(f2, 3.0, 2.0).unwrap()));
    let knots = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0], &[1, 1, 1, 1]).unwrap();
    let pts = vec![
        Point2::new(0.0, 0.0),
        Point2::new(1.0, 1.0),
        Point2::new(2.0, 0.0),
    ];
    let n2 = NurbsCurve2::new(knots, pts, Some(vec![1.0, 0.5, 2.0])).unwrap();
    let n2 = Arc::new(n2);
    b.add_curve2(Curve2::Nurbs2(n2.clone()));
    b.add_curve2(Curve2::Nurbs2(n2));
    b
}

#[test]
fn every_kind_of_geometry_round_trips_bit_for_bit() {
    round_trip(&every_kind());
}

#[test]
fn shared_geometry_stays_shared() {
    let b = every_kind();
    let d = round_trip(&b);
    let nurbs = |b: &Body| -> Vec<Arc<NurbsCurve>> {
        b.curves
            .live()
            .filter_map(|(_, c)| match c {
                Curve::Nurbs(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    };
    let n = nurbs(&d);
    assert_eq!(n.len(), 2);
    assert!(Arc::ptr_eq(&n[0], &n[1]));
    let swept: Vec<_> = d
        .surfaces
        .live()
        .filter_map(|(_, s)| match s {
            Surface::Extrusion(e) => Some(e.clone()),
            _ => None,
        })
        .collect();
    assert!(Arc::ptr_eq(&swept[0], &swept[1]));
    let Curve::Nurbs(inner) = swept[0].curve() else {
        panic!("a NURBS generator")
    };
    assert!(Arc::ptr_eq(inner, &n[0]), "shared across a swept surface");
    let n2: Vec<_> = d
        .curves2
        .live()
        .filter_map(|(_, c)| match c {
            Curve2::Nurbs2(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    assert!(Arc::ptr_eq(&n2[0], &n2[1]));
}

#[test]
fn tombstones_ids_and_compaction_survive() {
    let mut b = every_kind();
    b.curves.kill(CurveId::from_index(7));
    let mut d = round_trip(&b);
    assert_eq!(d.curves.slot_count(), b.curves.slot_count());
    assert_eq!(d.curve(CurveId::from_index(7)), None);
    let (rb, rd) = (b.compact(), d.compact());
    assert_eq!(rb, rd);
    assert_eq!(b.dump(), d.dump());
}

#[test]
fn names_recipes_and_their_absence_survive() {
    let b = tetrahedron();
    let d = round_trip(&b);
    for f in b.faces() {
        let n = b.name_of(EntityId::Face(f)).unwrap();
        assert_eq!(d.by_name(n), Some(EntityId::Face(f)));
        assert_eq!(d.recipe(n), b.recipe(n));
    }
    let mut quiet = tetrahedron();
    quiet.set_recipes(false);
    let d = round_trip(&quiet);
    assert!(d.recipes.is_none());
}

#[test]
fn primitives_and_cavities_round_trip_and_check() {
    let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let k = OpKey(1);
    let bodies = [
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
        primitive::sphere(k, 1.0, &t).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
        hollow_cube(),
        Body::new(),
    ];
    for b in &bodies {
        let d = round_trip(b);
        assert_eq!(crate::check::check(&d), crate::check::check(b));
    }
}

#[test]
fn bytes_cut_short_or_added_to_are_refused() {
    let bytes = encode_body(&every_kind());
    for n in 0..bytes.len() {
        assert!(
            matches!(decode_body(&bytes[..n]), Err(Error::InvalidEncoding { .. })),
            "{n}"
        );
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(
        decode_body(&longer).unwrap_err(),
        Error::InvalidEncoding {
            what: "bytes after the end"
        }
    );
    let mut other = bytes;
    other[12] = b'2';
    assert_eq!(
        decode_body(&other).unwrap_err(),
        Error::InvalidEncoding {
            what: "not this format or version"
        }
    );
}

#[test]
fn corrupted_bytes_are_refused_or_decode_never_panicking() {
    let bytes = encode_body(&tetrahedron());
    for i in BODY_MAGIC.len()..bytes.len() {
        for flip in [0x01u8, 0x80, 0xff] {
            let mut b = bytes.clone();
            b[i] ^= flip;
            if let Ok(d) = decode_body(&b) {
                // Whatever decodes is structurally sound.
                assert_eq!(check_topology(&d), []);
            }
        }
    }
}

#[test]
fn a_broken_ring_is_refused() {
    let mut b = tetrahedron();
    let f = b.faces().next().unwrap();
    let other = b.faces().nth(2).unwrap();
    b.faces[f].next = other;
    assert_eq!(
        decode_body(&encode_body(&b)).unwrap_err(),
        Error::InvalidEncoding {
            what: "a body the structural checks refuse"
        }
    );
}

#[test]
fn journals_round_trip_every_event() {
    let n = |k| Name::derive(OpKey(k), Role::NewFace, &[]);
    let mut j = Journal::new();
    j.push(Event::Created(n(1)));
    j.push(Event::Deleted(n(2)));
    j.push(Event::Modified(n(3)));
    j.push(Event::Split {
        parent: n(4),
        children: vec![n(5), n(6)],
    });
    j.push(Event::merged(n(7), n(8), n(9)));
    j.push(Event::Placed {
        tool: ToolKey(u128::MAX - 3),
        from: n(10),
        to: n(11),
    });
    j.push(Event::Replaced {
        old: n(12),
        new: vec![],
    });
    let bytes = encode_journal(&j);
    assert!(bytes.starts_with(JOURNAL_MAGIC));
    assert_eq!(decode_journal(&bytes).unwrap(), j);
    assert_eq!(
        decode_journal(&encode_journal(&Journal::new())).unwrap(),
        Journal::new()
    );
    for k in 0..bytes.len() {
        assert!(decode_journal(&bytes[..k]).is_err(), "{k}");
    }
    assert!(decode_journal(&encode_body(&tetrahedron())).is_err());
    assert!(decode_body(&bytes).is_err());
}
