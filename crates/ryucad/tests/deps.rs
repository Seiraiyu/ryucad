//! Three repositories on disk, through the CLI as their agents would
//! drive them: a library of fasteners, a subassembly repository placing
//! one of its releases, and a product placing both. A release in the
//! library, fetched, placed and released in the product; then the
//! library's history rewritten under the product's lock.

use assert_cmd::Command;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Runs ryucad in process in `dir`, sharing one machine's dependency
/// cache, and returns the envelope.
fn run(dir: &Path, cache: &Path, args: &[&str]) -> Value {
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .current_dir(dir)
        .args(args)
        .env("RYUCAD_NO_DAEMON", "1")
        .env("RYUCAD_CONFIG_DIR", cache.join("config"))
        .env("RYUCAD_CACHE_DIR", cache.join("derived"))
        .env("RYUCAD_DEPS_DIR", cache.join("deps"))
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn ok(dir: &Path, cache: &Path, args: &[&str]) -> Value {
    let v = run(dir, cache, args);
    assert_eq!(v["ok"], true, "{args:?}: {v}");
    v["result"].clone()
}

/// Commits everything in `dir`.
fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A new project in a repository at `dir`, on `main`.
fn repo(dir: &Path, cache: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    ok(dir, cache, &["init", "--unit", "mm"]);
}

/// Releases `part` as its next label and commits it.
fn release(dir: &Path, cache: &Path, part: &str) {
    ok(dir, cache, &["release", "propose", part]);
    commit(dir, &format!("release {part}"));
}

fn url(dir: &Path) -> String {
    format!("file://{}", dir.display())
}

#[test]
fn three_repositories_place_each_others_releases() {
    let top = tempfile::tempdir().unwrap();
    let cache = top.path().join("machine");
    let (lib, sub, product) = (
        top.path().join("fasteners"),
        top.path().join("frames"),
        top.path().join("rig"),
    );
    // The library: a bolt, released as A.
    repo(&lib, &cache);
    ok(&lib, &cache, &["part", "create", "bolt"]);
    ok(
        &lib,
        &cache,
        &[
            "feature",
            "add",
            "bolt",
            "cylinder",
            "--id",
            "shank",
            "--param",
            "radius=3",
            "--param",
            "height=20",
        ],
    );
    ok(&lib, &cache, &["props", "set", "bolt", "number=B-6"]);
    commit(&lib, "bolt");
    release(&lib, &cache, "bolt");

    // The subassembly: a frame placing the library's bolt at A, released.
    repo(&sub, &cache);
    ok(&sub, &cache, &["deps", "add", &url(&lib)]);
    ok(&sub, &cache, &["part", "create", "frame", "--assembly"]);
    ok(
        &sub,
        &cache,
        &[
            "feature",
            "add",
            "frame",
            "instance",
            "--id",
            "b",
            "--of",
            "fasteners:bolt",
            "--rev",
            "A",
        ],
    );
    ok(&sub, &cache, &["props", "set", "frame", "number=F-1"]);
    commit(&sub, "frame");
    release(&sub, &cache, "frame");

    // The product places the frame at A, and through it the bolt.
    repo(&product, &cache);
    ok(&product, &cache, &["deps", "add", &url(&sub)]);
    ok(&product, &cache, &["part", "create", "rig", "--assembly"]);
    // The library is already in the machine's cache (the subassembly
    // fetched it), so the frame and the bolt through it lock at once.
    ok(
        &product,
        &cache,
        &[
            "feature",
            "add",
            "rig",
            "instance",
            "--id",
            "f",
            "--of",
            "frames:frame",
            "--rev",
            "A",
        ],
    );
    let lock: Value =
        serde_json::from_str(&fs::read_to_string(product.join("ryucad.lock")).unwrap()).unwrap();
    let projects = lock["projects"].as_object().unwrap();
    assert_eq!(projects.len(), 2, "{lock}");
    let transitive = projects
        .values()
        .find(|p| !p["via"].as_array().unwrap().is_empty())
        .unwrap();
    assert_eq!(transitive["url"], url(&lib));
    assert!(transitive["labels"]["bolt/A"]["commit"].is_string());
    // It rebuilds, two levels deep.
    let tree = ok(&product, &cache, &["assembly", "tree", "rig"]);
    assert_eq!(tree["instances"][0]["kind"], "assembly", "{tree}");
    assert_eq!(
        tree["instances"][0]["instances"][0]["part"], "bolt",
        "{tree}"
    );
    // #386: an assembly placing another project's parts has an
    // appearance, and the dependency's parts take none of this project's.
    let looks = ok(&product, &cache, &["appearance", "rig"]);
    assert_eq!(looks["solids"].as_array().unwrap().len(), 1, "{looks}");
    assert!(looks["revisions"].get("bolt").is_none(), "{looks}");
    // #383: on a machine that fetched nothing, the instance says so and
    // names the command, in a rebuild and in an export's failures.
    let bare = top.path().join("bare");
    let v = run(&product, &bare, &["rebuild", "rig"]);
    let f = &v["error"]["details"]["parts"][0]["features"][0];
    assert_eq!(f["error"]["code"], "dependency_not_fetched", "{v}");
    let message = f["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("f: frame@A of "), "{v}");
    assert!(message.contains("ryucad deps fetch"), "{v}");
    let v = run(&product, &bare, &["export", "rig", "--out", "x.3mf"]);
    assert_eq!(v["error"]["code"], "export_failed", "{v}");
    assert_eq!(
        v["error"]["details"]["failing"][0]["code"], "dependency_not_fetched",
        "{v}"
    );
    // Released: every instance at any depth is an official release.
    ok(&product, &cache, &["props", "set", "rig", "number=R-1"]);
    ok(&product, &cache, &["bom", "renumber", "rig", "--compact"]);
    commit(&product, "rig");
    let r = ok(
        &product,
        &cache,
        &["release", "propose", "rig", "--dry-run"],
    );
    assert_eq!(r["problems"], json!([]), "{r}");

    // The library's history rewritten: a new main releasing A again.
    git(&lib, &["checkout", "-q", "--orphan", "rewritten"]);
    git(&lib, &["commit", "-q", "-m", "again"]);
    git(&lib, &["branch", "-f", "main", "rewritten"]);
    git(&lib, &["checkout", "-q", "main"]);
    ok(&product, &cache, &["deps", "fetch"]);
    // The frame still reads (its own lock entry stands); the bolt under it
    // fails, never followed to the rewritten history.
    let tree = ok(&product, &cache, &["assembly", "tree", "rig"]);
    let bolt = &tree["instances"][0]["instances"][0];
    // The instance's own error (#383), naming what it places.
    assert_eq!(bolt["error"]["code"], "dependency_rewritten", "{tree}");
    let r = ok(
        &product,
        &cache,
        &["release", "propose", "rig", "--dry-run"],
    );
    assert!(
        r["problems"].to_string().contains("dependency_rewritten"),
        "{r}"
    );
    // #377: `deps update` is the decision that waits for; it moves the lock
    // to where the rewritten branch makes the label official, and says so.
    let again = git(&lib, &["rev-parse", "HEAD"]).trim().to_owned();
    let v = ok(&product, &cache, &["deps", "update", "--offline"]);
    assert_eq!(v["relocked"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["relocked"][0]["label"], "A", "{v}");
    assert_eq!(v["relocked"][0]["to"], again.as_str(), "{v}");
    assert_eq!(v["lock_changed"], true, "{v}");
    let tree = ok(&product, &cache, &["assembly", "tree", "rig"]);
    let bolt = &tree["instances"][0]["instances"][0];
    assert!(bolt.get("error").is_none(), "{tree}");
    // Nothing more to move.
    let v = ok(&product, &cache, &["deps", "update", "--offline"]);
    assert_eq!(v["relocked"], json!([]), "{v}");
}
