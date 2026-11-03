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
