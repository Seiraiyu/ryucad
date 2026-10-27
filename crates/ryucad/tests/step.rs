//! Real files imported into projects: the committed corpus (NIST's test
//! models, as ryustep's `tests/corpus/manifest.txt` lists them) and the
//! files OCCT wrote. Each body becomes an import feature that heals or
//! fails as the manifest says, to the very bits ryustep's own read gives
//! under the feature's key; the OCCT files' parts also pass `check`.
//! Release only (its own CI step): minutes in a debug build.

use ryucad::doc::files::Feature;
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
        .map(|f| call(&mut s, "import", json!({"path": f})).unwrap())
        .collect();
    (dir, s, answers)
}

/// The manifest's outcome per file and representation.
fn manifest() -> BTreeMap<(String, u64), String> {
    let text = std::fs::read_to_string(ryustep_tests().join("corpus/manifest.txt")).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            ((w[0].to_owned(), w[1].parse().unwrap()), w[2].to_owned())
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
                        heal: Default::default(),
                    },
                );
                let rep = opened.held[k].representation;
                let outcome = match &f["error"]["details"]["reason"] {
                    Value::String(r) => r.clone(),
                    _ if f["ok"] == true => "ok".to_owned(),
                    e => panic!("{name} #{rep}: {e}"),
                };
                seen.insert((name.clone(), rep), outcome.clone());
                if outcome == "ok" {
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
    assert_eq!(ok, want.values().filter(|o| *o == "ok").count());
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
    assert_eq!(v["findings"], json!([]), "{v}");
}
