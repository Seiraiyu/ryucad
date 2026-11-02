//! Booleans and blends on tolerant twins from bytes
//! ([`ryukei_testkit::tolerant_bytes`], design 299): an even first byte
//! takes the rest as a boolean (`tolerant_case`), an odd one as a blend
//! (`tolerant_blend_case`). Every result must be a structured refusal (a
//! boolean uncertain, unsupported, non-manifold or past the tolerance cap;
//! any blend refusal), or a body that passes the checker, doesn't cross
//! or overlap itself, and meshes with no faults. Any panic, and any other
//! error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei::{BooleanError, Error};
use ryukei_testkit::tolerant_bytes::{tolerant_blend_case, tolerant_case};

fn booleaned(data: &[u8]) -> Option<Body> {
    let case = tolerant_case(data)?;
    let c = &case.tolerant;
    match boolean(OpKey(9), c.op, &c.target, &c.tools()) {
        Ok((body, _)) => Some(body),
        Err(Error::InvalidBoolean(
            BooleanError::Uncertain { .. }
            | BooleanError::Unsupported { .. }
            | BooleanError::NonManifold { .. }
            | BooleanError::ToleranceTooLarge { .. },
        )) => None,
        Err(e) => panic!("unexpected {e:?}"),
    }
}

fn blended(data: &[u8]) -> Option<Body> {
    let case = tolerant_blend_case(data)?;
    let c = &case.tolerant;
    match blend(OpKey(0xb1e), &c.body, &c.blends) {
        Ok((body, _)) => Some(body),
        Err(Error::InvalidBlend(_)) => None,
        Err(e) => panic!("unexpected {e:?}"),
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&first, rest)) = data.split_first() else {
        return;
    };
    let body = if first % 2 == 1 {
        blended(rest)
    } else {
        booleaned(rest)
    };
    let Some(body) = body else {
        return;
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
