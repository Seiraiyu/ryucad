//! USD against its references (design 193, plans 8.B and 8.C): every
//! stage ryuformats writes opened by OpenUSD (PyPI's usd-core), its
//! BrepArrays passing usd-brep's validator and imported into SMLib as
//! manifold solids of the volume ryukei measures, and sheets of the area
//! it measures; its usdc and usdz read
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

/// What a brep must import as: a manifold solid of a volume with its
/// bound, or a sheet of an area with its bound and a count of faces (an
/// open one never a manifold solid), in `unit`.
#[derive(Clone, Copy, Debug)]
enum Expect {
    Solid(f64, f64),
    Sheet {
        area: f64,
        bound: f64,
        faces: usize,
        closed: bool,
    },
}

/// What `b` must import as, in `unit`.
fn expect(b: &Body, unit: Unit) -> Expect {
    let k = unit.per_meter();
    match properties(b, Entity::Body).unwrap() {
        Properties::Body { volume, .. } => {
            Expect::Solid(volume.value() * k * k * k, volume.bound() * k * k * k)
        }
        Properties::Sheet { area } => Expect::Sheet {
            area: area.value() * k * k,
            bound: area.bound() * k * k,
            faces: b.faces().count(),
            closed: ryucad::kind::boundaries(b) == 0,
        },
        p => panic!("{p:?}"),
    }
}

/// [`judge`] for breps of either kind: opened, valid and imported, each
/// brep as `want` says (SMLib's area to a millionth, where it can say).
fn judge_kinds(r: &Value, want: &[Expect]) -> Result<(), String> {
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
    if breps.len() != want.len() {
        return bad(&format!("{} breps against {}", breps.len(), want.len()));
    }
    for (b, w) in breps.iter().zip(want) {
        match *w {
            Expect::Solid(v, bound) => {
                if b["manifold"] != true {
                    return bad("a solid isn't a manifold solid");
                }
                let got = b["volume"].as_f64().unwrap_or(f64::NAN);
                if (got - v).abs() > bound + 1e-6 * v.abs() {
                    return bad(&format!("volume {got} against {v} ± {bound}"));
                }
            }
            Expect::Sheet {
                area,
                bound,
                faces,
                closed,
            } => {
                if !closed && b["manifold"] != false {
                    return bad("an open sheet imported as a manifold solid");
                }
                if let Some(got) = b["area"].as_f64()
                    && (got - area).abs() > bound + 1e-6 * area.abs()
                {
                    return bad(&format!("area {got} against {area} ± {bound}"));
                }
                if let Some(n) = b["faces"].as_u64()
                    && n as usize != faces
                {
                    return bad(&format!("{n} faces against {faces}"));
                }
            }
        }
    }
    Ok(())
}

/// A block's faces but its top, each extracted as a sheet and sewn back:
/// an open box of five faces, its rim four laminar edges.
fn sewn_open_box() -> Body {
    use ryukei::heal::{SewOptions, sew};
    use ryukei::name::{FaceRole, Name, OpKey, Role};
    use ryukei::sheet::extract;
    let k = OpKey(1);
    let (block, _) =
        ryukei::primitive::block(k, 0.01, 0.02, 0.03, &ryukei::geom::Rigid::IDENTITY).unwrap();
    let top = Name::derive(k, Role::Face(FaceRole::PlusZ), &[]);
    let sheets: Vec<Body> = block
        .faces()
        .map(|f| block.face(f).unwrap().name)
        .filter(|&n| n != top)
        .map(|n| extract(OpKey(2), &block, &[n]).unwrap().0)
        .collect();
    let options = SewOptions {
        tolerance: 1e-6,
        solid: false,
    };
    let (mut pieces, _) = sew(OpKey(3), &sheets, &options).unwrap();
    assert_eq!(pieces.len(), 1);
    pieces.remove(0)
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
                    brep: true,
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

/// Every race body (`race::writable(1600)`) as a usda of its BrepArray
/// alone, opened by OpenUSD, valid by usd-brep's validator, and imported
/// into SMLib as manifold solids of the volume ryukei measures: design
/// 296's BrepArray bar. About a quarter of an hour on a Ryzen 7 9700X
/// (987 s): run by name in the devcontainer,
/// `cargo test -p ryukei-bench --features usd --release --test usd --
/// --ignored every_race_body --nocapture`.
#[test]
#[ignore = "about a quarter of an hour on a Ryzen 7 9700X: run by name (plan 15.F's bars)"]
fn every_race_body_is_valid_and_imports() {
    use rayon::prelude::*;
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    // `RYUCAD_RACE_KEEP=<dir>` writes the files there, to look at after.
    let kept = std::env::var("RYUCAD_RACE_KEEP").ok().map(PathBuf::from);
    let dir = kept.clone().unwrap_or_else(|| dir.path().to_path_buf());
    std::fs::create_dir_all(&dir).unwrap();
    // `RYUCAD_RACE_BODIES=155,200` judges those bodies alone.
    let only: Option<Vec<usize>> = std::env::var("RYUCAD_RACE_BODIES")
        .ok()
        .map(|s| s.split(',').map(|k| k.trim().parse().unwrap()).collect());
    let bodies: Vec<(usize, Body)> = ryukei_testkit::race::writable(1600)
        .into_iter()
        .enumerate()
        .filter(|(k, _)| only.as_ref().is_none_or(|o| o.contains(k)))
        .collect();
    let opts = Options {
        unit: Unit::Millimeter,
        brep: true,
        mesh: false,
        pipe_tolerance: 1e-7,
        layer_data: vec![],
    };
    // Each body's file and its volume with its bound (none where mass
    // can't mesh a face of it: then only its validity and manifold breps
    // are judged), or why it's refused.
    type Written = Result<(PathBuf, Option<(f64, f64)>), String>;
    let written: Vec<Written> = bodies
        .par_iter()
        .map(|(k, b)| {
            let k = *k;
            let model = Model {
                products: vec![Product {
                    id: format!("B-{k}"),
                    name: format!("Body {k}"),
                    part: format!("b{k}"),
                    label: None,
                    solids: vec![Solid {
                        name: "s".into(),
                        body: Arc::new(b.clone()),
                        mesh: None,
                        brep: true,
                    }],
                    children: vec![],
                }],
            };
            let l = layer(&model, &opts).map_err(|e| format!("{k}: {e}"))?;
            let path = dir.join(format!("b{k}.usda"));
            std::fs::write(&path, usda::to_string(&l).unwrap()).unwrap();
            Ok((path, measured(b, Unit::Millimeter)))
        })
        .collect();
    let refused: Vec<&String> = written.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(refused.is_empty(), "refused: {refused:#?}");
    let (files, volumes): (Vec<PathBuf>, Vec<Option<(f64, f64)>>) =
        written.into_iter().map(Result::unwrap).unzip();
    let unmeasured: Vec<usize> = (0..volumes.len())
        .filter(|&k| volumes[k].is_none())
        .collect();
    let report = reference(&dir, &files);
    let wrong: Vec<String> = report
        .iter()
        .zip(&volumes)
        .filter_map(|(r, v)| judge(r, v.as_slice()).err())
        .collect();
    println!(
        "{} race bodies written, {} wrong, {} not measured (their volumes unjudged: {unmeasured:?}), on {}",
        files.len(),
        wrong.len(),
        unmeasured.len(),
        machine()
    );
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// `b`'s volume and its bound, as [`exact`], or `None` where
/// `measure::properties` refuses it (a face mass can't mesh).
fn measured(b: &Body, unit: Unit) -> Option<(f64, f64)> {
    let Ok(Properties::Body { volume, .. }) = properties(b, Entity::Body) else {
        return None;
    };
    let k = unit.per_meter();
    let k = k * k * k;
    Some((volume.value() * k, volume.bound() * k))
}

/// The CPU's name, from `/proc/cpuinfo` (the devcontainer is Linux).
fn machine() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| std::env::consts::ARCH.into())
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

/// Sheets as their BrepArrays (#624): a planar square beside a box, an
/// extruded arc, a dome and CTC 04's surface model, each exported by
/// ryucad as usda; and a sewn open box and a whole ball written by the
/// stage in all three encodings. Each opened, valid by usd-brep's
/// validator, and imported by SMLib: each solid of its volume, each sheet
/// of the area ryukei measures, an open one never a manifold solid.
#[test]
fn sheets_read_through_openusd_as_breparrays() {
    use core::f64::consts::{FRAC_PI_2, TAU};
    use ryucad::doc::assembly::Rev;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: Value| {
        let req = ryucad::proto::Request::from_rpc(m, Some(p)).unwrap();
        ryucad::server::handle(s, req).unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let sketch = |s: &mut ryucad::session::Session, part: &str, plane: &str, ops: Value| {
        call(
            s,
            "feature.add",
            json!({"part": part, "type": "sketch", "id": "s", "plane": plane}),
        );
        call(
            s,
            "sketch.edit",
            json!({"part": part, "sketch": "s", "ops": ops}),
        );
    };
    for part in ["skin", "wall", "dome"] {
        call(&mut s, "part.create", json!({"part": part}));
    }
    // A 10 mm planar square beside a 5 mm box.
    sketch(
        &mut s,
        "skin",
        "xy",
        json!([
            {"op": "add", "id": "p1", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p2", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "p3", "type": "point", "at": [10, 10]},
            {"op": "add", "id": "p4", "type": "point", "at": [0, 10]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "id": "l4", "type": "line", "start": "p4", "end": "p1"}]),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "skin", "type": "planar", "id": "w", "sketch": "s"}),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "skin", "type": "box", "id": "b", "params": {"x": 5, "y": 5, "z": 5},
               "placement": {"origin": [20, 0, 0]}}),
    );
    // A quarter of a 10 mm circle extruded 20 mm.
    sketch(
        &mut s,
        "wall",
        "xy",
        json!([
            {"op": "add", "id": "o", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "q", "type": "point", "at": [0, 10]},
            {"op": "add", "id": "a1", "type": "arc", "center": "o", "start": "p", "end": "q"}]),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "wall", "type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "blind", "length": "20 mm"}}),
    );
    // A quarter circle revolved a whole turn: a 10 mm hemisphere.
    sketch(
        &mut s,
        "dome",
        "zx",
        json!([
            {"op": "add", "id": "o", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p", "type": "point", "at": [0, 10]},
            {"op": "add", "id": "q", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "a1", "type": "arc", "center": "o", "start": "q", "end": "p"},
            {"op": "add", "id": "ax", "type": "line", "start": "o", "end": "q", "construction": true}]),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "dome", "type": "revolve", "id": "d", "sketch": "s", "axis": "ax",
               "angle": "full", "surface": true}),
    );
    // CTC 04: a surface model beside its solid.
    let ctc = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ryustep/tests/corpus/nist_ctc_04_asme1_rd.stp");
    let v = call(&mut s, "import", json!({"path": ctc, "solid": true}));
    let mut parts: Vec<String> = vec!["skin".into(), "wall".into(), "dome".into()];
    parts.extend(
        v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["part"].as_str().unwrap().to_owned()),
    );
    let (mut files, mut wants, mut sheets) = (Vec::new(), Vec::new(), 0);
    for part in &parts {
        let out = dir.path().join(format!("{part}.usda"));
        let params = json!({"part": part, "out": out, "time_stamp": "2026-11-12T00:00:00"});
        let req = ryucad::proto::Request::from_rpc("export", Some(params)).unwrap();
        let a = ryucad::server::answer_live(&mut s, None, None, req).unwrap();
        assert!(
            a.result["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|w| w["code"] != "export_sheet_brep_skipped"),
            "{part}: {}",
            a.result
        );
        let id = ryucad::doc::id::Id::parse(part, "part").unwrap();
        let want: Vec<Expect> = s
            .model(&id, &Rev::Working)
            .unwrap()
            .occurrences
            .iter()
            .flat_map(|o| &o.solids)
            .map(|sd| expect(&sd.built.body, Unit::Millimeter))
            .collect();
        sheets += want
            .iter()
            .filter(|w| matches!(w, Expect::Sheet { .. }))
            .count();
        files.push(out);
        wants.push(want);
    }
    // The square, the wall, the dome and CTC 04's surface model at least.
    assert!(sheets >= 4, "{sheets} sheets");
    let report = reference(dir.path(), &files);
    let mut wrong: Vec<String> = report
        .iter()
        .zip(&wants)
        .filter_map(|(r, w)| judge_kinds(r, w).err())
        .collect();
    // Written by the stage alone: a sewn open box and a whole ball, a
    // closed sheet with no edges of its own (its seams and poles are the
    // writer's), in usda, usdc and usdz.
    let ball = ryukei::geom::Surface::Sphere(
        ryukei::geom::Sphere::new(ryukei::geom::Frame::WORLD, 0.01).unwrap(),
    );
    let globe = ryukei::sheet::from_surface(
        ryukei::name::OpKey(4),
        &ball,
        [[0.0, TAU], [-FRAC_PI_2, FRAC_PI_2]],
    )
    .unwrap()
    .0;
    let opts = Options {
        unit: Unit::Millimeter,
        brep: true,
        mesh: true,
        pipe_tolerance: 1e-7,
        layer_data: vec![],
    };
    let (mut files, mut wants) = (Vec::new(), Vec::new());
    for (k, b) in [sewn_open_box(), globe].into_iter().enumerate() {
        let model = Model {
            products: vec![Product {
                id: format!("S-{k}"),
                name: format!("Sheet {k}"),
                part: format!("s{k}"),
                label: None,
                solids: vec![Solid {
                    name: "s".into(),
                    mesh: meshed(&b),
                    body: Arc::new(b.clone()),
                    brep: true,
                }],
                children: vec![],
            }],
        };
        let l = layer(&model, &opts).unwrap_or_else(|e| panic!("sheet {k}: {e}"));
        files.extend(write_all(dir.path(), &format!("s{k}"), &l));
        wants.push(vec![expect(&b, Unit::Millimeter)]);
    }
    let report = reference(dir.path(), &files);
    for (r, w) in report.chunks(3).zip(&wants) {
        wrong.extend(judge_kinds(&r[0], w).err());
        wrong.extend(judge_binary(&r[1], &r[0]).err());
        wrong.extend(judge_binary(&r[2], &r[0]).err());
    }
    for r in &report {
        println!("sheet: {r}");
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
