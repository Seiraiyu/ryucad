//! Blends' names and journals.

use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::{BlendRole, Event, FaceRole, Journal, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::{Body, EntityId};
use std::collections::{BTreeMap, BTreeSet};

const K: OpKey = OpKey(0xb1);

/// Every face, edge and vertex name of a body.
fn names(b: &Body) -> BTreeSet<Name> {
    b.faces()
        .map(EntityId::Face)
        .chain(b.edges().map(EntityId::Edge))
        .chain(b.vertices().map(EntityId::Vertex))
        .map(|x| b.name_of(x).unwrap())
        .collect()
}

fn fillet(edge: Name, radius: f64) -> EdgeBlend {
    EdgeBlend {
        edge,
        kind: BlendKind::Fillet { radius },
    }
}

fn face(key: OpKey, r: FaceRole) -> Name {
    Name::derive(key, Role::Face(r), &[])
}

fn edge(key: OpKey, a: FaceRole, b: FaceRole) -> Name {
    let mut f = [face(key, a), face(key, b)];
    f.sort();
    Name::derive(key, Role::Edge, &f)
}

/// What the journal says became of names, checked against the bodies:
/// every result name is an input name or a new one, every input name that
/// is gone was replaced, and every replacement is a result face.
fn accounts(before: &Body, after: &Body, j: &Journal) -> BTreeMap<Name, Vec<Name>> {
    let (old, new) = (names(before), names(after));
    let mut created = BTreeSet::new();
    let mut replaced = BTreeMap::new();
    for e in j.events() {
        match e {
            Event::Created(n) => {
                assert!(new.contains(n) && !old.contains(n), "{n}");
                created.insert(*n);
            }
            Event::Modified(n) => assert!(new.contains(n) && old.contains(n), "{n}"),
            Event::Replaced { old: o, new: faces } => {
                assert!(old.contains(o) && !new.contains(o), "{o}");
                for f in faces {
                    assert!(matches!(after.by_name(*f), Some(EntityId::Face(_))));
                }
                replaced.insert(*o, faces.clone());
            }
            other => panic!("a blend reported {other:?}"),
        }
    }
    let faces: BTreeSet<Name> = replaced.values().flatten().copied().collect();
    for n in &new {
        assert!(
            old.contains(n) || created.contains(n) || faces.contains(n),
            "{n} has no history"
        );
    }
    for n in &old {
        assert!(new.contains(n) || replaced.contains_key(n), "{n} vanished");
    }
    replaced
}

#[test]
fn a_box_edge_is_replaced_by_its_fillet() {
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let e = edge(k, FaceRole::PlusX, FaceRole::PlusZ);
    let (r, j) = blend(K, &b, &[fillet(e, 0.25)]).unwrap();
    let replaced = accounts(&b, &r, &j);
    let f = Name::derive(K, Role::Blend(BlendRole::Face), &[e]);
    assert_eq!(replaced[&e], [f]);
    // Its two vertices are replaced by it too; the rest of the box keeps
    // its names, the faces either side and across its ends included.
    assert_eq!(replaced.len(), 3);
    assert!(replaced.values().all(|v| v == &[f]));
    for role in [
        FaceRole::PlusX,
        FaceRole::PlusZ,
        FaceRole::PlusY,
        FaceRole::MinusY,
    ] {
        assert!(r.by_name(face(k, role)).is_some());
    }
    // The rails are named by the fillet and the faces it rolls on.
    for side in [FaceRole::PlusX, FaceRole::PlusZ] {
        let rail = Name::derive(K, Role::Blend(BlendRole::Rail), &[f, face(k, side)]);
        assert!(
            matches!(r.by_name(rail), Some(EntityId::Edge(_))),
            "{side:?}"
        );
    }
    for (end, near) in [
        (FaceRole::PlusY, FaceRole::PlusY),
        (FaceRole::MinusY, FaceRole::MinusY),
    ] {
        let mut corner = [
            face(k, FaceRole::PlusX),
            face(k, FaceRole::PlusZ),
            face(k, near),
        ];
        corner.sort();
        let v = Name::derive(k, Role::Vertex, &corner);
        let n = Name::derive(K, Role::Blend(BlendRole::End), &[f, face(k, end), v]);
        assert!(matches!(r.by_name(n), Some(EntityId::Edge(_))), "{end:?}");
    }
}

#[test]
fn resizing_and_moving_a_blend_renames_nothing() {
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let far = Rigid::translation(Vector3::new(5.0, -3.0, 2.0));
    let moved = primitive::block(k, 1.5, 2.0, 3.0, &far).unwrap().0;
    let e = edge(k, FaceRole::PlusX, FaceRole::PlusZ);
    let (x, _) = blend(K, &b, &[fillet(e, 0.25)]).unwrap();
    let (y, _) = blend(K, &moved, &[fillet(e, 0.4)]).unwrap();
    assert_eq!(names(&x), names(&y));
    // Another key names the new entities apart.
    let (z, _) = blend(OpKey(7), &b, &[fillet(e, 0.25)]).unwrap();
    assert_ne!(names(&x), names(&z));
    assert_eq!(names(&x).len(), names(&z).len());
}

/// A slot: a stadium of length 2 and width 1 extruded 1 along `z`.
fn slot() -> Body {
    use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    use std::f64::consts::PI;
    let line = |key: u128, a: (f64, f64), b: (f64, f64)| {
        let (a, b) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
        Segment {
            key: SegmentKey(key),
            curve: Curve2::Line2(Line2::through(a, b).unwrap()),
            t0: 0.0,
            t1: a.distance(b),
        }
    };
    let arc = |key: u128, x: f64, t0: f64| {
        let f = Frame2::new(Point2::new(x, 0.5), Vector2::new(1.0, 0.0), true).unwrap();
        Segment {
            key: SegmentKey(key),
            curve: Curve2::Circle2(Circle2::new(f, 0.5).unwrap()),
            t0,
            t1: t0 + PI,
        }
    };
    let profile = Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![
                line(11, (0.0, 0.0), (2.0, 0.0)),
                arc(12, 2.0, -PI / 2.0),
                line(13, (2.0, 1.0), (0.0, 1.0)),
                arc(14, 0.0, PI / 2.0),
            ],
        }],
    };
    let profile = ValidProfile::new(profile).unwrap();
    extrude(OpKey(1), &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
        .unwrap()
        .0
}

#[test]
fn a_slots_top_is_four_faces_meeting_at_its_old_vertices() {
    let b = slot();
    let top: Vec<Name> = b
        .edges()
        .filter(|&e| {
            let ed = b.edge(e).unwrap();
            [ed.start, ed.end]
                .iter()
                .all(|&v| b.vertex(v).unwrap().point.z > 0.5)
        })
        .map(|e| b.edge(e).unwrap().name)
        .collect();
    let blends: Vec<EdgeBlend> = top.iter().map(|&e| fillet(e, 0.1)).collect();
    let (r, j) = blend(K, &b, &blends).unwrap();
    let replaced = accounts(&b, &r, &j);
    // Four edges, each by its own face; four vertices, each by two.
    assert_eq!(replaced.len(), 8);
    for e in &top {
        assert_eq!(replaced[e].len(), 1);
    }
    assert_eq!(replaced.values().filter(|v| v.len() == 2).count(), 4);
    let bigger: Vec<EdgeBlend> = top.iter().map(|&e| fillet(e, 0.15)).collect();
    let (s, _) = blend(K, &b, &bigger).unwrap();
    assert_eq!(names(&r), names(&s));
}

#[test]
fn a_rim_accounts_for_every_name() {
    let b = primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rim = b.edges().next().unwrap();
    let n = b.edge(rim).unwrap().name;
    let (r, j) = blend(K, &b, &[fillet(n, 0.2)]).unwrap();
    let replaced = accounts(&b, &r, &j);
    assert_eq!(replaced.len(), 2);
    let (s, _) = blend(K, &b, &[fillet(n, 0.3)]).unwrap();
    assert_eq!(names(&r), names(&s));
}

#[test]
fn edges_and_faces_two_blends_cut_keep_their_names() {
    use FaceRole::*;
    let k = OpKey(1);
    // The box's four upright edges: every edge round its top and bottom
    // is cut at both ends.
    let b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let upright = [
        edge(k, PlusX, PlusY),
        edge(k, PlusX, MinusY),
        edge(k, MinusX, PlusY),
        edge(k, MinusX, MinusY),
    ];
    let (r, j) = blend(K, &b, &upright.map(|e| fillet(e, 0.2))).unwrap();
    accounts(&b, &r, &j);
    // A cylinder's two rims: each rail is named from the face it rolls
    // on as it was before the blend, whichever rim was built first.
    let c = primitive::cylinder(k, 1.0, 1.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rims: Vec<Name> = c.edges().map(|e| c.edge(e).unwrap().name).collect();
    let (r, j) = blend(
        K,
        &c,
        &rims.iter().map(|&e| fillet(e, 0.2)).collect::<Vec<_>>(),
    )
    .unwrap();
    accounts(&c, &r, &j);
    for e in rims {
        let f = Name::derive(K, Role::Blend(BlendRole::Face), &[e]);
        let on = |side| {
            let rail = Name::derive(K, Role::Blend(BlendRole::Rail), &[f, face(k, side)]);
            matches!(r.by_name(rail), Some(EntityId::Edge(_)))
        };
        assert!(on(Side) && (on(Top) || on(Bottom)), "{e}");
    }
}

/// A box's vertex between three of its faces.
fn vertex(key: OpKey, a: FaceRole, b: FaceRole, c: FaceRole) -> Name {
    let mut f = [face(key, a), face(key, b), face(key, c)];
    f.sort();
    Name::derive(key, Role::Vertex, &f)
}

#[test]
fn a_ball_corner_replaces_its_vertex() {
    use FaceRole::*;
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let edges = [
        edge(k, PlusX, PlusY),
        edge(k, PlusY, PlusZ),
        edge(k, PlusX, PlusZ),
    ];
    let (r, j) = blend(K, &b, &edges.map(|e| fillet(e, 0.2))).unwrap();
    let replaced = accounts(&b, &r, &j);
    let v = vertex(k, PlusX, PlusY, PlusZ);
    // The corner face, from the vertex and the faces it touches.
    let mut faces = [face(k, PlusX), face(k, PlusY), face(k, PlusZ)];
    faces.sort();
    let corner = Name::derive(
        K,
        Role::Blend(BlendRole::Corner),
        &[v, faces[0], faces[1], faces[2]],
    );
    assert!(matches!(r.by_name(corner), Some(EntityId::Face(_))));
    let blend_faces = edges.map(|e| Name::derive(K, Role::Blend(BlendRole::Face), &[e]));
    let mut want: Vec<Name> = blend_faces.into_iter().chain([corner]).collect();
    want.sort();
    assert_eq!(replaced[&v], want);
    // Each section between a blend face and the corner face.
    for f in blend_faces {
        let mut parents = [f, corner];
        parents.sort();
        let n = Name::derive(K, Role::Blend(BlendRole::Section), &parents);
        assert!(matches!(r.by_name(n), Some(EntityId::Edge(_))));
    }
    let (s, _) = blend(K, &b, &edges.map(|e| fillet(e, 0.25))).unwrap();
    assert_eq!(names(&r), names(&s));
}
