//! The solver: damped least-norm Gauss-Newton (Levenberg-Marquardt) on
//! each component, from the positions the sketch holds.
//!
//! Each step is the smallest change that would satisfy the linearized
//! equations, `dx = -Jᵀ (J Jᵀ + λI)⁻¹ r`, so geometry the constraints leave
//! free moves as little as it can and a solve stays on the branch the
//! sketch is already on. A sketch that already holds is returned with the
//! very same bits.
//!
//! Stage 2 solves a component in the pieces of its Dulmage-Mendelsohn
//! decomposition ([`crate::blocks`]): its over-determined part, then its
//! square blocks in order, then its under-determined part, each by the
//! same steps over its own variables. A piece that doesn't converge sends
//! the whole component back to stage 1 from where it started.

use crate::blocks::{Blocks, decompose};
use crate::linalg::{Mat, cholesky_solve};
use crate::loops::{self, Chain, Loop};
use crate::report::{Free, State, diagnose};
use crate::sketch::{Entity, Id, Sketch, SketchError};
#[cfg(doc)]
use crate::system::SOLVE_TOL;
use crate::system::{Component, Coord, Scalar, System};

/// Steps tried per component, accepted or not.
const MAX_STEPS: u32 = 200;
/// A component holds when every residual is within its tolerance, and the
/// solver stops only well inside it, so a solved sketch rounded a little
/// differently (at another scale, say) still holds.
const AIM: f64 = 0.25;
/// The longest step, in solver units: the sketch fits in ±1 of them, so a
/// step can't carry geometry off towards infinity, where angle residuals
/// shrink without anything holding.
const MAX_STEP: f64 = 0.5;
/// The first damping, and the bounds it moves between.
const DAMP_START: f64 = 1e-6;
const DAMP_MIN: f64 = 1e-15;
const DAMP_MAX: f64 = 1e12;

/// A solved sketch and what the solve found.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    /// The sketch with its positions and radii solved.
    pub sketch: Sketch,
    pub report: Report,
}

/// What a solve found.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Whether every constraint holds: lengths within [`SOLVE_TOL`], angles
    /// within [`ANGLE_TOL`](crate::ANGLE_TOL).
    pub converged: bool,
    /// Steps taken, over every component.
    pub iterations: u32,
    /// Whether any position or radius changed. A sketch that already held
    /// comes back with the same bits and `changed` false.
    pub changed: bool,
    pub state: State,
    /// Degrees of freedom left.
    pub dof: usize,
    /// What can still move, by entity.
    pub free: Vec<Free>,
    /// Minimal sets of constraints that say the same thing and hold.
    pub redundant: Vec<Vec<Id>>,
    /// Minimal sets of constraints that can't all hold.
    pub conflicting: Vec<Vec<Id>>,
    /// The closed loops of curves that aren't construction geometry.
    pub loops: Vec<Loop>,
    /// Curves that join but don't close.
    pub open: Vec<Chain>,
}

/// Solves `sketch`: every constraint within [`SOLVE_TOL`] (lengths) or
/// [`ANGLE_TOL`](crate::ANGLE_TOL) (angles) of holding,
/// moving the geometry as little as the solver can. Fails only when the
/// sketch refers to what isn't there ([`Sketch::validate`]).
pub fn solve(sketch: &Sketch) -> Result<Solved, SketchError> {
    solve_with(sketch, true)
}

/// [`solve`] by stage 1 alone: each component as a whole, diagnosed from
/// its whole Jacobian. For comparing the stages in tests and fuzzing.
#[doc(hidden)]
pub fn solve_whole(sketch: &Sketch) -> Result<Solved, SketchError> {
    solve_with(sketch, false)
}

fn solve_with(sketch: &Sketch, staged: bool) -> Result<Solved, SketchError> {
    sketch.validate()?;
    let sys = System::new(sketch);
    let mut x = sys.vars.clone();
    let mut iterations = 0;
    let mut converged = sys.faults.iter().all(|f| !f.conflicting)
        && sys.constant_eqs.iter().all(|&i| sys.misfit(i, &x) <= 1.0);
    let mut held = Vec::with_capacity(sys.components.len());
    let mut pieces = Vec::with_capacity(sys.components.len());
    for comp in &sys.components {
        let (ok, steps, blocks) = if staged {
            in_pieces(&sys, comp, &mut x)
        } else {
            let (ok, steps) = component(&sys, comp, &mut x);
            (ok, steps, None)
        };
        converged &= ok;
        held.push(ok);
        pieces.push(blocks);
        iterations += steps;
    }
    let d = diagnose(&sys, &x, &held, &pieces);
    let (loops, open) = loops::find(sketch, &sys.group_of);
    let solved = write_back(sketch, &sys, &x);
    let changed = solved != *sketch;
    Ok(Solved {
        sketch: solved,
        report: Report {
            converged,
            iterations,
            changed,
            state: d.state(),
            dof: d.dof,
            free: d.free,
            redundant: d.redundant,
            conflicting: d.conflicting,
            loops,
            open,
        },
    })
}

/// The largest misfit of `comp`'s equations at `x`
/// ([`System::misfit`]).
fn worst(sys: &System, comp: &Component, x: &[f64]) -> f64 {
    comp.eqs
        .iter()
        .map(|&i| sys.misfit(i, x))
        .fold(0.0, f64::max)
}

fn norm2(sys: &System, comp: &Component, x: &[f64]) -> f64 {
    comp.eqs
        .iter()
        .map(|&i| {
            let r = sys.residual(i, x);
            r * r
        })
        .sum()
}

/// The Jacobian and residuals of `comp` at `x`, columns in `comp.vars`
/// order (ascending); variables outside `comp` are left out.
pub(crate) fn jacobian(sys: &System, comp: &Component, x: &[f64]) -> (Mat, Vec<f64>) {
    let at = |v: usize| comp.vars.binary_search(&v).ok();
    let mut jac = Mat::zeros(comp.eqs.len(), comp.vars.len());
    let mut r = Vec::with_capacity(comp.eqs.len());
    for (i, &e) in comp.eqs.iter().enumerate() {
        let n = jac.cols;
        let row = &mut jac.data[i * n..(i + 1) * n];
        r.push(sys.gradient(e, x, &at, row));
    }
    (jac, r)
}

/// Solves one component in place, piece by piece. Returns whether it
/// converged, the steps tried, and the pieces it was solved in (`None`
/// when it went back to stage 1).
fn in_pieces(sys: &System, comp: &Component, x: &mut [f64]) -> (bool, u32, Option<Blocks>) {
    if worst(sys, comp, x) <= 1.0 {
        return (true, 0, Some(decompose(sys, comp)));
    }
    let start: Vec<f64> = comp.vars.iter().map(|&v| x[v]).collect();
    let blocks = decompose(sys, comp);
    let mut steps = 0;
    for piece in blocks.pieces() {
        let (ok, n) = component(sys, piece, x);
        steps += n;
        if !ok {
            for (&v, &s) in comp.vars.iter().zip(&start) {
                x[v] = s;
            }
            let (ok, n) = component(sys, comp, x);
            return (ok, steps + n, None);
        }
    }
    (worst(sys, comp, x) <= 1.0, steps, Some(blocks))
}

/// Solves one component (or one piece of one) in place. Returns whether
/// it converged and the steps tried.
fn component(sys: &System, comp: &Component, x: &mut [f64]) -> (bool, u32) {
    if worst(sys, comp, x) <= 1.0 {
        return (true, 0);
    }
    let mut damp = DAMP_START;
    let mut current = norm2(sys, comp, x);
    for step in 1..=MAX_STEPS {
        let (jac, r) = jacobian(sys, comp, x);
        let mut a = jac.gram();
        for i in 0..a.rows {
            *a.at_mut(i, i) += damp;
        }
        let Some(y) = cholesky_solve(&a, &r) else {
            damp *= 10.0;
            if damp > DAMP_MAX {
                return (false, step);
            }
            continue;
        };
        let dx: Vec<f64> = (0..jac.cols)
            .map(|j| (0..jac.rows).map(|i| jac.at(i, j) * y[i]).sum())
            .collect();
        let longest = dx.iter().fold(0.0, |m: f64, d| m.max(d.abs()));
        let cut = if longest > MAX_STEP {
            MAX_STEP / longest
        } else {
            1.0
        };
        // The step is tried in place (only this component's variables
        // move), and taken back if it doesn't help.
        let before: Vec<f64> = comp.vars.iter().map(|&v| x[v]).collect();
        for (j, &v) in comp.vars.iter().enumerate() {
            x[v] -= dx[j] * cut;
        }
        // A circle's radius can't go below zero: a step that would turn
        // one inside out stops it at zero, and the rest of the step stands.
        for &v in &comp.vars {
            if sys.var_owner[v].1 == Coord::R && x[v] < 0.0 {
                x[v] = 0.0;
            }
        }
        let next = norm2(sys, comp, x);
        if next < current {
            current = next;
            damp = (damp / 10.0).max(DAMP_MIN);
            if worst(sys, comp, x) <= AIM {
                return (true, step);
            }
        } else {
            for (&v, &b) in comp.vars.iter().zip(&before) {
                x[v] = b;
            }
            damp *= 10.0;
            if damp > DAMP_MAX {
                return (false, step);
            }
        }
    }
    (worst(sys, comp, x) <= 1.0, MAX_STEPS)
}

/// `sketch` with the solved variables put back, in meters.
fn write_back(sketch: &Sketch, sys: &System, x: &[f64]) -> Sketch {
    let s = sys.scale;
    let mut out = sketch.clone();
    for (&id, e) in out.entities.iter_mut() {
        match e {
            Entity::Point { at, .. } => {
                let g = sys.group_of[&id];
                for (k, sc) in sys.group_at[&g].iter().enumerate() {
                    match *sc {
                        Scalar::Var(v) => at[k] = x[v] * s,
                        // A constant group sits at its fixed point.
                        Scalar::Const(c) => at[k] = c * s,
                    }
                }
            }
            Entity::Circle { r, .. } => {
                if let Scalar::Var(v) = sys.radius_of[&id] {
                    *r = x[v] * s;
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sketch::tests::{line, point, rectangle};
    use crate::sketch::{Constraint as C, Id};
    use std::collections::BTreeMap;

    pub(crate) fn at(s: &Sketch, id: Id) -> [f64; 2] {
        match s.entities[&id] {
            Entity::Point { at, .. } => at,
            _ => panic!("{id} is not a point"),
        }
    }

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    /// A rectangle not quite square, fully dimensioned from a fixed corner.
    pub(crate) fn dimensioned() -> Sketch {
        let mut s = rectangle();
        s.entities.insert(1, point(0.01, -0.02));
        s.entities.insert(3, point(2.1, 0.95));
        s.constraints = BTreeMap::from([
            (1, C::Fix(1)),
            (2, C::Horizontal(11)),
            (3, C::Horizontal(13)),
            (4, C::Vertical(12)),
            (5, C::Vertical(14)),
            (6, C::Length(11, 3.0)),
            (7, C::Length(12, 1.5)),
        ]);
        s
    }

    #[test]
    fn a_dimensioned_rectangle_solves() {
        let r = solve(&dimensioned()).unwrap();
        assert!(r.report.converged, "{:?}", r.report);
        assert!(r.report.changed);
        let s = &r.sketch;
        // Fixed by equations, so within the tolerance, not to the bit.
        assert!(near(at(s, 1), [0.01, -0.02]), "{:?}", at(s, 1));
        assert!(near(at(s, 2), [3.01, -0.02]), "{:?}", at(s, 2));
        assert!(near(at(s, 3), [3.01, 1.48]), "{:?}", at(s, 3));
        assert!(near(at(s, 4), [0.01, 1.48]), "{:?}", at(s, 4));
    }

    #[test]
    fn a_solved_sketch_comes_back_with_the_same_bits() {
        let once = solve(&dimensioned()).unwrap().sketch;
        let twice = solve(&once).unwrap();
        assert!(twice.report.converged);
        assert!(!twice.report.changed);
        assert_eq!(twice.report.iterations, 0);
        assert_eq!(twice.sketch, once);
    }

    #[test]
    fn free_geometry_moves_as_little_as_it_can() {
        // A point put on a horizontal line moves straight down to it.
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
        let r = solve(&s).unwrap();
        assert!(r.report.converged);
        assert!(near(at(&r.sketch, 3), [1.5, 0.0]), "{:?}", at(&r.sketch, 3));
    }

    #[test]
    fn roots_dont_flip() {
        // A point 1 from each of two fixed points stays on its side.
        for y in [0.4, -0.4] {
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
                    at: [1.2, 0.0],
                    fixed: true,
                },
            );
            s.entities.insert(3, point(0.6, y));
            s.constraints.insert(1, C::Distance(1, 3, 1.0));
            s.constraints.insert(2, C::Distance(2, 3, 1.0));
            let r = solve(&s).unwrap();
            assert!(r.report.converged);
            assert!(at(&r.sketch, 3)[1].signum() == y.signum());
        }
    }

    #[test]
    fn coincident_points_meet() {
        let mut s = rectangle();
        s.entities.insert(5, point(2.3, 0.1));
        s.entities.insert(15, line(5, 3));
        s.constraints.insert(1, C::Coincident(2, 5));
        let r = solve(&s).unwrap();
        assert!(r.report.converged);
        assert_eq!(at(&r.sketch, 5), at(&r.sketch, 2));
        assert_eq!(at(&r.sketch, 2), [2.0, 0.0]);
    }

    #[test]
    fn tangent_arcs_and_circles() {
        // A line, and an arc meeting it at its end, made tangent there; a
        // circle of radius 0.5 tangent to the line.
        let mut s = Sketch::default();
        for (id, x, y) in [
            (1, 0.0, 0.0),
            (2, 2.0, 0.0),
            (3, 2.2, 1.0),
            (4, 3.0, 1.1),
            (5, 1.0, 0.8),
        ] {
            s.entities.insert(id, point(x, y));
        }
        s.entities.insert(10, line(1, 2));
        s.entities.insert(
            11,
            Entity::Arc {
                center: 3,
                start: 2,
                end: 4,
                construction: false,
            },
        );
        s.entities.insert(
            12,
            Entity::Circle {
                center: 5,
                r: 0.3,
                fixed: false,
                construction: false,
            },
        );
        s.constraints.insert(1, C::Fix(10));
        s.constraints.insert(2, C::Tangent(10, 11));
        s.constraints.insert(3, C::Tangent(12, 10));
        s.constraints.insert(4, C::Radius(12, 0.5));
        let r = solve(&s).unwrap();
        assert!(r.report.converged, "{:?}", r.report);
        let c = at(&r.sketch, 3);
        assert!((c[0] - 2.0).abs() < 1e-9, "{c:?}");
        let Entity::Circle { r: radius, .. } = r.sketch.entities[&12] else {
            panic!()
        };
        assert!((radius - 0.5).abs() < 1e-12);
        assert!((at(&r.sketch, 5)[1] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn conflicts_dont_converge() {
        let mut s = rectangle();
        s.constraints.insert(1, C::Length(11, 2.0));
        s.constraints.insert(2, C::Length(11, 3.0));
        let r = solve(&s).unwrap();
        assert!(!r.report.converged);
    }

    #[test]
    fn fixed_points_never_move() {
        let mut s = rectangle();
        s.entities.insert(
            1,
            Entity::Point {
                at: [0.1, 0.2],
                fixed: true,
            },
        );
        s.constraints.insert(1, C::Length(11, 5.0));
        let r = solve(&s).unwrap();
        assert!(r.report.converged);
        assert_eq!(at(&r.sketch, 1), [0.1, 0.2]);
    }

    #[test]
    fn radii_never_turn_negative() {
        // A circle of radius 0 made tangent inside a larger one: the
        // smallest step would make its radius negative. Found by fuzzing.
        let mut s = Sketch::default();
        s.entities
            .insert(1, point(0.6509803921568627, -1.3725490196078431));
        s.entities.insert(
            2,
            Entity::Point {
                at: [1.1529411764705881, -1.6705882352941177],
                fixed: true,
            },
        );
        for (id, center, r) in [(10, 1, 0.0), (11, 2, 1.1607843137254903)] {
            s.entities.insert(
                id,
                Entity::Circle {
                    center,
                    r,
                    fixed: false,
                    construction: false,
                },
            );
        }
        s.constraints.insert(1, C::Tangent(10, 11));
        let r = solve(&s).unwrap();
        assert!(r.report.converged, "{:?}", r.report);
        assert!(r.sketch.validate().is_ok(), "{:?}", r.sketch.entities[&10]);
    }

    #[test]
    fn collapsed_lines_hold_no_direction() {
        // A line from a free point to a fixed one at the same place, made
        // tangent to an arc: the solver shrank it to 2.5e-12 m, where its
        // tangency read as holding at one scale and not at the next, so the
        // solved sketch didn't solve again. Found by fuzzing.
        let mut s = Sketch::default();
        for (id, x, y) in [(2, 2.0, 2.0), (3, 2.0, 2.0), (4, -1.780392156862745, 2.0)] {
            s.entities.insert(id, point(x, y));
        }
        s.entities.insert(6, point(2.0, 2.0));
        s.entities.insert(
            5,
            Entity::Point {
                at: [2.0, 2.0],
                fixed: true,
            },
        );
        s.entities.insert(100, line(2, 5));
        s.entities.insert(
            101,
            Entity::Arc {
                center: 6,
                start: 4,
                end: 3,
                construction: false,
            },
        );
        s.constraints.insert(1, C::Tangent(100, 101));
        let r = solve(&s).unwrap();
        if r.report.converged {
            let (a, b) = (at(&r.sketch, 2), at(&r.sketch, 5));
            let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
            assert!((dx * dx + dy * dy).sqrt() >= crate::SOLVE_TOL, "{a:?}");
            let again = solve(&r.sketch).unwrap();
            assert!(again.report.converged, "{:?}", again.report);
        }
        // A fixed line of no length is square to nothing.
        let mut flat = rectangle();
        for p in [1, 2] {
            flat.entities.insert(
                p,
                Entity::Point {
                    at: [0.0, 0.0],
                    fixed: true,
                },
            );
        }
        flat.constraints.insert(1, C::Perpendicular(11, 12));
        assert!(!solve(&flat).unwrap().report.converged);
    }

    #[test]
    fn angle_residuals_dont_lead_off_to_infinity() {
        // Two lines through a free point made parallel: the sine shrinks
        // as the point runs away, but the nearby solution is the one to
        // find, and once found it holds as it is. Found by fuzzing.
        let mut s = Sketch::default();
        s.entities.insert(
            0,
            Entity::Point {
                at: [-2.0, 2.0],
                fixed: true,
            },
        );
        s.entities
            .insert(1, point(-0.16470588235294126, -0.16470588235294126));
        s.entities.insert(
            2,
            Entity::Point {
                at: [-2.0, -1.7490196078431373],
                fixed: true,
            },
        );
        s.entities.insert(101, line(1, 2));
        s.entities.insert(102, line(0, 1));
        s.constraints.insert(0, C::Parallel(102, 101));
        let r = solve(&s).unwrap();
        assert!(r.report.converged);
        let p = at(&r.sketch, 1);
        assert!((p[0] + 2.0).abs() < 1e-9 && p[1].abs() < 1.0, "{p:?}");
        let again = solve(&r.sketch).unwrap();
        assert!(!again.report.changed);
    }

    #[test]
    fn invalid_sketches_are_refused() {
        let mut s = rectangle();
        s.constraints.insert(1, C::Horizontal(1));
        assert!(solve(&s).is_err());
    }
}
