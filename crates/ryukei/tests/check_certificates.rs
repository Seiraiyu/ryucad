//! Guards for the check's last certificates (#699): bodies whose pairs
//! settle only by them, so a certificate that stops firing fails here.

use ryukei::check::check_intersections;

fn body(name: &str) -> ryukei::topo::Body {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name),
    )
    .unwrap();
    ryukei::codec::decode_body(&bytes).unwrap()
}

/// #264: a plane of NIST FTC 07 (AP242 edition 2, public domain) tangent
/// to a fillet torus at a saddle of its tube, at the one vertex they
/// share, cut out with their neighbours. The trace fails on the cells at
/// the vertex; traced outside a cube round it within the touch reach
/// (`outside_vertices`), the pair settles. Without that certificate it is
/// `CannotCheck`.
#[test]
fn a_saddle_tangency_at_a_shared_vertex_checks() {
    let b = body("nist_ftc_07_saddle.body");
    assert_eq!(b.faces().count(), 8);
    assert_eq!(check_intersections(&b), []);
}
