//! The head-to-head with OCCT and SMLib: the same bodies meshed by all
//! three kernels at the same chord and angle, on one thread. Prints a
//! report (run with `--nocapture`) and asserts what each kernel promises:
//! every mesh encloses the exact volume to within its chord times the
//! exact area, SMLib's mesh is closed, and ryukei's sampled deviation is
//! within its chord. OCCT's and SMLib's deviations are reported, not
//! asserted; both are measured to OCCT's exact solid.
//!
//! Then the three kernels' extrudes and revolves of a fixed set of
//! profiles against closed forms.

use ryukei::check::check;
use ryukei::geom::{Axis, Point3, Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{extrude, revolve};
use ryukei::topo::Body;
use ryukei_bench::occt::Solid;
use ryukei_bench::smlib;
use ryukei_bench::{BenchProfile, BenchSegment, Primitive};
use ryukei_testkit::mesh::{deviation, volume_and_area};
use ryukei_testkit::nurbs::convert_body;
use std::f64::consts::PI;
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

/// How a sweep case moves its profile.
#[derive(Clone, Copy)]
enum Sweep {
    /// Along a direction, by a distance.
    Extrude([f64; 3], f64),
    /// About the profile's y axis (through its origin), by an angle.
    Revolve(f64),
}

/// A sweep case: a name, the profile, the sweep, and the exact volume
/// and area.
struct SweepCase {
    name: &'static str,
    profile: BenchProfile,
    sweep: Sweep,
    volume: f64,
    area: f64,
}

fn line(a: [f64; 2], b: [f64; 2]) -> BenchSegment {
    BenchSegment::Line { a, b }
}

fn arc(centre: [f64; 2], radius: f64, a0: f64, a1: f64) -> BenchSegment {
    BenchSegment::Arc {
        centre,
        radius,
        a0,
        a1,
    }
}

fn rectangle([u0, v0]: [f64; 2], [u1, v1]: [f64; 2]) -> Vec<BenchSegment> {
    vec![
        line([u0, v0], [u1, v0]),
        line([u1, v0], [u1, v1]),
        line([u1, v1], [u0, v1]),
        line([u0, v1], [u0, v0]),
    ]
}

fn sweep_cases() -> Vec<SweepCase> {
    // A frame off the origin and askew to the axes; `n` is its normal.
    let s2 = 2f64.sqrt();
    let s3 = 3f64.sqrt();
    let x = [1.0 / s2, 1.0 / s2, 0.0];
    let y = [-1.0 / s3, 1.0 / s3, 1.0 / s3];
    let n = [
        x[1] * y[2] - x[2] * y[1],
        x[2] * y[0] - x[0] * y[2],
        x[0] * y[1] - x[1] * y[0],
    ];
    let profile = |loops: Vec<Vec<BenchSegment>>| BenchProfile {
        origin: [0.5, -0.25, 1.0],
        x,
        y,
        loops,
    };
    let (w, h, d) = (1.0, 0.5, 0.75);
    // Oblique: along n + x / 2, so the height is d / |n + x / 2|.
    let oblique = [n[0] + x[0] / 2.0, n[1] + x[1] / 2.0, n[2] + x[2] / 2.0];
    let height = d / 1.25f64.sqrt();
    let r = 0.25;
    let hole = 2.0 - PI * r * r;
    let slot = 2.0 * r * w + PI * r * r;
    // A rectangle from a to b off the axis, turned by t.
    let (a, b) = (0.5, 1.0);
    let ring = |t: f64| {
        let caps = if t < 2.0 * PI { 2.0 * (b - a) * h } else { 0.0 };
        (
            t / 2.0 * (b * b - a * a) * h,
            t * (a + b) * h + t * (b * b - a * a) + caps,
        )
    };
    let (cone_r, cone_h) = (0.5, 1.0);
    vec![
        SweepCase {
            name: "rectangle",
            profile: profile(vec![rectangle([0.0, 0.0], [w, h])]),
            sweep: Sweep::Extrude(n, d),
            volume: w * h * d,
            area: 2.0 * w * h + 2.0 * (w + h) * d,
        },
        SweepCase {
            name: "oblique",
            profile: profile(vec![rectangle([0.0, 0.0], [w, h])]),
            sweep: Sweep::Extrude(oblique, d),
            volume: w * h * height,
            area: 2.0 * w * h + 2.0 * w * height + 2.0 * h * d,
        },
        SweepCase {
            name: "hole",
            profile: profile(vec![
                rectangle([0.0, 0.0], [2.0, 1.0]),
                vec![arc([1.0, 0.5], r, 0.0, 2.0 * PI)],
            ]),
            sweep: Sweep::Extrude(n, d),
            volume: hole * d,
            area: 2.0 * hole + (6.0 + 2.0 * PI * r) * d,
        },
        SweepCase {
            name: "slot",
            profile: profile(vec![vec![
                line([0.0, -r], [w, -r]),
                arc([w, 0.0], r, -PI / 2.0, PI / 2.0),
                line([w, r], [0.0, r]),
                arc([0.0, 0.0], r, PI / 2.0, 1.5 * PI),
            ]]),
            sweep: Sweep::Extrude(n, d),
            volume: slot * d,
            area: 2.0 * slot + (2.0 * w + 2.0 * PI * r) * d,
        },
        SweepCase {
            name: "sphere",
            profile: profile(vec![vec![
                arc([0.0, 0.0], r, -PI / 2.0, PI / 2.0),
                line([0.0, r], [0.0, -r]),
            ]]),
            sweep: Sweep::Revolve(2.0 * PI),
            volume: 4.0 / 3.0 * PI * r * r * r,
            area: 4.0 * PI * r * r,
        },
        SweepCase {
            name: "ring-90",
            profile: profile(vec![rectangle([a, 0.0], [b, h])]),
            sweep: Sweep::Revolve(PI / 2.0),
            volume: ring(PI / 2.0).0,
            area: ring(PI / 2.0).1,
        },
        SweepCase {
            name: "ring",
            profile: profile(vec![rectangle([a, 0.0], [b, h])]),
            sweep: Sweep::Revolve(2.0 * PI),
            volume: ring(2.0 * PI).0,
            area: ring(2.0 * PI).1,
        },
        SweepCase {
            name: "cone",
            profile: profile(vec![vec![
                line([0.0, 0.0], [cone_r, 0.0]),
                line([cone_r, 0.0], [0.0, cone_h]),
                line([0.0, cone_h], [0.0, 0.0]),
            ]]),
            sweep: Sweep::Revolve(2.0 * PI),
            volume: PI * cone_r * cone_r * cone_h / 3.0,
            area: PI * cone_r * cone_r + PI * cone_r * (cone_r * cone_r + cone_h * cone_h).sqrt(),
        },
    ]
}

/// The three kernels' extrudes and revolves of the same profiles against
/// the exact volume and area: OCCT's from its B-rep, SMLib's and
/// ryukei's from their meshes. Prints a report (run with `--nocapture`)
/// and asserts the cases each kernel gets right.
#[test]
fn sweeps() {
    let (chord, angle) = (1e-4, 0.1);
    println!(
        "{:<10} {:>9} {:>9} | {:>9} {:>9} {:>5} | {:>9} {:>9} {:>5} {:>8} | {:>9} {:>9} {:>5} {:>8}",
        "case",
        "volume",
        "area",
        "occt dV",
        "dA",
        "faces",
        "smlib dV",
        "dA",
        "faces",
        "tris",
        "ryukei dV",
        "dA",
        "faces",
        "tris"
    );
    for c in sweep_cases() {
        let (o, s) = match c.sweep {
            Sweep::Extrude(dir, dist) => (
                Solid::extrude(&c.profile, dir, dist),
                smlib::Solid::extrude(&c.profile, dir, dist),
            ),
            Sweep::Revolve(t) => (
                Solid::revolve(&c.profile, c.profile.origin, c.profile.y, t),
                smlib::Solid::revolve(&c.profile, c.profile.origin, c.profile.y, t),
            ),
        };
        let rel = |v: f64, exact: f64| (v - exact) / exact;
        let p = c.profile.to_ryukei().expect("ryukei takes the profile");
        let r = match c.sweep {
            Sweep::Extrude(dir, dist) => {
                let dir = Vector3::new(dir[0], dir[1], dir[2]);
                extrude(OpKey(1), &p, dir, 0.0, dist)
            }
            Sweep::Revolve(t) => {
                let [ox, oy, oz] = c.profile.origin;
                let [yx, yy, yz] = c.profile.y;
                let axis = Axis::new(Point3::new(ox, oy, oz), Vector3::new(yx, yy, yz)).unwrap();
                revolve(OpKey(1), &p, axis, 0.0, t)
            }
        }
        .map(|(b, _)| b);
        let ry = r.as_ref().map(|b| {
            let m = tessellate(b, &MeshParams::new(chord, angle)).unwrap();
            let (v, a) = volume_and_area(&m);
            (v, a, b.counts().faces, m.triangles.len(), m)
        });
        let occt = match &o {
            Ok(o) => format!(
                "{:>9.1e} {:>9.1e} {:>5}",
                rel(o.volume(), c.volume),
                rel(o.area(), c.area),
                o.faces()
            ),
            Err(e) => format!("fails: {e}"),
        };
        let sm = match &s {
            Ok(s) => {
                let m = s.mesh(chord, angle, false);
                let (v, a) = m.soup().volume_and_area();
                format!(
                    "{:>9.1e} {:>9.1e} {:>5} {:>8}{}",
                    rel(v, c.volume),
                    rel(a, c.area),
                    s.faces(),
                    m.triangles.len() / 3,
                    if m.failed_faces || m.lamina_edges > 0 {
                        " (open)"
                    } else {
                        ""
                    }
                )
            }
            Err(e) => format!("fails: {e}"),
        };
        let ours = match &ry {
            Ok((v, a, f, t, _)) => format!(
                "{:>9.1e} {:>9.1e} {:>5} {:>8}",
                rel(*v, c.volume),
                rel(*a, c.area),
                f,
                t
            ),
            Err(e) => format!("fails: {e}"),
        };
        println!(
            "{:<10} {:>9.5} {:>9.5} | {occt} | {sm} | {ours}",
            c.name, c.volume, c.area
        );
        // Every case held when this was written: both kernels build it,
        // with the same faces; OCCT's exact volume and area are right to
        // rounding, and SMLib's closed mesh encloses the volume to within
        // its chord times the area, with the area right to 1e-3.
        let name = c.name;
        let o = o.unwrap_or_else(|e| panic!("{name}: OCCT fails: {e}"));
        let s = s.unwrap_or_else(|e| panic!("{name}: SMLib fails: {e}"));
        assert!(
            rel(o.volume(), c.volume).abs() <= 1e-12,
            "{name}: OCCT volume"
        );
        assert!(rel(o.area(), c.area).abs() <= 1e-12, "{name}: OCCT area");
        assert_eq!(o.faces(), s.faces(), "{name}: faces");
        let m = s.mesh(chord, angle, false);
        assert!(!m.failed_faces && m.lamina_edges == 0, "{name}: SMLib mesh");
        let (v, a) = m.soup().volume_and_area();
        assert!(
            (v - c.volume).abs() <= chord * c.area,
            "{name}: SMLib volume"
        );
        assert!(rel(a, c.area).abs() <= 1e-3, "{name}: SMLib area");
        // ryukei builds every case too, with OCCT's faces, a valid body and
        // a sound mesh that encloses the volume to within its chord times
        // the area.
        let (v, a, faces, _, m) = ry.unwrap_or_else(|e| panic!("{name}: ryukei fails: {e}"));
        let body = r.unwrap();
        assert_eq!(check(&body), [], "{name}: ryukei body");
        assert_eq!(check_mesh(&body, &m), Ok(()), "{name}: ryukei mesh");
        assert!(m.faults.is_empty(), "{name}: ryukei faults");
        assert_eq!(faces, o.faces() as usize, "{name}: ryukei faces");
        assert!(
            (v - c.volume).abs() <= chord * c.area,
            "{name}: ryukei volume"
        );
        assert!(rel(a, c.area).abs() <= 1e-3, "{name}: ryukei area");
    }
}
