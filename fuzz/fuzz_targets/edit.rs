//! Direct edits from bytes ([`ryukei_testkit::edit_bytes`]): faces and
//! features deleted, topology-changing tweaks, blends deleted and
//! resized. Every result must be a typed refusal
//! ([`Error::InvalidTweak`]), or a body that passes the checker, doesn't
//! cross or overlap itself, meshes with no faults, and whose journal
//! accounts for every face the body had. The edit (recognition and
//! feature finding included), its checks and its mesh share one
//! [`ryukei_testkit::FUZZ_BUDGET`] and are cancelled at
//! [`ryukei_testkit::FUZZ_DEADLINE`]; past either is a typed refusal.
//! Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections_with};
use ryukei::mesh::{MeshParams, check_mesh, tessellate_with};
use ryukei::name::{Event, OpKey};
use ryukei_testkit::edit_bytes::{edit_case, run};

fuzz_target!(|data: &[u8]| {
    let Some(case) = edit_case(data) else {
        return;
    };
    ryukei_testkit::fuzz_case(|work| {
        let body = match work.run(|w| run(&case, OpKey(0xed1), w)) {
            Ok((body, journal)) => {
                for f in case.body.faces() {
                    let n = case.body.face(f).expect("a face").name;
                    let told = journal.events().iter().any(|e| match e {
                        Event::Deleted(x) | Event::Modified(x) => *x == n,
                        Event::Split { parent, .. } => *parent == n,
                        Event::Merged { parents, .. } => parents.contains(&n),
                        Event::Replaced { old, .. } => *old == n,
                        _ => false,
                    });
                    assert!(body.by_name(n).is_some() || told, "{n:?} lost");
                }
                body
            }
            Err(Error::InvalidTweak(_) | Error::BudgetExceeded { .. } | Error::Cancelled) => {
                return;
            }
            Err(e) => panic!("unexpected {e:?}"),
        };
        assert_eq!(check(&body), []);
        let faults = match work.run(|w| check_intersections_with(&body, w)) {
            Ok(faults) => faults,
            Err(Error::BudgetExceeded { .. } | Error::Cancelled) => return,
            Err(e) => panic!("unexpected {e:?}"),
        };
        for fault in faults {
            assert!(matches!(fault, Fault::CannotCheck { .. }), "{fault:?}");
        }
        let params = MeshParams {
            max_triangles: 200_000,
            ..MeshParams::new(1e-2, 0.5)
        };
        match work.run(|w| tessellate_with(&body, &params, w)) {
            Ok(m) => {
                assert_eq!(m.faults, []);
                assert_eq!(check_mesh(&body, &m), Ok(()));
            }
            Err(Error::MeshTooLarge { .. } | Error::BudgetExceeded { .. } | Error::Cancelled) => {}
            Err(e) => panic!("unexpected {e:?}"),
        }
    });
});
