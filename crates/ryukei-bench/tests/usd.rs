//! USD against its references (design 193, plans 8.B and 8.C): every
//! stage ryuformats writes opened by OpenUSD (PyPI's usd-core), its
//! BrepArrays passing usd-brep's validator and imported into SMLib as
//! manifold solids of the volume ryukei measures; its usdc and usdz read
//! by OpenUSD to the same layer as its usda, the usdz passing OpenUSD's
//! package checks; and ryucad's exports the same, an assembly's
//! instances seen as instances. Devcontainer only:
//! `cargo test -p ryukei-bench --features usd --release --test usd -- --nocapture`.
//! The Python is `RYUCAD_USD_PYTHON`, else the devcontainer's
//! `/opt/usd/bin/python`.

use ryuformats::Unit;
use ryuformats::usd::stage::{Model, Options, Product, Solid, layer};
use ryuformats::usd::{usda, usdc, usdz};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::mesh::{BodyMesh, MeshParams, tessellate};
use ryukei::topo::Body;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What the references say of each file (see `tests/usd/reference.py`).
fn reference(dir: &Path, files: &[PathBuf]) -> Vec<Value> {
    let python =
        std::env::var("RYUCAD_USD_PYTHON").unwrap_or_else(|_| "/opt/usd/bin/python".into());
    let manifest = dir.join("manifest.json");
    std::fs::write(&manifest, json!({ "files": files }).to_string()).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/usd/reference.py");
    let out = std::process::Command::new(&python)
        .arg(&script)
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| panic!("{python}: {e}"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// `b`'s volume and its bound, in `unit`'s cube.
fn exact(b: &Body, unit: Unit) -> (f64, f64) {
    let Properties::Body { volume, .. } = properties(b, Entity::Body).unwrap() else {
        unreachable!()
    };
    let k = unit.per_meter();
    let k = k * k * k;
    (volume.value() * k, volume.bound() * k)
}

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

/// The references' verdict on one file: opened, valid and imported, each
/// brep a manifold solid; and, where `volumes` names them, each brep's
/// volume within its bound and SMLib's integration (a millionth).
fn judge(r: &Value, volumes: &[(f64, f64)]) -> Result<(), String> {
    let bad = |why: &str| Err(format!("{}: {why}: {r}", r["path"]));
    if r["opened"] != true || r["flattened"] != true {
        return bad("OpenUSD didn't open it");
    }
    if r["valid"] != true {
        return bad("usd-brep's validator refused it");
    }
    let Some(breps) = r["breps"].as_array() else {
        return bad("SMLib didn't import it");
    };
    if breps.iter().any(|b| b["manifold"] != true) {
        return bad("not every brep is a manifold solid");
    }
    if !volumes.is_empty() {
        if breps.len() != volumes.len() {
            return bad("a brep missing");
        }
        for (b, &(v, bound)) in breps.iter().zip(volumes) {
            let got = b["volume"].as_f64().unwrap_or(f64::NAN);
            if (got - v).abs() > bound + 1e-6 * v.abs() {
                return bad(&format!("volume {got} against {v} ± {bound}"));
            }
        }
    }
    Ok(())
}

/// The references' verdict on a usdc or usdz: opened and valid, the same
/// layer as the usda `text` was read from, and a usdz a sound package.
fn judge_binary(r: &Value, usda: &Value) -> Result<(), String> {
    let bad = |why: &str| Err(format!("{}: {why}: {r}", r["path"]));
    if r["opened"] != true || r["flattened"] != true {
        return bad("OpenUSD didn't open it");
    }
    if r["valid"] != true {
        return bad("usd-brep's validator refused it");
    }
    if r["layer"].is_null() || r["layer"] != usda["layer"] {
        return bad(&format!("not the usda's layer ({})", usda["layer"]));
    }
    if let Some(p) = r.get("package")
        && p.as_array().is_none_or(|p| !p.is_empty())
    {
        return bad("OpenUSD's package checks refused it");
    }
    Ok(())
}

/// Writes `l` as `<stem>.usda`, `.usdc` and `.usdz` in `dir`: the three
/// paths, the usda first.
fn write_all(dir: &Path, stem: &str, l: &ryuformats::usd::layer::Layer) -> [PathBuf; 3] {
    let [a, c, z] = ["usda", "usdc", "usdz"].map(|e| dir.join(format!("{stem}.{e}")));
    std::fs::write(&a, usda::to_string(l).unwrap()).unwrap();
    std::fs::write(&c, usdc::to_bytes(l).unwrap()).unwrap();
    let mut bytes = Vec::new();
    usdz::write(l, stem, &mut bytes).unwrap();
    std::fs::write(&z, bytes).unwrap();
    [a, c, z]
}

/// Each triple's verdicts: the usda by `judge`, its usdc and usdz by
/// `judge_binary` against it.
fn judge_all(report: &[Value], volumes: &[Vec<(f64, f64)>]) -> Vec<String> {
    report
        .chunks(3)
        .zip(volumes)
        .flat_map(|(r, v)| {
            [
                judge(&r[0], v),
                judge_binary(&r[1], &r[0]),
                judge_binary(&r[2], &r[0]),
            ]
        })
        .filter_map(Result::err)
        .collect()
}

#[test]
fn the_references_read_every_seeded_body() {
    let dir = tempfile::tempdir().unwrap();
    let bodies = ryukei_testkit::race::bodies(24);
    let opts = Options {
        unit: Unit::Millimeter,
        brep: true,
        mesh: true,
        pipe_tolerance: 1e-7,
        layer_data: vec![],
    };
    let (mut files, mut volumes, mut refused) = (Vec::new(), Vec::new(), Vec::new());
    for (k, b) in bodies.iter().enumerate() {
        let model = Model {
            products: vec![Product {
                id: format!("B-{k}"),
                name: format!("Body {k}"),
                part: format!("b{k}"),
                label: None,
                solids: vec![Solid {
                    name: "s".into(),
                    body: Arc::new(b.clone()),
                    mesh: meshed(b),
                }],
                children: vec![],
            }],
        };
        match layer(&model, &opts) {
            Ok(l) => {
                files.extend(write_all(dir.path(), &format!("b{k}"), &l));
                volumes.push(vec![exact(b, Unit::Millimeter)]);
            }
            Err(e) => refused.push(format!("{k}: {e}")),
        }
    }
    println!(
        "{} of {} seeded bodies written; refused: {refused:#?}",
        volumes.len(),
        bodies.len()
    );
    assert!(
        volumes.len() * 10 >= bodies.len() * 9,
        "too few written to test"
    );
    let wrong = judge_all(&reference(dir.path(), &files), &volumes);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A plate drilled through, a pin, a ball, and an assembly placing the
/// pin twice (once turned) on the plate, and the ball.
fn project() -> (tempfile::TempDir, ryucad::session::Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let mut call = |m: &str, p: Value| {
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
    call("part.create", json!({"part": "ball"}));
    call(
        "feature.add",
        json!({"part": "ball", "type": "sphere", "id": "b", "params": {"radius": 5}}),
    );
    call("part.create", json!({"part": "frame", "kind": "assembly"}));
    for (id, of, origin) in [
        ("base", "plate", json!([0, 0, 0])),
        ("p1", "pin", json!([20, 20, -10])),
        ("p2", "pin", json!([45, 20, 8])),
        ("b", "ball", json!([0, 60, 0])),
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

/// ryucad's usda, usdc and usdz of every part and of the assembly: each
/// opened and valid, the usda imported, a part's breps of its solids'
/// volumes, the usdc and usdz the usda's layer; the assembly's parts
/// defined once and placed as instances in every encoding.
#[test]
fn the_references_read_what_ryucad_exports() {
    use ryucad::doc::assembly::Rev;
    let (dir, mut s) = project();
    let mut files = Vec::new();
    let mut volumes = Vec::new();
    for part in ["plate", "pin", "ball", "frame"] {
        for encoding in ["usda", "usdc", "usdz"] {
            let out = dir.path().join(format!("{part}.{encoding}"));
            let params = json!({"part": part, "out": out, "time_stamp": "2026-10-29T00:00:00"});
            let req = ryucad::proto::Request::from_rpc("export", Some(params)).unwrap();
            ryucad::server::answer_live(&mut s, None, None, req).unwrap();
            files.push(out);
        }
        let id = ryucad::doc::id::Id::parse(part, "part").unwrap();
        let model = s.model(&id, &Rev::Working).unwrap();
        volumes.push(if part == "frame" {
            Vec::new()
        } else {
            model
                .occurrences
                .iter()
                .flat_map(|o| &o.solids)
                .map(|sd| exact(&sd.built.body, Unit::Millimeter))
                .collect()
        });
    }
    let report = reference(dir.path(), &files);
    let mut wrong = judge_all(&report, &volumes);
    // Four occurrences, each an instance, of three prototypes, and a
    // BrepArray and a mesh for each through the instances, whichever the
    // encoding.
    for frame in &report[9..] {
        println!("frame: {frame}");
        let seen = (
            &frame["instances"],
            &frame["prototypes"],
            &frame["breparrays"],
            &frame["meshes"],
            &frame["meters_per_unit"],
            &frame["up_axis"],
        );
        if seen
            != (
                &json!(4),
                &json!(3),
                &json!(4),
                &json!(4),
                &json!(0.001),
                &json!("Z"),
            )
        {
            wrong.push(format!("{}: {seen:?}", frame["path"]));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
