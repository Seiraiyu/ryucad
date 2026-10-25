//! Random mechanisms built from poses that hold, jostled, solve back to
//! poses that hold, with the freedom their mates leave; a solve of an
//! assembly that holds gives back its very bits; and solving in pieces
//! reports what solving whole components does.

mod common;

use common::*;
use proptest::prelude::*;
use ryukei_testkit::Lcg;
use ryumate::{BodyState, Kind, Mate, MateState, Report, solve, solve_whole};

/// What both ways of solving must agree on: everything but the step
/// count and the free motions' coordinates, which differ by rounding.
fn said(r: &Report) -> impl PartialEq + std::fmt::Debug {
    let bodies: Vec<_> = r
        .bodies
        .iter()
        .map(|(id, b)| {
            (
                *id,
                b.state,
                b.freedom.dof,
                b.freedom.translations.len(),
                b.freedom.rotations.len(),
            )
        })
        .collect();
    (
        r.converged,
        r.state,
        r.dof,
        bodies,
        r.mates.clone(),
        r.redundant.clone(),
        r.conflicting.clone(),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn mechanisms_solve_back_to_poses_that_hold(seed in any::<u64>(), n in 2u32..12, size in 0.001f64..20.0) {
        let mut g = Lcg::new(seed);
        let (mut asm, dof) = mechanism(&mut g, n, size);
        // Built where it holds: nothing moves, to the bit.
        let held = solve(&asm).unwrap();
        prop_assert!(held.report.converged, "{:?}", held.report);
        prop_assert!(!held.report.changed);
        prop_assert_eq!(held.report.iterations, 0);
        for (id, b) in &asm.bodies {
            prop_assert_eq!(held.poses[id], b.pose);
        }
        prop_assert_eq!(held.report.dof, dof);
        jostle(&mut asm, &mut g, 0.05, size * 0.01);
        let s = solve(&asm).unwrap();
        prop_assert!(s.report.converged, "{:?}", s.report);
        prop_assert_eq!(s.report.dof, dof);
        for b in s.report.bodies.values() {
            prop_assert!(b.state == BodyState::Well || b.state == BodyState::Under);
        }
        let again = solve(&s.assembly(&asm)).unwrap();
        prop_assert!(again.report.converged);
        prop_assert!(!again.report.changed);
        prop_assert_eq!(&again.poses, &s.poses);
    }

    #[test]
    fn pieces_and_whole_components_report_alike(seed in any::<u64>(), n in 2u32..10, size in 0.001f64..20.0, variant in 0u8..3) {
        let mut g = Lcg::new(seed);
        let (mut asm, _) = mechanism(&mut g, n, size);
        match variant {
            // A mate given twice: redundant.
            1 => {
                let m = asm.mates[&1];
                asm.mates.insert(100, m);
            }
            // A mate given twice, once moved off: in conflict.
            2 => {
                let mut m: Mate = asm.mates[&1];
                m.a.frame.origin[0] += size * 0.1;
                m.kind = Kind::Fastened;
                asm.mates.insert(1, Mate { kind: Kind::Fastened, ..asm.mates[&1] });
                asm.mates.insert(100, m);
            }
            _ => {}
        }
        jostle(&mut asm, &mut g, 0.02, size * 0.005);
        let (two, one) = (solve(&asm).unwrap(), solve_whole(&asm).unwrap());
        prop_assert_eq!(two.report.converged, one.report.converged);
        // Where stage 2 left it, both read it alike.
        let at = two.assembly(&asm);
        let (two, one) = (solve(&at).unwrap(), solve_whole(&at).unwrap());
        prop_assert_eq!(said(&two.report), said(&one.report));
        if variant == 1 {
            prop_assert_eq!(&two.report.redundant, &vec![vec![1, 100]]);
            prop_assert_eq!(two.report.mates[&100], MateState::Redundant);
        }
        if variant == 2 {
            prop_assert!(!two.report.converged);
            prop_assert_eq!(&two.report.conflicting, &vec![vec![1, 100]]);
        }
    }
}
