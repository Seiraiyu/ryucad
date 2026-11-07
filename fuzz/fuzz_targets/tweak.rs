//! Face tweaks and drafts from bytes ([`ryukei_testkit::tweak_bytes`]).
//! Every result must be a typed refusal ([`Error::InvalidTweak`]), or a
//! body with the original's names (a tweak keeps its topology) that
//! passes the checker, doesn't cross or overlap itself, and meshes with
//! no faults. The operation, its checks and its mesh share one
//! [`ryukei_testkit::FUZZ_BUDGET`] and are cancelled at
//! [`ryukei_testkit::FUZZ_DEADLINE`]; past either is a typed refusal.
//! Any panic, and any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::{Fault, check, check_intersections_with};
use ryukei::mesh::{MeshParams, check_mesh, tessellate_with};
use ryukei::name::{Name, OpKey};
use ryukei::topo::Body;
use ryukei::tweak::{draft_with, move_faces_with, offset_faces_with, replace_faces_with};
use ryukei_testkit::tweak_bytes::{Op, tweak_case};

fn names(b: &Body) -> Vec<Name> {
    let mut n: Vec<Name> = b
        .faces()
        .map(|f| b.face(f).expect("a face").name)
        .chain(b.edges().map(|e| b.edge(e).expect("an edge").name))
        .chain(b.vertices().map(|v| b.vertex(v).expect("a vertex").name))
        .collect();
    n.sort();
    n
}

fuzz_target!(|data: &[u8]| {
    let Some(case) = tweak_case(data) else {
        return;
    };
    ryukei_testkit::fuzz_case(|work| {
        let key = OpKey(0x7e4);
        let result = match &case.op {
            Op::Move(s) => work.run(|w| move_faces_with(key, &case.body, s, w)),
            Op::Offset(s) => work.run(|w| offset_faces_with(key, &case.body, s, w)),
            Op::Replace(s) => work.run(|w| replace_faces_with(key, &case.body, s, w)),
            Op::Draft(s) => work.run(|w| draft_with(key, &case.body, s, w).map(|(b, j, _)| (b, j))),
        };
        let body = match result {
            Ok((body, _)) => body,
            Err(Error::InvalidTweak(_) | Error::BudgetExceeded { .. } | Error::Cancelled) => {
                return;
            }
            Err(e) => panic!("unexpected {e:?}"),
        };
        assert_eq!(names(&body), names(&case.body));
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
