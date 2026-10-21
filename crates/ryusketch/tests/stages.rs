//! Stage 2 (in pieces) against stage 1 (whole components) on the property
//! corpus: the same report, and for well-conditioned, well-defined
//! sketches the same positions within what the solver's tolerance allows.

mod common;

use common::{at, jostle, polygon, slot};
use proptest::prelude::*;
use ryukei_testkit::Lcg;
use ryusketch::{Constraint, Entity, Report, SOLVE_TOL, Sketch, State, solve, solve_whole};

/// Everything but the step count, which the stages spend differently.
fn same_report(a: &Report, b: &Report) -> Result<(), TestCaseError> {
    prop_assert_eq!(a.converged, b.converged);
    prop_assert_eq!(a.state, b.state);
    prop_assert_eq!(a.dof, b.dof);
    prop_assert_eq!(&a.free, &b.free);
    prop_assert_eq!(&a.redundant, &b.redundant);
    prop_assert_eq!(&a.conflicting, &b.conflicting);
    prop_assert_eq!(&a.loops, &b.loops);
    prop_assert_eq!(&a.open, &b.open);
    Ok(())
}

/// Both stages on `s`: the same report.
fn both(s: &Sketch) -> Result<(ryusketch::Solved, ryusketch::Solved), TestCaseError> {
    let (two, one) = (solve(s).unwrap(), solve_whole(s).unwrap());
    same_report(&two.report, &one.report)?;
    Ok((two, one))
}

/// How far apart two solves of one well-conditioned sketch may put a
/// point, in `SOLVE_TOL`s per unit of its size: each holds its equations
/// within the tolerance, so each point is within the tolerance times the
/// sketch's conditioning (about 1/sin of the closing turn here, at most
/// 20) of the exact root.
const APART: f64 = 50.0;

/// Every point of `a` within [`APART`] of `b`'s.
fn same_positions(a: &Sketch, b: &Sketch, size: f64) -> Result<(), TestCaseError> {
    for (id, e) in &a.entities {
        if let Entity::Point { .. } = e {
            let (p, q) = (at(a, *id), at(b, *id));
            for k in 0..2 {
                prop_assert!(
                    (p[k] - q[k]).abs() <= APART * SOLVE_TOL * size.max(1.0),
                    "{id}: {p:?} vs {q:?}"
                );
            }
        }
    }
    Ok(())
}

/// The sine of the turn at the last point of a solved polygon of `n`
/// points: where two circles meet.
fn last_turn(s: &Sketch, n: usize) -> f64 {
    let (p, q, r) = (at(s, n as u32 - 1), at(s, n as u32), at(s, 1));
    let (u, w) = ([q[0] - p[0], q[1] - p[1]], [r[0] - q[0], r[1] - q[1]]);
    (u[0] * w[1] - u[1] * w[0])
        / ryukei::math::sqrt((u[0] * u[0] + u[1] * u[1]) * (w[0] * w[0] + w[1] * w[1]))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn polygons_report_alike(seed in any::<u64>(), n in 3usize..12, size in 0.001f64..10.0, variant in 0u8..4) {
        let mut g = Lcg::new(seed);
        let mut s = polygon(&mut g, n, size);
        jostle(&mut s, &mut g, size * 0.02);
        match variant {
            // Under-defined: a side's length let go.
            1 => {
                s.constraints.remove(&2);
            }
            // Over-defined, consistently: a diagonal the others imply.
            2 if n >= 4 => {
                let solved = solve_whole(&s).unwrap().sketch;
                let (a, b) = (at(&solved, 1), at(&solved, 3));
                let d = ryukei::math::hypot(b[0] - a[0], b[1] - a[1]);
                s = solved;
                s.constraints.insert(999, Constraint::Distance(1, 3, d));
            }
            // In conflict: a side given a second length.
            3 => {
                let side = common::SIDES + 1;
                s.constraints.insert(999, Constraint::Length(side, size * 7.0));
            }
            _ => {}
        }
        both(&s)?;
    }

    /// Jostled by a thousandth of their size, well inside the gap to any
    /// other root, well-defined polygons solve to the same positions. (By
    /// a fiftieth, the closing point's mirror root can be as near the
    /// start as its own, and either stage may find either.) As in the
    /// property tests, not where the closing point's turn is nearly
    /// straight: its two roots are nearly one, and it moves by about
    /// 1/sin of the turn times any rounding.
    #[test]
    fn polygons_solve_alike(seed in any::<u64>(), n in 3usize..12, size in 0.001f64..10.0) {
        let mut g = Lcg::new(seed);
        let mut s = polygon(&mut g, n, size);
        jostle(&mut s, &mut g, size * 0.001);
        let (two, one) = both(&s)?;
        prop_assert_eq!(two.report.state, State::Well);
        prop_assume!(last_turn(&one.sketch, n).abs() > 0.05);
        same_positions(&two.sketch, &one.sketch, size)?;
    }

    #[test]
    fn slots_report_alike(l in 0.001f64..100.0, w in 0.001f64..10.0, seed in any::<u64>()) {
        let mut g = Lcg::new(seed);
        let mut s = slot(l, w);
        jostle(&mut s, &mut g, l.min(w) * 0.05);
        let (two, one) = both(&s)?;
        prop_assert_eq!(two.report.state, State::Well);
        same_positions(&two.sketch, &one.sketch, l.max(w))?;
    }
}

/// A dependency only the values show: two horizontal lines made parallel
/// too. Structurally the parallel has a variable of its own; numerically
/// it says what the horizontals say. Both stages call it redundant.
#[test]
fn redundancy_by_value_is_found_by_both_stages() {
    let mut s = Sketch::default();
    for (id, x, y) in [(1, 0.0, 0.0), (2, 2.0, 0.1), (3, 0.0, 1.0), (4, 2.0, 1.2)] {
        s.entities.insert(
            id,
            Entity::Point {
                at: [x, y],
                fixed: false,
            },
        );
    }
    for (id, a, b) in [(10, 1, 2), (11, 3, 4)] {
        s.entities.insert(
            id,
            Entity::Line {
                a,
                b,
                construction: false,
            },
        );
    }
    s.constraints.insert(1, Constraint::Fix(1));
    s.constraints.insert(2, Constraint::Fix(3));
    s.constraints.insert(3, Constraint::Horizontal(10));
    s.constraints.insert(4, Constraint::Horizontal(11));
    s.constraints.insert(5, Constraint::Parallel(10, 11));
    s.constraints.insert(6, Constraint::Length(10, 2.0));
    s.constraints.insert(7, Constraint::Length(11, 2.0));
    let (two, _) = both(&s).unwrap();
    assert_eq!(two.report.state, State::Over);
    assert_eq!(two.report.redundant, [vec![3, 4, 5]]);
}
