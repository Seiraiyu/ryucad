//! Bodies written and read back: the same faces, edges and vertices on
//! the same geometry, and from the first reading on, the same bytes and
//! bodies however often they go round.

use ryukei::check::check;
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryustep::schema::Schema;
use ryustep::{Child, LengthUnit, Product, ReadOptions, WriteModel, WriteOptions, read, write};

fn opts(unit: LengthUnit) -> WriteOptions {
    WriteOptions {
        unit,
        file_name: "test".into(),
        time_stamp: "2026-10-22T00:00:00".into(),
        ..WriteOptions::default()
    }
}

fn one(b: &Body, o: &WriteOptions) -> Vec<u8> {
    write(
        &WriteModel {
            products: vec![Product {
                id: "part".into(),
                name: "part".into(),
                bodies: vec![b.clone()],
                children: vec![],
                ..Default::default()
            }],
        },
        o,
    )
    .unwrap()
}

fn back(bytes: &[u8]) -> Body {
    let m = read(
        bytes,
        &ReadOptions {
            key: OpKey(5),
            heal: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(m.bodies.len(), 1);
    let b = m.bodies[0].result.clone().unwrap().0;
    assert_eq!(check(&b), []);
    b
}

/// Each face's surface (with its sense), each edge's curve and each
/// vertex's point, as the dump gives their bits, sorted.
fn geometry(b: &Body) -> Vec<String> {
    let dump = b.dump();
    let lines: Vec<&str> = dump.lines().collect();
    let entry = |kind: &str, id: &str| {
        let head = format!("{kind} {id} ");
        lines
            .iter()
            .find_map(|l| l.strip_prefix(&head))
            .unwrap_or_default()
            .to_string()
    };
    let field = |l: &str, key: &str| {
        l.split(' ')
            .find_map(|w| w.strip_prefix(key))
            .unwrap_or_default()
            .to_string()
    };
    let mut out: Vec<String> = lines
        .iter()
        .filter_map(|l| {
            if l.starts_with("face ") {
                Some(format!(
                    "face {} {}",
                    entry("surface", &field(l, "surface=")),
                    field(l, "sense=")
                ))
            } else if l.starts_with("edge ") {
                Some(format!("edge {}", entry("curve", &field(l, "curve="))))
            } else if l.starts_with("vertex ") {
                Some(format!("vertex {}", field(l, "point=")))
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

fn tilted() -> Rigid {
    Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7)
        .then_after(&Rigid::translation(Vector3::new(0.1, -0.2, 0.3)))
}

fn primitives() -> Vec<Body> {
    let (k, t) = (OpKey(1), tilted());
    vec![
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
        primitive::frustum(k, 1.0, 0.4, 2.0, &t).unwrap().0,
        primitive::sphere(k, 1.0, &t).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
    ]
}

#[test]
fn primitives_come_back_the_same_and_then_bit_for_bit() {
    let o = opts(LengthUnit::Meter);
    for b in primitives() {
        let w1 = one(&b, &o);
        let b1 = back(&w1);
        assert_eq!(b1.counts(), b.counts());
        assert_eq!(geometry(&b1), geometry(&b));
        let w2 = one(&b1, &o);
        let b2 = back(&w2);
        assert_eq!(b2.dump(), b1.dump());
        assert_eq!(one(&b2, &o), w2);
    }
}

#[test]
fn millimetres_come_back_within_an_ulp() {
    let o = opts(LengthUnit::Millimeter);
    for b in primitives() {
        let b1 = back(&one(&b, &o));
        assert_eq!(b1.counts(), b.counts());
        // Every vertex comes back within a few ulps of where it was.
        let mut near = 0;
        for v in b.vertices() {
            let p = b.vertex(v).unwrap().point;
            if b1
                .vertices()
                .any(|w| b1.vertex(w).unwrap().point.distance(p) <= 4.0 * f64::EPSILON * 10.0)
            {
                near += 1;
            }
        }
        assert_eq!(near, b.counts().vertices);
    }
}

#[test]
fn ap214_names_its_schema() {
    let b = &primitives()[0];
    let o = WriteOptions {
        schema: Schema::Ap214,
        ..opts(LengthUnit::Meter)
    };
    let bytes = one(b, &o);
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("AUTOMOTIVE_DESIGN"));
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(5),
            heal: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(m.schema, Schema::Ap214);
}

#[test]
fn an_assembly_keeps_its_tree() {
    let block = primitives()[0].clone();
    let pin = primitives()[1].clone();
    let model = WriteModel {
        products: vec![
            Product {
                id: "asm".into(),
                name: "assembly".into(),
                bodies: vec![],
                children: vec![
                    Child {
                        product: 1,
                        place: Rigid::IDENTITY,
                        id: String::new(),
                    },
                    Child {
                        product: 1,
                        place: Rigid::translation(Vector3::new(5.0, 0.0, 0.0)),
                        id: String::new(),
                    },
                    Child {
                        product: 2,
                        place: Rigid::rotation(
                            Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(),
                            1.0,
                        ),
                        id: String::new(),
                    },
                ],
                ..Default::default()
            },
            Product {
                id: "block".into(),
                name: "block".into(),
                bodies: vec![block],
                children: vec![],
                ..Default::default()
            },
            Product {
                id: "pin".into(),
                name: "pin".into(),
                bodies: vec![pin],
                children: vec![],
                ..Default::default()
            },
        ],
    };
    let bytes = write(&model, &opts(LengthUnit::Millimeter)).unwrap();
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(5),
            heal: Default::default(),
        },
    )
    .unwrap();
    let p = &m.products;
    assert_eq!(p.definitions.len(), 3);
    assert_eq!(p.roots.len(), 1);
    let root = &p.definitions[p.roots[0]];
    assert_eq!(&*root.name, "assembly");
    assert_eq!(root.children.len(), 3);
    assert!(root.children.iter().all(|c| c.transform.is_some()));
    // Each child where it was put, read back in meters.
    let o = ryustep::open(&bytes).unwrap();
    for (k, c) in model.products[0].children.iter().enumerate() {
        let r = o.placement(p.roots[0], k).unwrap();
        let (a, b) = (r.rotation_matrix(), c.place.rotation_matrix());
        for i in 0..3 {
            for j in 0..3 {
                assert!((a[i][j] - b[i][j]).abs() < 1e-15, "{k}: {a:?} {b:?}");
            }
        }
        let d = r.translation_part() - c.place.translation_part();
        assert!(d.length() < 1e-15, "{k}: {d:?}");
    }
    assert_eq!(m.bodies.len(), 2);
    assert!(
        m.bodies
            .iter()
            .all(|b| b.result.is_ok() && b.definition.is_some())
    );
    assert!(p.faults.is_empty(), "{:?}", p.faults);
}

#[test]
fn products_keep_their_descriptions_revisions_and_usage_ids() {
    // Two uses of one block with ids of their own, then one with none,
    // which takes its place among its parent's.
    let block = primitives()[0].clone();
    let child = |place: Rigid, id: &str| Child {
        product: 1,
        place,
        id: id.into(),
    };
    let model = WriteModel {
        products: vec![
            Product {
                id: "ASM-1".into(),
                name: "Frame".into(),
                description: "A welded frame".into(),
                revision: "B".into(),
                bodies: vec![],
                children: vec![
                    child(Rigid::IDENTITY, "left"),
                    child(Rigid::translation(Vector3::new(5.0, 0.0, 0.0)), "s1"),
                    child(Rigid::translation(Vector3::new(9.0, 0.0, 0.0)), ""),
                ],
                ..Default::default()
            },
            Product {
                id: "BLK-1".into(),
                name: "Block".into(),
                bodies: vec![block],
                ..Default::default()
            },
        ],
    };
    let bytes = write(&model, &opts(LengthUnit::Millimeter)).unwrap();
    let p = ryustep::open(&bytes).unwrap().products;
    assert!(p.faults.is_empty(), "{:?}", p.faults);
    let root = &p.definitions[p.roots[0]];
    assert_eq!(
        (
            &*root.product_id,
            &*root.name,
            &*root.description,
            &*root.revision
        ),
        ("ASM-1", "Frame", "A welded frame", "B")
    );
    let ids: Vec<&str> = root.children.iter().map(|c| &*c.id).collect();
    assert_eq!(ids, ["left", "s1", "3"]);
    let block = &p.definitions[root.children[0].definition];
    assert_eq!(
        (&*block.product_id, &*block.description, &*block.revision),
        ("BLK-1", "", "")
    );
}

#[test]
fn bodies_and_lumps_come_back_as_they_were() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    // Two lumps: a block with a slab cut through its middle.
    let block = primitive::block(OpKey(1), 3.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let slab = primitive::block(
        OpKey(2),
        1.0,
        2.0,
        2.0,
        &Rigid::translation(Vector3::new(1.0, -0.5, -0.5)),
    )
    .unwrap()
    .0;
    let two = boolean(
        OpKey(3),
        BooleanOp::Subtract,
        &block,
        &[Tool {
            key: ToolKey(1),
            body: &slab,
            place: Rigid::IDENTITY,
        }],
    )
    .unwrap()
    .0;
    assert_eq!(two.regions().count(), 3);
    // One product with that body and a sphere: two bodies, the first
    // with two solid regions.
    let model = WriteModel {
        products: vec![Product {
            id: "p".into(),
            name: "p".into(),
            bodies: vec![two.clone(), primitives()[4].clone()],
            children: vec![],
            ..Default::default()
        }],
    };
    let m = read(
        &write(&model, &opts(LengthUnit::Meter)).unwrap(),
        &ReadOptions {
            key: OpKey(5),
            heal: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(m.bodies.len(), 2);
    let counts: Vec<_> = m
        .bodies
        .iter()
        .map(|b| b.result.as_ref().unwrap().0.counts())
        .collect();
    assert!(counts.contains(&two.counts()), "{counts:?}");
    assert!(m.bodies.iter().all(|b| b.definition == Some(0)));
    assert_eq!(
        back(&one(&two, &opts(LengthUnit::Meter))).counts(),
        two.counts()
    );
}

#[test]
fn the_same_input_gives_the_same_bytes() {
    let b = &primitives()[4];
    let o = opts(LengthUnit::Meter);
    let one_thread = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(|| one(b, &o));
    assert_eq!(one(b, &o), one_thread);
}

#[test]
fn writing_is_the_same_on_one_thread_and_many() {
    // Bodies are cut open in parallel and numbered in one fixed walk.
    let model = WriteModel {
        products: vec![Product {
            id: "parts".into(),
            name: "parts".into(),
            bodies: primitives(),
            children: vec![],
            ..Default::default()
        }],
    };
    let o = opts(LengthUnit::Meter);
    let many = write(&model, &o).unwrap();
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(|| write(&model, &o).unwrap());
    assert_eq!(one, many);
}

/// The volume a body's mesh encloses.
fn volume(b: &Body) -> f64 {
    let m = ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(1e-3, 0.4)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    ryukei_testkit::mesh::volume_and_area(&m).0
}

/// Sweeps along paths come back with their sweep faces as NURBS within
/// the pipe tolerance, and lofts with their skins as written: the same
/// volume, checked.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn swept_and_lofted_bodies_come_back() {
    use core::f64::consts::TAU;
    use ryukei::geom::{
        Circle2, Curve, Curve2, Frame, Frame2, KnotVector, Line2, NurbsCurve, Point2, Point3,
        Surface,
    };
    use ryukei::sweep::{
        AlongOptions, LoftOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey,
        ValidProfile, along, loft,
    };
    use std::sync::Arc;
    let square = |first: u128, s: f64| {
        let h = s / 2.0;
        let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
        Loop {
            key: LoopKey(1),
            segments: (0..4)
                .map(|i| {
                    let (p, q) = (pts[i], pts[(i + 1) % 4]);
                    let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
                    Segment {
                        key: SegmentKey(first + i as u128),
                        curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                        t0: 0.0,
                        t1: p.distance(q),
                    }
                })
                .collect(),
        }
    };
    let ring = |r: f64| {
        let f = Frame2::new(
            Point2::new(0.0, 0.0),
            ryukei::geom::Vector2::new(1.0, 0.0),
            true,
        )
        .unwrap();
        Loop {
            key: LoopKey(1),
            segments: vec![Segment {
                key: SegmentKey(9),
                curve: Curve2::Circle2(Circle2::new(f, r).unwrap()),
                t0: 0.0,
                t1: TAU,
            }],
        }
    };
    let section = |origin: Point3, z: Vector3, x: Vector3, l: Loop| {
        ValidProfile::new(Profile {
            frame: Frame::new(origin, z, x).unwrap(),
            loops: vec![l],
        })
        .unwrap()
    };
    // A bar along a wavy spline, a tube round a closed one, a frustum.
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let wavy = NurbsCurve::new(
        k,
        vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(2.0, 1.0, 0.5),
            Point3::new(3.0, 0.5, 1.5),
            Point3::new(4.0, 0.0, 2.0),
        ],
        None,
    )
    .unwrap();
    let path = |c: NurbsCurve, range: [f64; 2]| Path {
        segments: vec![PathSegment {
            key: SegmentKey(100),
            curve: Curve::Nurbs(Arc::new(c)),
            range,
        }],
    };
    let x = Vector3::new(1.0, 0.0, 0.0);
    let y = Vector3::new(0.0, 1.0, 0.0);
    let z = Vector3::new(0.0, 0.0, 1.0);
    let (bar, _) = along(
        OpKey(1),
        &section(Point3::ORIGIN, x, y, square(10, 0.3)),
        &path(wavy, [0.0, 2.0]),
        &AlongOptions::default(),
    )
    .unwrap();
    let k = KnotVector::periodic(3, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1; 5]).unwrap();
    let loop_ = NurbsCurve::new(
        k,
        vec![
            Point3::new(-4.0, 0.0, 0.0),
            Point3::new(0.0, -4.0, 0.0),
            Point3::new(4.0, 0.0, 2.0),
            Point3::new(0.0, 5.0, 1.0),
        ],
        None,
    )
    .unwrap();
    let (p0, t0) = (loop_.point(0.0), loop_.eval(0.0).d1);
    let (tube, _) = along(
        OpKey(2),
        &section(p0, t0, z.cross(t0), ring(0.2)),
        &path(loop_, [0.0, 4.0]),
        &AlongOptions::default(),
    )
    .unwrap();
    let (frustum, _) = loft(
        OpKey(3),
        &[
            section(Point3::ORIGIN, z, x, square(10, 2.0)),
            section(Point3::new(0.0, 0.0, 1.0), z, x, square(20, 1.0)),
        ],
        &LoftOptions::default(),
    )
    .unwrap();
    let swept = bar.clone();
    // The tube comes back as one face with no loops on a NURBS periodic
    // both ways.
    for b in [tube, bar, frustum] {
        let read = back(&one(&b, &opts(LengthUnit::Meter)));
        assert_eq!(read.faces().count(), b.faces().count());
        assert!(read.faces().all(|f| !matches!(
            read.surface(read.face(f).unwrap().surface),
            Some(Surface::Sweep(_))
        )));
        let (v, w) = (volume(&b), volume(&read));
        assert!((v - w).abs() <= 1e-3 * v, "{v} against {w}");
    }
    // Its sweep faces are written as NURBS within the pipe tolerance:
    // the report says which and the bound used.
    let w = ryustep::write_report(
        &WriteModel {
            products: vec![Product {
                id: "swept".into(),
                bodies: vec![swept.clone()],
                ..Default::default()
            }],
        },
        &[],
        &opts(LengthUnit::Meter),
    )
    .unwrap();
    assert!(!w.approximated.is_empty(), "a sweep's faces are NURBS");
    for a in &w.approximated {
        assert_eq!(a.product, 0);
        assert!(a.faces > 0 && a.bound > 0.0, "{a:?}");
        assert!(a.bound <= WriteOptions::default().pipe_tolerance, "{a:?}");
    }
}

/// A spline block whose top face lies on an offset of a spline plane: it
/// is written as STEP's `OFFSET_SURFACE` of the spline and reads back an
/// offset, the same distance off the same spline, enclosing the same
/// volume.
#[test]
fn offsets_are_written_exactly() {
    use ryukei::geom::{Offset, Surface};
    use ryukei::name::FaceRole;
    use std::sync::Arc;
    let k = OpKey(1);
    let mut b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    ryukei_testkit::nurbs::convert_body(&mut b).unwrap();
    let top = ryukei::name::Name::derive(k, ryukei::name::Role::Face(FaceRole::PlusZ), &[]);
    let Some(ryukei::topo::EntityId::Face(f)) = b.by_name(top) else {
        panic!()
    };
    let s = b.surface(b.face(f).unwrap().surface).unwrap().clone();
    let [du, dv] = s.domain();
    let n = s
        .normal((du[0] + du[1]) / 2.0, (dv[0] + dv[1]) / 2.0)
        .unwrap()
        .get();
    // The spline moved a tenth back against its normal, and offset a tenth
    // along it: the same plane, on the same parameters.
    let base = s.transformed(&Rigid::translation(n * -0.1));
    let o = Offset::new(base, 0.1, s.domain()).unwrap();
    let id = b.add_surface(Surface::Offset(Arc::new(o)));
    // The face keeps its pcurves: the offset is on its spline's parameters.
    let fins: Vec<_> = b
        .face_loops(f)
        .flat_map(|l| b.loop_fins(l).collect::<Vec<_>>())
        .map(|fin| (fin, b.fin(fin).unwrap().pcurve))
        .collect();
    let sense = b.face(f).unwrap().sense;
    ryukei::euler::set_surface(&mut b, f, id, sense).unwrap();
    for (fin, pc) in fins {
        ryukei::euler::set_pcurve(&mut b, fin, pc).unwrap();
    }
    assert_eq!(check(&b), []);
    let bytes = one(&b, &opts(LengthUnit::Millimeter));
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(text.matches("OFFSET_SURFACE").count(), 1);
    let r = back(&bytes);
    let offsets: Vec<f64> = r
        .faces()
        .filter_map(|g| match r.surface(r.face(g).unwrap().surface) {
            Some(Surface::Offset(o)) => Some(o.distance()),
            _ => None,
        })
        .collect();
    assert_eq!(offsets.len(), 1);
    assert!((offsets[0] - 0.1).abs() < 1e-12, "{offsets:?}");
    let m = ryukei::mesh::tessellate(&r, &ryukei::mesh::MeshParams::new(1e-4, 0.2)).unwrap();
    let v = ryukei_testkit::mesh::volume_and_area(&m).0;
    assert!((v - 6.0).abs() < 1e-6, "{v}");
}

/// Each periodic face of `b` (a NURBS form of a primitive) moved onto an
/// offset by `d` of the same primitive's NURBS form `inner`, a distance
/// `d` inside it on the same parameters: the same surface, its pcurves
/// kept.
fn offset_periodic_faces(b: &mut Body, inner: &Body, d: f64) {
    use ryukei::geom::{Offset, Surface};
    use std::sync::Arc;
    let faces: Vec<_> = b.faces().collect();
    for f in faces {
        let s = b.surface(b.face(f).unwrap().surface).unwrap().clone();
        let p = s.periods();
        if p.0.is_none() && p.1.is_none() {
            continue;
        }
        let base = inner
            .faces()
            .map(|g| inner.surface(inner.face(g).unwrap().surface).unwrap())
            .find(|t| t.periods() == p)
            .unwrap()
            .clone();
        let o = Offset::new(base, d, s.domain()).unwrap();
        let id = b.add_surface(Surface::Offset(Arc::new(o)));
        let fins: Vec<_> = b
            .face_loops(f)
            .flat_map(|l| b.loop_fins(l).collect::<Vec<_>>())
            .map(|fin| (fin, b.fin(fin).unwrap().pcurve))
            .collect();
        let sense = b.face(f).unwrap().sense;
        ryukei::euler::set_surface(b, f, id, sense).unwrap();
        for (fin, pc) in fins {
            ryukei::euler::set_pcurve(b, fin, pc).unwrap();
        }
    }
}

/// Offset faces that wrap a periodic base get seams fitted along the
/// offset's parameter lines: a spline cylinder's side (one seam) and a
/// spline torus (two seams through one vertex) are written as
/// `OFFSET_SURFACE`s and read back offsets enclosing the same volume.
#[test]
fn offsets_that_wrap_are_written() {
    use ryukei::geom::Surface;
    let k = OpKey(1);
    let cases: Vec<(Body, Body, f64)> = vec![
        (
            primitive::cylinder(k, 1.0, 2.0, &Rigid::IDENTITY)
                .unwrap()
                .0,
            primitive::cylinder(k, 0.9, 2.0, &Rigid::IDENTITY)
                .unwrap()
                .0,
            2.0 * std::f64::consts::PI,
        ),
        (
            primitive::torus(k, 2.0, 0.5, &Rigid::IDENTITY).unwrap().0,
            primitive::torus(k, 2.0, 0.4, &Rigid::IDENTITY).unwrap().0,
            2.0 * std::f64::consts::PI * std::f64::consts::PI * 2.0 * 0.25,
        ),
    ];
    for (mut b, mut inner, volume) in cases {
        ryukei_testkit::nurbs::convert_body(&mut b).unwrap();
        ryukei_testkit::nurbs::convert_body(&mut inner).unwrap();
        offset_periodic_faces(&mut b, &inner, 0.1);
        assert_eq!(check(&b), []);
        let bytes = one(&b, &opts(LengthUnit::Millimeter));
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text.matches("OFFSET_SURFACE").count(), 1);
        let r = back(&bytes);
        let offsets = r
            .faces()
            .filter(|&g| {
                matches!(
                    r.surface(r.face(g).unwrap().surface),
                    Some(Surface::Offset(_))
                )
            })
            .count();
        assert_eq!(offsets, 1);
        // Within the mesh's certified chord of the solid all over.
        let chord = 1e-3;
        let m = ryukei::mesh::tessellate(&r, &ryukei::mesh::MeshParams::new(chord, 0.3)).unwrap();
        let (v, area) = ryukei_testkit::mesh::volume_and_area(&m);
        assert!(
            (v - volume).abs() <= 2.0 * chord * area,
            "{v} against {volume}"
        );
    }
}

/// Sheets of the primitives of more than one face: each with a face taken away (an open sheet
/// with laminar edges), and the block whole (a closed sheet).
fn sheets() -> Vec<Body> {
    let mut out: Vec<Body> = primitives()
        .iter()
        .filter(|b| b.faces().count() > 1)
        .map(|b| {
            let first = b.faces().next().unwrap();
            ryukei::sheet::remove(OpKey(2), b, &[b.face(first).unwrap().name])
                .unwrap()
                .0
        })
        .collect();
    out.push(
        ryukei::sheet::remove(OpKey(2), &primitives()[0], &[])
            .unwrap()
            .0,
    );
    out
}

fn back_sheet(bytes: &[u8]) -> Body {
    let m = read(
        bytes,
        &ReadOptions {
            key: OpKey(5),
            heal: ryukei::heal::HealOptions {
                open: ryukei::heal::Open::Sheet,
                ..Default::default()
            },
        },
    )
    .unwrap();
    assert_eq!(m.bodies.len(), 1);
    let b = m.bodies[0].result.clone().unwrap().0;
    assert_eq!(check(&b), []);
    assert_eq!(b.kind(), ryukei::topo::BodyKind::Sheet);
    b
}

#[test]
fn sheets_come_back_the_same_and_then_bit_for_bit() {
    let o = opts(LengthUnit::Meter);
    for s in sheets() {
        let w1 = one(&s, &o);
        let text = String::from_utf8(w1.clone()).unwrap();
        assert!(text.contains("MANIFOLD_SURFACE_SHAPE_REPRESENTATION"));
        assert!(text.contains("SHELL_BASED_SURFACE_MODEL"));
        let laminar = |b: &Body| b.edges().filter(|&e| b.is_laminar(e)).count();
        assert_eq!(text.contains("OPEN_SHELL"), laminar(&s) > 0);
        let s1 = back_sheet(&w1);
        assert_eq!(s1.counts(), s.counts());
        assert_eq!(laminar(&s1), laminar(&s));
        assert_eq!(geometry(&s1), geometry(&s));
        let w2 = one(&s1, &o);
        let s2 = back_sheet(&w2);
        assert_eq!(s2.dump(), s1.dump());
        assert_eq!(one(&s2, &o), w2);
        // Read without asking for sheets, the surface model is refused.
        let m = read(
            &w1,
            &ReadOptions {
                key: OpKey(5),
                heal: Default::default(),
            },
        )
        .unwrap();
        if laminar(&s) > 0 {
            assert!(m.bodies[0].result.is_err());
        }
    }
}

/// A bracket's flat drilled across its bend line and refolded: its bent
/// hole wall goes out as its NURBS form within the pipe tolerance and
/// comes back a body that checks, of the same volume, recognized as sheet
/// metal with the NURBS wall accepted as radial (within a hundredth of the
/// thickness).
#[test]
fn a_refolded_body_with_a_bent_wall_comes_back() {
    use ryukei::boolean::{BooleanOp, Tool, boolean};
    use ryukei::geom::{Frame, Surface};
    use ryukei::name::ToolKey;
    use ryukei::sheet_metal::{
        Allowance, RefoldSpec, SheetOptions, UnfoldSpec, recognize, refold, unfold,
    };
    use ryukei_testkit::sheet_metal::{Leg, strip};
    let legs = [
        Leg::Run(0.02),
        Leg::Turn {
            angle: core::f64::consts::FRAC_PI_2,
            radius: 0.002,
        },
        Leg::Run(0.015),
    ];
    let b = strip(OpKey(11), &legs, 0.001, 0.012);
    let m = recognize(&b, &SheetOptions::default()).unwrap();
    let u = unfold(OpKey(11), &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.5))).unwrap();
    let s = &u.map.strips[0];
    let at = s.frame.to_world(0.0, s.span[0] + 0.006, -0.001);
    let place = Rigid::from_frame(&Frame::new(at, s.frame.z().get(), s.frame.x().get()).unwrap());
    let drill = primitive::cylinder(OpKey(99), 0.0008, 0.003, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (flat, _) = boolean(
        OpKey(98),
        BooleanOp::Subtract,
        &u.body,
        &[Tool {
            key: ToolKey(1),
            body: &drill,
            place,
        }],
    )
    .unwrap();
    let (folded, _) = refold(OpKey(97), &flat, &u.map, &RefoldSpec::default()).unwrap();
    assert!(folded.faces().any(|f| matches!(
        folded.surface(folded.face(f).unwrap().surface).unwrap(),
        Surface::Bent(_)
    )));
    let bytes = one(&folded, &opts(LengthUnit::Millimeter));
    let read = back(&bytes);
    let mass = |b: &Body| {
        ryukei::measure::mass(b, 1.0, &ryukei::measure::MassOptions::default())
            .unwrap()
            .volume
    };
    let (a, c) = (mass(&folded), mass(&read));
    // The wall strays at most the pipe tolerance over its area.
    let slack = 1e-7 * 1e-5;
    assert!(
        (a.value() - c.value()).abs() <= a.bound() + c.bound() + slack,
        "{a:?} against {c:?}"
    );
    assert_eq!(
        recognize(&folded, &SheetOptions::default())
            .unwrap()
            .bends
            .len(),
        1
    );
    // The NURBS wall is within the pipe tolerance of radial; the cells
    // certify that at a hundredth of the thickness.
    let again = recognize(&read, &SheetOptions { tolerance: 1e-5 }).unwrap();
    assert_eq!(again.bends.len(), 1);
}
/// A NURBS body's periodic curves and surfaces come back periodic. ryukei
/// writes a periodic spline with its first `degree` control points again,
/// whatever its joints' multiplicity: a NURBS circle's quarter knots are
/// double, and #325's reading rule (OCCT's `degree + 1 - m`) alone read a
/// NURBS cylinder's rims back clamped, so its own export didn't re-import
/// as it was written (issue #403).
#[test]
fn periodic_splines_with_double_joints_come_back_periodic() {
    use ryukei::geom::{Curve, Surface};
    let kinds = |b: &Body| {
        let mut v: Vec<String> = b
            .edges()
            .map(|e| match b.curve(b.edge(e).unwrap().curve).unwrap() {
                Curve::Nurbs(n) => format!("curve {}", n.knots().is_periodic()),
                c => format!("{c:?}"),
            })
            .collect();
        v.extend(
            b.faces()
                .map(|f| match b.surface(b.face(f).unwrap().surface).unwrap() {
                    Surface::Nurbs(n) => format!(
                        "surface {} {}",
                        n.knots()[0].is_periodic(),
                        n.knots()[1].is_periodic()
                    ),
                    s => format!("{s:?}"),
                }),
        );
        v.sort();
        v
    };
    let k = OpKey(1);
    for mut b in [
        primitive::cylinder(k, 1.0, 2.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
        primitive::torus(k, 2.0, 0.5, &Rigid::IDENTITY).unwrap().0,
        primitive::sphere(k, 1.0, &Rigid::IDENTITY).unwrap().0,
    ] {
        ryukei_testkit::nurbs::convert_body(&mut b).unwrap();
        for unit in [LengthUnit::Meter, LengthUnit::Millimeter] {
            let r = back(&one(&b, &opts(unit)));
            assert_eq!(kinds(&r), kinds(&b), "{unit:?}");
        }
    }
}

/// Each body's name is its solids' and, with several, its
/// representation's, and reads back as the held body's name; a body
/// without one is unnamed (#412).
#[test]
fn bodies_keep_their_names() {
    let ps = primitives();
    for count in [1, 3] {
        let names = ["base", "boss"];
        let model = WriteModel {
            products: vec![Product {
                id: "p".into(),
                name: "p".into(),
                bodies: ps[..count].to_vec(),
                names: names.iter().map(|n| n.to_string()).collect(),
                ..Default::default()
            }],
        };
        let bytes = write(&model, &opts(LengthUnit::Millimeter)).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("MANIFOLD_SOLID_BREP('base',"), "{count}");
        assert_eq!(text.contains("MANIFOLD_SOLID_BREP('boss',"), count > 1);
        let opened = ryustep::open(&bytes).unwrap();
        let got: Vec<&str> = opened.held.iter().map(|h| h.name.as_str()).collect();
        let want: Vec<&str> = (0..count)
            .map(|k| names.get(k).copied().unwrap_or(""))
            .collect();
        assert_eq!(got, want);
    }
}

/// Every body the committed corpus heals, written in millimetres, read
/// back, then written in meters and read back again: from the first
/// meters file on, the bytes are the same (step-hunt's `fixpoint`
/// stage). A tolerant edge's file pcurve misses its vertices by the
/// edge's own gap, and healing pinned it to its parameters nearest the
/// edge's ends by projection: a map a few ulps from the identity, which
/// writing folded into the knots, so the next reading found another and
/// the knots never settled (NIST CTC 05 here, STC 07 in the pinned
/// corpus, issue #402).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn the_corpus_comes_back_to_a_fixed_point() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "stp"))
        .collect();
    files.sort();
    let mut tried = 0;
    let mut moved = Vec::new();
    for path in files {
        let o = ryustep::open(&std::fs::read(&path).unwrap()).unwrap();
        for k in 0..o.held.len() {
            let options = ReadOptions {
                key: OpKey(7),
                heal: Default::default(),
            };
            let Ok((b, _, _)) = o.body(k, &options).result else {
                continue;
            };
            tried += 1;
            let mm = one(&b, &opts(LengthUnit::Millimeter));
            let first = one(&back(&mm), &opts(LengthUnit::Meter));
            let again = one(&back(&first), &opts(LengthUnit::Meter));
            if first != again {
                moved.push(format!("{} body {k}", path.display()));
            }
        }
    }
    assert!(tried > 10, "{tried}");
    assert!(moved.is_empty(), "not a fixed point: {moved:#?}");
}

/// A body whose pcurves on cylinders and planes are fitted splines (a
/// cross bore's) goes round a millimetre file to a fixed point: a pcurve
/// coordinate that is a length is written as a 3D point's is, divided by
/// the unit, so reading (times the unit) and writing again settle as
/// points do. Multiplied by the unit's inverse, 440 control points of
/// the Switchwire's threaded insert moved an ulp or two every time the
/// file went round (issue #495).
#[test]
fn fitted_pcurves_come_back_to_a_fixed_point_in_millimetres() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let shaft = primitive::cylinder(OpKey(1), 0.0125, 0.06, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let bore = primitive::cylinder(OpKey(2), 0.0043, 0.05, &Rigid::IDENTITY)
        .unwrap()
        .0;
    // Through the shaft's middle, askew to its axis.
    let place = Rigid::translation(Vector3::new(0.0017, -0.0021, 0.0313)).then_after(
        &Rigid::rotation(Vector3::new(1.0, 0.2, 0.0).normalize().unwrap(), 1.3)
            .then_after(&Rigid::translation(Vector3::new(0.0, 0.0, -0.025))),
    );
    let drilled = boolean(
        OpKey(3),
        BooleanOp::Subtract,
        &shaft,
        &[Tool {
            key: ToolKey(1),
            body: &bore,
            place,
        }],
    )
    .unwrap()
    .0;
    let o = opts(LengthUnit::Millimeter);
    let first = one(&back(&one(&drilled, &o)), &o);
    let again = one(&back(&first), &o);
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let moved: Vec<(String, String)> = text(&first)
        .lines()
        .zip(text(&again).lines())
        .filter(|(a, b)| a != b)
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
        .take(4)
        .collect();
    assert_eq!(drilled.counts(), back(&first).counts());
    assert!(
        text(&first).contains("B_SPLINE_CURVE_WITH_KNOTS"),
        "{:?}",
        drilled.counts()
    );
    assert_eq!(moved, Vec::<(String, String)>::new());
}

#[test]
fn a_passed_representation_reads_back_as_its_products_body() {
    let path = format!(
        "{}/tests/data/occt/box_ap214.stp",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(path).unwrap();
    let file = ryustep::part21::parse(&bytes).unwrap();
    let opened = ryustep::open(&bytes).unwrap();
    let h = &opened.held[0];
    let cut =
        ryustep::part21::closure::cut(&file, h.representation, h.context, &h.items, "r").unwrap();
    let model = WriteModel {
        products: vec![Product {
            id: "p".into(),
            name: "p".into(),
            ..Default::default()
        }],
    };
    let o = opts(LengthUnit::Millimeter);
    let passed = |instances| ryustep::Passed {
        product: 0,
        instances,
    };
    let out = ryustep::write_with(&model, &[passed(cut)], &o).unwrap();
    let again = ryustep::open(&out).unwrap();
    assert_eq!(again.held.len(), 1);
    assert_eq!(again.held[0].definition, Some(0));
    // The same reference, named the same, under one key.
    let ro = ReadOptions {
        key: OpKey::from_bytes(b"k"),
        heal: Default::default(),
    };
    let a = opened.reference(0, &ro).result.unwrap().0;
    let b = again.reference(0, &ro).result.unwrap().0;
    assert_eq!(a.dump(), b.dump());
    // Written again, the same bytes.
    let file2 = ryustep::part21::parse(&out).unwrap();
    let h2 = &again.held[0];
    let cut2 = ryustep::part21::closure::cut(&file2, h2.representation, h2.context, &h2.items, "r")
        .unwrap();
    let out2 = ryustep::write_with(&model, &[passed(cut2)], &o).unwrap();
    assert_eq!(out, out2);
}

/// `write_report` writes `write`'s bytes, and counts every instance it
/// wrote by category and entity name (design 448).
#[test]
fn a_report_counts_what_was_written() {
    use ryustep::census::Category;
    let o = opts(LengthUnit::Millimeter);
    let model = WriteModel {
        products: vec![Product {
            id: "part".into(),
            name: "part".into(),
            bodies: primitives()[..2].to_vec(),
            ..Default::default()
        }],
    };
    let w = ryustep::write_report(&model, &[], &o).unwrap();
    assert_eq!(w.bytes, write(&model, &o).unwrap());
    let count = |c: Category, n: &str| {
        w.counts
            .iter()
            .find(|(k, name, _)| *k == c && name == n)
            .map_or(0, |x| x.2)
    };
    assert_eq!(count(Category::ProductStructure, "PRODUCT"), 1);
    assert_eq!(count(Category::Topology, "MANIFOLD_SOLID_BREP"), 2);
    assert_eq!(count(Category::Units, "LENGTH_UNIT NAMED_UNIT SI_UNIT"), 1);
    assert!(
        w.counts.iter().all(|(c, _, _)| *c != Category::Unknown),
        "{:?}",
        w.counts
    );
    let total: u64 = w.counts.iter().map(|x| x.2).sum();
    let file = ryustep::part21::parse(&w.bytes).unwrap();
    assert_eq!(total, file.instances.len() as u64);
    assert!(w.approximated.is_empty(), "primitives are exact");
}
