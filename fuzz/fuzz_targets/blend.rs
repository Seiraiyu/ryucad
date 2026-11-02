//! Blends from bytes ([`ryukei_testkit::blend_bytes`]): an odd first byte
//! takes the rest as a case on spline faces (`spline_case`), an even one
//! as a case on the body as it is made (`blend_case`). Every result must
//! be a typed refusal ([`Error::InvalidBlend`]), or a body that passes the
//! checker, doesn't cross or overlap itself, and meshes with no faults.
//! The blend, its checks and its mesh share one
//! [`ryukei_testkit::FUZZ_BUDGET`] and are cancelled at
//! [`ryukei_testkit::FUZZ_DEADLINE`]; past either is a typed refusal.
//! Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::blend::blend_with;
use ryukei::check::{Fault, check, check_intersections_with};
use ryukei::mesh::{MeshParams, check_mesh, tessellate_with};
use ryukei::name::OpKey;
use ryukei_testkit::blend_bytes::{blend_case, spline_case};

fuzz_target!(|data: &[u8]| {
    let Some((&first, rest)) = data.split_first() else {
        return;
    };
    let case = if first % 2 == 1 {
        spline_case(rest)
    } else {
        blend_case(rest)
    };
    let Some(case) = case else {
        return;
    };
    ryukei_testkit::fuzz_case(|work| {
        let made = work.run(|w| blend_with(OpKey(0xb1e), &case.body, &case.blends, w));
        let body = match made {
            Ok((body, _)) => body,
            Err(Error::InvalidBlend(_) | Error::BudgetExceeded { .. } | Error::Cancelled) => {
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
