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
                    },
                    Child {
                        product: 1,
                        place: Rigid::translation(Vector3::new(5.0, 0.0, 0.0)),
                    },
                    Child {
                        product: 2,
                        place: Rigid::rotation(
                            Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(),
                            1.0,
                        ),
                    },
                ],
            },
            Product {
                id: "block".into(),
                name: "block".into(),
                bodies: vec![block],
                children: vec![],
            },
            Product {
                id: "pin".into(),
                name: "pin".into(),
                bodies: vec![pin],
                children: vec![],
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
