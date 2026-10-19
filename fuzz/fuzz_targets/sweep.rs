//! Sweeps from bytes ([`ryukei_testkit::sweep_bytes`]). Every result must
//! be a structured error, or a body that passes the checker, doesn't cross
//! or overlap itself, and meshes with no faults at a chord well under the
//! profile's smallest feature (the mesher can't yet resolve a feature
//! thinner than its chord, such as a thin torus, whatever made it). Any
//! panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei_testkit::sweep_bytes::sweep;

fuzz_target!(|data: &[u8]| {
    let Some((body, small)) = sweep(data) else {
        return;
    };
    assert_eq!(check(&body), []);
    // A pair it can't check (a swept face, or an intersection that stayed
    // uncertain) is a structured answer; a crossing or an overlap in a
    // sweep is a bug.
    for fault in check_intersections(&body) {
        assert!(matches!(fault, Fault::CannotCheck { .. }), "{fault:?}");
    }
    let params = MeshParams {
        max_triangles: 20_000,
        ..MeshParams::new((small / 8.0).clamp(1e-3, 0.05), 0.5)
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
