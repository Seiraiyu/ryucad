//! How a solved sketch is drawn: its curves as polylines in model space
//! (lines exactly, arcs and circles to a display angle), its points, each
//! coloured by how well the solve defines it, and a label per dimension,
//! placed beside what it measures. `sketch.get`, `render` and the app all
//! draw from this, so they show the same thing.

use crate::doc::id::Id;
use crate::doc::sketch::{ConstraintType, Geometry, Sketch};
use crate::sketch::{Report, Solved};
use crate::units::{Dim, Unit};
use core::f64::consts::{PI, TAU};
use ryukei::geom::Point3;
use ryukei::math;
use std::collections::BTreeMap;

/// How well the solve defines an entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntityState {
    /// Nothing about it can move.
    Well,
    /// It can still move.
    Under,
    /// A redundant constraint names it.
    Over,
    /// A constraint that can't hold names it.
    Failing,
}

impl EntityState {
    pub fn as_str(self) -> &'static str {
        match self {
            EntityState::Well => "well",
            EntityState::Under => "under",
            EntityState::Over => "over",
            EntityState::Failing => "failing",
        }
    }

    pub fn parse(s: &str) -> Option<EntityState> {
        [
            EntityState::Well,
            EntityState::Under,
            EntityState::Over,
            EntityState::Failing,
        ]
        .into_iter()
        .find(|e| e.as_str() == s)
    }
}

/// Whether a constraint holds alone, says what others already say, or
/// can't hold with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintState {
    Ok,
    Redundant,
    Conflicting,
}

impl ConstraintState {
    pub fn as_str(self) -> &'static str {
        match self {
            ConstraintState::Ok => "ok",
            ConstraintState::Redundant => "redundant",
            ConstraintState::Conflicting => "conflicting",
        }
    }
}

/// Each entity's state: over or failing where a redundant or conflicting
/// set names it (failing when the solve didn't converge), else under where
/// it or one of its points can move, else well.
pub fn entity_states(s: &Sketch, report: &Report) -> BTreeMap<Id, EntityState> {
    let free: Vec<&Id> = report.free.iter().map(|(id, _)| id).collect();
    let in_set = |sets: &[Vec<Id>], id: &Id| sets.iter().any(|s| s.contains(id));
    let mut over: Vec<&Id> = Vec::new();
    for (cid, c) in &s.constraints {
        if in_set(&report.redundant, cid) || in_set(&report.conflicting, cid) {
            over.extend(&c.entities);
        }
    }
    s.entities
        .iter()
        .map(|(id, e)| {
            let moves =
                free.contains(&id) || e.geometry.points().iter().any(|(_, p)| free.contains(p));
            let state = if over.contains(&id) {
                if report.converged {
                    EntityState::Over
                } else {
                    EntityState::Failing
                }
            } else if moves {
                EntityState::Under
            } else {
                EntityState::Well
            };
            (id.clone(), state)
        })
        .collect()
}

/// Each constraint's state.
pub fn constraint_states(s: &Sketch, report: &Report) -> BTreeMap<Id, ConstraintState> {
    let in_set = |sets: &[Vec<Id>], id: &Id| sets.iter().any(|s| s.contains(id));
    s.constraints
        .keys()
        .map(|id| {
            let state = if in_set(&report.conflicting, id) {
                ConstraintState::Conflicting
            } else if in_set(&report.redundant, id) {
                ConstraintState::Redundant
            } else {
                ConstraintState::Ok
            };
            (id.clone(), state)
        })
        .collect()
}

/// A curve as drawn: a polyline in model space.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub entity: Id,
    pub construction: bool,
    pub state: EntityState,
    pub points: Vec<[f64; 3]>,
}

/// A point as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Dot {
    pub entity: Id,
    pub state: EntityState,
    pub at: [f64; 3],
}

/// A dimension's label: its value, and its expression when it isn't a
/// literal, at a point beside what it measures.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub constraint: Id,
    pub state: ConstraintState,
    pub at: [f64; 3],
    pub text: String,
}

/// A sketch as drawn, in ID order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawing {
    pub curves: Vec<Curve>,
    pub dots: Vec<Dot>,
    pub labels: Vec<Label>,
}

/// A number for people: at most 6 decimals, no trailing zeros, never `-0`.
pub fn number(x: f64) -> String {
    let s = format!("{x:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

/// What a dimension's label says: a literal as written; an expression as
/// written, then its value in `unit` (degrees for angles).
pub fn label_text(q: &crate::units::Quantity, unit: Unit) -> String {
    if q.is_literal() {
        return q.text();
    }
    let value = if q.dim() == Dim::ANGLE {
        format!("{} deg", number(q.radians() * 180.0 / PI))
    } else {
        format!("{} {}", number(q.meters() / unit.factor()), unit.as_str())
    };
    format!("{} = {value}", q.text())
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn mid(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

fn along(a: [f64; 2], d: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + d[0] * t, a[1] + d[1] * t]
}

/// `d` scaled to length 1, or `None` for no length.
fn unit2(d: [f64; 2]) -> Option<[f64; 2]> {
    let n = math::hypot(d[0], d[1]);
    (n > 0.0).then(|| [d[0] / n, d[1] / n])
}

/// The foot of `p` on the line through `a` and `b` (`a` if they meet).
fn foot(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let d = sub(b, a);
    let l2 = d[0] * d[0] + d[1] * d[1];
    if l2 == 0.0 {
        return a;
    }
    let t = ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2;
    along(a, d, t)
}

/// The angle of `d`, in `[0, 2π)`.
fn angle_of(d: [f64; 2]) -> f64 {
    let a = math::atan2(d[1], d[0]);
    if a < 0.0 { a + TAU } else { a }
}

/// Points along a circle about `c` of radius `r`, from angle `a0` turning
/// counterclockwise by `sweep`, at most `step` apart in angle (and at
/// least 8 pieces to a whole turn).
fn arc_points(c: [f64; 2], r: f64, a0: f64, sweep: f64, step: f64) -> Vec<[f64; 2]> {
    let n = ((sweep / step).ceil() as usize)
        .max((8.0 * sweep / TAU).ceil() as usize)
        .max(1);
    (0..=n)
        .map(|k| {
            let a = a0 + sweep * k as f64 / n as f64;
            [c[0] + r * math::cos(a), c[1] + r * math::sin(a)]
        })
        .collect()
}

/// An arc's center, radius, start angle and counterclockwise sweep.
fn arc(
    m: &dyn Fn(&Id) -> [f64; 2],
    center: &Id,
    start: &Id,
    end: &Id,
) -> ([f64; 2], f64, f64, f64) {
    let (c, s, e) = (m(center), m(start), m(end));
    let a0 = angle_of(sub(s, c));
    let mut sweep = angle_of(sub(e, c)) - a0;
    if sweep <= 0.0 {
        sweep += TAU;
    }
    (c, math::hypot(s[0] - c[0], s[1] - c[1]), a0, sweep)
}

/// How `solved` is drawn: arcs and circles in pieces of at most `step`
/// radians, labels in `unit`.
pub fn drawing(solved: &Solved, step: f64, unit: Unit) -> Drawing {
    let s = &solved.sketch;
    let states = entity_states(s, &solved.report);
    let cstates = constraint_states(s, &solved.report);
    let m = |p: &Id| match s.entities.get(p).map(|e| &e.geometry) {
        Some(Geometry::Point { m }) => *m,
        _ => unreachable!("validated: a point"),
    };
    let lift = |q: [f64; 2]| {
        let p = solved.frame.apply_point(Point3::new(q[0], q[1], 0.0));
        [p.x, p.y, p.z]
    };
    let mut out = Drawing::default();
    // How far labels stand off what they measure: a fraction of the
    // sketch's size.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut grow = |q: [f64; 2]| {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    };
    for (id, e) in &s.entities {
        let state = states[id];
        let flat = match &e.geometry {
            Geometry::Point { m } => {
                grow(*m);
                out.dots.push(Dot {
                    entity: id.clone(),
                    state,
                    at: lift(*m),
                });
                continue;
            }
            Geometry::Line { start, end } => vec![m(start), m(end)],
            Geometry::Arc { center, start, end } => {
                let (c, r, a0, sweep) = arc(&m, center, start, end);
                arc_points(c, r, a0, sweep, step)
            }
            Geometry::Circle { center, r_m } => arc_points(m(center), *r_m, 0.0, TAU, step),
        };
        flat.iter().for_each(|q| grow(*q));
        out.curves.push(Curve {
            entity: id.clone(),
            construction: e.construction,
            state,
            points: flat.into_iter().map(lift).collect(),
        });
    }
    let size = if lo[0] <= hi[0] {
        math::hypot(hi[0] - lo[0], hi[1] - lo[1])
    } else {
        0.0
    };
    let off = 0.04 * size;
    for (cid, c) in &s.constraints {
        let Some(q) = &c.value else { continue };
        let Some(at) = place(s, &m, c.ty, &c.entities, off) else {
            continue;
        };
        out.labels.push(Label {
            constraint: cid.clone(),
            state: cstates[cid],
            at: lift(at),
            text: label_text(q, unit),
        });
    }
    out
}

/// Where a dimension's label goes, in the sketch's plane: beside the
/// middle of what it measures, `off` away from it.
fn place(
    s: &Sketch,
    m: &dyn Fn(&Id) -> [f64; 2],
    ty: ConstraintType,
    entities: &[Id],
    off: f64,
) -> Option<[f64; 2]> {
    use ConstraintType as T;
    let geometry = |id: &Id| s.entities.get(id).map(|e| &e.geometry);
    let line = |id: &Id| match geometry(id) {
        Some(Geometry::Line { start, end }) => Some((m(start), m(end))),
        _ => None,
    };
    let point = |id: &Id| match geometry(id) {
        Some(Geometry::Point { m }) => Some(*m),
        _ => None,
    };
    // Off the middle of `a`..`b`, to its left.
    let beside = |a: [f64; 2], b: [f64; 2]| {
        let c = mid(a, b);
        match unit2(sub(b, a)) {
            Some(d) => along(c, [-d[1], d[0]], off),
            None => c,
        }
    };
    let a = entities.first()?;
    Some(match ty {
        T::Length => {
            let (p, q) = line(a)?;
            beside(p, q)
        }
        T::Distance | T::DistanceX | T::DistanceY => {
            let b = entities.get(1)?;
            match (point(a), point(b), line(a), line(b)) {
                (Some(p), Some(q), ..) => match ty {
                    T::DistanceX => along(mid(p, q), [0.0, 1.0], off),
                    T::DistanceY => along(mid(p, q), [1.0, 0.0], off),
                    _ => beside(p, q),
                },
                (Some(p), None, _, Some((u, w))) | (None, Some(p), Some((u, w)), _) => {
                    mid(p, foot(p, u, w))
                }
                (None, None, Some((u, w)), Some((x, y))) => {
                    let c = mid(u, w);
                    mid(c, foot(c, x, y))
                }
                _ => return None,
            }
        }
        T::Radius | T::Diameter => match geometry(a)? {
            Geometry::Circle { center, r_m } => {
                let d = core::f64::consts::FRAC_1_SQRT_2;
                along(m(center), [d, d], r_m + off)
            }
            Geometry::Arc { center, start, end } => {
                let (c, r, a0, sweep) = arc(m, center, start, end);
                let t = a0 + sweep / 2.0;
                along(c, [math::cos(t), math::sin(t)], r + off)
            }
            _ => return None,
        },
        T::Angle => {
            let ((p, q), (u, w)) = (line(a)?, line(entities.get(1)?)?);
            let (d, e) = (sub(q, p), sub(w, u));
            let den = d[0] * e[1] - d[1] * e[0];
            let scale = math::hypot(d[0], d[1]) * math::hypot(e[0], e[1]);
            if den.abs() <= 1e-12 * scale {
                mid(mid(p, q), mid(u, w))
            } else {
                // Where the lines meet, off along the bisector of their
                // directions.
                let t = ((u[0] - p[0]) * e[1] - (u[1] - p[1]) * e[0]) / den;
                let x = along(p, d, t);
                let (d, e) = (unit2(d)?, unit2(e)?);
                match unit2([d[0] + e[0], d[1] + e[1]]) {
                    Some(b) => along(x, b, off),
                    None => x,
                }
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Id {
        Id::parse(s, "entity").unwrap()
    }

    #[test]
    fn numbers_for_people() {
        assert_eq!(number(20.0), "20");
        assert_eq!(number(0.1 + 0.2), "0.3");
        assert_eq!(number(-1e-9), "0");
        assert_eq!(number(1.234_567_89), "1.234568");
    }

    #[test]
    fn a_rectangle_is_drawn() {
        let solved = crate::sketch::solve(&id("s"), &crate::sketch::tests::rectangle()).unwrap();
        let d = drawing(&solved, PI / 12.0, Unit::Mm);
        let ids: Vec<&str> = d.curves.iter().map(|c| c.entity.as_str()).collect();
        assert_eq!(ids, ["l1", "l2", "l3", "l4", "l5"]);
        assert_eq!(d.curves[0].points, [[0.0, 0.0, 0.0], [0.02, 0.0, 0.0]]);
        assert!(d.curves[4].construction && !d.curves[0].construction);
        assert_eq!(d.dots.len(), 4);
        // Unfixed, it slides about.
        assert!(d.curves.iter().all(|c| c.state == EntityState::Under));
        let texts: Vec<&str> = d.labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["width = 20 mm", "22.360679774997898 mm"]);
        // The length's label stands off the middle of l1, inside.
        let at = d.labels[0].at;
        assert!(
            (at[0] - 0.01).abs() < 1e-12 && at[1] > 0.0 && at[2] == 0.0,
            "{at:?}"
        );
        assert_eq!(d.labels[0].state, ConstraintState::Ok);
    }

    #[test]
    fn arcs_and_circles_come_in_pieces() {
        let pts = arc_points([0.0, 0.0], 1.0, 0.0, TAU, PI / 12.0);
        assert_eq!(pts.len(), 25);
        // A small step makes more; a whole turn is never under 8 pieces.
        assert_eq!(arc_points([0.0, 0.0], 1.0, 0.0, TAU, PI).len(), 9);
        assert_eq!(arc_points([0.0, 0.0], 1.0, 0.0, PI / 2.0, PI).len(), 3);
        let end = pts[24];
        assert!((end[0] - 1.0).abs() < 1e-15 && end[1].abs() < 1e-15);
    }

    #[test]
    fn states_follow_the_report() {
        let s = crate::sketch::tests::rectangle();
        let mut r = crate::sketch::solve(&id("s"), &s).unwrap().report;
        r.free.clear();
        r.redundant = vec![vec![id("c1"), id("c2")]];
        let st = entity_states(&s, &r);
        assert_eq!(st[&id("l1")], EntityState::Over);
        assert_eq!(st[&id("l3")], EntityState::Well);
        r.converged = false;
        assert_eq!(entity_states(&s, &r)[&id("l2")], EntityState::Failing);
        let cs = constraint_states(&s, &r);
        assert_eq!(cs[&id("c1")], ConstraintState::Redundant);
        assert_eq!(cs[&id("d1")], ConstraintState::Ok);
    }
}
