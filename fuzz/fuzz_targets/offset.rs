//! Offsets and shells from bytes ([`ryukei_testkit::offset_bytes`]). Every
//! result must be a typed refusal ([`Error::InvalidOffset`], or for a
//! shell the boolean's [`Error::InvalidBoolean`]), or a body that passes
//! the checker, doesn't cross or overlap itself, and meshes with no
//! faults. The operation, its checks and its mesh spend one
//! [`ryukei_testkit::FUZZ_BUDGET`]; past it is a typed refusal. Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections_with};
use ryukei::mesh::{MeshParams, check_mesh, tessellate_with};
use ryukei::name::OpKey;
use ryukei::offset::{offset_body_with, shell_with};
use ryukei::work::Work;
use ryukei_testkit::offset_bytes::{Op, offset_case};

fuzz_target!(|data: &[u8]| {
    let Some(case) = offset_case(data) else {
        return;
    };
    let work = Work::unlimited().with_budget(ryukei_testkit::FUZZ_BUDGET);
    let result = match &case.op {
        Op::Offset(spec) => offset_body_with(OpKey(0x0f5), &case.body, spec, &work),
        Op::Shell(spec) => shell_with(OpKey(0x5e1), &case.body, spec, &work),
    };
    let body = match result {
        Ok((body, _)) => body,
        Err(Error::InvalidOffset(_) | Error::InvalidBoolean(_) | Error::BudgetExceeded { .. }) => {
            return ryukei_testkit::note_units(&work);
        }
        Err(e) => panic!("unexpected {e:?}"),
    };
    assert_eq!(check(&body), []);
    let faults = match check_intersections_with(&body, &work) {
        Ok(faults) => faults,
        Err(Error::BudgetExceeded { .. }) => return ryukei_testkit::note_units(&work),
        Err(e) => panic!("unexpected {e:?}"),
    };
    for fault in faults {
        assert!(matches!(fault, Fault::CannotCheck { .. }), "{fault:?}");
    }
    let params = MeshParams {
        max_triangles: 200_000,
        ..MeshParams::new(1e-2, 0.5)
    };
    match tessellate_with(&body, &params, &work) {
        Ok(m) => {
            assert_eq!(m.faults, []);
            assert_eq!(check_mesh(&body, &m), Ok(()));
        }
        Err(Error::MeshTooLarge { .. } | Error::BudgetExceeded { .. }) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
    ryukei_testkit::note_units(&work);
});
