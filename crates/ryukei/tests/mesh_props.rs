//! Meshes keep their promises: every face within the chord of its
//! triangles both ways, normals within the angle across each triangle,
//! watertight and outward, enclosing the right volume, and the same on
//! one thread or many.

use core::f64::consts::PI;
use proptest::prelude::*;
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::predicates::{Sign, volume_sign};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::mesh::{bump_block, deviation, volume_and_area};
use ryukei_testkit::nurbs::convert_body;

fn placement() -> impl Strategy<Value = Rigid> {
    (-1.0f64..1.0, -1.0f64..1.0, -3.0f64..3.0, -20.0f64..20.0).prop_map(|(x, y, angle, shift)| {
        let axis = Vector3::new(x, y, 1.0).normalize().unwrap();
        Rigid::translation(Vector3::new(shift, -shift / 2.0, shift / 3.0))
            .then_after(&Rigid::rotation(axis, angle))
    })
}

/// A primitive with its exact volume and area.
fn primitive(kind: u8, a: f64, b: f64, at: &Rigid) -> (Body, f64, f64) {
    let k = OpKey(3);
    match kind {
        0 => (
            primitive::block(k, a, b, a + b, at).unwrap().0,
            a * b * (a + b),
            2.0 * (a * b + (a + b) * (a + b)),
        ),
        1 => (
            primitive::cylinder(k, a, b, at).unwrap().0,
            PI * a * a * b,
            2.0 * PI * a * (a + b),
        ),
        2 => (
            primitive::cone(k, a, b, at).unwrap().0,
            PI * a * a * b / 3.0,
            PI * a * (a + (a * a + b * b).sqrt()),
        ),
        3 => {
            let (r0, r1, h) = (a, a / 2.0, b);
            (
                primitive::frustum(k, r0, r1, h, at).unwrap().0,
                PI * h * (r0 * r0 + r0 * r1 + r1 * r1) / 3.0,
                PI * (r0 * r0 + r1 * r1) + PI * (r0 + r1) * ((r0 - r1) * (r0 - r1) + h * h).sqrt(),
            )
        }
        4 => (
            primitive::sphere(k, a, at).unwrap().0,
            4.0 * PI * a * a * a / 3.0,
            4.0 * PI * a * a,
        ),
        _ => {
            let (major, minor) = (a + b, b);
            (
                primitive::torus(k, major, minor, at).unwrap().0,
                2.0 * PI * PI * major * minor * minor,
                4.0 * PI * PI * major * minor,
            )
        }
    }
}

/// The checks every sound mesh passes.
fn sound(b: &Body, p: &MeshParams, samples: u32, every: usize) -> ryukei::mesh::BodyMesh {
    let m = tessellate(b, p).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(b, &m), Ok(()));
    let tris: Vec<_> = m
        .welded_triangles()
        .map(|t| t.map(|i| m.positions[i as usize]))
        .collect();
    assert_eq!(volume_sign(&tris), Sign::Positive);
    let d = deviation(b, &m, samples, every);
    assert!(d <= p.chord, "deviation {d} over {}", p.chord);
    m
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]

    #[test]
    fn primitives(
        kind in 0u8..6,
        a in 0.2f64..2.0,
        b in 0.2f64..2.0,
        chord in 1e-3f64..2e-2,
        angle in 0.2f64..1.5,
        at in placement(),
    ) {
        let (body, volume, area) = primitive(kind, a, b, &at);
        let p = MeshParams::new(chord, angle);
        let m = sound(&body, &p, 3, 1);
        let (v, s) = volume_and_area(&m);
        prop_assert!((v - volume).abs() <= chord * area, "volume {} vs {}", v, volume);
        // A mesh of a saddle can have more area than the surface, so only
        // nearness: the relative error of a chord of a curve of radius r
        // with sagitta δ is about 2δ / r.
        let slack = 3.0 * chord / a.min(b) * area;
        prop_assert!((s - area).abs() <= slack, "area {} vs {}", s, area);
        // Corner normals across a triangle turn by at most the angle,
        // except across triangles smaller than the chord, where the angle
        // is waived.
        for t in &m.triangles {
            let n = t.map(|c| m.corners[c as usize].normal.get());
            let p = t.map(|c| m.positions[m.corners[c as usize].position as usize]);
            let size = p[0].distance(p[1]).max(p[1].distance(p[2])).max(p[2].distance(p[0]));
            if size < chord {
                continue;
            }
            for (x, y) in [(n[0], n[1]), (n[1], n[2]), (n[2], n[0])] {
                let turn = ryukei::math::atan2(x.cross(y).length(), x.dot(y));
                prop_assert!(turn <= angle * (1.0 + 1e-9), "turn {} over {}", turn, angle);
            }
        }
    }

    #[test]
    fn nurbs_bodies(
        kind in 0u8..5,
        a in 0.3f64..2.0,
        b in 0.3f64..2.0,
        chord in 1e-2f64..4e-2,
        at in placement(),
    ) {
        let mut body = match kind {
            0 => primitive::block(OpKey(4), a, b, a + b, &at).unwrap().0,
            1 => primitive::cylinder(OpKey(4), a, b, &at).unwrap().0,
            2 => primitive::cone(OpKey(4), a, b, &at).unwrap().0,
            3 => primitive::frustum(OpKey(4), a, a / 2.0, b, &at).unwrap().0,
            _ => bump_block(OpKey(4), [a, b, a], b / 2.0, &at).unwrap(),
        };
        if kind < 4 {
            convert_body(&mut body).unwrap();
        }
        sound(&body, &MeshParams::new(chord, 0.5), 1, 7);
    }
}

#[test]
fn threads_do_not_change_the_mesh() {
    let at = Rigid::rotation(Vector3::new(1.0, 1.0, 0.0).normalize().unwrap(), 0.4);
    let mut cone = primitive::cone(OpKey(5), 1.0, 2.0, &at).unwrap().0;
    convert_body(&mut cone).unwrap();
    let bodies = [
        bump_block(OpKey(5), [1.0, 2.0, 0.5], 0.4, &at).unwrap(),
        cone,
        primitive::torus(OpKey(5), 2.0, 0.5, &at).unwrap().0,
    ];
    let p = MeshParams::new(1e-2, 0.5);
    for b in &bodies {
        let on = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| tessellate(b, &p).unwrap())
        };
        let one = on(1);
        assert_eq!(one, on(3));
        assert_eq!(one, on(8));
    }
}

/// A NURBS cylinder turned by a hair off the axes: one interval bound over
/// its whole side let a derivative reach zero, and the estimate refused it
/// as billions of triangles. The estimate now bounds cell by cell.
#[test]
fn a_nurbs_cylinder_nearly_on_the_axes() {
    let axis = Vector3::new(-2.0e-5, -0.005154, 0.007831)
        .normalize()
        .unwrap();
    let at = Rigid::rotation(axis, 0.009361);
    let (mut b, _) =
        primitive::cylinder(OpKey(4), 1.5275634849023805, 1.4562347129475794, &at).unwrap();
    convert_body(&mut b).unwrap();
    sound(&b, &MeshParams::new(0.01, 0.5), 1, 7);
}
