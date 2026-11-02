//! Meshes that once took far longer than they should.

use ryukei::mesh::{MeshParams, check_mesh, tessellate};

/// Issue #288: one of the race's bodies (three faces, healed from the
/// race's STEP file) took 112 s and 1,423,372 triangles at a chord of 1 mm
/// and 15 degrees, where its neighbours take tens of milliseconds and
/// tens of thousands.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build: run it in release")]
fn the_race_s_slow_body_meshes_to_tens_of_thousands_of_triangles() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/race-slow.body"),
    )
    .unwrap();
    let body = ryukei::codec::decode_body(&bytes).unwrap();
    let m = tessellate(&body, &MeshParams::new(1e-3, 15f64.to_radians())).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(&body, &m).unwrap();
    let n = m.welded_triangles().count();
    assert!(n < 100_000, "{n} triangles");
}

/// Issue #367: a refolded bracket drilled across its bend line, read
/// back from STEP, whose hole's wall is a bent surface's NURBS form:
/// curved round the hole, straight to within rounding through the sheet
/// a millimetre thick. The stretched plane (#288) shortened that short,
/// straight direction 2¹⁸ times, and at the chord `measure::mass` meshes
/// at (a hundred-thousandth of the body) the wall never finished; at a
/// thousandth it took 86,848 triangles, against 342 unstretched.
#[test]
fn a_refolded_bent_wall_meshes() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/refolded-bent-wall.body"),
    )
    .unwrap();
    let body = ryukei::codec::decode_body(&bytes).unwrap();
    // A thousandth of the body's size: the finer chord is ryustep's
    // `a_refolded_body_with_a_bent_wall_comes_back`, a minute and a half
    // in a debug build.
    let m = tessellate(
        &body,
        &MeshParams::new(0.031_575_306_807_693_89 * 1e-3, 0.3),
    )
    .unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(&body, &m).unwrap();
    let n = m.welded_triangles().count();
    assert!(n < 5_000, "{n} triangles");
}
