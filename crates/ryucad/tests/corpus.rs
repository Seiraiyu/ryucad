//! ryucad's corpus (design 192, plan 7.D): projects modelled through the
//! CLI by the scripts beside them in `tests/corpus`, committed, each
//! rebuilt and checked from nothing against `tests/corpus/manifest.json`
//! (per part what builds and what fails with which code, each solid's
//! hash and counts, each assembly's occurrences and solve, and `check
//! --interference`'s findings by code); and, when `scripts/fetch-corpus.sh` has fetched
//! them, openly licensed STEP assemblies pinned in
//! `tests/corpus/pinned.txt`, imported into a new project each, which
//! must build no fewer parts than recorded there (the `slow` ones only
//! when run by name), and whose every part that builds must build to the
//! same counts exported to STEP and imported again, but for the known
//! failures its `roundtrip` lines name with their issues.
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
    // Interference runs only when asked (design 525): the manifest
    // records it, so the corpus asks.
    let checked = report(call(&mut s, "check", json!({"interference": true})));
    let mut codes = Map::new();
    for f in checked["findings"].as_array().unwrap() {
        let code = f["code"].as_str().unwrap().to_owned();
        let n = codes.get(&code).and_then(Value::as_u64).unwrap_or(0);
        codes.insert(code, json!(n + 1));
    }
    json!({
        "parts": parts,
        "check": {"errors": checked["errors"], "warnings": checked["warnings"], "codes": codes},
        "drawings": drawings(&mut s),
    })
}

/// Each drawing of the project (design 417, plan 9.D), by `<part>/<id>`:
/// its views with their pieces drawn, its annotations' texts, its
/// findings by code, each sheet's score, and the BLAKE3 of its PDF as the
/// daemon writes it: the bytes are the same on every platform.
fn drawings(s: &mut Session) -> Value {
    let out_dir = tempfile::tempdir().unwrap();
    let listed = call(s, "drawing.list", json!({})).unwrap();
    let mut out = Map::new();
    for d in listed["drawings"].as_array().unwrap() {
        let (part, id) = (d["part"].as_str().unwrap(), d["drawing"].as_str().unwrap());
        let got = call(s, "drawing.get", json!({"part": part, "drawing": id})).unwrap();
        let mut views = Map::new();
        let mut notes = Map::new();
        for sheet in got["sheets"].as_array().unwrap() {
            for v in sheet["views"].as_array().unwrap() {
                views.insert(
                    v["view"].as_str().unwrap().into(),
                    json!({"type": v["type"], "scale": v["scale"], "pieces": v["pieces"],
                           "failed": v.get("error").map(|e| e["code"].clone())}),
                );
            }
            for a in sheet["annotations"].as_array().unwrap() {
                notes.insert(a["annotation"].as_str().unwrap().into(), a["text"].clone());
            }
        }
        let mut codes = Map::new();
        for f in got["findings"].as_array().unwrap() {
            let code = f["code"].as_str().unwrap().to_owned();
            let n = codes.get(&code).and_then(Value::as_u64).unwrap_or(0);
            codes.insert(code, json!(n + 1));
        }
        let pdf = out_dir.path().join(format!("{part}-{id}.pdf"));
        call(
            s,
            "drawing.export",
            json!({"part": part, "drawing": id, "out": pdf.to_str().unwrap()}),
        )
        .unwrap();
        let bytes = std::fs::read(&pdf).unwrap();
        std::fs::remove_file(&pdf).unwrap();
        out.insert(
            format!("{part}/{id}"),
            json!({"views": views, "annotations": notes, "findings": codes,
                   "scores": got["scores"], "pdf": blake3::hash(&bytes).to_hex().to_string()}),
        );
    }
    Value::Object(out)
}

/// Views of the committed drawings past design 484's bar, each with its
/// issue: the crankcase's, whose rays meet mirrored bosses' edges exactly
/// (plan 17.B's tally).
const KNOWN_UNCERTAIN: &[(&str, &str)] = &[
    ("radial-engine/crankcase/main/v1", "#643"),
    ("radial-engine/crankcase/main/v2", "#643"),
    ("radial-engine/crankcase/main/v3", "#643"),
];

/// Every view of every committed drawing draws, with at most a tenth of
/// its pieces uncertain (design 484, plan 17.B), unless it is in
/// [`KNOWN_UNCERTAIN`].
fn views_certify(now: &Value) {
    for (project, p) in now["projects"].as_object().unwrap() {
        for (drawing, d) in p["drawings"].as_object().unwrap() {
            for (id, v) in d["views"].as_object().unwrap() {
                let key = format!("{project}/{drawing}/{id}");
                if KNOWN_UNCERTAIN.iter().any(|(k, _)| *k == key) {
                    continue;
                }
                assert!(v["failed"].is_null(), "{key}: {}", v["failed"]);
                let n = |k: &str| v["pieces"][k].as_u64().unwrap_or(0);
                let all = n("visible") + n("hidden") + n("uncertain");
                assert!(
                    n("uncertain") * 10 <= all,
                    "{key}: {} of {all} uncertain",
                    n("uncertain")
                );
            }
        }
    }
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
    views_certify(&now);
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
/// name only): (parts at least, the file in the cache). `RYUCAD_PINNED`
/// keeps only the files whose path holds it (one Voron file at a time).
fn pinned(kind: &str) -> Vec<(usize, String)> {
    let prefix = format!("{kind} ");
    let only = std::env::var("RYUCAD_PINNED").unwrap_or_default();
    std::fs::read_to_string(corpus().join("pinned.txt"))
        .unwrap()
        .lines()
        .filter_map(|l| l.strip_prefix(prefix.as_str()))
        .map(|l| {
            let (n, path) = l.split_once(' ').unwrap();
            (n.parse().unwrap(), path.to_owned())
        })
        .filter(|(_, path)| path.contains(only.as_str()))
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

/// `pinned.txt`'s `roundtrip <part> #<issue> <file>` lines: parts of
/// `file` known not to build to the same counts through STEP and back,
/// each with the open issue that says why.
fn known_round_trips(file: &str) -> std::collections::BTreeSet<String> {
    std::fs::read_to_string(corpus().join("pinned.txt"))
        .unwrap()
        .lines()
        .filter_map(|l| l.strip_prefix("roundtrip "))
        .filter_map(|l| {
            let mut words = l.splitn(3, ' ');
            let (part, issue, path) = (words.next()?, words.next()?, words.next()?);
            assert!(issue.starts_with('#'), "pinned.txt: {l}: no issue");
            (path == file).then(|| part.to_owned())
        })
        .collect()
}

/// Each pinned assembly of `pinned.txt`'s `builds` lines, imported into a
/// project of its own, builds no fewer parts than it records; every part
/// that builds, exported and imported again into a second project, builds
/// to the same counts, but for the parts its `roundtrip` lines name, which
/// must still fail (a line whose part now passes is removed). Every part
/// is tried before the test fails, and every mismatch is listed. Skipped (with a note) when the file hasn't been
/// fetched, unless `RYUCAD_REQUIRE_CORPUS` is set.
#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn pinned_assemblies_build_no_fewer_parts() {
    check_pinned(pinned("builds"));
}

/// The same for the `slow` lines (files whose import takes over half an
/// hour: the Voron 2.4 takes hours, most of it healing one threaded
/// insert's 48 copies), run by name:
/// `cargo test -p ryucad --release --test corpus -- --ignored slow_pinned`.
#[test]
#[ignore = "hours: run by name"]
fn slow_pinned_assemblies_build_no_fewer_parts() {
    check_pinned(pinned("slow"));
}

/// Every file's faults are gathered and the test fails once at the end,
/// listing them all: one file's mismatches mustn't keep the files after
/// it from being read (each takes from 40 minutes to hours).
fn check_pinned(files: Vec<(usize, String)>) {
    let require = std::env::var_os("RYUCAD_REQUIRE_CORPUS").is_some();
    let mut faults = Vec::new();
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
        let answer = call(&mut s, "import", json!({"solid": true, "path": path})).unwrap();
        let built = built_parts(&mut s);
        let parts = answer["parts"].as_array().map_or(0, Vec::len);
        println!(
            "{file}: {} of {parts} parts and assemblies made build, in {:.0} s",
            built.len(),
            t.elapsed().as_secs_f64()
        );
        if built.len() < at_least {
            faults.push(format!(
                "{file}: {} parts build, pinned.txt records {at_least}",
                built.len()
            ));
        }
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
        let known = known_round_trips(&file);
        let (mut new, mut passing, mut failing) = (Vec::new(), Vec::new(), 0);
        for part in &built {
            let step = out.path().join(format!("{part}.step"));
            let moved = call(&mut s, "export", json!({"part": part, "out": step})).and_then(|_| {
                call(
                    &mut b,
                    "import",
                    json!({"solid": true, "path": step, "part": part}),
                )
            });
            let (back, here) = match moved {
                Ok(_) => (counts(&mut b, part), counts(&mut s, part)),
                // A part that can't be written or read again doesn't come
                // back either.
                Err(e) => (vec![json!(e.code)], counts(&mut s, part)),
            };
            match (back == here, known.contains(part)) {
                (true, true) => passing.push(part.clone()),
                (false, false) => new.push(format!("{part}: {here:?} came back as {back:?}")),
                (false, true) => failing += 1,
                (true, false) => {}
            }
        }
        println!(
            "  {} parts exported and imported again, in {:.0} s in all; {failing} known not to \
             come back",
            built.len(),
            t.elapsed().as_secs_f64()
        );
        for n in &new {
            println!("  doesn't come back through STEP: {n}");
        }
        for p in &passing {
            println!("  comes back now; remove its roundtrip line: {p}");
        }
        faults.extend(
            new.into_iter()
                .map(|n| format!("{file}: doesn't come back: {n}")),
        );
        faults.extend(
            passing
                .into_iter()
                .map(|p| format!("{file}: {p} comes back now; remove its roundtrip line")),
        );
    }
    assert!(faults.is_empty(), "{faults:#?}");
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

/// Every committed project's feature files, written by older builds, write
/// back byte for byte: a file without inserts or deletes keeps its format
/// and its bytes (ryucad 25).
#[test]
fn every_committed_feature_file_writes_back_byte_for_byte() {
    let mut seen = 0;
    for e in std::fs::read_dir(corpus()).unwrap().flatten() {
        let root = e.path();
        if !root.join("ryucad.json").is_file() {
            continue;
        }
        let project = ryucad::doc::project::Project::load(&root).unwrap();
        for (p, part) in &project.parts {
            for (f, feature) in &part.features {
                let Ok(feature) = feature else { continue };
                let path = root.join(format!("parts/{p}/features/{f}.json"));
                let bytes = std::fs::read(&path).unwrap();
                let out = canonical::to_string(&feature.to_json());
                let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
                let out = out.strip_suffix('\n').unwrap_or(&out);
                assert_eq!(out.as_bytes(), bytes, "{}", path.display());
                seen += 1;
            }
        }
    }
    assert!(seen > 0);
}
