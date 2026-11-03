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
