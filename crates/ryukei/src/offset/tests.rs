use super::classify::{Class, Plan};
use super::edges::EdgeGeo;
use super::vertices::FillEnd;
use super::*;
use crate::geom::{Curve, Line2, Point2, Point3, Rigid, Surface, Vector3};
use crate::name::{FaceRole, Role};
use crate::primitive;
use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
use crate::topo::{EdgeId, VertexId};
use crate::{OffsetCase, OffsetError};

const K: OpKey = OpKey(1);

fn by(d: f64) -> OffsetSpec {
    OffsetSpec {
        distance: d,
        faces: Vec::new(),
    }
}

fn block() -> Body {
    primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap().0
}

/// `[0, 2] × [0, 2]` less `[1, 2] × [1, 2]`, `h` high.
pub(crate) fn l_block(h: f64) -> Body {
    let pts = [
        (0.0, 0.0),
        (2.0, 0.0),
        (2.0, 1.0),
        (1.0, 1.0),
        (1.0, 2.0),
        (0.0, 2.0),
    ];
    let segments = (0..6)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 6]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: crate::geom::Curve2::Line2(Line2::through(p, q).unwrap()),
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
    extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, h)
        .unwrap()
        .0
}

/// The L's upright edge at its reflex corner.
fn reflex(b: &Body) -> EdgeId {
    b.edges()
        .find(|&e| {
            let x = &b.edges[e];
            let (p, q) = (b.vertices[x.start].point, b.vertices[x.end].point);
            p.x == 1.0 && p.y == 1.0 && q.x == 1.0 && q.y == 1.0
        })
        .expect("the reflex edge")
}

/// The vertex of `b` at `p`.
fn vertex_at(b: &Body, p: Point3) -> VertexId {
    b.vertices()
        .find(|&v| b.vertices[v].point.distance(p) < 1e-12)
        .expect("a vertex there")
}

fn classes(b: &Body, d: f64) -> Vec<Class> {
    let plan = Plan::new(b, &by(d)).unwrap();
    plan.edges.iter().map(|e| plan.class[e].0).collect()
}

#[test]
fn a_boxs_edges_open_outwards_and_close_inwards() {
    let b = block();
    assert!(classes(&b, 0.1).iter().all(|&c| c == Class::Opening));
    assert!(classes(&b, -0.1).iter().all(|&c| c == Class::Closing));
}

#[test]
fn an_ls_reflex_edge_closes_outwards() {
    let b = l_block(0.5);
    let plan = Plan::new(&b, &by(0.1)).unwrap();
    assert_eq!(plan.class[&reflex(&b)].0, Class::Closing);
    let plan = Plan::new(&b, &by(-0.1)).unwrap();
    assert_eq!(plan.class[&reflex(&b)].0, Class::Opening);
}

#[test]
fn faces_at_their_own_distances() {
    let b = block();
    let top = Name::derive(K, Role::Face(FaceRole::PlusZ), &[]);
    // A face held still closes every edge it shares with moving ones.
    let spec = OffsetSpec {
        distance: 0.1,
        faces: vec![(top, 0.0)],
    };
    let plan = Plan::new(&b, &spec).unwrap();
    let on_top = |e: EdgeId| {
        plan.fins(e)
            .iter()
            .any(|&f| b.faces[plan.face_of(f)].name == top)
    };
    for &e in &plan.edges {
        let want = if on_top(e) {
            Class::Closing
        } else {
            Class::Opening
        };
        assert_eq!(plan.class[&e].0, want);
    }
    // Every face still: kept, and refused as no offset at all.
    assert_eq!(
        Plan::new(&b, &by(0.0)).err(),
        Some(OffsetError::BadDistance.into())
    );
    let twice = OffsetSpec {
        distance: 0.1,
        faces: vec![(top, 0.0), (top, 0.2)],
    };
    assert_eq!(
        Plan::new(&b, &twice).err(),
        Some(OffsetError::NoSuchFace { face: top }.into())
    );
    assert_eq!(
        Plan::new(&b, &by(1e-9)).err(),
        Some(OffsetError::BadDistance.into())
    );
}

#[test]
fn a_rounded_edges_rails_are_smooth() {
    let b = block();
    let e = Name::derive(
        K,
        Role::Edge,
        &{
            let mut f = [
                Name::derive(K, Role::Face(FaceRole::PlusX), &[]),
                Name::derive(K, Role::Face(FaceRole::PlusZ), &[]),
            ];
            f.sort();
            f
        },
    );
    let blends = [crate::blend::EdgeBlend {
        edge: e,
        kind: crate::blend::BlendKind::Fillet { radius: 0.1 },
    }];
    let (r, _) = crate::blend::blend(OpKey(2), &b, &blends).unwrap();
    let plan = Plan::new(&r, &by(0.05)).unwrap();
    let smooth: Vec<f64> = plan
        .class
        .values()
        .filter(|c| c.0 == Class::Smooth)
        .map(|c| c.1)
        .collect();
    // The fillet's two rails.
    assert_eq!(smooth.len(), 2);
    assert!(smooth.iter().all(|&g| g <= classify::SMOOTH_GAP));
}

#[test]
fn a_cylinder_shrunk_past_its_axis_collapses() {
    let (c, _) = primitive::cylinder(K, 0.3, 1.0, &Rigid::IDENTITY).unwrap();
    let side = Name::derive(K, Role::Face(FaceRole::Side), &[]);
    assert_eq!(
        Plan::new(&c, &by(-0.3)).err(),
        Some(OffsetError::FaceCollapses { face: side }.into())
    );
}

#[test]
fn a_boxs_edges_fill_with_cylinders_or_meet_on_lines() {
    let b = block();
    let plan = Plan::new(&b, &by(0.1)).unwrap();
    let geo = edges::curves(&plan).unwrap();
    for (&e, g) in &geo {
        let EdgeGeo::Open { rails, fill, .. } = g else {
            panic!("an opening edge");
        };
        let Surface::Cylinder(c) = fill else {
            panic!("a cylinder");
        };
        assert_eq!(c.radius(), 0.1);
        // Each rail is the edge moved along its face's normal, exactly.
        let edge = &b.edges[e];
        for (k, r) in rails.iter().enumerate() {
            assert!(matches!(r.curve, Curve::Line(_)));
            let fin = plan.fins(e)[k];
            let t = edge.interval[0];
            let want = b.curves[edge.curve].point(t) + plan.normal(fin, t) * 0.1;
            assert!(r.curve.point(t).distance(want) < 1e-15);
        }
    }
    let plan = Plan::new(&b, &by(-0.1)).unwrap();
    let geo = edges::curves(&plan).unwrap();
    for g in geo.values() {
        let EdgeGeo::One(c) = g else {
            panic!("a closing edge");
        };
        assert!(matches!(c.curve, Curve::Line(_)));
    }
}

#[test]
fn a_cylinders_rims_fill_with_tori() {
    let (c, _) = primitive::cylinder(K, 0.3, 0.5, &Rigid::IDENTITY).unwrap();
    let plan = Plan::new(&c, &by(0.05)).unwrap();
    let geo = edges::curves(&plan).unwrap();
    for g in geo.values() {
        let EdgeGeo::Open { rails, fill, .. } = g else {
            panic!("an opening rim");
        };
        assert!(matches!(fill, Surface::Torus(_)));
        assert!(rails.iter().all(|r| matches!(r.curve, Curve::Circle(_))));
    }
    // A rim of a cylinder thinner than the distance can't roll a ring.
    let (thin, _) = primitive::cylinder(K, 0.04, 0.5, &Rigid::IDENTITY).unwrap();
    let plan = Plan::new(&thin, &by(0.05)).unwrap();
    assert!(matches!(
        edges::curves(&plan).err(),
        Some(crate::Error::InvalidOffset(OffsetError::Unsupported {
            case: OffsetCase::Fill,
            ..
        }))
    ));
}

#[test]
fn vertex_classes() {
    let b = block();
    // Grown, a corner is a sphere with three arcs.
    let plan = Plan::new(&b, &by(0.1)).unwrap();
    let geo = edges::curves(&plan).unwrap();
    let vg = vertices::vertices(&plan, &geo).unwrap();
    let corner = &vg[&vertex_at(&b, Point3::ORIGIN)];
    assert!(!corner.kept);
    assert_eq!(corner.points.len(), 3);
    assert_eq!(corner.edges.len(), 3);
    let (s, _, lp) = corner.sphere.as_ref().unwrap();
    assert!(matches!(s, Surface::Sphere(_)));
    assert_eq!(lp.len(), 3);
    // Shrunk, it is one point, where three planes meet.
    let plan = Plan::new(&b, &by(-0.1)).unwrap();
    let geo = edges::curves(&plan).unwrap();
    let vg = vertices::vertices(&plan, &geo).unwrap();
    let corner = &vg[&vertex_at(&b, Point3::ORIGIN)];
    assert!(corner.kept);
    assert!(corner.points[0].distance(Point3::new(0.1, 0.1, 0.1)) < 1e-15);
    // A cylinder's rims, grown: each torus wraps round at its vertex.
    let (c, _) = primitive::cylinder(K, 0.3, 0.5, &Rigid::IDENTITY).unwrap();
    let plan = Plan::new(&c, &by(0.05)).unwrap();
    let geo = edges::curves(&plan).unwrap();
    let vg = vertices::vertices(&plan, &geo).unwrap();
    for g in vg.values() {
        assert_eq!(g.points.len(), 2);
        assert!(g.fill_ends.values().all(|f| matches!(f, FillEnd::Wraps)));
    }
}

#[test]
fn an_ls_inner_corner_mixes() {
    let h = 0.5;
    let b = l_block(h);
    let top = vertex_at(&b, Point3::new(1.0, 1.0, h));
    // Shrunk, the reflex edge's quarter cylinder ends on the top's offset:
    // an arc there, two points.
    let plan = Plan::new(&b, &by(-0.1)).unwrap();
    let geo = edges::curves(&plan).unwrap();
    let vg = vertices::vertices(&plan, &geo).unwrap();
    let g = &vg[&top];
    assert_eq!((g.points.len(), g.edges.len(), g.inserts.len()), (2, 1, 1));
    let end = &g.edges[0];
    assert!(matches!(end.curve, Curve::Circle(_)));
    for p in &g.points {
        assert!((p.z - (h - 0.1)).abs() < 1e-12);
    }
    // Grown, the top edges' cylinders trim each other down to where the
    // sides' offsets meet.
    let plan = Plan::new(&b, &by(0.1)).unwrap();
    let geo = edges::curves(&plan).unwrap();
    let vg = vertices::vertices(&plan, &geo).unwrap();
    let g = &vg[&top];
    assert_eq!((g.points.len(), g.edges.len()), (2, 1));
    assert!(g.points[0].distance(Point3::new(1.0, 1.0, h + 0.1)) < 1e-12);
    assert!(g.points[1].distance(Point3::new(1.1, 1.1, h)) < 1e-9);
}

#[test]
fn a_cones_apex_is_refused() {
    let (c, _) = primitive::cone(K, 0.3, 0.5, &Rigid::IDENTITY).unwrap();
    let side = Name::derive(K, Role::Face(FaceRole::Side), &[]);
    assert_eq!(
        offset_body(OpKey(2), &c, &by(0.05)).err(),
        Some(
            OffsetError::Unsupported {
                at: side,
                case: OffsetCase::LoneVertex
            }
            .into()
        )
    );
}
