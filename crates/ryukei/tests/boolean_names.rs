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
