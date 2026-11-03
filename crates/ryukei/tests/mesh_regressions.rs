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

/// Issue #360: boolean seed 78 of `sheet_props` (a cone with a plane, and
/// a lens of two sphere faces meeting on one circle, 5.64 m across) left
/// its first sphere face out as `RefinementStalled` at a chord of 5e-5 m
/// and of 4.9749e-5 m, a narrow band the lens sewn back alone was measured
/// in, so `measure::properties` refused it. It meshes at both, and the
/// lens is measured.
#[test]
fn boolean_seed_78s_lens_meshes_in_the_band_that_stalled() {
    use ryukei::heal::{SewOptions, sew};
    use ryukei::measure::{Entity, properties};
    use ryukei::name::OpKey;
    let hex = "fce7f6f881da1db1521767ab613cb24598932c3fe8bae0e8e09669d06beaa0d7\
               33e62f6806564c185b52da58da129f2fa4ec3e2251d519fb86afc562404c0156\
               08df37a94c794773e2bdfc4520a5131b132b776e8262a829328e5daf85a62bf5";
    let data: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let case = ryukei_testkit::boolean_bytes::boolean_case(&data).unwrap();
    let (body, _) =
        ryukei::boolean::boolean(OpKey(9), case.op, &case.target, &case.tools()).unwrap();
    for chord in [5.0e-5, 4.9749371855331e-5] {
        let m = tessellate(&body, &MeshParams::new(chord, 0.05)).unwrap();
        assert_eq!(m.faults, [], "at {chord}");
        check_mesh(&body, &m).unwrap();
    }
    // The lens: the first shell's two sphere faces, sewn back alone.
    let shell = body.shells().next().unwrap();
    let sheets: Vec<_> = body
        .shell_faces(shell)
        .map(|f| {
            ryukei::sheet::extract(OpKey(10), &body, &[body.face(f).unwrap().name])
                .unwrap()
                .0
        })
        .collect();
    assert_eq!(sheets.len(), 2);
    let (lens, _) = sew(OpKey(11), &sheets, &SewOptions::default()).unwrap();
    assert_eq!(lens.len(), 1);
    properties(&lens[0], Entity::Body).unwrap();
}
