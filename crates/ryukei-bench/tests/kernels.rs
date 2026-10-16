//! The head-to-head with OCCT and SMLib: the same bodies meshed by all
//! three kernels at the same chord and angle, on one thread. Prints a
//! report (run with `--nocapture`) and asserts what each kernel promises:
//! every mesh encloses the exact volume to within its chord times the
//! exact area, SMLib's mesh is closed, and ryukei's sampled deviation is
//! within its chord. OCCT's and SMLib's deviations are reported, not
//! asserted; both are measured to OCCT's exact solid.

use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_bench::Primitive;
use ryukei_bench::occt::Solid;
use ryukei_bench::smlib;
use ryukei_testkit::mesh::{deviation, volume_and_area};
use ryukei_testkit::nurbs::convert_body;
use std::time::Instant;

fn placement() -> Rigid {
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    Rigid::translation(Vector3::new(3.5, -1.25, 2.0)).then_after(&Rigid::rotation(axis, 0.8))
}

/// The cases: a name, the primitive, and whether the kernels convert it
/// to NURBS first.
fn cases() -> Vec<(&'static str, Primitive, bool)> {
    vec![
        ("block", Primitive::Block([1.0, 0.75, 0.5]), false),
        ("cylinder", Primitive::Cylinder { r: 0.5, h: 1.0 }, false),
        (
            "cone",
            Primitive::Cone {
                r0: 0.5,
                r1: 0.0,
                h: 1.0,
            },
            false,
        ),
        (
            "frustum",
            Primitive::Cone {
                r0: 0.5,
                r1: 0.25,
                h: 1.0,
            },
            false,
        ),
        ("sphere", Primitive::Sphere(0.5), false),
        (
            "torus",
            Primitive::Torus {
                major: 0.5,
                minor: 0.2,
            },
            false,
        ),
        (
            "nurbs-cylinder",
            Primitive::Cylinder { r: 0.5, h: 1.0 },
            true,
        ),
        (
            "nurbs-cone",
            Primitive::Cone {
                r0: 0.5,
                r1: 0.0,
                h: 1.0,
            },
            true,
        ),
    ]
}

fn ryukei_body(p: Primitive, nurbs: bool, at: &Rigid) -> Body {
    let k = OpKey(1);
    let mut b = match p {
        Primitive::Block([x, y, z]) => primitive::block(k, x, y, z, at),
        Primitive::Cylinder { r, h } => primitive::cylinder(k, r, h, at),
        Primitive::Cone { r0, r1: 0.0, h } => primitive::cone(k, r0, h, at),
        Primitive::Cone { r0, r1, h } => primitive::frustum(k, r0, r1, h, at),
        Primitive::Sphere(r) => primitive::sphere(k, r, at),
        Primitive::Torus { major, minor } => primitive::torus(k, major, minor, at),
    }
    .unwrap()
    .0;
    if nurbs {
        convert_body(&mut b).unwrap();
    }
    b
}

#[test]
fn head_to_head() {
    let at = placement();
    let angle = 0.5;
    println!(
        "{:<15} {:>7} | {:>9} {:>8} {:>10} | {:>9} {:>8} {:>10} | {:>9} {:>8} {:>10}",
        "case",
        "chord",
        "ryukei ms",
        "tris",
        "deviation",
        "occt ms",
        "tris",
        "deviation",
        "smlib ms",
        "tris",
        "deviation"
    );
    for (name, p, nurbs) in cases() {
        let body = ryukei_body(p, nurbs, &at);
        let analytic = Solid::new(p, &at);
        // The exact values, from the analytic B-rep (OCCT's integration
        // over the converted NURBS is not as accurate).
        let (volume, area) = (analytic.volume(), analytic.area());
        let solid = if nurbs { analytic.to_nurbs() } else { analytic };
        let sm = smlib::Solid::new(p, &at, nurbs);
        for chord in [1e-2, 1e-3] {
            let start = Instant::now();
            let m = tessellate(&body, &MeshParams::new(chord, angle)).unwrap();
            let ours = start.elapsed();
            assert_eq!(m.faults, []);
            assert_eq!(check_mesh(&body, &m), Ok(()));
            let start = Instant::now();
            let o = solid.mesh(chord, angle, false);
            let theirs = start.elapsed();
            assert_eq!(o.faces_without_mesh, 0);
            let start = Instant::now();
            let s = sm.mesh(chord, angle, false);
            let smlibs = start.elapsed();
            assert!(!s.failed_faces, "{name}: SMLib failed faces");
            assert_eq!(s.lamina_edges, 0, "{name}: SMLib mesh is open");
            let every = (m.triangles.len() / 2000).max(1);
            let dev = deviation(&body, &m, 2, every);
            let odev = o.deviation(&solid, (o.triangles.len() / 2000).max(1));
            let sdev = s
                .soup()
                .deviation((s.triangles.len() / 2000).max(1), |q| solid.distance(q));
            println!(
                "{name:<15} {chord:>7.0e} | {:>9.3} {:>8} {dev:>10.2e} | {:>9.3} {:>8} {odev:>10.2e} | {:>9.3} {:>8} {sdev:>10.2e}",
                ours.as_secs_f64() * 1e3,
                m.triangles.len(),
                theirs.as_secs_f64() * 1e3,
                o.triangles.len() / 3,
                smlibs.as_secs_f64() * 1e3,
                s.triangles.len() / 3,
            );
            assert!(dev <= chord, "{name}: ryukei deviation {dev} over {chord}");
            let (v, _) = volume_and_area(&m);
            assert!(
                (v - volume).abs() <= chord * area,
                "{name}: ryukei volume {v} vs {volume}"
            );
            let (v, _) = o.volume_and_area();
            assert!(
                (v - volume).abs() <= chord * area,
                "{name}: OCCT volume {v} vs {volume}"
            );
            let (v, _) = s.soup().volume_and_area();
            assert!(
                (v - volume).abs() <= chord * area,
                "{name}: SMLib volume {v} vs {volume}"
            );
        }
    }
}
