//! Booleans' names and journals.

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::{Event, Journal, Name, OpKey};
use ryukei::primitive;
use ryukei::topo::{Body, EntityId};
use std::collections::BTreeSet;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// Every face, edge and vertex name of a body.
fn names(b: &Body) -> BTreeSet<Name> {
    b.faces()
        .map(EntityId::Face)
        .chain(b.edges().map(EntityId::Edge))
        .chain(b.vertices().map(EntityId::Vertex))
        .map(|x| b.name_of(x).unwrap())
        .collect()
}

fn holes(plate: &Body, rod: &Body, places: &[Rigid]) -> (Body, Journal) {
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, &place)| Tool {
            key: ToolKey(i as u128 + 1),
            body: rod,
            place,
        })
        .collect();
    boolean(OpKey(9), BooleanOp::Subtract, plate, &tools).unwrap()
}

#[test]
fn moving_and_resizing_a_tool_renames_nothing() {
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let thin = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let thick = primitive::cylinder(OpKey(2), 0.7, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (a, _) = holes(&plate, &thin, &[at(2.0, 2.0, -1.0)]);
    let (b, _) = holes(&plate, &thick, &[at(1.5, 2.6, -1.0)]);
    assert_eq!(names(&a), names(&b));
}

#[test]
fn copies_of_one_tool_get_distinct_names() {
    let plate = primitive::block(OpKey(1), 6.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.4, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = (0..3)
        .map(|i| at(1.0 + 2.0 * i as f64, 1.0, -1.0))
        .collect();
    let (b, _) = holes(&plate, &rod, &places);
    let n = b.counts();
    assert_eq!(names(&b).len(), n.faces + n.edges + n.vertices);
}

#[test]
fn untouched_faces_keep_their_names_and_split_faces_get_pieces() {
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (b, _) = holes(&plate, &rod, &[at(2.0, 2.0, -1.0)]);
    let before = names(&plate);
    // The four sides and their edges and corners are untouched; the top
    // and bottom keep their names too, with a hole each.
    let faces: BTreeSet<Name> = plate
        .faces()
        .map(|f| plate.name_of(EntityId::Face(f)).unwrap())
        .collect();
    let after = names(&b);
    assert!(faces.iter().all(|n| after.contains(n)));
    assert_eq!(
        before.iter().filter(|n| after.contains(n)).count(),
        6 + 12 + 8
    );
    // A slab cutting the plate in two: its top and bottom and two sides
    // are split, each into two pieces with names of their own.
    let slab = primitive::block(OpKey(3), 1.0, 6.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools = [Tool {
        key: ToolKey(1),
        body: &slab,
        place: at(1.5, -1.0, -1.0),
    }];
    let (two, j) = boolean(OpKey(9), BooleanOp::Subtract, &plate, &tools).unwrap();
    assert_eq!(two.regions().count(), 3);
    let splits = j
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Split { children, .. } if children.len() == 2))
        .count();
    assert!(splits >= 4, "{splits}");
}

#[test]
fn the_journal_accounts_for_every_name() {
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (b, j) = holes(&plate, &rod, &[at(2.0, 2.0, -1.0), at(1.0, 1.0, -1.0)]);
    let mut made: BTreeSet<Name> = names(&plate);
    let mut gone: BTreeSet<Name> = BTreeSet::new();
    let mut inputs: BTreeSet<Name> = names(&plate);
    for e in j.events() {
        match e {
            Event::Created(n) | Event::Modified(n) => {
                made.insert(*n);
            }
            Event::Deleted(n) => {
                gone.insert(*n);
            }
            Event::Split { parent, children } => {
                gone.insert(*parent);
                made.extend(children.iter().copied());
            }
            Event::Merged { parents, child } => {
                gone.extend(parents.iter().copied());
                made.insert(*child);
            }
            Event::Placed { from, to, .. } => {
                inputs.insert(*from);
                made.insert(*to);
            }
            Event::Replaced { .. } => panic!("a boolean replaces nothing"),
        }
    }
    let after = names(&b);
    // Every result name came from somewhere.
    for n in &after {
        assert!(made.contains(n), "{n} has no history");
    }
    // Every placed tool name is in the result, split, merged or deleted.
    for e in j.events() {
        if let Event::Placed { to, .. } = e {
            assert!(after.contains(to) || gone.contains(to), "{to} vanished");
        }
    }
    assert!(!gone.is_empty());
}

#[test]
fn a_face_touched_flush_from_outside_keeps_its_name() {
    // Issue #89: a block under another, its top flush with the other's
    // bottom. The bottom is split where the tool's top lies on it and the
    // pieces merged back: it is the face it was, under its own name, and
    // the journal says nothing happened to it.
    let a = primitive::block(OpKey(1), 10.0, 10.0, 10.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let t = primitive::block(OpKey(2), 4.0, 4.0, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools = [Tool {
        key: ToolKey(1),
        body: &t,
        place: at(3.0, 3.0, -4.0),
    }];
    let (b, journal) = boolean(OpKey(9), BooleanOp::Subtract, &a, &tools).unwrap();
    let faces = |x: &Body| -> BTreeSet<Name> {
        x.faces()
            .map(|f| x.name_of(EntityId::Face(f)).unwrap())
            .collect()
    };
    let (before, after) = (faces(&a), faces(&b));
    assert_eq!(before, after);
    for e in journal.events() {
        let touched = match e {
            Event::Split { parent, .. } => before.contains(parent),
            Event::Merged { parents, .. } => parents.iter().any(|p| before.contains(p)),
            Event::Modified(n) | Event::Deleted(n) => before.contains(n),
            _ => false,
        };
        assert!(!touched, "{e:?}");
    }
}

/// A disc of radius 1 and height 1 whose side is two faces on one
/// cylinder (the circle drawn as two half arcs), as files written by
/// other CAD hold a plated hole's wall: two faces on one surface either
/// side of an edge, and two edges on one circle either side of a vertex.
fn split_disc() -> Body {
    use core::f64::consts::PI;
    use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Point2, Vector2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    let f = Frame2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.0), true).unwrap();
    let arc = |key: u128, t0: f64| Segment {
        key: SegmentKey(key),
        curve: Curve2::Circle2(Circle2::new(f, 1.0).unwrap()),
        t0,
        t1: t0 + PI,
    };
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![arc(11, 0.0), arc(12, PI)],
        }],
    })
    .unwrap();
    extrude(OpKey(1), &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
        .unwrap()
        .0
}

/// Issue #204: a boolean merges only what it made. Faces on one surface
/// either side of an edge the target had, and edges on one curve either
/// side of a vertex it had, stay as they were when the tool is nowhere
/// near them: a hole through the disc's middle renames none of its side.
#[test]
fn faces_on_one_surface_the_target_had_keep_their_names() {
    let disc = split_disc();
    assert_eq!(disc.counts().faces, 4);
    let rod = primitive::cylinder(OpKey(2), 0.2, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (b, journal) = holes(&disc, &rod, &[at(0.0, 0.0, -1.0)]);
    // Both side faces, both laterals, all four cap arcs and their
    // vertices are as they were; the caps keep their names with a hole.
    let n = b.counts();
    assert_eq!((n.faces, n.edges, n.vertices), (5, 8, 6));
    let lost: Vec<Name> = names(&disc).difference(&names(&b)).copied().collect();
    assert_eq!(lost, []);
    assert!(
        journal
            .events()
            .iter()
            .all(|e| !matches!(e, Event::Merged { .. })),
        "{journal:?}"
    );
}
