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
//! Stage 2 reads them from a component's pieces ([`crate::blocks`]) when
//! they are clean: the over part of full column rank, every square block
//! of full rank, the under part of full row rank. The Jacobian is then
//! block triangular with those pieces on its diagonal, so its null space
//! is the under part's and its left null space the over part's, and each
//! is read from its piece alone. Ranks are judged against the whole
//! Jacobian's scale, as stage 1 judges them. A piece that isn't clean
//! (a dependency only the values show) sends the component back to stage
//! 1's diagnosis.

use crate::blocks::Blocks;
use crate::linalg::{Mat, qr, qr_against};
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

/// The largest column and row norms of `comp`'s Jacobian at `x`: what
/// its pivoted QR and its transpose's take as their scale.
fn norms(sys: &System, comp: &Component, x: &[f64]) -> (f64, f64) {
    let mut cols: BTreeMap<usize, f64> = BTreeMap::new();
    let mut row_max: f64 = 0.0;
    for &e in &comp.eqs {
        let mut row = 0.0;
        for (v, g) in sys.partials(e, x) {
            if comp.vars.binary_search(&v).is_ok() {
                *cols.entry(v).or_default() += g * g;
                row += g * g;
            }
        }
        row_max = row_max.max(row);
    }
    let col_max = cols.values().fold(0.0, |m: f64, c| m.max(*c));
    (ryukei::math::sqrt(col_max), ryukei::math::sqrt(row_max))
}

/// What one component adds to a diagnosis: its freedom, the variables
/// that can move, and its dependent sets with whether each conflicts.
#[derive(Default)]
struct Part {
    dof: usize,
    free: BTreeSet<usize>,
    sets: Vec<(Vec<Id>, bool)>,
}

/// The variables of `comp` a null space moves.
fn support(comp: &Component, null: &[Vec<f64>]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for v in null {
        for (j, &var) in comp.vars.iter().enumerate() {
            if v[j].abs() > SUPPORT_TOL {
                out.insert(var);
            }
        }
    }
    out
}

/// A component's dependent sets, each with whether it conflicts.
fn sets(
    sys: &System,
    comp: &Component,
    jac: &Mat,
    r: &[f64],
    ok: bool,
    scale: Option<f64>,
) -> Vec<(Vec<Id>, bool)> {
    let tol = SOLVE_TOL / sys.scale;
    dependent_sets(sys, comp, jac, scale)
        .into_iter()
        .map(|set| {
            let rows = rows_of(sys, comp, &set);
            let conflict = !ok && inconsistent(&jac.rows_of(&rows), &pick(r, &rows), tol);
            (set, conflict)
        })
        .collect()
}

/// Stage 1: a component read from its whole Jacobian.
fn whole(sys: &System, comp: &Component, x: &[f64], ok: bool) -> Part {
    let (jac, r) = jacobian(sys, comp, x);
    let f = qr(&jac, RANK_TOL);
    Part {
        dof: comp.vars.len() - f.rank,
        free: support(comp, &f.null_space()),
        sets: sets(sys, comp, &jac, &r, ok, None),
    }
}

/// Stage 2: a component read from its pieces, when they are clean (see
/// the module docs); `None` when they aren't.
fn by_pieces(sys: &System, comp: &Component, b: &Blocks, x: &[f64], ok: bool) -> Option<Part> {
    let (cols, rows) = norms(sys, comp, x);
    let factor = |m: &Mat| qr_against(m, RANK_TOL, Some(cols));
    let mut part = Part::default();
    if !b.over.eqs.is_empty() {
        let (jac, r) = jacobian(sys, &b.over, x);
        if factor(&jac).rank < b.over.vars.len() {
            return None;
        }
        part.sets = sets(sys, &b.over, &jac, &r, ok, Some(rows));
    }
    for blk in &b.square {
        if factor(&jacobian(sys, blk, x).0).rank < blk.vars.len() {
            return None;
        }
    }
    if !b.under.vars.is_empty() {
        let f = factor(&jacobian(sys, &b.under, x).0);
        if f.rank < b.under.eqs.len() {
            return None;
        }
        part.dof = b.under.vars.len() - f.rank;
        part.free = support(&b.under, &f.null_space());
    }
    Some(part)
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
        let part = blocks
            .as_ref()
            .and_then(|b| by_pieces(sys, comp, b, x, ok))
            .unwrap_or_else(|| whole(sys, comp, x, ok));
        dof += part.dof;
        for var in part.free {
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
/// vector of the left null space (its rank judged against `scale`, the
/// largest row norm of the Jacobian it is part of, if not `jac` itself),
/// each cut down until removing any of its constraints makes it
/// independent.
fn dependent_sets(sys: &System, comp: &Component, jac: &Mat, scale: Option<f64>) -> Vec<Vec<Id>> {
    let mut out = BTreeSet::new();
    for y in qr_against(&jac.transpose(), RANK_TOL, scale).null_space() {
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
    fn clean_pieces_are_read_alone_and_others_whole() {
        use crate::blocks::decompose;
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
            let pieces = super::by_pieces(&sys, &comp, &b, &sys.vars, true);
            let whole = super::whole(&sys, &comp, &sys.vars, true);
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
        assert_eq!((pieces.dof, &pieces.free), (whole.dof, &whole.free));
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
