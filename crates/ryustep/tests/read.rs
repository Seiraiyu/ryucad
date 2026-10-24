//! The OCCT files read into bodies: each solid heals, passes the checker
//! and meshes to its volume; the sheet offset from a spline is refused
//! (offsets of splines come with healing).

use core::f64::consts::PI;
use ryukei::check::check;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryustep::{ImportError, Model, ReadOptions, read};

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
