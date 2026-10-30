//! Offsets and shells from bytes ([`ryukei_testkit::offset_bytes`]). Every
//! result must be a typed refusal ([`Error::InvalidOffset`], or for a
//! shell the boolean's [`Error::InvalidBoolean`]), or a body that passes
//! the checker, doesn't cross or overlap itself, and meshes with no
//! faults. Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::offset::{offset_body, shell};
use ryukei_testkit::offset_bytes::{Op, offset_case};

fuzz_target!(|data: &[u8]| {
    let Some(case) = offset_case(data) else {
        return;
    };
    let result = match &case.op {
        Op::Offset(spec) => offset_body(OpKey(0x0f5), &case.body, spec),
        Op::Shell(spec) => shell(OpKey(0x5e1), &case.body, spec),
    };
    let body = match result {
        Ok((body, _)) => body,
        Err(Error::InvalidOffset(_) | Error::InvalidBoolean(_)) => return,
        Err(e) => panic!("unexpected {e:?}"),
    };
    assert_eq!(check(&body), []);
    for fault in check_intersections(&body) {
        assert!(matches!(fault, Fault::CannotCheck { .. }), "{fault:?}");
    }
    let params = MeshParams {
        max_triangles: 200_000,
        ..MeshParams::new(1e-2, 0.5)
    };
    match tessellate(&body, &params) {
        Ok(m) => {
            assert_eq!(m.faults, []);
            assert_eq!(check_mesh(&body, &m), Ok(()));
        }
        Err(Error::MeshTooLarge { .. }) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
});
