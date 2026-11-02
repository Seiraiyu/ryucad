//! Sweeps from bytes ([`ryukei_testkit::sweep_bytes`]), and the same bytes
//! read as a sweep along a path or a loft
//! ([`ryukei_testkit::along_bytes`]). Every result must be a structured
//! error, or a body that passes the checker, doesn't cross or overlap
//! itself, and meshes with no faults at a chord well under the profile's
//! smallest feature (the mesher can't yet resolve a feature thinner than
//! its chord, such as a thin torus, whatever made it). Any panic is a bug.
//! A loft has already passed the self-intersection check (it refuses a
//! body that fails it), and so has a sweep along a path that
//! `sweep::along` couldn't certify one to one; but one it certified skips
//! the check, so the check is run here on every open sweep along a path
//! (these paths never close), and a crossing or an overlap is a bug in the
//! certificate. A run can take a minute or two, so run it with
//! `-timeout=600`. The calls a case makes, checks and meshes included,
//! share one [`ryukei_testkit::FUZZ_BUDGET`] and are cancelled at
//! [`ryukei_testkit::FUZZ_DEADLINE`]; a case past either is a typed
//! refusal and stops there.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections_with};
use ryukei::mesh::{MeshParams, check_mesh, tessellate_with};
use ryukei::topo::Body;
use ryukei_testkit::FuzzCase;
use ryukei_testkit::along_bytes::carried_with;
use ryukei_testkit::sweep_bytes::sweep;

fuzz_target!(|data: &[u8]| {
    ryukei_testkit::fuzz_case(|c| {
        let _ = case(data, c);
    });
});

/// One case; `None` where its budget ran out or it was cancelled.
fn case(data: &[u8], work: &FuzzCase) -> Option<()> {
    if let Some((body, small)) = sweep(data) {
        never_crosses(&body, work)?;
        sound(&body, small, work)?;
    }
    if let Some((body, small, along)) = work.run(|w| carried_with(data, w)) {
        if along {
            never_crosses(&body, work)?;
        }
        sound(&body, small, work)?;
    }
    Some(())
}

/// A pair the check can't settle (a swept face, or an intersection that
/// stayed uncertain) is a structured answer; a crossing or an overlap is
/// a bug.
fn never_crosses(body: &Body, work: &FuzzCase) -> Option<()> {
    let faults = refused(work.run(|w| check_intersections_with(body, w)))?;
    for fault in faults {
        assert!(matches!(fault, Fault::CannotCheck { .. }), "{fault:?}");
    }
    Some(())
}

/// The body passes the checker and meshes without faults (or is too big
/// to mesh at the chord asked for).
fn sound(body: &Body, small: f64, work: &FuzzCase) -> Option<()> {
    assert_eq!(check(body), []);
    let params = MeshParams {
        max_triangles: 20_000,
        ..MeshParams::new((small / 8.0).clamp(1e-3, 0.05), 0.5)
    };
    match work.run(|w| tessellate_with(body, &params, w)) {
        Ok(m) => {
            assert_eq!(m.faults, []);
            assert_eq!(check_mesh(body, &m), Ok(()));
        }
        Err(Error::MeshTooLarge { .. }) => {}
        Err(Error::BudgetExceeded { .. } | Error::Cancelled) => return None,
        Err(e) => panic!("unexpected {e:?}"),
    }
    Some(())
}

/// What a call gave, `None` where the budget ran out or it was cancelled.
fn refused<T>(r: Result<T, Error>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(Error::BudgetExceeded { .. } | Error::Cancelled) => None,
        Err(e) => panic!("unexpected {e:?}"),
    }
}
