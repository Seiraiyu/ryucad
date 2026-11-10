//! Unit tests of blend recognition and editing.

use super::recognise::{self, Canal};
use super::remake::{close, follow};
use super::*;
use crate::blend::{Chamfer, EdgeBlend, blend};
use crate::geom::{Circle, Curve, Frame, Point3, Rigid, Vector3};
use crate::name::{Event, FaceRole, Role};
use crate::primitive;
use crate::topo::EntityId;

const K: OpKey = OpKey(0x13c0);

fn face(r: FaceRole) -> Name {
    Name::derive(K, Role::Face(r), &[])
}

fn block() -> Body {
    primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY)
        .unwrap()
        .0
}

fn every_edge(b: &Body, kind: BlendKind) -> Body {
    let all: Vec<EdgeBlend> = b
        .edges()
        .map(|e| EdgeBlend {
            edge: b.edges[e].name,
            kind,
        })
        .collect();
    blend(OpKey(7), b, &all).unwrap().0
}

#[test]
fn a_spine_stretch_unwraps_across_its_period() {
    let fr = Frame::new(
        Point3::ORIGIN,
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let canal = Canal {
        spine: Curve::Circle(Circle::new(fr, 1.0).unwrap()),
    };
    // A rounded box's top fillet on the -y side runs at angles near 3π/2;
    // any arc read back must come out increasing and under a turn.
    let r = every_edge(&block(), BlendKind::Fillet { radius: 0.05 });
    for e in r.edges() {
        let [a, b] = canal.under(&r, e);
        assert!(
            a <= b && b - a <= 2.0 * core::f64::consts::PI + 1e-12,
            "{a} {b}"
        );
    }
}

#[test]
fn a_mitred_chamfer_takes_the_supports_it_sets_back_least() {
    let c = every_edge(
        &block(),
        BlendKind::Chamfer(Chamfer::Equal { distance: 0.05 }),
    );
    let tol = DEFAULT_TOLERANCE;
    let mut found = 0;
    for f in c.faces() {
        if let Some(cand) = recognise::chamfer(&c, f, tol) {
            found += 1;
            // Its supports are box faces, never another chamfer.
            for s in cand.sides {
                assert!(recognise::chamfer(&c, s, tol).is_none());
            }
        }
    }
    assert_eq!(found, 12);
}

#[test]
fn picking_one_blend_closes_over_the_corners_it_reaches() {
    let r = every_edge(&block(), BlendKind::Fillet { radius: 0.05 });
    // unmetered: a test
    let rec = recognise::recognise(&r, &RecogniseOptions::default(), &Meter::free()).unwrap();
    // One edge's fillet reaches two corners, and through them every
    // edge: a rounded box's fillets go together.
    let (blends, faces) = close(&rec, [0].into());
    assert_eq!(blends.len(), 12);
    assert_eq!(faces.len(), 20);
}

#[test]
fn a_name_is_followed_through_placings_and_one_live_piece() {
    let b = block();
    let top = face(FaceRole::PlusZ);
    let (placed, piece) = (Name::from_u128(1), face(FaceRole::MinusZ));
    let mut j = Journal::new();
    j.push(Event::Placed {
        tool: crate::name::ToolKey(1),
        from: top,
        to: placed,
    });
    j.push(Event::Split {
        parent: placed,
        children: vec![piece, Name::from_u128(2)],
    });
    // Of the two pieces only `piece` lives in `b`.
    assert_eq!(follow(top, &[j], &b), b.by_name(piece));
    assert!(matches!(b.by_name(piece), Some(EntityId::Face(_))));
}
