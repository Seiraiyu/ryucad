//! Booleans from bytes ([`ryukei_testkit::boolean_bytes`]). Every result
//! must be a structured refusal (an intersection that stayed uncertain or
//! unsupported, a non-manifold result), or a body that passes the
//! checker, doesn't cross or overlap itself, and meshes with no faults.
//! Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::boolean::boolean;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::{BooleanError, Error};
use ryukei_testkit::boolean_bytes::boolean_case;

fuzz_target!(|data: &[u8]| {
    let Some(case) = boolean_case(data) else {
        return;
    };
    let body = match boolean(OpKey(9), case.op, &case.target, &case.tools()) {
        Ok((body, _)) => body,
        Err(Error::InvalidBoolean(
            BooleanError::Uncertain { .. }
            | BooleanError::Unsupported { .. }
            | BooleanError::NonManifold { .. },
        )) => return,
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
