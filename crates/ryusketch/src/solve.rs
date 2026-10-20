//! The solver: damped least-norm Gauss-Newton (Levenberg-Marquardt) on
//! each component, from the positions the sketch holds.
//!
//! Each step is the smallest change that would satisfy the linearized
//! equations, `dx = -Jᵀ (J Jᵀ + λI)⁻¹ r`, so geometry the constraints leave
//! free moves as little as it can and a solve stays on the branch the
//! sketch is already on. A sketch that already holds is returned with the
//! very same bits.

use crate::linalg::{Mat, cholesky_solve};
use crate::sketch::{Entity, Sketch, SketchError};
use crate::system::{Component, SOLVE_TOL, Scalar, System};

/// Steps tried per component, accepted or not.
const MAX_STEPS: u32 = 200;
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
    /// Whether every constraint holds within [`SOLVE_TOL`].
    pub converged: bool,
    /// Steps taken, over every component.
    pub iterations: u32,
    /// Whether any position or radius changed. A sketch that already held
    /// comes back with the same bits and `changed` false.
    pub changed: bool,
}

/// Solves `sketch`: every constraint within [`SOLVE_TOL`] of holding,
/// moving the geometry as little as the solver can. Fails only when the
/// sketch refers to what isn't there ([`Sketch::validate`]).
pub fn solve(sketch: &Sketch) -> Result<Solved, SketchError> {
    sketch.validate()?;
    let sys = System::new(sketch);
    let tol = SOLVE_TOL / sys.scale;
    let mut x = sys.vars.clone();
    let mut iterations = 0;
    let mut converged = sys.faults.iter().all(|f| !f.conflicting)
        && sys
            .constant_eqs
            .iter()
            .all(|&i| sys.residual(i, &x).abs() <= tol);
    for comp in &sys.components {
        let (ok, steps) = component(&sys, comp, &mut x, tol);
        converged &= ok;
        iterations += steps;
    }
    let solved = write_back(sketch, &sys, &x);
    let changed = solved != *sketch;
    Ok(Solved {
        sketch: solved,
        report: Report {
            converged,
            iterations,
            changed,
        },
    })
}

/// The largest residual of `comp`'s equations at `x`.
fn worst(sys: &System, comp: &Component, x: &[f64]) -> f64 {
    comp.eqs
        .iter()
        .map(|&i| sys.residual(i, x).abs())
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
/// order.
pub(crate) fn jacobian(sys: &System, comp: &Component, x: &[f64]) -> (Mat, Vec<f64>) {
    let mut col = vec![usize::MAX; sys.vars.len()];
    for (j, &v) in comp.vars.iter().enumerate() {
        col[v] = j;
    }
    let at = |v: usize| (col[v] != usize::MAX).then_some(col[v]);
    let mut jac = Mat::zeros(comp.eqs.len(), comp.vars.len());
    let mut r = Vec::with_capacity(comp.eqs.len());
    for (i, &e) in comp.eqs.iter().enumerate() {
        let n = jac.cols;
        let row = &mut jac.data[i * n..(i + 1) * n];
        r.push(sys.gradient(e, x, &at, row));
    }
    (jac, r)
}

/// Solves one component in place. Returns whether it converged and the
/// steps tried.
fn component(sys: &System, comp: &Component, x: &mut [f64], tol: f64) -> (bool, u32) {
    if worst(sys, comp, x) <= tol {
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
        let mut trial = x.to_vec();
        for (j, &v) in comp.vars.iter().enumerate() {
            let dx: f64 = (0..jac.rows).map(|i| jac.at(i, j) * y[i]).sum();
            trial[v] -= dx;
        }
        let next = norm2(sys, comp, &trial);
        if next < current {
            for &v in &comp.vars {
                x[v] = trial[v];
            }
            current = next;
            damp = (damp / 10.0).max(DAMP_MIN);
            if worst(sys, comp, x) <= tol {
                return (true, step);
            }
        } else {
            damp *= 10.0;
            if damp > DAMP_MAX {
                return (false, step);
            }
        }
    }
    (false, MAX_STEPS)
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
    fn invalid_sketches_are_refused() {
        let mut s = rectangle();
        s.constraints.insert(1, C::Horizontal(1));
        assert!(solve(&s).is_err());
    }
}
