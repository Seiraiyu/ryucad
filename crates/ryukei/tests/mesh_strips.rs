//! Faces narrower than the chord drawn as strips (#598).

use ryukei::heal::{ReferenceOptions, reference};
use ryukei::mesh::{MeshFaultKind, MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei_testkit::mesh::crossing_sliver;

/// A Coilcraft coil's wire end: a sliver 0.08 µm wide whose sides cross,
/// meshed at a coil's normal quality (a 2 µm chord): whole, sound, and
/// in a few dozen triangles.
#[test]
fn a_crossing_sliver_is_drawn_as_a_strip() {
    let r = 0.445e-3;
    let (body, _) = reference(
        OpKey(3),
        &crossing_sliver(r, r + 8e-8),
        &ReferenceOptions::default(),
    )
    .unwrap();
    let mesh = tessellate(&body, &MeshParams::new(2e-6, core::f64::consts::PI / 12.0)).unwrap();
    assert_eq!(mesh.faults, []);
    assert_eq!(check_mesh(&body, &mesh), Ok(()));
    assert!(mesh.triangles.len() < 1_000, "{}", mesh.triangles.len());
}

/// A sliver wider than the chord is no strip. Its sides cross, so its
/// loop makes no region and it stays `BadDomain`, as on main: strips are
/// kept to slivers narrower than the chord. (Every crossing sliver from
/// 10 to 400 µm wide is `BadDomain` on main.)
#[test]
fn a_sliver_wider_than_the_chord_is_no_strip() {
    let r = 0.445e-3;
    let (body, _) = reference(
        OpKey(3),
        &crossing_sliver(r, r + 2e-5),
        &ReferenceOptions::default(),
    )
    .unwrap();
    let mesh = tessellate(&body, &MeshParams::new(2e-6, core::f64::consts::PI / 12.0)).unwrap();
    let kinds: Vec<MeshFaultKind> = mesh.faults.iter().map(|f| f.kind).collect();
    assert_eq!(kinds, [MeshFaultKind::BadDomain]);
    assert!(mesh.faces.is_empty());
}
