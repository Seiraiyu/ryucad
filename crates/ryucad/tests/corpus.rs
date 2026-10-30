//! ryucad's corpus (design 192, plan 7.D): projects modelled through the
//! CLI by the scripts beside them in `tests/corpus`, committed, each
//! rebuilt and checked from nothing against `tests/corpus/manifest.json`
//! (per part what builds and what fails with which code, each solid's
//! hash and counts, each assembly's occurrences and solve, and `check`'s
//! findings by code); and, when `scripts/fetch-corpus.sh` has fetched
//! them, openly licensed STEP assemblies pinned in
//! `tests/corpus/pinned.txt`, imported into a new project each, which
//! must build no fewer parts than recorded there (the `slow` ones only
//! when run by name).
//!
//! Release only (its own step in full.yml): minutes in a debug build.
//! After a change that alters what the corpus builds on purpose:
//!
//! ```text
//! UPDATE_MANIFEST=1 cargo test -p ryucad --release --test corpus
//! ```
//!
//! and review the diff, as with the golden hashes.

use ryucad::doc::canonical;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// The committed projects, by folder.
const PROJECTS: [&str; 3] = ["radial-engine", "gearbox", "printer-frame"];

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params))?)
}

/// Copies a project folder: the corpus lies inside ryucad's own
/// repository, whose drivers and merge state are none of its business.
fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

/// A report that may come back as `rebuild_failed` or `check_failed`:
/// the report either way.
fn report(r: ryucad::error::Result<Value>) -> Value {
    match r {
        Ok(v) => v,
        Err(e) => match e.details.get("report") {
            Some(r) => r.clone(),
            None => e.details,
        },
    }
}

/// What the manifest records of one project: rebuilt and checked from
/// nothing in a fresh session.
fn summary(root: &Path) -> Value {
    let dir = tempfile::tempdir().unwrap();
    copy(root, dir.path());
    let mut s = Session::open(dir.path()).unwrap();
    let built = report(call(&mut s, "rebuild", json!({})));
    let mut parts = Map::new();
    for p in built["parts"].as_array().unwrap() {
        let mut failed = Map::new();
        let mut dependent = 0;
        for f in p["features"].as_array().unwrap() {
            if f["ok"] == true {
                continue;
            }
            let code = f["error"]["code"].as_str().unwrap();
            if code == "dependency_failed" {
                dependent += 1;
            } else {
                failed.insert(f["id"].as_str().unwrap().into(), json!(code));
            }
        }
        let mut row = json!({
            "features": p["features"].as_array().unwrap().len(),
            "failed": failed,
            "dependency_failed": dependent,
        });
        if p["kind"] == "assembly" {
            row["kind"] = json!("assembly");
            row["occurrences"] = p["occurrence_count"].clone();
            row["solve"] = json!({
                "state": p["solve"]["state"],
                "dof": p["solve"]["dof"],
            });
        } else {
            row["kind"] = json!("part");
            let mut solids = Map::new();
            for sd in p["solids"].as_array().unwrap() {
                solids.insert(
                    sd["solid"].as_str().unwrap().into(),
                    json!({"hash": sd["hash"], "counts": sd["counts"], "shown": sd["shown"]}),
                );
            }
            row["solids"] = Value::Object(solids);
        }
        parts.insert(p["part"].as_str().unwrap().into(), row);
    }
    let checked = report(call(&mut s, "check", json!({})));
    let mut codes = Map::new();
    for f in checked["findings"].as_array().unwrap() {
        let code = f["code"].as_str().unwrap().to_owned();
        let n = codes.get(&code).and_then(Value::as_u64).unwrap_or(0);
        codes.insert(code, json!(n + 1));
    }
    json!({
        "parts": parts,
        "check": {"errors": checked["errors"], "warnings": checked["warnings"], "codes": codes},
    })
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn the_committed_projects_build_as_the_manifest_says() {
    let path = corpus().join("manifest.json");
    let mut now = Map::new();
    for name in PROJECTS {
        now.insert(name.into(), summary(&corpus().join(name)));
    }
    let now = json!({"projects": now});
    if std::env::var_os("UPDATE_MANIFEST").is_some() {
        std::fs::write(&path, canonical::to_string(&now)).unwrap();
        return;
    }
    let recorded: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for name in PROJECTS {
        let (r, n) = (&recorded["projects"][name], &now["projects"][name]);
        for (part, row) in n["parts"].as_object().unwrap() {
            assert_eq!(&r["parts"][part], row, "{name}/{part}");
        }
        assert_eq!(r, n, "{name}");
    }
}

/// Where `scripts/fetch-corpus.sh` puts the pinned files.
fn fetched() -> PathBuf {
    std::env::var_os("RYUCAD_CORPUS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus"),
        PathBuf::from,
    )
}

/// `pinned.txt`'s lines of `kind` (`builds`, or `slow` for files run by
/// name only): (parts at least, the file in the cache).
fn pinned(kind: &str) -> Vec<(usize, String)> {
    let prefix = format!("{kind} ");
    std::fs::read_to_string(corpus().join("pinned.txt"))
        .unwrap()
        .lines()
        .filter_map(|l| l.strip_prefix(prefix.as_str()))
        .map(|l| {
            let (n, path) = l.split_once(' ').unwrap();
            (n.parse().unwrap(), path.to_owned())
        })
        .collect()
}

/// The parts (not assemblies) of `s`'s project whose every feature
/// builds, by ID.
fn built_parts(s: &mut Session) -> Vec<String> {
    let built = report(call(s, "rebuild", json!({})));
    built["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["kind"] != "assembly" && p["failed"] == 0)
        .map(|p| p["part"].as_str().unwrap().to_owned())
        .collect()
}

/// Each pinned assembly of `pinned.txt`'s `builds` lines, imported into a
/// project of its own, builds no fewer parts than it records; every part
/// that builds, exported and imported again into a second project, builds
/// to the same counts. Skipped (with a note) when the file hasn't been
/// fetched, unless `RYUCAD_REQUIRE_CORPUS` is set.
#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn pinned_assemblies_build_no_fewer_parts() {
    check_pinned(pinned("builds"));
}

/// The same for the `slow` lines (the Voron 2.4: hours, most of it healing
/// one threaded insert's 48 copies), run by name:
/// `cargo test -p ryucad --release --test corpus -- --ignored slow_pinned`.
#[test]
#[ignore = "hours: run by name"]
fn slow_pinned_assemblies_build_no_fewer_parts() {
    check_pinned(pinned("slow"));
}

fn check_pinned(files: Vec<(usize, String)>) {
    let require = std::env::var_os("RYUCAD_REQUIRE_CORPUS").is_some();
    for (at_least, file) in files {
        let path = fetched().join(&file);
        if !path.exists() {
            assert!(
                !require,
                "{} is missing: run scripts/fetch-corpus.sh",
                path.display()
            );
            eprintln!("skipped {file}: not fetched (scripts/fetch-corpus.sh)");
            continue;
        }
        let t = std::time::Instant::now();
        let dir = tempfile::tempdir().unwrap();
        ryucad::server::init(dir.path(), None).unwrap();
        let mut s = Session::open(dir.path()).unwrap();
        let answer = call(&mut s, "import", json!({"path": path})).unwrap();
        let built = built_parts(&mut s);
        let parts = answer["parts"].as_array().map_or(0, Vec::len);
        println!(
            "{file}: {} of {parts} parts and assemblies made build, in {:.0} s",
            built.len(),
            t.elapsed().as_secs_f64()
        );
        assert!(
            built.len() >= at_least,
            "{file}: {} parts build, pinned.txt records {at_least}",
            built.len()
        );
        if built.len() > at_least {
            println!("  more than pinned.txt's {at_least}: raise its count");
        }
        // Every part that builds, through STEP and back.
        let out = tempfile::tempdir().unwrap();
        let again = tempfile::tempdir().unwrap();
        ryucad::server::init(again.path(), None).unwrap();
        let mut b = Session::open(again.path()).unwrap();
        let counts = |s: &mut Session, part: &str| {
            let r = report(call(s, "rebuild", json!({"part": part})));
            r["parts"][0]["solids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|sd| sd["counts"].clone())
                .collect::<Vec<_>>()
        };
        for part in &built {
            let step = out.path().join(format!("{part}.step"));
            call(&mut s, "export", json!({"part": part, "out": step})).unwrap();
            call(&mut b, "import", json!({"path": step, "part": part})).unwrap();
            assert_eq!(counts(&mut b, part), counts(&mut s, part), "{file}: {part}");
        }
        println!(
            "  {} parts exported and imported again, in {:.0} s in all",
            built.len(),
            t.elapsed().as_secs_f64()
        );
    }
}

/// Every file under `root`, by its path relative to it, with its bytes.
fn files(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                todo.push(p);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_path_buf();
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// Each committed project is what its script makes with this build's CLI,
/// file for file: the corpus can be made again as the CLI changes, and a
/// change that alters what the CLI writes shows here. Needs Python 3
/// (skipped with a note without it, unless `RYUCAD_REQUIRE_PYTHON` is set).
#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn the_scripts_make_the_committed_projects() {
    let python = std::process::Command::new("python3")
        .arg("--version")
        .output();
    if python.is_err() {
        assert!(
            std::env::var_os("RYUCAD_REQUIRE_PYTHON").is_none(),
            "no python3"
        );
        eprintln!("skipped: no python3");
        return;
    }
    let exe = assert_cmd::cargo::cargo_bin("ryucad");
    for name in PROJECTS {
        let out = tempfile::tempdir().unwrap();
        let made = out.path().join(name);
        let script = corpus().join(format!("{}.py", name.replace('-', "_")));
        let run = std::process::Command::new("python3")
            .args(["-I", "-B"])
            .arg(&script)
            .arg(&exe)
            .arg(&made)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let (want, got) = (files(&corpus().join(name)), files(&made));
        assert_eq!(
            want.keys().collect::<Vec<_>>(),
            got.keys().collect::<Vec<_>>(),
            "{name}'s files"
        );
        for (path, bytes) in &want {
            assert!(got[path] == *bytes, "{name}/{}", path.display());
        }
    }
}
