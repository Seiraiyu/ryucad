//! What a solve tells its caller beyond poses: each body's degrees of
//! freedom and the motions they allow, each mate's state, and the minimal
//! sets of mates that are redundant or in conflict.
//!
//! The diagnosis is read at the solved poses, where every variable is
//! zero, so a motion's rotation variables are its angular velocity and its
//! translation variables the velocity of the body's origin: a twist. A
//! body's free motions are the twists the null space gives it, told as
//! translations (directions) and rotations (an axis through a point), in
//! a canonical basis: one direction with its largest coordinate positive,
//! a plane by its normal, or all three of the assembly's axes.

use crate::assembly::{Assembly, Id};
use crate::pose::{add, cross, dot, norm, scale as times, sub};
use crate::system::System;
use ryusketch::core::blocks::decompose;
use ryusketch::core::diagnose::{Part, SUPPORT_TOL, diagnose as read};
use ryusketch::core::solve::worst;
use std::collections::{BTreeMap, BTreeSet};

/// How well defined an assembly is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Something can still move.
    #[default]
    Under,
    /// Nothing can move, and no mate is redundant.
    Well,
    /// Some mates are redundant or in conflict.
    Over,
}

/// A body's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyState {
    /// Nothing moves it.
    Well,
    /// Something can still move it.
    Under,
    /// A mate on it is redundant or in conflict.
    Over,
    /// Its mates don't hold.
    Failing,
}

/// A mate's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MateState {
    Well,
    /// It says what other mates say, and holds.
    Redundant,
    /// It and others can't all hold.
    Conflicting,
    /// It doesn't hold, and no conflict explains it (the solver didn't
    /// reach a solution).
    Failing,
}

/// A rotation left free: about `direction` (unit), through `through`
/// (meters, the point of the axis nearest the body's origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub direction: [f64; 3],
    pub through: [f64; 3],
}

/// What can still move a body, in assembly coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Freedom {
    /// Degrees of freedom, 0 to 6.
    pub dof: usize,
    /// Directions it can slide in.
    pub translations: Vec<[f64; 3]>,
    /// Axes it can turn about.
    pub rotations: Vec<Axis>,
}

/// One body's diagnosis.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyReport {
    pub state: BodyState,
    pub freedom: Freedom,
}

/// What a solve found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Whether every mate holds at the poses given back.
    pub converged: bool,
    /// Steps taken, over every component.
    pub iterations: u32,
    /// Whether any body moved.
    pub changed: bool,
    pub state: State,
    /// Degrees of freedom left, over the whole assembly.
    pub dof: usize,
    pub bodies: BTreeMap<Id, BodyReport>,
    pub mates: BTreeMap<Id, MateState>,
    /// Minimal sets of mates that say the same thing and hold.
    pub redundant: Vec<Vec<Id>>,
    /// Minimal sets of mates that can't all hold.
    pub conflicting: Vec<Vec<Id>>,
}

/// Orthonormal vectors spanning `vectors`, by Gram-Schmidt (twice) in
/// order; what is left of a vector shorter than `SUPPORT_TOL` adds
/// nothing.
fn orthonormal(vectors: &[[f64; 3]]) -> Vec<[f64; 3]> {
    let mut out: Vec<[f64; 3]> = Vec::new();
    for &v in vectors {
        let mut u = v;
        for _ in 0..2 {
            for e in &out {
                u = sub(u, times(*e, dot(u, *e)));
            }
        }
        let n = norm(u);
        if n > SUPPORT_TOL {
            out.push(times(u, 1.0 / n));
        }
    }
    out
}

/// `v` with its largest coordinate (the first of equals) positive.
fn signed(v: [f64; 3]) -> [f64; 3] {
    let mut k = 0;
    for i in 1..3 {
        if v[i].abs() > v[k].abs() {
            k = i;
        }
    }
    if v[k] < 0.0 { times(v, -1.0) } else { v }
}

/// The canonical basis of the span of orthonormal `basis`: a direction
/// signed, a plane as the assembly axis nearest it and the normal's cross
/// with that, everything as the assembly's own axes.
fn canonical(basis: &[[f64; 3]]) -> Vec<[f64; 3]> {
    match basis {
        [] => vec![],
        [d] => vec![signed(*d)],
        [a, b] => {
            let n = signed(cross(*a, *b));
            let n = times(n, 1.0 / norm(n));
            let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            let in_plane = |e: [f64; 3]| sub(e, times(n, dot(e, n)));
            let mut best = in_plane(axes[0]);
            for e in &axes[1..] {
                let p = in_plane(*e);
                if norm(p) > norm(best) {
                    best = p;
                }
            }
            let e1 = times(best, 1.0 / norm(best));
            vec![e1, cross(n, e1)]
        }
        _ => vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    }
}

/// A body's freedom from its twists (each its six variables in the null
/// space: three of translation in solver units, then three of rotation),
/// with its origin in meters and the scale.
fn freedom(twists: &[[f64; 6]], origin: [f64; 3], scale: f64) -> Freedom {
    let split = |t: &[f64; 6]| ([t[0], t[1], t[2]], [t[3], t[4], t[5]]);
    // Twists whose rotations are orthonormal, by Gram-Schmidt on the
    // rotation parts carrying the whole twist along; what has no rotation
    // left is a translation.
    let mut turning: Vec<[f64; 6]> = Vec::new();
    let mut sliding: Vec<[f64; 3]> = Vec::new();
    for t in twists {
        let mut u = *t;
        for _ in 0..2 {
            for e in &turning {
                let d = dot(split(&u).1, split(e).1);
                for k in 0..6 {
                    u[k] -= d * e[k];
                }
            }
        }
        let (v, w) = split(&u);
        let n = norm(w);
        if n > SUPPORT_TOL {
            turning.push(u.map(|q| q / n));
        } else {
            sliding.push(v);
        }
    }
    let translations = orthonormal(&sliding);
    let rotations: Vec<([f64; 3], [f64; 3])> = turning
        .iter()
        .map(|t| {
            let (mut v, w) = split(t);
            // Sliding along a free direction says nothing of the axis.
            for e in &translations {
                v = sub(v, times(*e, dot(v, *e)));
            }
            (w, v)
        })
        .collect();
    let omegas: Vec<[f64; 3]> = rotations.iter().map(|(w, _)| *w).collect();
    let axes = canonical(&omegas)
        .into_iter()
        .map(|a| {
            // The twist turning about `a`: a's coordinates in the
            // orthonormal rotations, applied to their velocities.
            let v = rotations
                .iter()
                .fold([0.0; 3], |acc, (w, v)| add(acc, times(*v, dot(a, *w))));
            // v = a × (origin - p) for an axis through p: p is the origin
            // moved by a × v, to the point of the axis nearest it.
            Axis {
                direction: a,
                through: add(origin, times(cross(a, v), scale)),
            }
        })
        .collect();
    Freedom {
        dof: rotations.len() + translations.len(),
        translations: canonical(&translations),
        rotations: axes,
    }
}

/// Diagnoses `asm`, whose bodies are at their solved poses; `staged`
/// reads components from their pieces where it can.
pub(crate) fn diagnose(asm: &Assembly, staged: bool, iterations: u32, changed: bool) -> Report {
    let sys = System::new(asm);
    let x = vec![0.0; sys.owner.len()];
    let mut redundant = BTreeSet::new();
    let mut conflicting = BTreeSet::new();
    let mut converged = true;
    for &i in &sys.constant_eqs {
        let set = vec![sys.eqs[i].mate];
        if sys.misfit_at(i, &x) <= 1.0 {
            redundant.insert(set);
        } else {
            converged = false;
            conflicting.insert(set);
        }
    }
    // Mates read by a constant equation that holds count once, not per
    // row; one that fails conflicts.
    redundant.retain(|s| !conflicting.contains(s));
    // A mate whose frames point exactly opposite ways can't be judged
    // (its rows have no derivative there): it conflicts alone, whatever
    // its rows' ranks say.
    let flipped: BTreeSet<Id> = (0..sys.eqs.len())
        .filter(|&i| sys.misfit_at(i, &x).is_infinite())
        .map(|i| sys.eqs[i].mate)
        .collect();
    let mut dof = 0;
    let mut parts: Vec<(Part, bool)> = Vec::new();
    for comp in &sys.components {
        let ok = worst(&sys, comp, &x) <= 1.0;
        converged &= ok;
        let blocks = staged.then(|| decompose(&sys, comp));
        let part = read(&sys, comp, &x, ok, blocks.as_ref());
        dof += part.dof;
        for (set, conflict) in &part.sets {
            if set.iter().any(|m| flipped.contains(m)) {
                continue;
            }
            if *conflict {
                conflicting.insert(set.clone());
            } else {
                redundant.insert(set.clone());
            }
        }
        parts.push((part, ok));
    }
    for &m in &flipped {
        conflicting.insert(vec![m]);
    }
    let in_sets = |m: Id, sets: &BTreeSet<Vec<Id>>| sets.iter().any(|s| s.contains(&m));
    // Each mate's component, and whether it held.
    let mut held: BTreeMap<Id, bool> = BTreeMap::new();
    for (comp, (_, ok)) in sys.components.iter().zip(&parts) {
        for &e in &comp.eqs {
            held.insert(sys.eqs[e].mate, *ok);
        }
    }
    for &e in &sys.constant_eqs {
        let m = sys.eqs[e].mate;
        held.insert(m, !in_sets(m, &conflicting));
    }
    let mates: BTreeMap<Id, MateState> = asm
        .mates
        .keys()
        .map(|&m| {
            let state = if in_sets(m, &conflicting) {
                MateState::Conflicting
            } else if in_sets(m, &redundant) {
                MateState::Redundant
            } else if !held.get(&m).copied().unwrap_or(true) {
                MateState::Failing
            } else {
                MateState::Well
            };
            (m, state)
        })
        .collect();
    let mut bodies = BTreeMap::new();
    for (&b, body) in &asm.bodies {
        if body.fixed {
            bodies.insert(
                b,
                BodyReport {
                    state: BodyState::Well,
                    freedom: Freedom::default(),
                },
            );
            continue;
        }
        let first = sys.var_of[&b];
        let (k, (part, ok)) = sys
            .components
            .iter()
            .zip(&parts)
            .enumerate()
            .find(|(_, (c, _))| c.vars.binary_search(&first).is_ok())
            .map(|(k, (_, p))| (k, p))
            .expect("every free body is in a component");
        let twists: Vec<[f64; 6]> = part
            .null
            .iter()
            .map(|v| {
                let mut t = [0.0; 6];
                for (j, &var) in part.vars.iter().enumerate() {
                    if (first..first + 6).contains(&var) {
                        t[var - first] = v[j];
                    }
                }
                t
            })
            .collect();
        let freedom = freedom(&twists, asm.bodies[&b].pose.origin, sys.scale);
        let touches = |sets: &BTreeSet<Vec<Id>>| {
            sys.components[k]
                .eqs
                .iter()
                .any(|&e| sys.eqs[e].bodies.contains(&b) && in_sets(sys.eqs[e].mate, sets))
        };
        let state = if !ok {
            BodyState::Failing
        } else if touches(&redundant) || touches(&conflicting) {
            BodyState::Over
        } else if freedom.dof == 0 {
            BodyState::Well
        } else {
            BodyState::Under
        };
        bodies.insert(b, BodyReport { state, freedom });
    }
    let state = if !redundant.is_empty() || !conflicting.is_empty() {
        State::Over
    } else if dof == 0 {
        State::Well
    } else {
        State::Under
    };
    Report {
        converged,
        iterations,
        changed,
        state,
        dof,
        bodies,
        mates,
        redundant: redundant.into_iter().collect(),
        conflicting: conflicting.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases_are_canonical() {
        assert_eq!(canonical(&[[0.0, -1.0, 0.0]]), [[0.0, 1.0, 0.0]]);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        // The xy plane, given any way, is x then y.
        let plane = canonical(&[[h, h, 0.0], [h, -h, 0.0]]);
        assert_eq!(plane, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(
            canonical(&orthonormal(&[
                [1.0, 2.0, 3.0],
                [3.0, 1.0, 2.0],
                [0.0, 1.0, 0.0]
            ]))
            .len(),
            3
        );
        assert_eq!(orthonormal(&[[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]).len(), 1);
    }

    #[test]
    fn a_turn_about_an_offset_axis_is_told_through_it() {
        // Turning about z through (1, 0, 0), seen from a body at the
        // origin: the origin moves at a × (origin - p) = (0, -1, 0).
        let f = freedom(&[[0.0, -1.0, 0.0, 0.0, 0.0, 1.0]], [0.0; 3], 1.0);
        assert_eq!(f.dof, 1);
        assert!(f.translations.is_empty());
        let a = f.rotations[0];
        assert_eq!(a.direction, [0.0, 0.0, 1.0]);
        assert!((a.through[0] - 1.0).abs() < 1e-15 && a.through[1].abs() < 1e-15);
    }
}
