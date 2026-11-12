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

/// #303's shape: a prism whose every face and edge is a NURBS over the
/// face's own parameters, so the edge where its long sides cross (at 90°
/// and at 10°) is an isoline of both splines, as on KiCad's MSOP-8-1EP.
/// Its check settles within a budget a trace recursing along the isoline
/// would exhaust.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn nurbs_sides_crossing_along_an_isoline_check_in_bounded_work() {
    use ryukei::work::Work;
    for deg in [90.0f64, 10.0] {
        let b = ryukei_testkit::nurbs::isoline_wedge(deg * core::f64::consts::PI / 180.0);
        assert_eq!(ryukei::check::check(&b), [], "{deg}°");
        let w = Work::unlimited().with_budget(1 << 21);
        let faults = ryukei::check::check_intersections_with(&b, &w);
        assert_eq!(faults, Ok(vec![]), "{deg}°: {} units", w.spent());
        eprintln!("{deg}°: {} units", w.spent());
    }
}
