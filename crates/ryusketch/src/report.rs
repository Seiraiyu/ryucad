//! What a solve tells its caller beyond positions: the degrees of freedom
//! left and what they move, and which constraints are redundant or in
//! conflict, as small sets an agent can act on.
//!
//! Stage 1 reads these from the Jacobian at the solution, as SolveSpace
//! does: its rank gives the freedom left and its null space what moves; a
//! dependency among its rows (its left null space) is a set of constraints
//! that say the same thing, redundant if they all hold and conflicting if
//! they can't.

use crate::linalg::{Mat, qr};
use crate::sketch::Id;
use crate::solve::jacobian;
use crate::system::{Component, Coord, Owner, SOLVE_TOL, System};
use std::collections::{BTreeMap, BTreeSet};

/// Diagonal entries of `R` smaller than this, relative to the first, count
/// as zero.
const RANK_TOL: f64 = 1e-8;
/// Entries of a unit null vector smaller than this are not part of it.
const SUPPORT_TOL: f64 = 1e-6;

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
/// whether its equations hold.
pub(crate) fn diagnose(sys: &System, x: &[f64], converged: &[bool]) -> Diagnosis {
    let tol = SOLVE_TOL / sys.scale;
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
            if sys.residual(i, x).abs() <= tol {
                redundant.insert(vec![c]);
            } else {
                conflicting.insert(vec![c]);
            }
        }
    }
    for (comp, &ok) in sys.components.iter().zip(converged) {
        let (jac, r) = jacobian(sys, comp, x);
        let f = qr(&jac, RANK_TOL);
        dof += comp.vars.len() - f.rank;
        for v in f.null_space() {
            for (j, &var) in comp.vars.iter().enumerate() {
                if v[j].abs() > SUPPORT_TOL {
                    let (ids, coord) = &sys.var_owner[var];
                    for id in ids {
                        free.entry(*id).or_default().insert(*coord);
                    }
                }
            }
        }
        for set in dependent_sets(sys, comp, &jac) {
            let rows = rows_of(sys, comp, &set);
            let conflict = !ok && inconsistent(&jac.rows_of(&rows), &pick(&r, &rows), tol);
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

fn pick(r: &[f64], rows: &[usize]) -> Vec<f64> {
    rows.iter().map(|&i| r[i]).collect()
}

/// The rows (within the component) of the constraints in `set`, and of
/// every arc's own equation.
fn rows_of(sys: &System, comp: &Component, set: &[Id]) -> Vec<usize> {
    comp.eqs
        .iter()
        .enumerate()
        .filter(|(_, e)| match sys.eqs[**e].owner {
            Owner::Constraint(c) => set.contains(&c),
            Owner::Arc(_) => true,
        })
        .map(|(i, _)| i)
        .collect()
}

/// Whether `rows` of a Jacobian depend on each other.
fn dependent(m: &Mat) -> bool {
    qr(m, RANK_TOL).rank < m.rows
}

/// Whether residuals `r` of dependent rows `m` can't all be zero to first
/// order: some combination of the rows vanishes while the same combination
/// of the residuals doesn't.
fn inconsistent(m: &Mat, r: &[f64], tol: f64) -> bool {
    qr(&m.transpose(), RANK_TOL).null_space().iter().any(|y| {
        let s: f64 = y.iter().zip(r).map(|(a, b)| a * b).sum();
        s.abs() > tol
    })
}

/// Minimal sets of constraints whose rows depend on each other, one per
/// vector of the left null space, each cut down until removing any of its
/// constraints makes it independent.
fn dependent_sets(sys: &System, comp: &Component, jac: &Mat) -> Vec<Vec<Id>> {
    let mut out = BTreeSet::new();
    for y in qr(&jac.transpose(), RANK_TOL).null_space() {
        let mut set: Vec<Id> = y
            .iter()
            .enumerate()
            .filter(|(_, w)| w.abs() > SUPPORT_TOL)
            .filter_map(|(i, _)| match sys.eqs[comp.eqs[i]].owner {
                Owner::Constraint(c) => Some(c),
                Owner::Arc(_) => None,
            })
            .collect::<BTreeSet<Id>>()
            .into_iter()
            .collect();
        let mut k = 0;
        while k < set.len() {
            let mut fewer = set.clone();
            fewer.remove(k);
            if dependent(&jac.rows_of(&rows_of(sys, comp, &fewer))) {
                set = fewer;
            } else {
                k += 1;
            }
        }
        if !set.is_empty() {
            out.insert(set);
        }
    }
    out.into_iter().collect()
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
