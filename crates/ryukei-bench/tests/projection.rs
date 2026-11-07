//! Hidden lines against OCCT's exact `HLRBRep_Algo` (design 326, plan
//! 10.C): closed forms judged against exact lengths and OCCT's, seeded
//! bodies and the pinned corpus where OCCT and a mesh agree, and the
//! race on one thread. Devcontainer only (`--features occt`).

use ryukei::geom::{Rigid, Vector3};
use ryukei_bench::Primitive;
use ryukei_bench::occt;

#[test]
fn occt_draws_a_blocks_edges_from_a_corner() {
    let s = occt::Solid::new(Primitive::Block([1.0, 2.0, 3.0]), &Rigid::IDENTITY);
    let v = ryukei_testkit::projection::look(Vector3::new(1.0, -1.0, 1.0));
    let h = s.hlr(v.towards().get(), v.x().get()).unwrap();
    let f = (2.0f64 / 3.0).sqrt();
    assert!((h.visible[0] - 3.0 * 6.0 * f).abs() < 1e-9, "{h:?}");
    assert!((h.hidden[0] - 6.0 * f).abs() < 1e-9, "{h:?}");
    assert!(h.seconds >= 0.0);
}
