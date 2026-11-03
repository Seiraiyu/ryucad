//! The `cox-files:` source end to end, against `fake-cox` (which serves
//! sets from folders) and `fake-ryucad`.

use std::path::{Path, PathBuf};
use std::process::Command;

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
