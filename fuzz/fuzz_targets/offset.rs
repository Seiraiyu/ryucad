//! Offsets and shells from bytes ([`ryukei_testkit::offset_bytes`]), now
//! and then past their bodies' curvature. Every result must be a typed refusal ([`Error::InvalidOffset`], or for a
//! shell the boolean's [`Error::InvalidBoolean`]), or a body that passes
//! the checker, doesn't cross or overlap itself, and meshes with no
//! faults. The operation, its checks and its mesh share one
//! [`ryukei_testkit::FUZZ_BUDGET`] and are cancelled at
//! [`ryukei_testkit::FUZZ_DEADLINE`]; past either is a typed refusal.
//! Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections_with};
use ryukei::mesh::{MeshParams, check_mesh, tessellate_with};
use ryukei::name::OpKey;
use ryukei::offset::{offset_body_with, shell_with};
use ryukei_testkit::offset_bytes::{Op, offset_case, offset_case_past};

fuzz_target!(|data: &[u8]| {
    // A sixteenth of the inputs at distances past their bodies' curvature
    // (plan 17.A); the rest as they always were.
    let case = match data.split_first() {
        Some((&t, rest)) if t >= 0xf0 => offset_case_past(rest),
        _ => offset_case(data),
    };
    let Some(case) = case else {
        return;
    };
    ryukei_testkit::fuzz_case(|work| {
        let result = match &case.op {
            Op::Offset(spec) => work.run(|w| offset_body_with(OpKey(0x0f5), &case.body, spec, w)),
            Op::Shell(spec) => work.run(|w| shell_with(OpKey(0x5e1), &case.body, spec, w)),
        };
        let body = match result {
            Ok((body, _)) => body,
            Err(
                Error::InvalidOffset(_)
                | Error::InvalidBoolean(_)
                | Error::BudgetExceeded { .. }
                | Error::Cancelled,
            ) => {
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
