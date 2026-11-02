//! Self-union from bytes (design 299, plan 14.C): crossing shells of
//! placed primitives ([`ryukei_testkit::crossing`]) healed with repair,
//! given to `self_union` where healing left them crossing, and the result
//! given to `self_union` again. Every result must be
//! a typed refusal (healing's, or a repair's) or a body that passes the
//! checker, doesn't cross or overlap itself, meshes with no faults, and
//! comes back from `self_union` as it was. Any panic, and any other
//! error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::codec::encode_body;
use ryukei::heal::{HealOptions, heal};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::repair::self_union;
use ryukei_testkit::crossing::crossing_case;

fuzz_target!(|data: &[u8]| {
    let Some(case) = crossing_case(data) else {
        return;
    };
    let healed = match heal(OpKey(7), &case.shape, &HealOptions::default()) {
        Ok((b, _, _)) => b,
        Err(Error::InvalidHeal(_)) => return,
        Err(e) => panic!("unexpected {e:?}"),
    };
    // Shells crossing with no vertex of either inside the other heal as
    // they are: a caller's self-union repairs them.
    let crossing = check_intersections(&healed)
        .iter()
        .any(|f| !matches!(f, Fault::CannotCheck { .. }));
    let body = if crossing {
        match self_union(OpKey(9), &healed) {
            Ok((b, _)) => b,
            Err(Error::InvalidRepair(_)) => return,
            Err(e) => panic!("unexpected {e:?}"),
        }
    } else {
        healed
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
    match self_union(OpKey(9), &body) {
        Ok((again, journal)) => {
            assert!(journal.is_empty());
            assert_eq!(encode_body(&again), encode_body(&body));
        }
        Err(Error::InvalidRepair(_)) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
});
