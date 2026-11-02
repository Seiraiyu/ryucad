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

/// A plane sheet at `x`, facing +x, over `[y0, y1] × [z0, z1]`.
pub(super) fn wall(key: u128, x: f64, ys: [f64; 2], zs: [f64; 2]) -> Body {
    let frame = Frame::new(
        crate::geom::Point3::new(x, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    from_surface(OpKey(key), &Surface::Plane(Plane::new(frame)), [ys, zs])
        .unwrap()
        .0
}

pub(super) fn volume(b: &Body) -> f64 {
    match properties(b, Entity::Body).unwrap() {
        Properties::Body { volume, .. } => volume.value(),
        p => panic!("not a solid: {p:?}"),
    }
}

#[test]
fn a_sheet_is_divided_by_a_sheet() {
    let s = square(1, 0.0, [0.0, 4.0], [0.0, 4.0]);
    let w = wall(2, 1.0, [-1.0, 5.0], [-1.0, 1.0]);
    let (d, _) = divide(OpKey(3), &s, &[tool(4, &w)]).unwrap();
    assert_eq!((d.front.len(), d.back.len(), d.apart.len()), (1, 1, 0));
    sound_sheet(&d.front[0], 4);
    sound_sheet(&d.back[0], 4);
    assert!((area(&d.front[0]) - 12.0).abs() < 1e-9);
    assert!((area(&d.back[0]) - 4.0).abs() < 1e-9);
}

#[test]
fn a_box_is_divided_by_a_plane_sheet() {
    let b = block(1, [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let s = square(2, 0.5, [-1.0, 3.0], [-1.0, 3.0]);
    let (d, j) = divide(OpKey(3), &b, &[tool(4, &s)]).unwrap();
    assert_eq!((d.front.len(), d.back.len(), d.apart.len()), (1, 1, 0));
    for (body, v) in [(&d.front[0], 6.0), (&d.back[0], 2.0)] {
        assert_eq!(body.kind(), BodyKind::Solid);
        assert_eq!(check(body), []);
        assert_eq!(check_intersections(body), []);
        assert_eq!(body.faces().count(), 6);
        assert!((volume(body) - v).abs() < 1e-9);
    }
    // The tool's face is in both, by a name of its own on each side.
    let top = |b: &Body| -> Vec<Name> {
        let mut n: Vec<Name> = b.faces().map(|f| b.faces[f].name).collect();
        n.sort();
        n
    };
    let (f, k) = (top(&d.front[0]), top(&d.back[0]));
    assert!(f.iter().all(|n| !k.contains(n)));
    // The four sides and the four edges up them are split between the
    // sides, and nothing else.
    let splits = j
        .events()
        .iter()
        .filter(|e| matches!(e, crate::name::Event::Split { children, .. } if children.len() == 2))
        .count();
    assert_eq!(splits, 4 + 4, "{j:?}");
    assert!(
        j.events().iter().all(
            |e| !matches!(e, crate::name::Event::Split { children, .. } if children.len() == 1)
        ),
        "{j:?}"
    );
}

#[test]
fn moving_the_tool_renames_nothing() {
    let b = block(1, [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let names = |z: f64| -> Vec<Vec<Name>> {
        let s = square(2, z, [-1.0, 3.0], [-1.0, 3.0]);
        let (d, _) = divide(OpKey(3), &b, &[tool(4, &s)]).unwrap();
        [&d.front[0], &d.back[0]]
            .iter()
            .map(|b| {
                let mut n: Vec<Name> = b.faces().map(|f| b.faces[f].name).collect();
                n.extend(b.edges().map(|e| b.edges[e].name));
                n.extend(b.vertices().map(|v| b.vertices[v].name));
                n.sort();
                n
            })
            .collect()
    };
    assert_eq!(names(0.5), names(1.25));
}

#[test]
fn a_cut_ending_inside_is_refused() {
    let s = square(1, 0.0, [0.0, 4.0], [0.0, 4.0]);
    let w = wall(2, 1.0, [-1.0, 2.0], [-1.0, 1.0]);
    assert!(matches!(
        divide(OpKey(3), &s, &[tool(4, &w)]).unwrap_err(),
        Error::InvalidBoolean(BooleanError::ToolDoesNotDivide { .. })
    ));
    let b = block(5, [0.0, 0.0, -1.0], [4.0, 4.0, 2.0]);
    assert!(matches!(
        divide(OpKey(3), &b, &[tool(4, &w)]).unwrap_err(),
        Error::InvalidBoolean(BooleanError::ToolDoesNotDivide { .. })
    ));
    // A sheet wholly inside a solid: its open edge is inside.
    let inner = square(6, 0.0, [1.0, 2.0], [1.0, 2.0]);
    assert!(matches!(
        divide(OpKey(3), &b, &[tool(4, &inner)]).unwrap_err(),
        Error::InvalidBoolean(BooleanError::ToolDoesNotDivide { .. })
    ));
    // A solid tool is no sheet.
    assert_eq!(
        divide(OpKey(3), &s, &[tool(4, &b)]).unwrap_err(),
        Error::NotASheet { op: "divide" }
    );
}

#[test]
fn a_closed_sheet_inside_a_box_cuts_a_ball_out() {
    let b = block(1, [-2.0, -2.0, -2.0], [4.0, 4.0, 4.0]);
    let ball = primitive::sphere(OpKey(2), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let shell = crate::sheet::remove(OpKey(5), &ball, &[]).unwrap().0;
    let (d, _) = divide(OpKey(3), &b, &[tool(4, &shell)]).unwrap();
    assert_eq!((d.front.len(), d.back.len(), d.apart.len()), (1, 1, 0));
    let sphere = 4.0 / 3.0 * core::f64::consts::PI;
    assert_eq!(check(&d.front[0]), []);
    assert_eq!(d.front[0].shells.live().count(), 2);
    assert!((volume(&d.front[0]) - (64.0 - sphere)).abs() < 1e-4);
    assert!((volume(&d.back[0]) - sphere).abs() < 1e-4);
}

#[test]
fn every_lump_is_a_body_and_lumps_the_tools_miss_are_apart() {
    // Two blocks in one body, halved: two bodies a side.
    let a = block(1, [0.0, 0.0, 0.0], [1.0, 1.0, 2.0]);
    let c = block(2, [3.0, 0.0, 0.0], [1.0, 1.0, 2.0]);
    let (two, _) = boolean(OpKey(9), BooleanOp::Unite, &a, &[tool(8, &c)]).unwrap();
    let s = square(3, 1.0, [-1.0, 5.0], [-1.0, 2.0]);
    let (d, _) = divide(OpKey(4), &two, &[tool(5, &s)]).unwrap();
    assert_eq!((d.front.len(), d.back.len(), d.apart.len()), (2, 2, 0));
    for b in d.front.iter().chain(&d.back) {
        assert_eq!(check(b), []);
        assert!((volume(b) - 1.0).abs() < 1e-9);
    }
    // A sheet in two pieces, a wall through one of them: the other is
    // apart.
    let strip = square(6, 0.0, [0.0, 6.0], [0.0, 1.0]);
    let gap = block(7, [2.0, -1.0, -1.0], [2.0, 3.0, 2.0]);
    let (pair, _) = boolean(OpKey(10), BooleanOp::Subtract, &strip, &[tool(11, &gap)]).unwrap();
    let w = wall(12, 1.0, [-1.0, 2.0], [-1.0, 1.0]);
    let (d, _) = divide(OpKey(13), &pair, &[tool(14, &w)]).unwrap();
    assert_eq!((d.front.len(), d.back.len(), d.apart.len()), (1, 1, 1));
    assert!((area(&d.front[0]) - 1.0).abs() < 1e-9);
    assert!((area(&d.back[0]) - 1.0).abs() < 1e-9);
    assert!((area(&d.apart[0]) - 2.0).abs() < 1e-9);
}

#[test]
fn a_sheet_lying_on_the_tool_goes_by_its_facing() {
    // In the tool facing the same way: behind it, as a solid's skin with
    // its material behind would be; facing the other way, in front.
    let s = square(1, 0.0, [0.0, 4.0], [0.0, 1.0]);
    let up = square(2, 0.0, [-1.0, 6.0], [-1.0, 2.0]);
    let (d, _) = divide(OpKey(3), &s, &[tool(4, &up)]).unwrap();
    assert_eq!((d.front.len(), d.back.len(), d.apart.len()), (0, 1, 0));
    let frame = Frame::new(
        crate::geom::Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, -1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    // Its y is the world's -y.
    let down = from_surface(
        OpKey(5),
        &Surface::Plane(Plane::new(frame)),
        [[-1.0, 6.0], [-2.0, 1.0]],
    )
    .unwrap()
    .0;
    let (d, _) = divide(OpKey(3), &s, &[tool(4, &down)]).unwrap();
    assert_eq!((d.front.len(), d.back.len(), d.apart.len()), (1, 0, 0));
    sound_sheet(&d.front[0], 4);
}

#[test]
fn crossing_tools_are_not_manifold() {
    let b = block(1, [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let a = square(2, 1.0, [-1.0, 3.0], [-1.0, 3.0]);
    let w = wall(3, 1.0, [-1.0, 3.0], [-1.0, 3.0]);
    let r = divide(OpKey(4), &b, &[tool(5, &a), tool(6, &w)]);
    assert!(
        matches!(
            r,
            Err(Error::InvalidBoolean(BooleanError::NonManifold { .. }))
        ),
        "{r:?}"
    );
}

#[test]
fn a_slab_between_two_tools_is_on_no_one_side() {
    // In front of the lower sheet and behind the upper: the tools don't
    // divide it into a front and a back.
    let b = block(1, [0.0, 0.0, 0.0], [2.0, 2.0, 3.0]);
    let lo = square(2, 1.0, [-1.0, 3.0], [-1.0, 3.0]);
    let hi = square(3, 2.0, [-1.0, 3.0], [-1.0, 3.0]);
    let r = divide(OpKey(4), &b, &[tool(5, &lo), tool(6, &hi)]);
    assert!(
        matches!(
            r,
            Err(Error::InvalidBoolean(
                BooleanError::ToolDoesNotDivide { .. }
            ))
        ),
        "{r:?}"
    );
}
