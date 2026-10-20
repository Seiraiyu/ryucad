//! The solver's promises on random sketches: well-constrained ones solve
//! and are well defined, small changes move geometry a little, taking a
//! constraint away frees exactly one degree, and a constraint the others
//! imply is reported redundant.

mod common;

use common::{at, jostle, polygon, slot};
use proptest::prelude::*;
use ryukei_testkit::Lcg;
use ryusketch::{Constraint, State, solve};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn well_constrained_polygons_solve(seed in any::<u64>(), n in 3usize..9, size in 0.001f64..10.0) {
        let mut g = Lcg::new(seed);
        let mut s = polygon(&mut g, n, size);
        jostle(&mut s, &mut g, size * 0.02);
        let r = solve(&s).unwrap();
        prop_assert!(r.report.converged, "{:?}", r.report);
        prop_assert_eq!(r.report.state, State::Well);
        prop_assert_eq!(r.report.loops.len(), 1);
        // Solved, it holds as it is.
        let again = solve(&r.sketch).unwrap();
        prop_assert!(!again.report.changed);
        prop_assert_eq!(again.sketch, r.sketch);
    }

    #[test]
    fn a_small_change_moves_a_little(seed in any::<u64>(), n in 3usize..9) {
        let mut g = Lcg::new(seed);
        let solved = solve(&polygon(&mut g, n, 1.0)).unwrap().sketch;
        // Angles hold every side but the last two, so the last point sits
        // where two circles meet: it moves by about 1/sin of its turn
        // times the change. Nearly straight there, a small change moves it
        // a lot, rightly (seed 1295630540368118342, n = 8, turned 0.13°).
        let (p, q, r) = (at(&solved, n as u32 - 1), at(&solved, n as u32), at(&solved, 1));
        let (u, w) = ([q[0] - p[0], q[1] - p[1]], [r[0] - q[0], r[1] - q[1]]);
        let turn = (u[0] * w[1] - u[1] * w[0])
            / ryukei::math::sqrt((u[0] * u[0] + u[1] * u[1]) * (w[0] * w[0] + w[1] * w[1]));
        prop_assume!(turn.abs() > 0.05);
        let mut s = solved.clone();
        let Some(Constraint::Length(l, v)) = s.constraints.get(&2).cloned() else {
            unreachable!()
        };
        s.constraints.insert(2, Constraint::Length(l, v * (1.0 + 1e-6)));
        let r = solve(&s).unwrap();
        prop_assert!(r.report.converged);
        for p in 1..=n as u32 {
            let (a, b) = (at(&solved, p), at(&r.sketch, p));
            prop_assert!((a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4);
        }
    }

    #[test]
    fn taking_a_constraint_away_frees_one_degree(seed in any::<u64>(), n in 3usize..9, pick in any::<usize>()) {
        let mut g = Lcg::new(seed);
        let mut s = solve(&polygon(&mut g, n, 1.0)).unwrap().sketch;
        // Any but the fix, which holds two.
        let ids: Vec<u32> = s.constraints.keys().copied().filter(|&c| c != 1).collect();
        s.constraints.remove(&ids[pick % ids.len()]);
        let r = solve(&s).unwrap().report;
        prop_assert_eq!((r.dof, r.state), (1, State::Under));
        s.constraints.remove(&1);
        prop_assert_eq!(solve(&s).unwrap().report.dof, 3);
    }

    #[test]
    fn what_the_others_imply_is_redundant(seed in any::<u64>(), n in 4usize..9) {
        let mut g = Lcg::new(seed);
        let mut s = solve(&polygon(&mut g, n, 1.0)).unwrap().sketch;
        let (a, b) = (at(&s, 1), at(&s, 3));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let d = ryukei::math::sqrt(dx * dx + dy * dy);
        s.constraints.insert(999, Constraint::Distance(1, 3, d));
        let r = solve(&s).unwrap().report;
        prop_assert!(r.converged);
        prop_assert_eq!(r.state, State::Over);
        prop_assert!(r.redundant.iter().any(|set| set.contains(&999)), "{:?}", r.redundant);
        prop_assert!(r.conflicting.is_empty());
    }

    #[test]
    fn slots_solve_at_any_size(l in 0.001f64..100.0, w in 0.001f64..10.0, seed in any::<u64>()) {
        let mut g = Lcg::new(seed);
        let mut s = slot(l, w);
        // By its smaller size: jostled by more than its length, a long thin
        // slot's arc centres pass each other, and the solver never flips a
        // root to bring them back (l = 0.001, w = 5.57, seed = 648105).
        jostle(&mut s, &mut g, l.min(w) * 0.05);
        let r = solve(&s).unwrap();
        prop_assert!(r.report.converged, "{:?}", r.report);
        prop_assert_eq!(r.report.state, State::Well);
        prop_assert_eq!(r.report.loops.len(), 1);
        prop_assert_eq!(r.report.loops[0].segments.len(), 4);
    }
}
