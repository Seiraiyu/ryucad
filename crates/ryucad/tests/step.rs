//! Real files imported into projects: the committed corpus (NIST's test
//! models, as ryustep's `tests/corpus/manifest.txt` lists them) and the
//! files OCCT wrote. Each body becomes an import feature that heals or
//! fails as the manifest says, to the very bits ryustep's own read gives
//! under the feature's key; the OCCT files' parts also pass `check`.
//! Every part either project makes that builds is exported and imported
//! again, to the same solids. Release only (its own CI step): minutes in
//! a debug build.

use ryucad::doc::assembly::Rev;
use ryucad::doc::files::{Feature, PartKind};
use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::{handle, init};
use ryucad::session::Session;
use ryustep::ReadOptions;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn ryustep_tests() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryustep/tests")
}

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params))?)
}

/// A new project with every file in `files` imported, in order, and what
/// each import answered.
fn imported(files: &[PathBuf]) -> (tempfile::TempDir, Session, Vec<Value>) {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    let answers = files
        .iter()
        .map(|f| call(&mut s, "import", json!({"solid": true, "path": f})).unwrap())
        .collect();
    (dir, s, answers)
}

/// A part's solids as it shows them.
fn solids(s: &mut Session, part: &Id) -> Vec<ryukei::topo::Body> {
    let m = s.model(part, &Rev::Working).unwrap();
    m.occurrences[0]
        .solids
        .iter()
        .map(|sd| sd.built.body.clone())
        .collect()
}

/// A body's volume, or a sheet's area, from a mesh at a thousandth of
/// its size.
fn measure_of(b: &ryukei::topo::Body) -> f64 {
    let mesh = |chord| ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(chord, 0.5));
    let size = mesh(1e-3)
        .unwrap()
        .positions
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), p| {
            (lo.min(p.x.min(p.y).min(p.z)), hi.max(p.x.max(p.y).max(p.z)))
        });
    let m = mesh((size.1 - size.0) * 1e-3).unwrap();
    let (v, a) = ryukei_testkit::mesh::volume_and_area(&m);
    if b.kind() == ryukei::topo::BodyKind::Sheet {
        a
    } else {
        v
    }
}

/// Every part of `s`'s project whose features all build, exported in
/// meters and imported again into a project of its own as a part of the
/// same ID: the same solids, face for face, edge for edge and vertex for
/// vertex, of the same volumes. Assemblies that build export too. The
/// parts STEP can't describe (`export_unrepresentable`) must be
/// `unrepresentable`, so a kernel that writes them shows here.
fn exports_import_again(s: &mut Session, unrepresentable: &[&str]) {
    let out = tempfile::tempdir().unwrap();
    let parts: Vec<Id> = s.project().parts.keys().cloned().collect();
    let mut exported = 0;
    let mut refused = Vec::new();
    for pid in parts {
        if s.rebuilt(&pid).unwrap().failed() > 0 {
            continue;
        }
        let params = json!({"part": pid, "unit": "m", "time_stamp": "2026-10-27T00:00:00"});
        let req = Request::from_rpc("export", Some(params)).unwrap();
        let a = match ryucad::server::answer_live(s, None, None, req) {
            Ok(a) => a,
            Err(e) if e.code == ryucad::error::Code::ExportUnrepresentable => {
                refused.push(pid.to_string());
                continue;
            }
            Err(e) => panic!("{pid}: {e}"),
        };
        exported += 1;
        let p = s.project().part(&pid).unwrap();
        if p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly) {
            continue;
        }
        let path = out.path().join(format!("{pid}.step"));
        std::fs::write(&path, a.binary.unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init(dir.path(), None).unwrap();
        let mut t = Session::open(dir.path()).unwrap();
        call(
            &mut t,
            "import",
            json!({"solid": true, "path": path, "part": pid}),
        )
        .unwrap();
        let (before, after) = (solids(s, &pid), solids(&mut t, &pid));
        assert_eq!(before.len(), after.len(), "{pid}");
        for (k, (a, b)) in before.iter().zip(&after).enumerate() {
            assert_eq!(a.counts(), b.counts(), "{pid} solid {k}");
            let (va, vb) = (measure_of(a), measure_of(b));
            assert!(
                (va - vb).abs() <= 1e-3 * va.abs(),
                "{pid} solid {k}: {va} against {vb}"
            );
        }
    }
    assert!(exported > 0);
    assert_eq!(refused, unrepresentable);
}

/// The manifest's outcome per file and representation: `ok`, a refusal's
/// reason, or for a surface model `sheet <faces> <laminar>`, which
/// imports heal as a sheet since 18.C.
fn manifest() -> BTreeMap<(String, u64), String> {
    let text = std::fs::read_to_string(ryustep_tests().join("corpus/manifest.txt")).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let outcome = match w.iter().position(|x| *x == "sheet") {
                Some(k) => w[k..].join(" "),
                None => w[2].to_owned(),
            };
            ((w[0].to_owned(), w[1].parse().unwrap()), outcome)
        })
        .collect()
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn the_corpus_imports_as_ryustep_reads_it() {
    let want = manifest();
    let mut names: Vec<String> = want.keys().map(|(f, _)| f.clone()).collect();
    names.dedup();
    let files: Vec<PathBuf> = names
        .iter()
        .map(|n| ryustep_tests().join("corpus").join(n))
        .collect();
    let (_dir, mut s, answers) = imported(&files);
    let mut seen = BTreeMap::new();
    for (name, answer) in names.iter().zip(&answers) {
        let bytes = std::fs::read(ryustep_tests().join("corpus").join(name)).unwrap();
        let opened = ryustep::open(&bytes).unwrap();
        for part in answer["parts"].as_array().unwrap() {
            let pid = Id::parse(part["part"].as_str().unwrap(), "part").unwrap();
            for f in part["features"].as_array().unwrap() {
                let fid = Id::parse(f["id"].as_str().unwrap(), "feature").unwrap();
                let p = s.project().part(&pid).unwrap().clone();
                let Some(Ok(Feature::Import(i))) = p.features.get(&fid) else {
                    panic!("{pid}/{fid} isn't an import");
                };
                let k = ryucad::import::select(&fid, i, &opened).unwrap();
                let read = opened.body(
                    k,
                    &ReadOptions {
                        key: ryucad::rebuild::op_key(&pid, &fid),
                        heal: ryucad::import::heal_options(),
                    },
                );
                let rep = opened.held[k].representation;
                let outcome = match &f["error"]["details"]["reason"] {
                    Value::String(r) => r.clone(),
                    _ if f["ok"] == true => {
                        let body = &read.result.as_ref().unwrap().0;
                        match body.kind() {
                            ryukei::topo::BodyKind::Sheet => format!(
                                "sheet {} {}",
                                body.faces().count(),
                                body.edges().filter(|&e| body.is_laminar(e)).count()
                            ),
                            _ => "ok".to_owned(),
                        }
                    }
                    e => panic!("{name} #{rep}: {e}"),
                };
                seen.insert((name.clone(), rep), outcome.clone());
                if outcome == "ok" || outcome.starts_with("sheet") {
                    let body = &read.result.as_ref().unwrap().0;
                    let hash = blake3::hash(body.dump().as_bytes()).to_hex()[..32].to_owned();
                    assert_eq!(f["hash"], hash, "{name} #{rep}");
                }
            }
        }
    }
    assert_eq!(seen, want);
    // Every healed body rebuilds as the import said.
    let r = call(&mut s, "rebuild", json!({}));
    let parts = match r {
        Ok(v) => v["parts"].clone(),
        Err(e) => e.details["parts"].clone(),
    };
    let ok = parts
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["features"].as_array().unwrap())
        .filter(|f| f["ok"] == true)
        .count();
    let builds = want
        .values()
        .filter(|o| *o == "ok" || o.starts_with("sheet"))
        .count();
    assert_eq!(ok, builds);
    // Every part that builds exports, nist_ctc_05's second too: its
    // countersink's cone face, a lune through the apex, is cut open
    // (issue #146).
    exports_import_again(&mut s, &[]);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn occt_files_import_and_check_clean() {
    let names = [
        "box_ap203.stp",
        "box_inch.stp",
        "cavity.stp",
        "cone.stp",
        "cylinder.stp",
        "elliptic_prism.stp",
        "nurbs_cylinder.stp",
        "plate_hole.stp",
        "sphere.stp",
        "spline_revolve.stp",
        "torus.stp",
        "assembly.stp",
    ];
    let files: Vec<PathBuf> = names
        .iter()
        .map(|n| ryustep_tests().join("data/occt").join(n))
        .collect();
    let (_dir, mut s, answers) = imported(&files);
    for (name, a) in names.iter().zip(&answers) {
        for p in a["parts"].as_array().unwrap() {
            for f in p["features"].as_array().unwrap() {
                assert_eq!(f["ok"], true, "{name}: {f}");
            }
        }
    }
    let v = call(&mut s, "check", json!({})).unwrap();
    assert_eq!(v["passed"], true, "{v}");
    // Interference isn't run unasked (design 525): the file's assembly
    // says so, and nothing else is found.
    let findings = v["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f["code"] == "interference_not_checked"),
        "{v}"
    );
    assert!(
        findings
            .iter()
            .all(|f| f["code"] == "interference_not_checked" && f["severity"] == "info"),
        "{v}"
    );
    exports_import_again(&mut s, &[]);
}
