//! Blends from bytes ([`ryukei_testkit::blend_bytes`]). Every result must
//! be a typed refusal ([`Error::InvalidBlend`]), or a body that passes the
//! checker, doesn't cross or overlap itself, and meshes with no faults.
//! Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::blend::blend;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei_testkit::blend_bytes::blend_case;

fuzz_target!(|data: &[u8]| {
    let Some(case) = blend_case(data) else {
        return;
    };
    let body = match blend(OpKey(0xb1e), &case.body, &case.blends) {
        Ok((body, _)) => body,
        Err(Error::InvalidBlend(_)) => return,
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
