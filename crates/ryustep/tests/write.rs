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
