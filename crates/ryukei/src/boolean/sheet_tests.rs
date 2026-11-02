//! Sheets trimmed by solids and divided by sheets (plan 12.D).

use super::*;
use crate::check::{check, check_intersections};
use crate::geom::{Frame, Plane, Surface, Vector3};
use crate::measure::{Entity, Properties, properties};
use crate::primitive;
use crate::sheet::from_surface;

/// A plane sheet at height `z` over `[x0, x1] × [y0, y1]`, facing up.
pub(super) fn square(key: u128, z: f64, [x0, x1]: [f64; 2], [y0, y1]: [f64; 2]) -> Body {
    let frame = Frame::WORLD.transformed(&Rigid::translation(Vector3::new(0.0, 0.0, z)));
    from_surface(
        OpKey(key),
        &Surface::Plane(Plane::new(frame)),
        [[x0, x1], [y0, y1]],
    )
    .unwrap()
    .0
}

pub(super) fn block(key: u128, [x, y, z]: [f64; 3], [w, d, h]: [f64; 3]) -> Body {
    let at = Rigid::translation(Vector3::new(x, y, z));
    primitive::block(OpKey(key), w, d, h, &at).unwrap().0
}

pub(super) fn tool(key: u128, body: &Body) -> Tool<'_> {
    Tool {
        key: ToolKey(key),
        body,
        place: Rigid::IDENTITY,
    }
}

pub(super) fn area(b: &Body) -> f64 {
    match properties(b, Entity::Body).unwrap() {
        Properties::Sheet { area } => area.value(),
        p => panic!("not a sheet: {p:?}"),
    }
}

/// A sheet that checks clean, its edges laminar exactly where `laminar`
/// counts.
pub(super) fn sound_sheet(b: &Body, laminar: usize) {
    assert_eq!(b.kind(), BodyKind::Sheet);
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    assert_eq!(b.edges().filter(|&e| b.is_laminar(e)).count(), laminar);
}

#[test]
fn a_square_sheet_is_trimmed_by_a_block() {
    let s = square(1, 0.5, [-1.0, 3.0], [-1.0, 3.0]);
    let b = block(2, [0.0, 0.0, 0.0], [2.0, 2.0, 1.0]);
    let (inside, _) = boolean(OpKey(3), BooleanOp::Intersect, &s, &[tool(4, &b)]).unwrap();
    sound_sheet(&inside, 4);
    assert_eq!(inside.faces().count(), 1);
    assert!((area(&inside) - 4.0).abs() < 1e-9);
    let (outside, _) = boolean(OpKey(3), BooleanOp::Subtract, &s, &[tool(4, &b)]).unwrap();
    sound_sheet(&outside, 8);
    assert_eq!(outside.faces().count(), 1);
    assert!((area(&outside) - 12.0).abs() < 1e-9);
}

#[test]
fn a_sheet_is_drilled_by_a_cylinder() {
    let s = square(1, 0.5, [0.0, 4.0], [0.0, 4.0]);
    let at = Rigid::translation(Vector3::new(2.0, 2.0, -1.0));
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &at).unwrap().0;
    let (holed, _) = boolean(OpKey(3), BooleanOp::Subtract, &s, &[tool(4, &rod)]).unwrap();
    sound_sheet(&holed, 5);
    let disc = core::f64::consts::PI * 0.25;
    assert!((area(&holed) - (16.0 - disc)).abs() < 1e-6);
    let (plug, _) = boolean(OpKey(3), BooleanOp::Intersect, &s, &[tool(4, &rod)]).unwrap();
    sound_sheet(&plug, 1);
    assert!((area(&plug) - disc).abs() < 1e-6);
}

#[test]
fn a_sheet_lying_on_a_face_is_inside() {
    // On the block's top: the piece over the block is on it, and goes
    // with what is inside, so the two results still make the sheet.
    let s = square(1, 1.0, [-1.0, 3.0], [-1.0, 3.0]);
    let b = block(2, [0.0, 0.0, 0.0], [2.0, 2.0, 1.0]);
    let (on, _) = boolean(OpKey(3), BooleanOp::Intersect, &s, &[tool(4, &b)]).unwrap();
    sound_sheet(&on, 4);
    assert!((area(&on) - 4.0).abs() < 1e-9);
    let (off, _) = boolean(OpKey(3), BooleanOp::Subtract, &s, &[tool(4, &b)]).unwrap();
    sound_sheet(&off, 8);
    assert!((area(&off) - 12.0).abs() < 1e-9);
}

#[test]
fn a_sheet_the_tools_miss_is_kept_or_emptied() {
    let s = square(1, 5.0, [0.0, 1.0], [0.0, 1.0]);
    let b = block(2, [0.0, 0.0, 0.0], [2.0, 2.0, 1.0]);
    let (same, _) = boolean(OpKey(3), BooleanOp::Subtract, &s, &[tool(4, &b)]).unwrap();
    sound_sheet(&same, 4);
    let names = |b: &Body| {
        let mut n: Vec<Name> = b.faces().map(|f| b.faces[f].name).collect();
        n.extend(b.edges().map(|e| b.edges[e].name));
        n.sort();
        n
    };
    assert_eq!(names(&same), names(&s));
    let (none, _) = boolean(OpKey(3), BooleanOp::Intersect, &s, &[tool(4, &b)]).unwrap();
    assert_eq!(none.faces().count(), 0);
}

#[test]
fn unite_and_sheet_tools_are_refused() {
    let s = square(1, 0.5, [-1.0, 3.0], [-1.0, 3.0]);
    let b = block(2, [0.0, 0.0, 0.0], [2.0, 2.0, 1.0]);
    assert_eq!(
        boolean(OpKey(3), BooleanOp::Unite, &s, &[tool(4, &b)]).unwrap_err(),
        Error::NotASolid {
            op: "boolean unite"
        }
    );
    assert_eq!(
        boolean(OpKey(3), BooleanOp::Subtract, &b, &[tool(4, &s)]).unwrap_err(),
        Error::NotASolid { op: "boolean" }
    );
}

#[test]
fn trimming_by_two_tools_takes_their_union() {
    let s = square(1, 0.5, [-1.0, 5.0], [0.5, 1.5]);
    let a = block(2, [0.0, 0.0, 0.0], [2.0, 2.0, 1.0]);
    let b = block(3, [1.0, 0.0, 0.0], [3.0, 2.0, 1.0]);
    let (inside, _) = boolean(
        OpKey(4),
        BooleanOp::Intersect,
        &s,
        &[tool(5, &a), tool(6, &b)],
    )
    .unwrap();
    // [0, 4] × [0.5, 1.5]: one piece, merged back across the tools.
    assert!((area(&inside) - 4.0).abs() < 1e-9);
    assert_eq!(inside.faces().count(), 1);
    sound_sheet(&inside, 4);
}
