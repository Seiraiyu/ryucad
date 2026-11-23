//! Placement by constraint (design 924): a placement's constraints judged
//! on every check, exactly, and the legaliser that finds a place where
//! they hold (Task 5).

use super::drc::{Finding, finding};
use super::file::{Constraint, EdgeSide, Placement, PlacementSide};
use super::load::BoardFiles;
use super::occ::Occ;
use super::shapes::BoardShapes;
use crate::error::Code;
use crate::fault::Severity;
use ryu_board::geom::Rect;
use ryu_board::shape::Shape;
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};
use serde_json::{Value, json};

/// Every listed shape as one.
pub(crate) fn union<'a>(shapes: impl IntoIterator<Item = &'a Shape>) -> Shape {
    let mut out = Shape::default();
    for s in shapes {
        out.parts.extend(s.parts.iter().cloned());
    }
    out
}

/// A part's pads' copper, every layer as one shape.
pub(crate) fn part_copper(s: &BoardShapes, occurrence: &str) -> Shape {
    union(
        s.pads
            .iter()
            .filter(|p| p.occurrence == occurrence)
            .map(|p| &p.copper),
    )
}

/// A part's courtyard, else its pads' copper.
pub(crate) fn part_body_shape(s: &BoardShapes, owner: &str, occurrence: &str) -> Shape {
    s.courtyards
        .iter()
        .find(|c| c.owner == owner)
        .map(|c| c.shape.clone())
        .unwrap_or_else(|| part_copper(s, occurrence))
}

/// What `near`'s target names: a pad (`U1/7`) or a part (`U1`), with its
/// full reference; `None` when the board has neither.
pub(crate) fn target(s: &BoardShapes, board: &str, r: &str) -> Option<(String, Shape)> {
    let full = if r.contains(':') {
        r.to_owned()
    } else {
        format!("{board}:{r}")
    };
    if let Some(p) = s.pads.iter().find(|p| p.reference == full) {
        return Some((full, p.copper.clone()));
    }
    let prefix = format!("{full}/");
    let pads: Vec<&Shape> = s
        .pads
        .iter()
        .filter(|p| p.reference.starts_with(&prefix))
        .map(|p| &p.copper)
        .collect();
    (!pads.is_empty()).then(|| (full, union(pads)))
}

/// How far a box lies inside the outline's box from `side`, measured
/// perpendicular to it.
fn from_side(outline: Rect, b: Rect, side: EdgeSide) -> i64 {
    match side {
        EdgeSide::Left => b.min.x - outline.min.x,
        EdgeSide::Right => outline.max.x - b.max.x,
        EdgeSide::Bottom => b.min.y - outline.min.y,
        EdgeSide::Top => outline.max.y - b.max.y,
        EdgeSide::Any => unreachable!("any is measured against the edges"),
    }
}

fn unmet(
    board: &str,
    owner: &str,
    other: String,
    c: &Constraint,
    message: String,
    details: Value,
) -> Finding {
    let mut d = details;
    d["constraint"] = json!(c);
    finding(
        Code::PlacementConstraintUnmet,
        Severity::Error,
        board,
        vec![owner.to_owned(), other],
        message,
        d,
    )
}

/// One constraint of one part, judged: a finding when it isn't met.
pub(crate) fn judge_one(
    board: &str,
    label: &str,
    occurrence: &str,
    s: &BoardShapes,
    pl: &Placement,
    c: &Constraint,
) -> Option<Finding> {
    let owner = format!("{board}:{label}");
    let opts = BodyOptions::default();
    match c {
        Constraint::Side { side } => (pl.side != *side).then(|| {
            unmet(
                board,
                &owner,
                format!("{board}:side/{}", side.name()),
                c,
                format!("{owner} must be on the {}", side.name()),
                json!({}),
            )
        }),
        Constraint::Near { near, within_nm } => {
            let Some((full, shape)) = target(s, board, near) else {
                return Some(unmet(
                    board,
                    &owner,
                    format!("{board}:{near}"),
                    c,
                    format!("{owner} must be near {near}, which isn't on the board"),
                    json!({"missing": true}),
                ));
            };
            let a = Body::of_shape(&part_copper(s, occurrence), &opts);
            let b = Body::of_shape(&shape, &opts);
            let v = measure::within(&a, &b, *within_nm);
            (v != Verdict::Yes).then(|| {
                let d = measure::distance(&a, &b).map(|d| d.nm);
                unmet(board, &owner, full.clone(), c,
                    format!("{owner} must be within {within_nm} nm of {full}"),
                    json!({"distance_nm": d, "within_nm": within_nm, "uncertain": v == Verdict::Uncertain}))
            })
        }
        Constraint::Edge { edge, within_nm } => {
            let body = part_body_shape(s, &owner, occurrence);
            let other = format!("{board}:edge/{}", edge.name());
            let message = format!(
                "{owner} must be within {within_nm} nm of the board's {} edge",
                edge.name()
            );
            if *edge == EdgeSide::Any {
                let a = Body::of_shape(&body, &opts);
                let e = Body::of_shape(&s.edges, &opts);
                let v = measure::within(&a, &e, *within_nm);
                return (v != Verdict::Yes).then(|| {
                    let d = measure::distance(&a, &e).map(|d| d.nm);
                    unmet(board, &owner, other, c, message,
                        json!({"distance_nm": d, "within_nm": within_nm, "uncertain": v == Verdict::Uncertain}))
                });
            }
            let (Some(o), Some(b)) = (s.outline.bounds(), body.bounds()) else {
                return Some(unmet(
                    board,
                    &owner,
                    other,
                    c,
                    message,
                    json!({"missing": true}),
                ));
            };
            let d = from_side(o, b, *edge);
            (d > *within_nm).then(|| {
                unmet(
                    board,
                    &owner,
                    other,
                    c,
                    message,
                    json!({"distance_nm": d, "within_nm": within_nm, "uncertain": false}),
                )
            })
        }
    }
}

/// Every placed part's constraints, judged (a DRC rule).
pub fn constraints(board: &str, b: &BoardFiles, occs: &[Occ], s: &BoardShapes) -> Vec<Finding> {
    let mut out = Vec::new();
    for o in occs {
        let Some(Ok(pl)) = b.placements.get(&o.reference).map(|l| l.item.as_ref()) else {
            continue;
        };
        for c in &pl.constraints {
            out.extend(judge_one(board, o.label(), &o.reference, s, pl, c));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::testing::{MM, UM, board};

    fn near(t: &str, w: i64) -> Constraint {
        Constraint::Near {
            near: t.into(),
            within_nm: w,
        }
    }

    /// Two parts' pads 3 mm apart, edge to edge (each pad 1 mm square).
    fn two() -> BoardShapes {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("c1", "1", (4 * MM, 5 * MM), (MM, MM), Some("a"))
            .smd("u1", "7", (8 * MM, 5 * MM), (MM, MM), Some("a"))
            .courtyard("c1", (4 * MM, 5 * MM), (2 * MM, 2 * MM), false);
        s
    }

    #[test]
    fn near_holds_at_its_distance_and_not_a_nanometre_under() {
        let s = two();
        let at = |w| judge_one("b", "c1", "c1", &s, &Placement::default(), &near("u1/7", w));
        assert!(at(3 * MM).is_none(), "exactly 3 mm holds");
        let f = at(3 * MM - 1).unwrap();
        assert_eq!(f.code, Code::PlacementConstraintUnmet);
        assert_eq!(f.details["distance_nm"], 3 * MM);
        assert_eq!(f.refs, ["b:c1", "b:u1/7"]);
        let gone = judge_one(
            "b",
            "c1",
            "c1",
            &s,
            &Placement::default(),
            &near("u9/1", MM),
        )
        .unwrap();
        assert_eq!(gone.details["missing"], true);
    }

    #[test]
    fn edges_are_measured_from_the_outlines_box() {
        let s = two(); // c1's courtyard spans x 3..5 mm, y 4..6 mm on a 20 x 10 mm board
        let e = |side, w| Constraint::Edge {
            edge: side,
            within_nm: w,
        };
        let j = |c: &Constraint| judge_one("b", "c1", "c1", &s, &Placement::default(), c);
        assert!(j(&e(EdgeSide::Left, 3 * MM)).is_none());
        assert_eq!(
            j(&e(EdgeSide::Left, 3 * MM - 1)).unwrap().details["distance_nm"],
            3 * MM
        );
        assert!(j(&e(EdgeSide::Bottom, 4 * MM)).is_none());
        assert!(j(&e(EdgeSide::Top, 4 * MM - 1)).is_some());
        assert!(j(&e(EdgeSide::Right, 15 * MM)).is_none());
        assert!(
            j(&e(EdgeSide::Any, 3 * MM)).is_none(),
            "the left edge is 3 mm off"
        );
        assert!(j(&e(EdgeSide::Any, 3 * MM - 1)).is_some());
    }

    #[test]
    fn a_side_constraint_is_the_placements_side() {
        let s = two();
        let bottom = Placement {
            side: PlacementSide::Bottom,
            ..Placement::default()
        };
        let c = Constraint::Side {
            side: PlacementSide::Top,
        };
        assert!(judge_one("b", "c1", "c1", &s, &Placement::default(), &c).is_none());
        assert!(judge_one("b", "c1", "c1", &s, &bottom, &c).is_some());
        let _ = UM;
    }
}
