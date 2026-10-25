//! Damped least-norm Gauss-Newton (Levenberg-Marquardt) on a component of
//! a system, from the variables it is given.
//!
//! Each step is the smallest change that would satisfy the linearized
//! equations, `dx = -Jᵀ (J Jᵀ + λI)⁻¹ r`, so variables the equations leave
//! free move as little as they can and a solve stays on the branch it
//! starts on. A component that already holds is left with the very same
//! bits.
//!
//! [`in_pieces`] solves a component in the pieces of its
//! Dulmage-Mendelsohn decomposition ([`super::blocks`]): its
//! over-determined part, then its square blocks in order, then its
//! under-determined part, each by the same steps over its own variables.
//! A piece that doesn't converge sends the whole component back to
//! [`component`] from where it started.

use super::blocks::{Blocks, decompose};
use super::linalg::{Mat, cholesky_solve};
use super::{Component, Equations};

/// Steps tried per component, accepted or not.
pub const MAX_STEPS: u32 = 200;
/// A component holds when every misfit is at most 1, and the solver stops
/// only well inside it, so a solved system rounded a little differently
/// (at another scale, say) still holds.
pub const AIM: f64 = 0.25;
/// The first damping, and the bounds it moves between.
const DAMP_START: f64 = 1e-6;
const DAMP_MIN: f64 = 1e-15;
const DAMP_MAX: f64 = 1e12;

/// The largest misfit of `comp`'s equations at `x`
/// ([`Equations::misfit`]).
pub fn worst(sys: &impl Equations, comp: &Component, x: &[f64]) -> f64 {
    comp.eqs
        .iter()
        .map(|&i| sys.misfit(i, x))
        .fold(0.0, f64::max)
}

fn norm2(sys: &impl Equations, comp: &Component, x: &[f64]) -> f64 {
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
pub fn jacobian(sys: &impl Equations, comp: &Component, x: &[f64]) -> (Mat, Vec<f64>) {
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
/// when it went back to solving it whole).
pub fn in_pieces(
    sys: &impl Equations,
    comp: &Component,
    x: &mut [f64],
) -> (bool, u32, Option<Blocks>) {
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
pub fn component(sys: &impl Equations, comp: &Component, x: &mut [f64]) -> (bool, u32) {
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
        // The whole step is cut so no variable moves further than it may.
        // With one limit for all, this is that limit over the longest
        // move, to the bit.
        let cut = comp
            .vars
            .iter()
            .zip(&dx)
            .filter_map(|(&v, d)| {
                let limit = sys.max_step(v);
                (d.abs() > limit).then(|| limit / d.abs())
            })
            .fold(1.0, f64::min);
        // The step is tried in place (only this component's variables
        // move), and taken back if it doesn't help.
        let before: Vec<f64> = comp.vars.iter().map(|&v| x[v]).collect();
        for (j, &v) in comp.vars.iter().enumerate() {
            x[v] = sys.clamp(v, x[v] - dx[j] * cut);
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
