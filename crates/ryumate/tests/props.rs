//! Random mechanisms built from poses that hold, jostled, solve back to
//! poses that hold, with the freedom their mates leave; a solve of an
//! assembly that holds gives back its very bits; and solving in pieces
//! reports what solving whole components does.

mod common;

use common::*;
use proptest::prelude::*;
use ryukei_testkit::Lcg;
use ryumate::{
    Assembly, BodyState, Connector, Kind, Limits, Mate, MateState, Motion, On, Pattern, Pose,
    Report, solve, solve_whole,
};

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

    #[test]
    fn limited_mechanisms_stay_within_their_stops(seed in any::<u64>(), n in 2u32..12, size in 0.001f64..20.0) {
        let mut g = Lcg::new(seed);
        let (mut asm, dof) = mechanism(&mut g, n, size);
        limit(&mut asm, &mut g, size, 0.03);
        jostle(&mut asm, &mut g, 0.05, size * 0.01);
        let s = solve(&asm).unwrap();
        // A tree's stops can always hold: each pins its child alone.
        prop_assert!(s.report.converged, "{:?}", s.report);
        prop_assert_eq!(s.report.dof, dof);
        for (id, m) in &asm.mates {
            let Some(v) = s.report.values.get(id) else { continue };
            if let (Some(r), Some(a)) = (m.limits.angle, v.angle) {
                prop_assert!(r.past(a.value, 1e-9).is_none(), "{id}: {a:?} {r:?}");
            }
            if let (Some(r), Some(d)) = (m.limits.distance, v.distance) {
                prop_assert!(r.past(d.value, 1e-9 * size.max(1.0)).is_none(), "{id}: {d:?} {r:?}");
            }
        }
        let again = solve(&asm).unwrap();
        prop_assert_eq!(&again.poses, &s.poses);
        prop_assert_eq!(&again.report, &s.report);
    }
    #[test]
    fn copies_at_places_are_their_bodys_motions(seed in any::<u64>(), n in 1usize..8, size in 0.001f64..20.0) {
        let mut g = Lcg::new(seed);
        let lid = pose(&mut g, size);
        let screw = pose(&mut g, size);
        let places: Vec<Pose> = (0..n).map(|_| pose(&mut g, size)).collect();
        let mut asm = Assembly::default();
        asm.bodies.insert(1, free(lid));
        asm.bodies.insert(2, free(screw));
        asm.patterns.insert(20, Pattern {
            seeds: vec![2],
            motion: Motion::Places { body: 1, places: places.clone() },
        });
        prop_assert_eq!(asm.validate(), Ok(()));
        let wanted: Vec<_> = (1..=n as u32).map(|k| (20, 2, [k, 0])).collect();
        let got = ryumate::copy_poses(&asm, &wanted);
        let tol = 1e-12 * size.max(1.0);
        for (k, place) in places.iter().enumerate() {
            let want = lid.then_after(place).then_after(&lid.inverse()).then_after(&screw);
            prop_assert!(same(got[k].origin, want.origin, tol), "{k}: {:?} {:?}", got[k], want);
            for a in 0..3 {
                prop_assert!(same(got[k].axis(a), want.axis(a), 1e-12));
            }
        }
        // A fixed nut fastened where the last copy is: the solve holds.
        asm.bodies.insert(3, fixed(got[n - 1]));
        asm.mates.insert(10, Mate {
            kind: Kind::Fastened,
            a: Connector { on: On::Copy { pattern: 20, seed: 2, index: [n as u32, 0] }, frame: Pose::IDENTITY },
            b: on(3, Pose::IDENTITY),
            limits: Limits::NONE,
        });
        let s = solve(&asm).unwrap();
        prop_assert!(s.report.converged, "{:?}", s.report);
    }
}
