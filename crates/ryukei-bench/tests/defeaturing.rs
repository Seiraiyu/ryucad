//! Deleting faces head to head (design 399, plan 13.D): ryukei's
//! `tweak::delete_faces` against OCCT's `BRepAlgoAPI_Defeaturing`, which
//! removes faces and heals the gap by extending their neighbours. Each
//! case's body is made by ryukei and read by OCCT through STEP, so both
//! kernels start from one geometry. ryukei must hold the case's closed
//! form; OCCT's volume is printed beside it and its refusals printed,
//! never judged (SMLib has no defeaturing). Then design 399's bar: the
//! race plate's 1,000 holes deleted on one thread against OCCT. Prints a
//! report (run with `--nocapture`).

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Point3, Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::tweak::{DeleteSpec, Heal, delete_faces};
use ryukei_bench::{Primitive, occt};
use ryukei_testkit::mesh::volume_and_area;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
use std::path::Path;
use std::time::Instant;

const K: OpKey = OpKey(0xbe0);
const D: OpKey = OpKey(0xbe1);

/// Millimetres in a meter: OCCT reads STEP in millimetres.
const MM: f64 = 1e3;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

fn face(r: FaceRole) -> Name {
    Name::derive(K, Role::Face(r), &[])
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

/// `body` written to STEP in meters and read by OCCT, unfixed.
fn to_occt(dir: &Path, name: &str, body: &Body) -> occt::Solid {
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: name.into(),
                name: name.into(),
                bodies: vec![body.clone()],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Meter,
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let path = dir.join(format!("{name}.stp"));
    std::fs::write(&path, &bytes).unwrap();
    occt::Solid::read_step(&path, false).unwrap()
}

fn combine(op: BooleanOp, a: &Body, t: &Body, place: Rigid) -> Body {
    let tools = [Tool {
        key: ToolKey(1),
        body: t,
        place,
    }];
    boolean(OpKey(0xbe2), op, a, &tools).unwrap().0
}

/// The names of `b`'s faces `pick` takes.
fn faces_on(b: &Body, pick: impl Fn(&Surface, &Body, ryukei::topo::FaceId) -> bool) -> Vec<Name> {
    b.faces()
        .filter(|&f| pick(b.surface(b.face(f).unwrap().surface).unwrap(), b, f))
        .map(|f| b.face(f).unwrap().name)
        .collect()
}

fn not_planes(b: &Body) -> Vec<Name> {
    faces_on(b, |s, _, _| !matches!(s, Surface::Plane(_)))
}

/// Every edge of a 1 × 0.6 × 0.4 block blended with `kind`.
fn every_edge(kind: BlendKind) -> Body {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let edges: Vec<EdgeBlend> = b
        .edges()
        .map(|e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind,
        })
        .collect();
    blend(OpKey(0xbe3), &b, &edges).unwrap().0
}

struct Case {
    name: &'static str,
    body: Body,
    delete: Vec<Name>,
    heal: Heal,
    occt: occt::Defeature,
    exact: f64,
}

fn cases() -> Vec<Case> {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0xbe4), 0.1, 0.4, &Rigid::IDENTITY).unwrap();
    let (boss, _) = primitive::cylinder(OpKey(0xbe5), 0.1, 0.15, &Rigid::IDENTITY).unwrap();
    let drilled = combine(BooleanOp::Subtract, &plate, &pin, at(0.5, 0.5, -0.1));
    let bossed = combine(BooleanOp::Unite, &plate, &boss, at(0.5, 0.5, 0.2));
    let boss_faces = faces_on(&bossed, |s, _, _| match s {
        Surface::Cylinder(_) => true,
        Surface::Plane(p) => (p.frame().origin().z - 0.35).abs() < 1e-12,
        _ => false,
    });
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let top_front = b
        .edges()
        .find(|&e| {
            let n: Vec<Name> = b
                .edge_fins(e)
                .map(|fin| {
                    let l = b.fin(fin).unwrap().loop_;
                    b.face(b.loop_(l).unwrap().face).unwrap().name
                })
                .collect();
            n.contains(&face(FaceRole::PlusZ)) && n.contains(&face(FaceRole::MinusY))
        })
        .map(|e| b.edge(e).unwrap().name)
        .unwrap();
    let edged = |kind| {
        blend(
            OpKey(0xbe6),
            &b,
            &[EdgeBlend {
                edge: top_front,
                kind,
            }],
        )
        .unwrap()
        .0
    };
    let filleted = edged(BlendKind::Fillet { radius: 0.1 });
    let chamfered = edged(BlendKind::Chamfer(Chamfer::Equal { distance: 0.1 }));
    let rounded = every_edge(BlendKind::Fillet { radius: 0.1 });
    let block = 1.0 * 0.6 * 0.4;
    vec![
        Case {
            name: "a through hole",
            delete: not_planes(&drilled),
            body: drilled,
            heal: Heal::Auto,
            occt: occt::Defeature::Cylinders,
            exact: 0.2,
        },
        Case {
            name: "a boss",
            delete: boss_faces.clone(),
            body: bossed,
            heal: Heal::Auto,
            occt: occt::Defeature::Nearest(vec![
                Point3::new(0.6, 0.5, 0.275),
                Point3::new(0.5, 0.5, 0.35),
            ]),
            exact: 0.2,
        },
        Case {
            name: "an edge's fillet",
            delete: not_planes(&filleted),
            body: filleted,
            heal: Heal::Auto,
            occt: occt::Defeature::NotPlanes,
            exact: block,
        },
        Case {
            name: "an edge's chamfer",
            delete: faces_on(&chamfered, |s, b, f| {
                matches!(s, Surface::Plane(p) if {
                    let n = p.frame().z().get();
                    let n = if b.face(f).unwrap().sense == ryukei::topo::Sense::Forward { n } else { -n };
                    n.y < -0.1 && n.z > 0.1
                })
            }),
            body: chamfered,
            heal: Heal::Auto,
            occt: occt::Defeature::Nearest(vec![Point3::new(0.5, 0.05, 0.35)]),
            exact: block,
        },
        Case {
            name: "a rounded box's every blend",
            delete: not_planes(&rounded),
            body: rounded,
            heal: Heal::Auto,
            occt: occt::Defeature::NotPlanes,
            exact: block,
        },
    ]
}

/// Each case's closed form held by ryukei, OCCT's volume printed beside
/// it.
#[test]
fn deletes_against_occt() {
    let dir = tempfile::tempdir().unwrap();
    for c in cases() {
        assert!(!c.delete.is_empty(), "{}: no faces", c.name);
        let t = Instant::now();
        let (o, _) = delete_faces(
            D,
            &c.body,
            &DeleteSpec {
                faces: c.delete.clone(),
                heal: c.heal,
            },
        )
        .unwrap_or_else(|e| panic!("{}: {e:?}", c.name));
        let ryukei_s = t.elapsed().as_secs_f64();
        let (v, slack) = ryukei_volume(&o, 1e-4);
        assert!(
            (v - c.exact).abs() <= slack + 1e-9,
            "{}: ryukei {v} against {}",
            c.name,
            c.exact
        );
        // OCCT reads the file in millimetres: points go in scaled, and
        // its volume comes back in mm³.
        let solid = to_occt(dir.path(), "case", &c.body);
        let which = match &c.occt {
            occt::Defeature::Nearest(ps) => occt::Defeature::Nearest(
                ps.iter()
                    .map(|p| Point3::new(p.x * MM, p.y * MM, p.z * MM))
                    .collect(),
            ),
            w => w.clone(),
        };
        match solid.defeature(&which) {
            Ok((d, occt_s)) => println!(
                "{}: exact {:.9}, ryukei {v:.9} in {ryukei_s:.3} s, OCCT {:.9} in {occt_s:.3} s",
                c.name,
                c.exact,
                d.volume() / (MM * MM * MM)
            ),
            Err(e) => println!(
                "{}: exact {:.9}, ryukei {v:.9} in {ryukei_s:.3} s, OCCT refused: {e}",
                c.name, c.exact
            ),
        }
    }
}

/// A `cols` × `rows` plate 0.5 thick with a hole of radius 0.3 in each
/// unit square, made by both kernels.
fn plate(cols: u32, rows: u32) -> (Body, occt::Solid) {
    let places: Vec<Rigid> = (0..rows)
        .flat_map(|j| (0..cols).map(move |i| at(i as f64 + 0.5, j as f64 + 0.5, -0.25)))
        .collect();
    let (plate, _) =
        primitive::block(K, f64::from(cols), f64::from(rows), 0.5, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0xbe7), 0.3, 1.0, &Rigid::IDENTITY).unwrap();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, &place)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &pin,
            place,
        })
        .collect();
    let (body, _) = boolean(OpKey(0xbe8), BooleanOp::Subtract, &plate, &tools).unwrap();
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

/// Design 399's bar: every hole of the race plate deleted on one thread
/// faster than OCCT's `BRepAlgoAPI_Defeaturing` (its time taken in C++
/// around adding the faces and Build). OCCT's defeaturing grows faster
/// than the square of the holes (cox-worker-4: 25 holes 3.5 s, 100 holes
/// 72 s, 200 holes 415 s, 1,000 past an hour), so OCCT is timed on a
/// plate of 100 holes, a lower bound on its time for 1,000, and ryukei
/// must delete all 1,000 in less. Asserted as it stands with
/// `RYUKEI_BARS=1`, ten times looser otherwise. Where OCCT refuses, or
/// its volume misses its plate's, the bar isn't judged and the test says
/// so.
#[test]
fn deleting_1000_holes_beats_occt_on_one_thread() {
    let (body, _) = plate(40, 25);
    let walls = not_planes(&body);
    assert_eq!(walls.len(), 1000);
    let spec = DeleteSpec {
        faces: walls,
        heal: Heal::Auto,
    };
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = Instant::now();
    let (o, _) = one
        .install(|| delete_faces(OpKey(0xbe9), &body, &spec))
        .expect("ryukei's delete");
    let ryukei_s = t.elapsed().as_secs_f64();
    let exact = 40.0 * 25.0 * 0.5;
    assert_eq!(o.faces().count(), 6);
    let (v, slack) = ryukei_volume(&o, 1e-3);
    assert!(
        (v - exact).abs() <= slack + 1e-9,
        "ryukei {v} against {exact}"
    );
    let t = Instant::now();
    let all = rayon::ThreadPoolBuilder::new()
        .build()
        .unwrap()
        .install(|| delete_faces(OpKey(0xbe9), &body, &spec))
        .expect("ryukei's delete");
    let parallel_s = t.elapsed().as_secs_f64();
    assert_eq!(all.0.dump(), o.dump());
    println!("ryukei: 1,000 holes in {ryukei_s:.3} s on one thread, {parallel_s:.3} s on all");
    let (_, solid) = plate(10, 10);
    let bars = std::env::var_os("RYUKEI_BARS").is_some();
    match solid.defeature(&occt::Defeature::Cylinders) {
        Ok((d, occt_s)) => {
            let ov = d.volume();
            let small = 10.0 * 10.0 * 0.5;
            let agrees = (ov - small).abs() <= 1e-6 * small;
            println!(
                "OCCT: 100 holes in {occt_s:.3} s ({:.4} of it for ryukei's 1,000), its volume {ov:.9} against {small:.9} ({})",
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
        Err(e) => println!("OCCT refused 100 holes ({e}): the bar isn't judged"),
    }
}

/// How each kernel's time grows with the holes deleted (printed only):
/// OCCT's defeaturing of the 1,000-hole plate ran past an hour on
/// cox-worker-4 while 13.D was planned.
#[test]
#[ignore = "by hand: prints how both kernels scale"]
fn deleting_holes_scales() {
    for (cols, rows) in [(5, 5), (10, 10), (20, 10)] {
        let (body, solid) = plate(cols, rows);
        let spec = DeleteSpec {
            faces: not_planes(&body),
            heal: Heal::Auto,
        };
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let t = Instant::now();
        one.install(|| delete_faces(OpKey(0xbe9), &body, &spec))
            .unwrap();
        let ryukei_s = t.elapsed().as_secs_f64();
        let occt = solid
            .defeature(&occt::Defeature::Cylinders)
            .map(|(_, s)| format!("{s:.3} s"))
            .unwrap_or_else(|e| e);
        println!("{} holes: ryukei {ryukei_s:.3} s, OCCT {occt}", cols * rows);
    }
}
