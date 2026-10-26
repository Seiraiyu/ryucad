//! What a solve tells its caller beyond positions: the degrees of freedom
//! left and what they move, and which constraints are redundant or in
//! conflict, as small sets an agent can act on.
//!
//! Stage 1 reads these from the Jacobian at the solution, as SolveSpace
//! does: its rank gives the freedom left and its null space what moves; a
//! dependency among its rows (its left null space) is a set of constraints
//! that say the same thing, redundant if they all hold and conflicting if
//! they can't.
//!
//! Stage 2 reads them from a component's pieces
//! ([`crate::core::blocks`]) when they are clean, and from the whole
//! Jacobian when they aren't ([`crate::core::diagnose`] says how). This
//! module turns what the core reads into the sketch's terms: entities and
//! their coordinates, and constraints by ID, with the faults the system
//! found by structure alone.

use crate::core::blocks::Blocks;
use crate::core::diagnose::diagnose as read;
use crate::sketch::Id;
use crate::system::{Coord, Owner, System};
use std::collections::{BTreeMap, BTreeSet};

/// How well defined a sketch is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Something can still move.
    Under,
    /// Nothing can move, and no constraint is redundant.
    Well,
    /// Some constraints are redundant or in conflict.
    Over,
}

/// An entity that can still move, and the coordinates that can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Free {
    pub entity: Id,
    pub moves: Vec<Coord>,
}

/// The diagnosis of a solved system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Diagnosis {
    pub dof: usize,
    pub free: Vec<Free>,
    pub redundant: Vec<Vec<Id>>,
    pub conflicting: Vec<Vec<Id>>,
}

impl Diagnosis {
    pub fn state(&self) -> State {
        if !self.redundant.is_empty() || !self.conflicting.is_empty() {
            State::Over
        } else if self.dof == 0 {
            State::Well
        } else {
            State::Under
        }
    }
}

/// Diagnoses `sys` at the solution `x`; `converged` says, per component,
/// whether its equations hold, and `pieces` what it was solved in (stage
/// 2), if anything.
pub(crate) fn diagnose(
    sys: &System,
    x: &[f64],
    converged: &[bool],
    pieces: &[Option<Blocks>],
) -> Diagnosis {
    let mut dof = 0;
    let mut free: BTreeMap<Id, BTreeSet<Coord>> = BTreeMap::new();
    let mut redundant = BTreeSet::new();
    let mut conflicting = BTreeSet::new();
    for f in &sys.faults {
        let set = f.constraints.clone();
        if f.conflicting {
            conflicting.insert(set);
        } else {
            redundant.insert(set);
        }
    }
    for &i in &sys.constant_eqs {
        if let Owner::Constraint(c) = sys.eqs[i].owner {
            if sys.misfit(i, x) <= 1.0 {
                redundant.insert(vec![c]);
            } else {
                conflicting.insert(vec![c]);
            }
        }
    }
    for ((comp, &ok), blocks) in sys.components.iter().zip(converged).zip(pieces) {
        let part = read(sys, comp, x, ok, blocks.as_ref());
        dof += part.dof;
        for var in part.support() {
            let (ids, coord) = &sys.var_owner[var];
            for id in ids {
                free.entry(*id).or_default().insert(*coord);
            }
        }
        for (set, conflict) in part.sets {
            if conflict {
                conflicting.insert(set);
            } else {
                redundant.insert(set);
            }
        }
    }
    Diagnosis {
        dof,
        free: free
            .into_iter()
            .map(|(entity, moves)| Free {
                entity,
                moves: moves.into_iter().collect(),
            })
            .collect(),
        redundant: redundant.into_iter().collect(),
        conflicting: conflicting.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use crate::sketch::tests::{line, point, rectangle};
    use crate::sketch::{Constraint as C, Entity, Sketch};
    use crate::solve::solve;
    use crate::solve::tests::dimensioned;
    use crate::{Coord, Free, State};

    #[test]
    fn a_dimensioned_rectangle_is_well_defined() {
        let r = solve(&dimensioned()).unwrap().report;
        assert_eq!((r.dof, r.state), (0, State::Well));
        assert!(r.free.is_empty() && r.redundant.is_empty() && r.conflicting.is_empty());
    }

    #[test]
    fn what_is_left_free() {
        let mut s = dimensioned();
        s.constraints.remove(&1);
        let r = solve(&s).unwrap().report;
        // Unfixed, it can slide either way.
        assert_eq!((r.dof, r.state), (2, State::Under));
        assert_eq!(r.free.len(), 4);
        assert_eq!(r.free[0].moves, [Coord::X, Coord::Y]);
        s.constraints.remove(&6);
        s.constraints.remove(&7);
        assert_eq!(solve(&s).unwrap().report.dof, 4);
        let bare = solve(&rectangle()).unwrap().report;
        assert_eq!(bare.dof, 8);
    }

    #[test]
    fn a_point_on_a_line_slides_along_it() {
        let mut s = Sketch::default();
        s.entities.insert(
            1,
            Entity::Point {
                at: [0.0, 0.0],
                fixed: true,
            },
        );
        s.entities.insert(
            2,
            Entity::Point {
                at: [4.0, 0.0],
                fixed: true,
            },
        );
        s.entities.insert(3, point(1.5, 0.7));
        s.entities.insert(10, line(1, 2));
        s.constraints.insert(1, C::On(3, 10));
        let r = solve(&s).unwrap().report;
        assert_eq!(r.dof, 1);
        assert_eq!(
            r.free,
            [Free {
                entity: 3,
                moves: vec![Coord::X]
            }]
        );
    }

    #[test]
    fn a_dimension_the_others_imply_is_redundant() {
        let mut s = dimensioned();
        s.constraints.insert(8, C::Length(13, 3.0));
        let r = solve(&s).unwrap().report;
        assert!(r.converged);
        assert_eq!(r.state, State::Over);
        assert_eq!(r.redundant, [vec![4, 5, 6, 8]]);
        assert!(r.conflicting.is_empty());
    }

    #[test]
    fn a_dimension_the_others_contradict_conflicts() {
        let mut s = dimensioned();
        s.constraints.insert(8, C::Length(13, 4.0));
        let r = solve(&s).unwrap().report;
        assert!(!r.converged);
        assert_eq!(r.conflicting, [vec![4, 5, 6, 8]]);
        assert!(r.redundant.is_empty());
    }

    #[test]
    fn structural_and_constant_faults() {
        let mut s = rectangle();
        s.entities.insert(
            5,
            Entity::Point {
                at: [9.0, 9.0],
                fixed: true,
            },
        );
        s.entities.insert(6, point(0.0, 0.0));
        s.constraints.insert(1, C::Fix(5));
        s.constraints.insert(2, C::Coincident(1, 6));
        s.constraints.insert(3, C::Coincident(6, 1));
        let r = solve(&s).unwrap().report;
        assert_eq!(r.redundant, [vec![1], vec![2, 3]]);
    }

    #[test]
    fn a_dimension_repeated_is_redundant_with_the_first() {
        // Four lengths of one side: each later one with the first.
        let mut s = dimensioned();
        for id in [8, 9, 10] {
            s.constraints.insert(id, C::Length(11, 3.0));
        }
        let r = solve(&s).unwrap().report;
        assert_eq!(r.redundant, [vec![6, 8], vec![6, 9], vec![6, 10]]);
        assert_eq!(
            crate::solve_whole(&s).unwrap().report.redundant,
            r.redundant
        );
    }

    #[test]
    fn clean_pieces_are_read_alone_and_others_whole() {
        use crate::core::blocks::decompose;
        use crate::core::diagnose::{by_pieces, whole};
        use crate::system::System;
        let read = |s: &Sketch| {
            let solved = solve(s).unwrap().sketch;
            let sys = System::new(&solved);
            let comp = sys
                .components
                .iter()
                .max_by_key(|c| c.eqs.len())
                .unwrap()
                .clone();
            let b = decompose(&sys, &comp);
            let pieces = by_pieces(&sys, &comp, &b, &sys.vars, true);
            let whole = whole(&sys, &comp, &sys.vars, true);
            (pieces, whole)
        };
        // Square and clean: read from its pieces, as the whole reads it.
        let (pieces, whole) = read(&dimensioned());
        let pieces = pieces.expect("clean");
        assert_eq!((pieces.dof, whole.dof), (0, 0));
        // Free to slide: the under part holds all the freedom.
        let mut s = dimensioned();
        s.constraints.remove(&1);
        let (pieces, whole) = read(&s);
        let pieces = pieces.expect("clean");
        assert_eq!((pieces.dof, pieces.support()), (whole.dof, whole.support()));
        assert_eq!(pieces.dof, 2);
        // Redundant by structure: the over part's set, as the whole's (the
        // heights of the corners, by the horizontals, the vertical side's
        // length and the new one).
        let mut s = dimensioned();
        s.constraints.insert(8, C::DistanceY(1, 4, 1.5));
        let (pieces, whole) = read(&s);
        let pieces = pieces.expect("clean");
        assert_eq!(pieces.sets, whole.sets);
        assert_eq!(pieces.sets, [(vec![2, 3, 7, 8], false)]);
        // The vertical sides made parallel as well: an over part, read
        // alone, as the whole reads it.
        let mut s = dimensioned();
        s.constraints.insert(8, C::Parallel(12, 14));
        let (pieces, whole) = read(&s);
        assert_eq!(pieces.expect("clean").sets, whole.sets);
        // Two horizontal lines from fixed points, also parallel: square by
        // structure (the parallel solved for the second line's far x), but
        // that block is singular where the lines are level. Not clean:
        // read whole, which finds the parallel redundant and that x free.
        let mut s = Sketch::default();
        for (id, x, y, fixed) in [
            (1, 0.0, 0.0, true),
            (2, 2.0, 0.1, false),
            (3, 0.0, 1.0, true),
            (4, 2.0, 1.2, false),
        ] {
            s.entities.insert(id, Entity::Point { at: [x, y], fixed });
        }
        s.entities.insert(10, line(1, 2));
        s.entities.insert(11, line(3, 4));
        s.constraints.insert(1, C::Horizontal(10));
        s.constraints.insert(2, C::Horizontal(11));
        s.constraints.insert(3, C::Parallel(10, 11));
        s.constraints.insert(4, C::Length(10, 2.0));
        let (pieces, whole) = read(&s);
        assert!(pieces.is_none());
        assert_eq!(whole.dof, 1);
        assert_eq!(whole.sets, [(vec![1, 2, 3], false)]);
    }

    #[test]
    fn a_long_dimensioned_polygon_is_read_in_pieces() {
        // The benchmark's polygon of 1,000 entities, its sides about a
        // 80th of its size: each side's angle to the next reads the
        // points before it a hundred times as strongly as its length
        // does, yet the pieces stay clear of the coupling bound and are
        // read alone, not whole (a second, not milliseconds).
        use crate::core::blocks::decompose;
        use crate::core::diagnose::{WHOLE_LIMIT, by_pieces};
        use crate::system::System;
        use ryukei::math;
        let n = 500;
        let step = 2.0 * std::f64::consts::PI / n as f64;
        let pts: Vec<[f64; 2]> = (0..n)
            .map(|k| {
                let t = step * k as f64;
                let r = 1.0 - 0.3 * ((k * 7919) % 100) as f64 / 100.0;
                [r * math::cos(t), r * math::sin(t)]
            })
            .collect();
        let mut s = Sketch::default();
        for (k, p) in pts.iter().enumerate() {
            s.entities.insert(k as u32 + 1, point(p[0], p[1]));
        }
        let side = |k: usize| 100_000 + k as u32;
        for k in 0..n {
            s.entities
                .insert(side(k), line(k as u32 + 1, ((k + 1) % n) as u32 + 1));
        }
        let turn = |a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]| {
            let (u, w) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
            math::atan2(u[0] * w[1] - u[1] * w[0], u[0] * w[0] + u[1] * w[1])
        };
        let mut c = 0..;
        let mut add = |s: &mut Sketch, k: C| {
            s.constraints.insert(c.next().unwrap(), k);
        };
        add(&mut s, C::Fix(1));
        for k in 0..n {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            add(
                &mut s,
                C::Length(side(k), math::hypot(b[0] - a[0], b[1] - a[1])),
            );
        }
        for k in 0..n - 3 {
            let v = turn(pts[k], pts[k + 1], pts[k + 1], pts[k + 2]);
            add(&mut s, C::Angle(side(k), side(k + 1), v));
        }
        let solved = solve(&s).unwrap();
        assert_eq!((solved.report.dof, solved.report.state), (1, State::Under));
        let sys = System::new(&solved.sketch);
        let comp = sys.components.iter().max_by_key(|c| c.eqs.len()).unwrap();
        assert!(comp.vars.len() <= WHOLE_LIMIT, "{}", comp.vars.len());
        let b = decompose(&sys, comp);
        assert!(by_pieces(&sys, comp, &b, &sys.vars, true).is_some());
    }

    #[test]
    fn coincident_ends_hold_no_degrees_of_freedom_twice() {
        // Two lines joined end to end: 3 points, 6 freedoms.
        let mut s = Sketch::default();
        for (id, x, y) in [(1, 0.0, 0.0), (2, 1.0, 0.0), (3, 1.0, 0.0), (4, 2.0, 1.0)] {
            s.entities.insert(id, point(x, y));
        }
        s.entities.insert(10, line(1, 2));
        s.entities.insert(11, line(3, 4));
        s.constraints.insert(1, C::Coincident(2, 3));
        assert_eq!(solve(&s).unwrap().report.dof, 6);
    }
}
