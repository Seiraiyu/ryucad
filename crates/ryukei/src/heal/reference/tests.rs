use super::*;
use crate::check::check_topology;
use crate::geom::{Rigid, Vector3};
use crate::heal::tests::plain;
use crate::heal::{HealOptions, heal};
use crate::mesh::{MeshParams, tessellate};
use crate::primitive;
use crate::topo::BodyKind;
use std::collections::BTreeSet;

pub(super) fn tilted() -> Rigid {
    Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7)
}

/// A reference of `s` under key 7, checked as every reference must be:
/// its topology sound, a sheet, every face meshed.
pub(super) fn reference_ok(s: &Shape) -> (Body, ReferenceReport) {
    let (b, report) = reference(OpKey(7), s, &ReferenceOptions::default()).unwrap();
    assert_eq!(check_topology(&b), []);
    assert_eq!(b.kind(), BodyKind::Sheet);
    let m = tessellate(&b, &MeshParams::new(1e-2, 0.5)).unwrap();
    assert_eq!(m.faults, []);
    (b, report)
}

/// A body's counts but its regions: a sheet has the outer void alone,
/// a solid that and its solid region.
pub(super) fn counts(b: &Body) -> crate::topo::Counts {
    crate::topo::Counts {
        regions: 0,
        ..b.counts()
    }
}

pub(super) fn face_names(b: &Body) -> BTreeSet<Name> {
    b.faces().map(|f| b.faces[f].name).collect()
}

#[test]
fn a_block_builds_as_a_closed_sheet_named_as_healing_names_it() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &tilted())
        .unwrap()
        .0;
    let s = plain(&b);
    let (r, report) = reference_ok(&s);
    assert_eq!(counts(&r), counts(&b));
    // Closed: no edge is laminar.
    assert_eq!(report.laminar, []);
    assert_eq!(report, ReferenceReport::default());
    let (h, _, _) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    assert_eq!(face_names(&r), face_names(&h));
    // Every edge and vertex too: nothing was sewn or joined.
    let names = |b: &Body| -> BTreeSet<Name> {
        b.edges()
            .map(|e| b.edges[e].name)
            .chain(b.vertices().map(|v| b.vertices[v].name))
            .collect()
    };
    assert_eq!(names(&r), names(&h));
}

#[test]
fn an_open_shell_is_an_open_sheet_with_its_boundary_laminar() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    let gone = s.faces.len() - 1;
    let Loop::Edges(uses) = &s.faces[gone].loops[0] else {
        panic!()
    };
    let names = super::names::names(OpKey(7), &s);
    let mut free: Vec<Name> = uses.iter().map(|u| names.edges[u.edge]).collect();
    free.sort();
    // The last face taken out of its shell and the shape.
    s.faces.pop();
    s.shells[0].faces.retain(|&f| f != gone);
    let (r, report) = reference_ok(&s);
    assert_eq!(r.faces().count(), 5);
    assert_eq!(report.laminar, free);
}

#[test]
fn malformed_shapes_are_refused_as_healing_refuses_them() {
    let mut s = Shape::default();
    s.vertices.push(super::super::Vertex {
        point: crate::geom::Point3::new(0.0, 0.0, 0.0),
        tolerance: -1.0,
        seam: false,
    });
    assert!(matches!(
        reference(OpKey(7), &s, &ReferenceOptions::default()),
        Err(Error::InvalidHeal(HealError::Malformed { .. }))
    ));
    // Nothing to lay out at all.
    assert!(matches!(
        reference(OpKey(7), &Shape::default(), &ReferenceOptions::default()),
        Err(Error::InvalidHeal(HealError::Malformed { .. }))
    ));
}
