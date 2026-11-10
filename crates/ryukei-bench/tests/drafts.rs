//! Drafts and face offsets head to head (design 384, plan 9.C): ryukei's
//! `tweak::draft` against OCCT's `BRepOffsetAPI_DraftAngle`, and
//! `tweak::offset_faces` against `BRepOffset_MakeOffset` with
//! `SetOffsetOnFace`. ryukei must hold each case's closed-form volume
//! and its asked angle; OCCT's volume and angle are printed beside it,
//! and its refusals printed, never judged (SMLib has no draft). Then the
//! 1,000-hole plate's walls drafted on one thread against OCCT
//! (`drafting_1000_hole_walls`, Task 9). Prints a report (run with
//! `--nocapture`).

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Point3, Rigid, Surface, UnitVector3, Vector3};
use ryukei::math;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::tweak::{DraftFace, DraftSpec, Hinge, OffsetFacesSpec, Topology, draft, offset_faces};
use ryukei_bench::{Primitive, occt};
use ryukei_testkit::mesh::volume_and_area;
use std::f64::consts::PI;
use std::time::Instant;

const K: OpKey = OpKey(0xbd0);

/// OCCT's draft angle against ryukei's: `1.0` where a positive angle
/// tapers a boss inward along the pull in both. Fixed by Task 8's Step 4
/// against the box's closed form.
const OCCT_SIGN: f64 = 1.0;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

fn up() -> Vector3 {
    Vector3::new(0.0, 0.0, 1.0)
}

fn face(r: FaceRole) -> Name {
    Name::derive(K, Role::Face(r), &[])
}

fn walls() -> Vec<Name> {
    [
        FaceRole::PlusX,
        FaceRole::MinusX,
        FaceRole::PlusY,
        FaceRole::MinusY,
    ]
    .map(face)
    .to_vec()
}

fn cylinders(b: &Body) -> Vec<Name> {
    b.faces()
        .filter(|&f| {
            matches!(
                b.surface(b.face(f).unwrap().surface).unwrap(),
                Surface::Cylinder(_)
            )
        })
        .map(|f| b.face(f).unwrap().name)
        .collect()
}

fn spec(faces: &[Name], angle: f64, z: f64) -> DraftSpec {
    DraftSpec {
        pull: UnitVector3::exact(up()).unwrap(),
        angle,
        hinge: Hinge::Neutral {
            origin: Point3::new(0.0, 0.0, z),
        },
        faces: faces
            .iter()
            .map(|&face| DraftFace {
                face,
                angle: None,
                flip: false,
            })
            .collect(),
        tangent: false,
        topology: Topology::Keep,
        blends: Default::default(),
    }
}

/// The volume ryukei's body encloses, by a mesh at `chord`, with the
/// mesh's error (`chord` times its area).
fn ryukei_volume(b: &Body, chord: f64) -> (f64, f64) {
    let m = tessellate(b, &MeshParams::new(chord, 0.3)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    let (v, a) = volume_and_area(&m);
    (v, chord * a)
}

fn prismatoid(h: f64, a: impl Fn(f64) -> f64) -> f64 {
    h / 6.0 * (a(0.0) + 4.0 * a(h / 2.0) + a(h))
}

fn frustum(h: f64, r0: f64, r1: f64) -> f64 {
    PI * h / 3.0 * (r0 * r0 + r0 * r1 + r1 * r1)
}

fn subtract(a: &Body, t: &Body, place: Rigid) -> Body {
    let tool = Tool {
        key: ToolKey(1),
        body: t,
        place,
    };
    boolean(OpKey(0xbd1), BooleanOp::Subtract, a, &[tool])
        .unwrap()
        .0
}

fn unite(a: &Body, t: &Body, place: Rigid) -> Body {
    let tool = Tool {
        key: ToolKey(1),
        body: t,
        place,
    };
    boolean(OpKey(0xbd1), BooleanOp::Unite, a, &[tool])
        .unwrap()
        .0
}

/// One draft both kernels make: ryukei's body and spec, OCCT's body, a
/// point on each drafted face (where OCCT's face is found, and its
/// normal read after), the neutral plane's height and the exact volume.
struct DraftCase {
    name: &'static str,
    ryukei: Body,
    spec: DraftSpec,
    occt: occt::Solid,
    points: Vec<Point3>,
    exact: f64,
}

fn draft_cases() -> Vec<DraftCase> {
    let th = 0.05;
    let t = math::tan(th);
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let box_occt = || occt::Solid::new(Primitive::Block([1.0, 0.6, 0.4]), &Rigid::IDENTITY);
    let box_walls = vec![
        Point3::new(1.0, 0.3, 0.2),
        Point3::new(0.0, 0.3, 0.2),
        Point3::new(0.5, 0.6, 0.2),
        Point3::new(0.5, 0.0, 0.2),
    ];
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let plate_occt = || occt::Solid::new(Primitive::Block([1.0, 1.0, 0.2]), &Rigid::IDENTITY);
    let (boss, _) = primitive::cylinder(OpKey(0xbd2), 0.1, 0.15, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0xbd2), 0.1, 0.4, &Rigid::IDENTITY).unwrap();
    let bossed = unite(&plate, &boss, at(0.5, 0.5, 0.15));
    let drilled = subtract(&plate, &pin, at(0.5, 0.5, -0.1));
    let boss_occt = plate_occt()
        .boolean(
            BooleanOp::Unite,
            &[occt::Solid::new(
                Primitive::Cylinder { r: 0.1, h: 0.15 },
                &at(0.5, 0.5, 0.15),
            )],
            false,
        )
        .expect("OCCT's boss");
    let hole_occt = plate_occt().cut(
        &[occt::Solid::new(
            Primitive::Cylinder { r: 0.1, h: 0.4 },
            &at(0.5, 0.5, -0.1),
        )],
        false,
    );
    let th2 = 0.1;
    let t2 = math::tan(th2);
    vec![
        DraftCase {
            name: "box walls about the bottom",
            spec: spec(&walls(), th, 0.0),
            ryukei: b.clone(),
            occt: box_occt(),
            points: box_walls.clone(),
            exact: prismatoid(0.4, |z| (1.0 - 2.0 * z * t) * (0.6 - 2.0 * z * t)),
        },
        DraftCase {
            name: "box walls about the middle",
            spec: spec(&walls(), th, 0.2),
            ryukei: b,
            occt: box_occt(),
            points: box_walls,
            exact: {
                let a = |z: f64| (1.0 - 2.0 * z * t) * (0.6 - 2.0 * z * t);
                prismatoid(0.2, a) + prismatoid(0.2, |z| a(-z))
            },
        },
        DraftCase {
            name: "a boss about its foot",
            spec: spec(&cylinders(&bossed), th2, 0.2),
            ryukei: bossed,
            occt: boss_occt,
            points: vec![Point3::new(0.6, 0.5, 0.275)],
            exact: 0.2 + frustum(0.1, 0.1, 0.1 - 0.1 * t2),
        },
        DraftCase {
            name: "a hole about the bottom",
            spec: spec(&cylinders(&drilled), th2, 0.0),
            ryukei: drilled,
            occt: hole_occt,
            points: vec![Point3::new(0.6, 0.5, 0.1)],
            exact: 0.2 - frustum(0.2, 0.1, 0.1 + 0.2 * t2),
        },
    ]
}

#[test]
fn drafts_against_occt() {
    for c in draft_cases() {
        let (o, _, report) = draft(OpKey(0xbd3), &c.ryukei, &c.spec).expect(c.name);
        let (v, slack) = ryukei_volume(&o, 1e-5);
        assert!(
            (v - c.exact).abs() <= slack + 1e-9,
            "{}: ryukei {v} against {}",
            c.name,
            c.exact
        );
        for a in &report.faces {
            // A hole's wall reads its angle against the pull (negative).
            assert!(
                (a.least.abs() - c.spec.angle).abs() < 1e-9,
                "{}: {a:?}",
                c.name
            );
            assert!(
                (a.greatest.abs() - c.spec.angle).abs() < 1e-9,
                "{}: {a:?}",
                c.name
            );
        }
        let z = match &c.spec.hinge {
            Hinge::Neutral { origin } => *origin,
            Hinge::PartingLine(_) => unreachable!("neutral planes only"),
        };
        match c
            .occt
            .draft(&c.points, false, up(), OCCT_SIGN * c.spec.angle, z, up())
        {
            Ok((d, secs)) => {
                let ov = d.volume();
                let angles: Vec<f64> = c
                    .points
                    .iter()
                    .filter_map(|&p| d.face_normal(p))
                    .map(|n| math::asin(n.z.clamp(-1.0, 1.0)).abs())
                    .collect();
                println!(
                    "{}: exact {:.9}, ryukei {v:.9}, OCCT {ov:.9} ({}) in {secs:.3} s, its angles {angles:?} against {}",
                    c.name,
                    c.exact,
                    if (ov - c.exact).abs() <= 1e-6 * c.exact {
                        "agrees"
                    } else {
                        "differs"
                    },
                    c.spec.angle
                );
            }
            Err(e) => println!(
                "{}: exact {:.9}, ryukei {v:.9}, OCCT refused: {e}",
                c.name, c.exact
            ),
        }
    }
}

/// A face offset both kernels make: its name, ryukei's faces and
/// distances, OCCT's points on them with the same distances, and the
/// exact volume.
type OffsetCase = (&'static str, Vec<(Name, f64)>, Vec<(Point3, f64)>, f64);

#[test]
fn offset_faces_against_occt() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let cases: [OffsetCase; 2] = [
        (
            "a box's top up 0.1",
            vec![(face(FaceRole::PlusZ), 0.1)],
            vec![(Point3::new(0.5, 0.3, 0.4), 0.1)],
            1.0 * 0.6 * 0.5,
        ),
        (
            "a box's top up 0.05, a side in 0.03",
            vec![
                (face(FaceRole::PlusZ), 0.05),
                (face(FaceRole::PlusX), -0.03),
            ],
            vec![
                (Point3::new(0.5, 0.3, 0.4), 0.05),
                (Point3::new(1.0, 0.3, 0.2), -0.03),
            ],
            0.97 * 0.6 * 0.45,
        ),
    ];
    for (name, faces, points, exact) in cases {
        let spec = OffsetFacesSpec {
            faces,
            tangent: false,
            topology: Topology::Keep,
            blends: Default::default(),
        };
        let (o, _) = offset_faces(OpKey(0xbd4), &b, &spec).expect(name);
        let (v, slack) = ryukei_volume(&o, 1e-4);
        assert!(
            (v - exact).abs() <= slack + 1e-9,
            "{name}: ryukei {v} against {exact}"
        );
        let solid = occt::Solid::new(Primitive::Block([1.0, 0.6, 0.4]), &Rigid::IDENTITY);
        match solid.offset_faces(&points) {
            Ok(s) => println!(
                "{name}: exact {exact:.9}, ryukei {v:.9}, OCCT {:.9}",
                s.volume()
            ),
            Err(e) => println!("{name}: exact {exact:.9}, ryukei {v:.9}, OCCT refused: {e}"),
        }
    }
}

/// The booleans' race plate: 40 × 25 × 0.5 with a grid of 1,000 through
/// holes of radius 0.3, made by both kernels.
fn plate_1000() -> (Body, occt::Solid) {
    let (cols, rows) = (40, 25);
    let places: Vec<Rigid> = (0..rows)
        .flat_map(|j| (0..cols).map(move |i| at(i as f64 + 0.5, j as f64 + 0.5, -0.25)))
        .collect();
    let (plate, _) =
        primitive::block(K, f64::from(cols), f64::from(rows), 0.5, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0xbd5), 0.3, 1.0, &Rigid::IDENTITY).unwrap();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, &place)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &pin,
            place,
        })
        .collect();
    let (body, _) = boolean(OpKey(0xbd6), BooleanOp::Subtract, &plate, &tools).unwrap();
    let solid = occt::Solid::new(
        Primitive::Block([f64::from(cols), f64::from(rows), 0.5]),
        &Rigid::IDENTITY,
    );
    let pins: Vec<occt::Solid> = places
        .iter()
        .map(|p| occt::Solid::new(Primitive::Cylinder { r: 0.3, h: 1.0 }, p))
        .collect();
    (body, solid.cut(&pins, false))
}

/// Design 384's bar: every hole's wall drafted 1° about the plate's
/// bottom, on one thread, faster than OCCT's `DraftAngle` (its time
/// taken in C++ around Add and Build). Asserted as it stands with
/// `RYUKEI_BARS=1`, ten times looser otherwise. Where OCCT refuses, or
/// its volume misses the closed form, the bar isn't judged (as the shell
/// race, plan 14.D) and the test says so.
#[test]
fn drafting_1000_hole_walls_beats_occt_on_one_thread() {
    let th = PI / 180.0;
    let (body, solid) = plate_1000();
    let walls = cylinders(&body);
    assert_eq!(walls.len(), 1000);
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = Instant::now();
    let (o, _, report) = one
        .install(|| draft(OpKey(0xbd7), &body, &spec(&walls, th, 0.0)))
        .expect("ryukei's draft");
    let ryukei_s = t.elapsed().as_secs_f64();
    assert_eq!(report.faces.len(), 1000);
    let exact = 40.0 * 25.0 * 0.5 - 1000.0 * frustum(0.5, 0.3, 0.3 + 0.5 * math::tan(th));
    let (v, slack) = ryukei_volume(&o, 1e-4);
    assert!(
        (v - exact).abs() <= slack + 1e-9,
        "ryukei {v} against {exact}"
    );
    let t = Instant::now();
    let all = rayon::ThreadPoolBuilder::new()
        .build()
        .unwrap()
        .install(|| draft(OpKey(0xbd7), &body, &spec(&walls, th, 0.0)))
        .expect("ryukei's draft");
    let parallel_s = t.elapsed().as_secs_f64();
    assert_eq!(all.0.dump(), o.dump());
    let occt = solid.draft(&[], true, up(), OCCT_SIGN * th, Point3::ORIGIN, up());
    let bars = std::env::var_os("RYUKEI_BARS").is_some();
    match occt {
        Ok((d, occt_s)) => {
            let ov = d.volume();
            let agrees = (ov - exact).abs() <= 1e-6 * exact;
            println!(
                "1,000 hole walls drafted: ryukei {ryukei_s:.3} s on one thread, {parallel_s:.3} s on all; OCCT {occt_s:.3} s ({:.2} times), its volume {ov:.9} against {exact:.9} ({})",
                ryukei_s / occt_s,
                if agrees {
                    "agrees"
                } else {
                    "differs: the bar isn't judged"
                }
            );
            if agrees {
                let bar = if bars { occt_s } else { 10.0 * occt_s };
                assert!(ryukei_s < bar, "ryukei {ryukei_s:.3} s against {bar:.3} s");
            }
        }
        Err(e) => println!(
            "1,000 hole walls drafted: ryukei {ryukei_s:.3} s on one thread, {parallel_s:.3} s on all; OCCT refused ({e}): the bar isn't judged"
        ),
    }
}
