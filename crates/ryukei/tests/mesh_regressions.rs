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

/// Issue #552: race body 482 (blend seed 267's, written to STEP in
/// millimetres and healed as ryucad imports it, part `p482`) left its
/// ruled spline face out as `RefinementStalled` at a chord of 1 mm and 15
/// degrees. Its triangles there failed the turn test alone, by a turn
/// rate along `u` bounded at 8e6 rad per unit across triangles 27 µm wide
/// in `u` and 1.7 mm long in `v`, and the edge the normal turns most
/// along was the short one, below refinement's floor. They are split
/// along their longest edge instead, down to the waiver's size.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build: run it in release")]
fn race_body_482s_ruled_face_meshes_at_a_millimetre() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/race-482.body"),
    )
    .unwrap();
    let body = ryukei::codec::decode_body(&bytes).unwrap();
    let work = ryukei::work::Work::unlimited();
    let m = ryukei::mesh::tessellate_with(&body, &MeshParams::new(1e-3, 15f64.to_radians()), &work)
        .unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(&body, &m).unwrap();
    let n = m.welded_triangles().count();
    println!("{n} triangles, {} units", work.spent());
    assert!(n < 100_000, "{n} triangles");
    assert!(work.spent() < 1 << 18, "{} units", work.spent());
}
