//! #917: a mesh never depends on the order of a face's loops.
//!
//! Healing builds a body's entities in its import names' order, and the
//! names follow the caller's key, so one shape healed under two keys is
//! the same body with its loops in another order. The race's p484 (blend
//! seed 267, its tube between two caps) meshed under one key, and under
//! ryucad's key for part `p484` its tube stalled and its cap was refused
//! with `BadDomain`.

use ryukei::heal::{HealOptions, Open, heal, seamed};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;

#[test]
#[cfg_attr(debug_assertions, ignore = "seconds in release, minutes in debug")]
fn one_shape_healed_under_any_key_meshes_alike() {
    let body = ryukei_testkit::race::blend_body(267).expect("seed 267's blend");
    let shape = seamed(&body, 1e-7).unwrap();
    let options = HealOptions {
        open: Open::Sheet,
        ..Default::default()
    };
    let params = MeshParams::new(1e-3, 15f64.to_radians());
    let keys = [
        b"ryucad.feature\0p484\0body".as_slice(),
        b"ryucad.feature\0p0\0body",
    ]
    .into_iter()
    .map(OpKey::from_bytes)
    .chain((0..6).map(OpKey));
    let mut first: Option<(usize, usize)> = None;
    for key in keys {
        let (healed, _, _) = heal(key, &shape, &options).unwrap();
        let mesh = tessellate(&healed, &params).unwrap();
        assert!(mesh.faults.is_empty(), "{key:?}: {:?}", mesh.faults);
        let counts = (mesh.positions.len(), mesh.triangles.len());
        assert_eq!(*first.get_or_insert(counts), counts, "{key:?}");
    }
}
