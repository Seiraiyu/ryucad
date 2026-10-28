//! Meshes from bytes: a primitive or a NURBS body, placed anywhere, meshed
//! with any parameters. Bad parameters must be refused, a runaway budget
//! must be refused rather than run, and a mesh without faults must pass
//! the mesh checker. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei_testkit::euler_script::Bytes;
use ryukei_testkit::mesh::bump_block;
use ryukei_testkit::nurbs::convert_body;

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes::new(data);
    let kind = b.pick(7);
    let (x, y) = (b.range(0.05, 3.0), b.range(0.05, 3.0));
    let axis = Vector3::new(b.range(-1.0, 1.0), b.range(-1.0, 1.0), 1.0)
        .normalize()
        .expect("z is 1");
    let shift = Vector3::new(
        b.range(-50.0, 50.0),
        b.range(-50.0, 50.0),
        b.range(-50.0, 50.0),
    );
    let at = Rigid::translation(shift).then_after(&Rigid::rotation(axis, b.range(-3.2, 3.2)));
    let key = OpKey(9);
    let body = match kind {
        0 => primitive::block(key, x, y, x + y, &at).map(|r| r.0),
        1 => primitive::cylinder(key, x, y, &at).map(|r| r.0),
        2 => primitive::cone(key, x, y, &at).map(|r| r.0),
        3 => primitive::frustum(key, x, x / 2.0, y, &at).map(|r| r.0),
        4 => primitive::sphere(key, x, &at).map(|r| r.0),
        5 => primitive::torus(key, x + y, y, &at).map(|r| r.0),
        _ => bump_block(key, [x, y, x], y / 2.0, &at),
    };
    let Ok(mut body) = body else {
        return;
    };
    if kind < 4 && b.byte() % 2 == 0 {
        convert_body(&mut body).expect("converts");
    }
    // Now and then out of range.
    let chord = if b.byte() % 16 == 0 {
        b.range(-1.0, 2e-7)
    } else {
        b.range(1e-4, 0.5)
    };
    let angle = b.range(0.0, 2.0);
    let max_edge = (b.byte() % 4 == 0).then(|| b.range(0.0, 2.0));
    let params = MeshParams {
        chord,
        angle,
        max_edge,
        max_triangles: 200_000,
    };
    let valid = chord >= 1e-7
        && angle > 0.0
        && angle <= core::f64::consts::FRAC_PI_2
        && max_edge.is_none_or(|e| e >= chord);
    match tessellate(&body, &params) {
        Err(Error::InvalidMeshParams) => assert!(!valid),
        Err(Error::MeshTooLarge { .. }) => assert!(valid),
        Err(e) => panic!("unexpected {e:?}"),
        Ok(m) => {
            assert!(valid);
            if m.faults.is_empty() {
                assert_eq!(check_mesh(&body, &m), Ok(()));
            }
        }
    }
});
