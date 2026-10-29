//! The mesh formats against their references (design 193, plan 8.A):
//! every STL ryuformats writes read by OCCT's `RWStl` to triangles that
//! close up, and every 3MF read by lib3mf without warnings, both of the
//! volume ryukei's meshes enclose; and ryucad's exports of a project the
//! same. Devcontainer only:
//! `cargo test -p ryukei-bench --features occt,formats --release --test formats -- --nocapture`.

use ryuformats::stl::{self, Stl};
use ryuformats::threemf::{self, ThreeMf};
use ryuformats::{Child, Product, Scene, Solid, Unit};
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{BodyMesh, MeshParams, tessellate};
use ryukei::topo::Body;
use ryukei_bench::{lib3mf, occt};
use std::path::Path;
use std::sync::Arc;

/// `b` meshed at a thousandth of its size, unless a face won't mesh.
fn meshed(b: &Body) -> Option<Arc<BodyMesh>> {
    let size = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|x, y| x.union(&y))
        .map(|x| (x.max() - x.min()).length())?;
    let m = tessellate(b, &MeshParams::new((size * 1e-3).max(1e-7), 0.3)).ok()?;
    m.faults.is_empty().then(|| Arc::new(m))
}

/// The volume `m` encloses placed by `at`, in `unit`'s cube.
fn volume(m: &BodyMesh, at: &Rigid, unit: Unit) -> f64 {
    let k = unit.per_meter();
    let p: Vec<[f64; 3]> = m
        .positions
        .iter()
        .map(|&q| {
            let q = at.apply_point(q);
            [q.x * k, q.y * k, q.z * k]
        })
        .collect();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| p[i as usize]);
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum()
}

/// One product of one solid.
fn one(mesh: Arc<BodyMesh>) -> Scene {
    Scene {
        products: vec![Product {
            id: "B-1".into(),
            name: "Body".into(),
            part: "b".into(),
            solids: vec![Solid {
                name: "s".into(),
                mesh,
            }],
            children: vec![],
        }],
    }
}

fn write_stl(scene: &Scene, opts: &Stl, path: &Path) {
    let mut bytes = Vec::new();
    stl::write(scene, opts, &mut bytes).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn write_3mf(scene: &Scene, opts: &ThreeMf, path: &Path) {
    let mut bytes = Vec::new();
    threemf::write(scene, opts, &mut bytes).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Within the rounding of f32 coordinates.
fn agree(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-5 * b.abs().max(1e-9)
}

/// Each seeded body that meshes, written alone as binary and ASCII STL
/// and as 3MF, in millimetres, and all of them placed on a grid in one
/// scene: the references read each to the volume ryukei's mesh encloses.
#[test]
fn the_references_read_every_seeded_body() {
    let dir = tempfile::tempdir().unwrap();
    let bodies = ryukei_testkit::race::bodies(24);
    let meshes: Vec<Arc<BodyMesh>> = bodies.iter().filter_map(meshed).collect();
    println!("{} of {} seeded bodies mesh", meshes.len(), bodies.len());
    assert!(
        meshes.len() * 10 >= bodies.len() * 9,
        "too few mesh to test"
    );
    let mut wrong = Vec::new();
    for (k, m) in meshes.iter().enumerate() {
        let want = volume(m, &Rigid::IDENTITY, Unit::Millimeter);
        let scene = one(Arc::clone(m));
        for ascii in [false, true] {
            let path = dir.path().join(format!("{k}-{ascii}.stl"));
            write_stl(
                &scene,
                &Stl {
                    ascii,
                    ..Stl::default()
                },
                &path,
            );
            match occt::read_stl(&path) {
                Ok(m) if m.closed() && agree(m.soup().volume_and_area().0, want) => {}
                Ok(m) => wrong.push(format!(
                    "{k} STL ascii {ascii}: {want} against {:?}, closed {}",
                    m.soup().volume_and_area(),
                    m.closed()
                )),
                Err(e) => wrong.push(format!("{k} STL ascii {ascii}: {e}")),
            }
        }
        let path = dir.path().join(format!("{k}.3mf"));
        write_3mf(&scene, &ThreeMf::default(), &path);
        match lib3mf::read(&path) {
            Ok(r)
                if r.warnings.is_empty()
                    && r.unit == "millimeter"
                    && r.triangles == m.triangles.len() as u64
                    && agree(r.volume, want) => {}
            other => wrong.push(format!("{k} 3MF: {want} against {other:?}")),
        }
    }
    // All of them, each placed on a 10 m grid, several times over, in
    // metres: one STL, and one 3MF placing each object three times.
    let place = |i: usize, j: usize| {
        Rigid::translation(Vector3::new(10.0 * i as f64, 10.0 * j as f64, 0.0))
    };
    let mut root = Product {
        id: "R-1".into(),
        name: "Race".into(),
        part: "race".into(),
        ..Product::default()
    };
    let mut products = vec![];
    let mut want = 0.0;
    for (k, m) in meshes.iter().enumerate() {
        for j in 0..3 {
            root.children.push(Child {
                product: k + 1,
                place: place(k, j),
                name: format!("i{k}-{j}"),
            });
            want += volume(m, &place(k, j), Unit::Meter);
        }
        products.push(one(Arc::clone(m)).products.remove(0));
    }
    let scene = Scene {
        products: std::iter::once(root).chain(products).collect(),
    };
    let stl_path = dir.path().join("race.stl");
    write_stl(
        &scene,
        &Stl {
            unit: Unit::Meter,
            ..Stl::default()
        },
        &stl_path,
    );
    match occt::read_stl(&stl_path) {
        Ok(m) if m.closed() && agree(m.soup().volume_and_area().0, want) => {}
        Ok(m) => wrong.push(format!(
            "race STL: {want} against {:?}, closed {}",
            m.soup().volume_and_area(),
            m.closed()
        )),
        Err(e) => wrong.push(format!("race STL: {e}")),
    }
    let path = dir.path().join("race.3mf");
    let opts = ThreeMf {
        unit: Unit::Meter,
        metadata: vec![
            ("Title".into(), "Race".into()),
            ("ryucad:chord".into(), "a thousandth".into()),
        ],
    };
    write_3mf(&scene, &opts, &path);
    match lib3mf::read(&path) {
        Ok(r)
            if r.warnings.is_empty()
                && r.unit == "meter"
                && r.items == 1
                && r.objects.len() == meshes.len() + 1
                && r.metadata.contains(&"Title=Race".to_owned())
                && agree(r.volume, want) =>
        {
            println!(
                "race: {} objects, {} triangles placed",
                r.objects.len(),
                r.triangles
            )
        }
        other => wrong.push(format!("race 3MF: {want} against {other:?}")),
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A plate drilled through, a pin, a part of two solids, and an
/// assembly placing them, one pin turned.
fn project() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let mut call = |m: &str, p: serde_json::Value| {
        let req = ryucad::proto::Request::from_rpc(m, Some(p)).unwrap();
        ryucad::server::handle(&mut s, req).unwrap_or_else(|e| panic!("{m}: {e}"));
    };
    call("part.create", json!({"part": "plate"}));
    call(
        "feature.add",
        json!({"part": "plate", "type": "box", "id": "slab",
               "params": {"x": 60, "y": 40, "z": 8}}),
    );
    call(
        "feature.add",
        json!({"part": "plate", "type": "cylinder", "id": "bore", "operation": "cut",
               "targets": ["slab"], "params": {"radius": 4, "height": 20},
               "placement": {"origin": [20, 20, -5]}}),
    );
    call("part.create", json!({"part": "pin"}));
    call(
        "feature.add",
        json!({"part": "pin", "type": "cylinder", "id": "shank",
               "params": {"radius": 3.9, "height": 30}}),
    );
    call("part.create", json!({"part": "pair"}));
    call(
        "feature.add",
        json!({"part": "pair", "type": "box", "id": "cube", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    call(
        "feature.add",
        json!({"part": "pair", "type": "sphere", "id": "ball", "params": {"radius": 5},
               "placement": {"origin": [30, 0, 0]}}),
    );
    call("part.create", json!({"part": "frame", "kind": "assembly"}));
    for (id, of, origin) in [
        ("base", "plate", json!([0, 0, 0])),
        ("p1", "pin", json!([20, 20, -10])),
        ("p2", "pin", json!([45, 20, 8])),
        ("two", "pair", json!([0, 60, 0])),
    ] {
        call(
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": id, "of": of,
                   "placement": {"origin": origin}}),
        );
    }
    call(
        "assembly.move",
        json!({"part": "frame", "instance": "p2", "axis": [1, 0, 0], "angle": "30 deg"}),
    );
    (dir, s)
}

/// ryucad's STL and 3MF of every part and the assembly: OCCT and lib3mf
/// read the same volume from each (the same meshes), within the chord
/// times the area of the exact solids' volume.
#[test]
fn the_references_read_what_ryucad_exports() {
    use ryucad::doc::assembly::Rev;
    use ryukei::measure::{Entity, Properties, properties};
    let (dir, mut s) = project();
    let mut wrong = Vec::new();
    for (part, unit) in [
        ("plate", "mm"),
        ("pin", "cm"),
        ("pair", "m"),
        ("frame", "mm"),
    ] {
        let id = ryucad::doc::id::Id::parse(part, "part").unwrap();
        let k = Unit::parse(unit).unwrap().per_meter();
        let (mut exact, mut slack) = (0.0, 0.0);
        for o in &s.model(&id, &Rev::Working).unwrap().occurrences {
            for sd in &o.solids {
                let Properties::Body { volume, area } =
                    properties(&sd.built.body, Entity::Body).unwrap()
                else {
                    unreachable!()
                };
                exact += volume.value() * k * k * k;
                // The default chord, 0.01 mm, over the area, and the bounds.
                slack += (1e-5 * area.value() + volume.bound()) * k * k * k;
            }
        }
        let mut read = Vec::new();
        for format in ["stl", "3mf"] {
            let out = dir.path().join(format!("{part}.{format}"));
            let params = serde_json::json!({"part": part, "unit": unit,
                                            "out": out, "time_stamp": "2026-10-28T00:00:00"});
            let req = ryucad::proto::Request::from_rpc("export", Some(params)).unwrap();
            ryucad::server::answer_live(&mut s, None, None, req).unwrap();
            read.push(match format {
                "stl" => occt::read_stl(&out).and_then(|m| {
                    if m.closed() {
                        Ok(m.soup().volume_and_area().0)
                    } else {
                        Err("not closed".to_owned())
                    }
                }),
                _ => lib3mf::read(&out).and_then(|r| {
                    if r.warnings.is_empty() {
                        Ok(r.volume)
                    } else {
                        Err(format!("{:?}", r.warnings))
                    }
                }),
            });
        }
        match (&read[0], &read[1]) {
            (Ok(a), Ok(b)) if agree(*a, *b) && (b - exact).abs() <= slack => {
                println!("{part} in {unit}: {b:.6} against {exact:.6} ± {slack:.2e}")
            }
            other => wrong.push(format!(
                "{part} in {unit}: {exact} ± {slack}, read {other:?}"
            )),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
