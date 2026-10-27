//! The OCCT files read into bodies: each solid heals, passes the checker
//! and meshes to its volume; the sheet offset from a spline is refused
//! (offsets of splines come with healing).

use core::f64::consts::PI;
use ryukei::check::check;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryustep::{ImportError, Model, ReadOptions, open, read};

fn file(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/data/occt/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap()
}

fn opts() -> ReadOptions {
    ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    }
}

fn model(name: &str) -> Model {
    read(&file(name), &opts()).unwrap()
}

/// A body's volume in mm³ from its mesh, which must be sound.
fn volume(b: &Body) -> f64 {
    let m = tessellate(b, &MeshParams::new(1e-5, 0.2)).unwrap();
    assert_eq!(m.faults, []);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum::<f64>()
        * 1e9
}

/// The one body of a file, sound, and its volume.
fn one(name: &str) -> (Body, f64) {
    let m = model(name);
    assert_eq!(m.bodies.len(), 1, "{name}");
    let (b, _, _) = m.bodies[0].result.clone().unwrap();
    assert_eq!(check(&b), [], "{name}");
    let v = volume(&b);
    (b, v)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 2e-3 * b
}

#[test]
fn boxes_in_every_schema_and_unit() {
    // OCCT labels the inch file's lengths inches without converting
    // them: its box is 10 by 20 by 30 inches.
    let inch3 = 25.4 * 25.4 * 25.4;
    for (name, size) in [
        ("box_ap203.stp", 6000.0),
        ("box_ap214.stp", 6000.0),
        ("box_ap242.stp", 6000.0),
        ("box_inch.stp", 6000.0 * inch3),
    ] {
        let (b, v) = one(name);
        assert_eq!(b.counts().faces, 6, "{name}");
        assert_eq!(b.counts().edges, 12, "{name}");
        assert!(near(v, size), "{name}: {v}");
    }
}

#[test]
fn turned_and_closed_primitives() {
    let (b, v) = one("cylinder.stp");
    assert_eq!(b.counts().faces, 3);
    assert_eq!(b.counts().edges, 2);
    assert!(near(v, PI * 25.0 * 12.0), "{v}");
    let (b, v) = one("sphere.stp");
    assert_eq!((b.counts().faces, b.counts().loops), (1, 0));
    assert!(near(v, 4.0 / 3.0 * PI * 343.0), "{v}");
    let (b, v) = one("torus.stp");
    assert_eq!((b.counts().faces, b.counts().loops), (1, 0));
    assert!(near(v, 2.0 * PI * PI * 10.0 * 9.0), "{v}");
    let (b, v) = one("cone.stp");
    assert_eq!(b.counts().faces, 3);
    assert!(near(v, PI * 3.0 * (36.0 + 12.0 + 4.0)), "{v}");
}

#[test]
fn splines_holes_and_cavities() {
    let (_, v) = one("nurbs_cylinder.stp");
    assert!(near(v, PI * 25.0 * 12.0), "{v}");
    let (_, v) = one("plate_hole.stp");
    assert!(near(v, 6000.0 - PI * 9.0 * 30.0), "{v}");
    let (_, v) = one("elliptic_prism.stp");
    assert!(near(v, PI * 8.0 * 3.0 * 5.0), "{v}");
    let (b, v) = one("cavity.stp");
    assert_eq!(b.counts().shells, 2);
    assert!(near(v, 1000.0 - 64.0), "{v}");
    let (b, v) = one("spline_revolve.stp");
    assert!(v > 0.0);
    assert!(b.counts().faces >= 4);
}

#[test]
fn a_sheet_offset_from_a_spline_is_refused() {
    let m = model("offset_sheet.stp");
    assert!(!m.bodies.is_empty());
    for b in &m.bodies {
        assert!(
            matches!(b.result, Err(ImportError::NotBounded { .. })),
            "{:?}",
            b.result
        );
    }
}

#[test]
fn an_assemblys_parts_are_read_once_each() {
    let m = model("assembly.stp");
    assert_eq!(m.bodies.len(), 2);
    let mut v: Vec<f64> = m
        .bodies
        .iter()
        .map(|b| volume(&b.result.as_ref().unwrap().0))
        .collect();
    v.sort_by(f64::total_cmp);
    assert!(near(v[0], PI * 25.0 * 12.0) && near(v[1], 6000.0), "{v:?}");
    for b in &m.bodies {
        assert!(b.definition.is_some());
    }
}

#[test]
fn reading_is_the_same_on_one_thread_and_many() {
    let bytes = file("plate_hole.stp");
    let a = read(&bytes, &opts()).unwrap();
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(|| read(&bytes, &opts()).unwrap());
    let dump = |m: &Model| m.bodies[0].result.as_ref().unwrap().0.dump();
    assert_eq!(dump(&a), dump(&one));
}

/// A block 1 by 2 by 3 written as a file, in meters.
fn block_file() -> String {
    use ryukei::geom::Rigid;
    use ryustep::{Product, WriteModel, WriteOptions, write};
    let b = ryukei::primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: "block".into(),
                name: "block".into(),
                bodies: vec![b],
                children: vec![],
                ..Default::default()
            }],
        },
        &WriteOptions::default(),
    )
    .unwrap();
    String::from_utf8(bytes).unwrap()
}

/// The entity name of instance `id` in `text`.
fn kind(text: &str, id: u64) -> String {
    let start = text.find(&format!("\n#{id}=")).unwrap() + format!("\n#{id}=").len();
    text[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

#[test]
fn refusals_name_the_files_instances() {
    let text = block_file();
    // Every import name of a body read comes from a vertex, an edge or a
    // face of the file.
    let m = read(text.as_bytes(), &opts()).unwrap();
    let b = &m.bodies[0];
    assert_eq!(b.instances.len(), 8 + 12 + 6);
    let (body, _, _) = b.result.as_ref().unwrap();
    for e in body.edges() {
        let id = b.instances[&body.edge(e).unwrap().name];
        assert_eq!(kind(&text, id), "EDGE_CURVE");
    }
    // A face left out of the shell: the free edges it leaves are named
    // by their EDGE_CURVEs.
    let at = text.find("CLOSED_SHELL('',(").unwrap() + "CLOSED_SHELL('',(".len();
    let comma = at + text[at..].find(',').unwrap();
    let open = format!("{}{}", &text[..at], &text[comma + 1..]);
    let m = read(open.as_bytes(), &opts()).unwrap();
    let b = &m.bodies[0];
    let Err(ImportError::Heal(ryukei::HealError::OpenShell { free_edges })) = &b.result else {
        panic!("{:?}", b.result)
    };
    assert_eq!(free_edges.len(), 4);
    for n in free_edges {
        assert_eq!(kind(&open, b.instances[n]), "EDGE_CURVE");
    }
}

#[test]
fn representations_holding_one_solid_read_alike() {
    // A second representation of the block's solid, in its context.
    let text = block_file();
    let id_of = |name: &str| -> u64 {
        let at = text.find(&format!("={name}(")).unwrap();
        let start = text[..at].rfind('#').unwrap() + 1;
        text[start..at].parse().unwrap()
    };
    let brep = id_of("MANIFOLD_SOLID_BREP");
    let rep = text.find("=ADVANCED_BREP_SHAPE_REPRESENTATION(").unwrap();
    let line_end = rep + text[rep..].find(';').unwrap();
    let context = &text[text[..line_end].rfind('#').unwrap()..line_end - 1];
    let end = text.rfind("ENDSEC;").unwrap();
    let twice = format!(
        "{}#900000=SHAPE_REPRESENTATION('',(#{brep}),{context});\n{}",
        &text[..end],
        &text[end..]
    );
    let m = read(twice.as_bytes(), &opts()).unwrap();
    assert_eq!(m.bodies.len(), 2);
    let dump = |k: usize| m.bodies[k].result.as_ref().unwrap().0.dump();
    assert_eq!(dump(0), dump(1));
    assert_eq!(m.bodies[0].instances, m.bodies[1].instances);
    assert_eq!(m.bodies[1].representation, 900000);
}

#[test]
fn bodies_opened_one_at_a_time_read_as_read_reads_them() {
    for name in ["assembly.stp", "plate_hole.stp", "spline_revolve.stp"] {
        let bytes = file(name);
        let m = read(&bytes, &opts()).unwrap();
        let o = open(&bytes).unwrap();
        assert_eq!(o.products, m.products, "{name}");
        assert_eq!(o.held.len(), m.bodies.len(), "{name}");
        for (k, b) in m.bodies.iter().enumerate() {
            let one = o.body(k, &opts());
            assert_eq!(one.representation, b.representation, "{name}");
            assert_eq!(one.definition, b.definition, "{name}");
            assert_eq!(one.items, b.items, "{name}");
            assert_eq!(one.instances, b.instances, "{name}");
            let dump = |i: &ryustep::Imported| i.result.as_ref().unwrap().0.dump();
            assert_eq!(dump(&one), dump(b), "{name}");
        }
    }
    // Another key names everything differently (and so lays the body
    // out in another order: healing builds in the order of the names),
    // and builds the same shape.
    let o = open(&file("plate_hole.stp")).unwrap();
    let other = ReadOptions {
        key: OpKey(8),
        ..opts()
    };
    let (a, b) = (o.body(0, &opts()), o.body(0, &other));
    let (a, b) = (&a.result.unwrap().0, &b.result.unwrap().0);
    assert_eq!(a.counts(), b.counts());
    assert!(near(volume(a), volume(b)));
    assert_ne!(a.dump(), b.dump());
}

/// Where each usage puts its child, as a point and the image of x.
fn placed(o: &ryustep::Opened, parent: usize) -> Vec<([f64; 3], [f64; 3])> {
    use ryukei::geom::{Point3, Vector3};
    (0..o.products.definitions[parent].children.len())
        .map(|k| {
            let r = o.placement(parent, k).unwrap();
            let p = r.apply_point(Point3::ORIGIN);
            let x = r.apply_vector(Vector3::new(1.0, 0.0, 0.0));
            (
                [p.x, p.y, p.z].map(|c| (c * 1e12).round() / 1e12),
                [x.x, x.y, x.z].map(|c| (c * 1e12).round() / 1e12),
            )
        })
        .collect()
}

#[test]
fn usages_are_placed_in_meters() {
    // XDE's assembly: the block at its origin and 100 mm along x, the
    // pin 50 mm along y, turned a quarter about z.
    let bytes = file("assembly.stp");
    let o = open(&bytes).unwrap();
    assert_eq!(
        placed(&o, 0),
        [
            ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([0.1, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([0.0, 0.05, 0.0], [0.0, 1.0, 0.0]),
        ]
    );
    // The second block placed by an operator instead, the same way, and
    // related the other way round: the operator is then inverted.
    let text = String::from_utf8(bytes).unwrap();
    let operator = text.replace(
        "#384 = ITEM_DEFINED_TRANSFORMATION('','',#11,#19);",
        "#384 = CARTESIAN_TRANSFORMATION_OPERATOR_3D('',$,$,#900,$,$);\n\
         #900 = CARTESIAN_POINT('',(100.,0.,0.));",
    );
    assert_ne!(operator, text);
    let o = open(operator.as_bytes()).unwrap();
    assert_eq!(placed(&o, 0)[1], ([0.1, 0.0, 0.0], [1.0, 0.0, 0.0]));
    let swapped = operator.replace(
        "#383 = ( REPRESENTATION_RELATIONSHIP('','',#40,#10)",
        "#383 = ( REPRESENTATION_RELATIONSHIP('','',#10,#40)",
    );
    let o = open(swapped.as_bytes()).unwrap();
    assert_eq!(placed(&o, 0)[1], ([-0.1, 0.0, 0.0], [1.0, 0.0, 0.0]));
    // A scale isn't a placement.
    let scaled = operator.replace(
        "CARTESIAN_TRANSFORMATION_OPERATOR_3D('',$,$,#900,$,$)",
        "CARTESIAN_TRANSFORMATION_OPERATOR_3D('',$,$,#900,2.,$)",
    );
    let o = open(scaled.as_bytes()).unwrap();
    assert!(matches!(
        o.placement(0, 1),
        Err(ImportError::Malformed { instance: 384, .. })
    ));
}
