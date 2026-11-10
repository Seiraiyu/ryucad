//! Bodies `heal::seamed` cuts open for export.

/// Issue #298: NIST STC 07 (AP242 edition 3) heals and checks, but
/// heal::seamed couldn't cut its countersink cone (half-angle about 59
/// degrees, a reversed face) open: the face is a lune between two rulings
/// through the apex, and the rulings stop 3.4 nm short of it, so their
/// samples weren't on the pole and the loop read as winding round the
/// cone. The body is NIST's (public domain), as ryustep read it with key
/// 7, encoded.
#[test]
fn nist_stc_07_s_countersink_is_cut_open() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/nist_stc_07_countersink.body"),
    )
    .unwrap();
    let body = ryukei::codec::decode_body(&bytes).unwrap();
    ryukei::heal::seamed(&body, 1e-7).unwrap();
}

/// Issue #620: race body 1318 (boolean seed 731) has a cylinder face of
/// one loop that touches itself at a vertex: a whole circle and a path
/// round the other way, so the loop winds round nothing and covers the
/// whole turn, and `seamed` leaves it whole. `seamed_with_origins_unpinched`
/// (USD's) cuts it at that vertex into the two pieces, which wind one each
/// way, and seams the face between them; every other face is cut as
/// `seamed` cuts it.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "seconds in a debug build: CI runs it in release"
)]
fn a_loop_touching_itself_round_a_cylinder_is_seamed_for_usd() {
    let body = ryukei_testkit::race::boolean_body(731).expect("it builds");
    let (whole, o) = ryukei::heal::seamed_with_origins(&body, 1e-7).unwrap();
    let (cut, p) = ryukei::heal::seamed_with_origins_unpinched(&body, 1e-7).unwrap();
    let seams = |o: &ryukei::heal::Origins| o.edges.iter().filter(|e| e.is_none()).count();
    assert_eq!(seams(&p), seams(&o) + 1);
    assert_eq!(cut.faces.len(), whole.faces.len());
    let loops = |s: &ryukei::heal::Shape| s.faces.iter().map(|f| f.loops.len()).sum::<usize>();
    assert_eq!(loops(&cut), loops(&whole));
}
