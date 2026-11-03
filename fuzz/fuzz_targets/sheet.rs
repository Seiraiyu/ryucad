//! Sheets from bytes ([`ryukei_testkit::sheet_bytes`]): faces taken out
//! of bodies, surfaces bounded by boxes of their parameters, planar faces
//! and open chains swept as surfaces (design 295, plan 12.B), and faces
//! sewn back (plan 12.C). Every result
//! must be a typed refusal, or a sheet that passes the checker, doesn't
//! cross or overlap itself, and meshes with no faults. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::topo::BodyKind;
use ryukei_testkit::sheet_bytes::sheet;

fuzz_target!(|data: &[u8]| {
    let Some((body, small)) = sheet(data) else {
        return;
    };
    assert_eq!(body.kind(), BodyKind::Sheet);
    assert_eq!(check(&body), []);
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
