//! The `cox-files:` source end to end, against `fake-cox` (which serves
//! sets from folders) and `fake-ryucad`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn occt(name: &str) -> PathBuf {
    root().join("crates/ryustep/tests/data/occt").join(name)
}

/// A set `owner` of two versions under `dir/sets`: v1 holds a box and a
/// cone, v2 the box again (the same bytes) and a cylinder under the
/// cone's name.
fn sets(dir: &Path) -> PathBuf {
    let sets = dir.join("sets");
    for (v, files) in [
        (
            "owner@v1",
            [("a.stp", "box_ap214.stp"), ("b.stp", "cone.stp")],
        ),
        (
            "owner@v2",
            [("a.stp", "box_ap214.stp"), ("b.stp", "cylinder.stp")],
        ),
    ] {
        std::fs::create_dir_all(sets.join(v)).unwrap();
        for (to, from) in files {
            std::fs::copy(occt(from), sets.join(v).join(to)).unwrap();
        }
    }
    sets
}

fn cox(dir: &Path, args: &[&str]) -> Value {
    let o = Command::new(env!("CARGO_BIN_EXE_fake-cox"))
        .args(args)
        .env("FAKE_COX_SETS", sets(dir))
        .output()
        .unwrap();
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn the_fake_shows_and_pulls_as_cox_does() {
    let dir = tempfile::tempdir().unwrap();
    let v = cox(dir.path(), &["--json", "files", "show", "owner"]);
    assert_eq!(v["version"], 2);
    let e = &v["entries"][1];
    assert_eq!(e["path"], "b.stp");
    assert_eq!(
        e["size"],
        std::fs::metadata(occt("cylinder.stp")).unwrap().len()
    );
    assert_eq!(
        e["sha256"],
        step_hunt::cox_files::sha256_hex(&std::fs::read(occt("cylinder.stp")).unwrap())
    );
    let v = cox(dir.path(), &["--json", "files", "pull", "owner@v1"]);
    assert!(
        PathBuf::from(v["path"].as_str().unwrap())
            .join("b.stp")
            .is_file()
    );
    let v = cox(dir.path(), &["--json", "files", "show", "owner@v3"]);
    assert_eq!(v["error"]["code"], "not_found");
}

use step_hunt::cox_files::{self, Cox, Root};

fn fake(sets: Option<PathBuf>, lie: Option<&str>) -> Cox {
    let mut env = Vec::new();
    if let Some(s) = sets {
        env.push(("FAKE_COX_SETS".to_owned(), s.into_os_string()));
    }
    if let Some(l) = lie {
        env.push(("FAKE_COX_LIE".to_owned(), l.into()));
    }
    Cox {
        exe: env!("CARGO_BIN_EXE_fake-cox").into(),
        env,
    }
}

fn root_of(r: &str) -> Root {
    Root::parse(r).unwrap().unwrap()
}

#[test]
fn a_set_is_pulled_up_to_its_version_and_linked() {
    let dir = tempfile::tempdir().unwrap();
    let cox = fake(Some(sets(dir.path())), None);
    let p = cox_files::materialize(
        &cox,
        &root_of("cox-files:owner@v2"),
        &dir.path().join("links"),
    )
    .unwrap();
    assert_eq!((p.set.as_str(), p.version, p.pinned), ("owner", 2, true));
    assert_eq!(p.root, dir.path().join("links/owner@v2"));
    let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["a.stp", "b.stp", "v2/b.stp"]);
    for e in &p.entries {
        let bytes = std::fs::read(p.root.join(&e.name)).unwrap();
        assert_eq!(cox_files::sha256_hex(&bytes), e.sha256);
    }
    assert!(p.mismatched.is_empty());
    // v1 alone, and the latest.
    let p = cox_files::materialize(
        &cox,
        &root_of("cox-files:owner@v1"),
        &dir.path().join("links"),
    )
    .unwrap();
    assert_eq!(p.entries.len(), 2);
    let p = cox_files::materialize(&cox, &root_of("cox-files:owner"), &dir.path().join("links"))
        .unwrap();
    assert_eq!((p.version, p.pinned), (2, false));
}

#[test]
fn a_file_whose_bytes_differ_is_listed_not_linked() {
    let dir = tempfile::tempdir().unwrap();
    let cox = fake(Some(sets(dir.path())), Some("a.stp"));
    let p = cox_files::materialize(
        &cox,
        &root_of("cox-files:owner@v1"),
        &dir.path().join("links"),
    )
    .unwrap();
    assert_eq!(p.mismatched.len(), 1);
    let m = &p.mismatched[0];
    assert_eq!(
        (m.name.as_str(), m.expected.as_str()),
        ("a.stp", "0".repeat(64).as_str())
    );
    assert!(!p.root.join("a.stp").exists());
    assert!(p.root.join("b.stp").is_file());
}

#[test]
fn no_cox_no_login_or_no_set_is_an_error_to_skip_on() {
    let dir = tempfile::tempdir().unwrap();
    let links = dir.path().join("links");
    let gone = Cox {
        exe: dir.path().join("no-cox"),
        env: Vec::new(),
    };
    assert!(cox_files::materialize(&gone, &root_of("cox-files:owner@v1"), &links).is_err());
    let out = fake(None, None);
    assert!(
        cox_files::materialize(&out, &root_of("cox-files:owner@v1"), &links)
            .unwrap_err()
            .contains("not signed in")
    );
    let cox = fake(Some(sets(dir.path())), None);
    assert!(cox_files::materialize(&cox, &root_of("cox-files:other@v1"), &links).is_err());
    assert!(cox_files::materialize(&cox, &root_of("cox-files:owner@v9"), &links).is_err());
}

fn step_hunt(dir: &Path, args: &[&str], env: &[(&str, &Path)]) -> Output {
    let sources = dir.join("sources.txt");
    std::fs::write(
        &sources,
        "source owner cox-files:owner@v2 **/* LicenseRef-owner-private whole\n",
    )
    .unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_step-hunt"));
    cmd.args(args)
        .arg("--root")
        .arg(root())
        .arg("--sources")
        .arg(&sources)
        .env("HUNT_COX", env!("CARGO_BIN_EXE_fake-cox"))
        .env("HUNT_COX_FILES", dir.join("links"))
        .env_remove("FAKE_COX_SETS")
        .env_remove("FAKE_COX_LIE");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

#[test]
fn split_reads_a_set_as_any_root() {
    let dir = tempfile::tempdir().unwrap();
    let sets = sets(dir.path());
    let out = dir.path().join("split");
    let o = step_hunt(
        dir.path(),
        &["split", "--out", out.to_str().unwrap()],
        &[("FAKE_COX_SETS", &sets)],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&std::fs::read(out.join("cases.json")).unwrap()).unwrap();
    let mut files: Vec<String> = v["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "whole")
        .map(|c| c["origins"][0]["file"].as_str().unwrap().to_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["a.stp", "b.stp", "v2/b.stp"],
        "every file once, whole"
    );
}

#[test]
fn without_cox_the_source_is_skipped_or_required() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("split");
    let o = step_hunt(dir.path(), &["split", "--out", out.to_str().unwrap()], &[]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("not signed in"));
    let o = step_hunt(
        dir.path(),
        &["split", "--require", "--out", out.to_str().unwrap()],
        &[],
    );
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn a_file_whose_hash_differs_is_an_open_finding_and_the_run_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let sets = sets(dir.path());
    let out = dir.path().join("out");
    let o = step_hunt(
        dir.path(),
        &[
            "run",
            "--ryucad",
            env!("CARGO_BIN_EXE_fake-ryucad"),
            "--ledger",
            ledger(dir.path(), &sets, None).to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--timeout",
            "5",
        ],
        &[
            ("FAKE_COX_SETS", &sets),
            ("FAKE_COX_LIE", Path::new("a.stp")),
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value =
        serde_json::from_slice(&std::fs::read(out.join("report.json")).unwrap()).unwrap();
    let sigs: Vec<&str> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["signature"].as_str().unwrap())
        .collect();
    assert_eq!(sigs, ["open/cox_files_hash_mismatch"]);
    assert!(
        v["cases"].as_u64().unwrap() > 1,
        "the other files still ran"
    );
}

fn ledger(_dir: &Path, _sets: &Path, _lie: Option<&str>) -> PathBuf {
    PathBuf::from("/nonexistent")
}
