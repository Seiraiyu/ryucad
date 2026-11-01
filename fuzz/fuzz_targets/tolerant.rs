//! Booleans on tolerant twins from bytes
//! ([`ryukei_testkit::tolerant_bytes`], design 299): every result must be
//! a structured refusal (uncertain, unsupported, non-manifold, past the
//! tolerance cap), or a body that passes the checker, doesn't cross or
//! overlap itself, and meshes with no faults. Any panic, and any other
//! error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::boolean::boolean;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::{BooleanError, Error};
use ryukei_testkit::tolerant_bytes::tolerant_case;

fuzz_target!(|data: &[u8]| {
    let Some(case) = tolerant_case(data) else {
        return;
    };
    let c = &case.tolerant;
    let body = match boolean(OpKey(9), c.op, &c.target, &c.tools()) {
        Ok((body, _)) => body,
        Err(Error::InvalidBoolean(
            BooleanError::Uncertain { .. }
            | BooleanError::Unsupported { .. }
            | BooleanError::NonManifold { .. }
            | BooleanError::ToleranceTooLarge { .. },
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
