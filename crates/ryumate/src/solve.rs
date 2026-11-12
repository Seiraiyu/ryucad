//! Solving an assembly: each component of its system by
//! `ryusketch::core`'s steps, piece by piece, from the stored poses; then
//! the diagnosis, read at the solved poses.

use crate::assembly::Kind;
use crate::assembly::{Assembly, Id, MateError};
use crate::pose::Pose;
use crate::report::MateState;
use crate::report::{Report, diagnose};
use crate::system::{System, pose_at};
use ryusketch::core::solve::{component, in_pieces};
use ryusketch::{ANGLE_TOL, SOLVE_TOL};
use std::collections::BTreeMap;

/// A solved assembly: every body's pose, and what the solve found.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    /// Each body's pose. A body that didn't move has its stored pose, to
    /// the bit.
    pub poses: BTreeMap<Id, Pose>,
    pub report: Report,
}

impl Solved {
    /// `asm` with its bodies at their solved poses.
    pub fn assembly(&self, asm: &Assembly) -> Assembly {
        let mut out = asm.clone();
        for (id, b) in out.bodies.iter_mut() {
            b.pose = self.poses[id];
        }
        out
    }
}

/// Solves `asm`: every mate within `ryusketch::SOLVE_TOL` (lengths) or
/// `ANGLE_TOL` (angles) of holding, moving bodies as little as the solver
/// can, from their stored poses. Fails only when the assembly refers to
/// what isn't there or holds geometry it can't use
/// ([`Assembly::validate`]).
pub fn solve(asm: &Assembly) -> Result<Solved, MateError> {
    solve_with(asm, true)
}

/// [`solve`] with each component solved and diagnosed whole, never in
/// pieces: for comparing the two in tests and fuzzing.
#[doc(hidden)]
pub fn solve_whole(asm: &Assembly) -> Result<Solved, MateError> {
    solve_with(asm, false)
}

/// The turn a body takes before solving when a mate on it starts with its
/// frames exactly opposite: a rotation vector a few hundredths of a
/// radian long, off every axis, so the residuals have a direction to go.
const UNFLIP: [f64; 3] = [0.01, 0.02, 0.03];

/// Turns the last free body each mate reads by [`UNFLIP`] where the mate
/// starts at a pose its rows can't judge (frames exactly opposite each
/// other, where a turn either way is as good), so a solve from there finds
/// the frames agreeing rather than failing.
fn unflip(sys: &System, comp: &ryusketch::core::Component, x: &mut [f64]) {
    for &e in &comp.eqs {
        if sys.misfit_at(e, x).is_finite() {
            continue;
        }
        let Some(b) = sys.eqs[e].bodies.last() else {
            continue;
        };
        let v = sys.var_of[b];
        if x[v + 3..v + 6].iter().all(|&w| w == 0.0) {
            x[v + 3..v + 6].copy_from_slice(&UNFLIP);
        }
    }
}

fn solve_round(asm: &Assembly, staged: bool) -> Result<Solved, MateError> {
    asm.validate()?;
    let sys = System::new(asm);
    let mut x = vec![0.0; sys.owner.len()];
    let mut iterations = 0;
    for comp in &sys.components {
        unflip(&sys, comp, &mut x);
        let (_, steps) = if staged {
            let (ok, steps, _) = in_pieces(&sys, comp, &mut x);
            (ok, steps)
        } else {
            component(&sys, comp, &mut x)
        };
        iterations += steps;
    }
    let poses: BTreeMap<Id, Pose> = asm
        .bodies
        .keys()
        .map(|&b| (b, pose_at(&sys, b, &x)))
        .collect();
    let changed = poses.iter().any(|(b, p)| *p != asm.bodies[b].pose);
    let mut solved = Solved {
        poses,
        report: Report::default(),
    };
    // Read where the bodies now are, so what moves is told from there, and
    // whether each component holds is judged at the poses given back.
    let at = solved.assembly(asm);
    solved.report = diagnose(&at, staged, iterations, changed);
    Ok(solved)
}

/// The driven angle and distance of a kind, if it drives them.
fn driven(kind: &Kind) -> (Option<f64>, Option<f64>) {
    match *kind {
        Kind::Revolute { angle } => (angle, None),
        Kind::Slider { distance } => (None, distance),
        Kind::Cylindrical { angle, distance } => (angle, distance),
        Kind::Planar { angle, distance } => (angle, Some(distance)),
        Kind::Fastened | Kind::Ball => (None, None),
    }
}

/// Drives `kind`'s angle (`turn`) or distance to `v`: pins a value at a
/// stop, as the row the kind already has for it.
fn pin(kind: &mut Kind, turn: bool, v: f64) {
    match (kind, turn) {
        (Kind::Revolute { angle }, true)
        | (Kind::Cylindrical { angle, .. }, true)
        | (Kind::Planar { angle, .. }, true) => *angle = Some(v),
        (Kind::Slider { distance }, false) | (Kind::Cylindrical { distance, .. }, false) => {
            *distance = Some(v)
        }
        _ => unreachable!("validated: a limit is on a value its kind leaves free"),
    }
}

/// Every limited value of `asm` that isn't driven, of a mate that held
/// in `solved`, past a bound by more than its tolerance: by mate ID, the
/// angle first, each with the bound it passed.
fn past(asm: &Assembly, solved: &Solved) -> Vec<(Id, bool, f64)> {
    let mut out = Vec::new();
    for (id, m) in &asm.mates {
        if !matches!(
            solved.report.mates.get(id),
            Some(MateState::Well | MateState::Redundant)
        ) {
            continue;
        }
        let Some(v) = solved.report.values.get(id) else {
            continue;
        };
        let (da, dd) = driven(&m.kind);
        if let (Some(r), None, Some(x)) = (m.limits.angle, da, v.angle)
            && let Some(s) = r.past(x.value, ANGLE_TOL)
        {
            out.push((*id, true, r.bound(s).expect("past a bound it has")));
        }
        if let (Some(r), None, Some(x)) = (m.limits.distance, dd, v.distance)
            && let Some(s) = r.past(x.value, SOLVE_TOL)
        {
            out.push((*id, false, r.bound(s).expect("past a bound it has")));
        }
    }
    out
}

fn solve_with(asm: &Assembly, staged: bool) -> Result<Solved, MateError> {
    let mut solved = solve_round(asm, staged)?;
    if asm.mates.values().all(|m| m.limits.is_none()) {
        return Ok(solved);
    }
    // Each round pins at least one more value at its stop and solves
    // again from the stored poses; a pinned value is never released.
    let mut pinned = asm.clone();
    let mut iterations = solved.report.iterations;
    loop {
        let found = past(&pinned, &solved);
        if found.is_empty() {
            break;
        }
        for (id, turn, bound) in found {
            pin(
                &mut pinned.mates.get_mut(&id).expect("a mate").kind,
                turn,
                bound,
            );
        }
        solved = solve_round(&pinned, staged)?;
        iterations += solved.report.iterations;
    }
    solved.report.iterations = iterations;
    if pinned != *asm && solved.report.converged {
        // Held at its stops: freedom, states and stops as the mates say
        // without the pins (a limit takes no freedom away).
        let at = solved.assembly(asm);
        solved.report = diagnose(&at, staged, iterations, solved.report.changed);
    }
    Ok(solved)
}
