//! The disk cache beneath sessions: a session opened over a warm cache
//! builds no body and gives the same hashes as the one that filled it;
//! a join, cut or hole's own body comes from it when only what it acts
//! on changed; a corrupt entry is a miss, rebuilt and written again; and
//! trimming only costs rebuilds.

use ryucad::cache::Disk;
use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::{handle, init};
use ryucad::session::Session;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn id(s: &str) -> Id {
    Id::parse(s, "part").unwrap()
}

fn open(root: &Path, cache: &Path) -> Session {
    Session::open(root)
        .unwrap()
        .with_disk(Some(Arc::new(Disk::open(cache, [3; 32]))))
}

/// Part `p`: a plate, a sketch on its top with a point, a blind hole
/// there and a row of three; and an assembly placing `p` twice.
fn project(root: &Path, cache: &Path) -> Session {
    init(root, None).unwrap();
    let mut s = open(root, cache);
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "plate",
               "params": {"x": 40, "y": 30, "z": 5}}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "s",
               "placement": {"origin": [0, 0, 5]}}),
    );
    let ops = json!([
        {"op": "add", "type": "point", "at": [10, 10]},
        {"op": "add", "type": "fix", "a": "p1"},
    ]);
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": ops}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "h", "sketch": "s",
               "diameter": 3, "depth": {"blind": 3}}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "pattern", "id": "row", "features": ["h"],
               "direction": "x", "spacing": 5, "count": 3}),
    );
    must(
        &mut s,
        "part.create",
        json!({"part": "pair", "kind": "assembly"}),
    );
    for (i, x) in [("a", 0), ("b", 50)] {
        must(
            &mut s,
            "feature.add",
            json!({"part": "pair", "type": "instance", "id": i, "of": "p",
                   "placement": {"origin": [x, 0, 0]}}),
        );
    }
    s
}

fn hashes(s: &mut Session) -> Vec<String> {
    ["p", "pair"]
        .iter()
        .map(|p| s.rebuilt(&id(p)).unwrap().hash.clone())
        .collect()
}

fn names(ids: &[Id]) -> Vec<&str> {
    ids.iter().map(Id::as_str).collect()
}

#[test]
fn a_warm_cache_builds_no_body_and_gives_the_same_hashes() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut first = project(dir.path(), cache.path());
    let want = hashes(&mut first);
    let disk = Disk::open(cache.path(), [3; 32]);
    let stats = disk.stats();
    assert_eq!(stats.kinds[0].1, 3, "the plate, the hole and the row");
    assert_eq!(stats.kinds[1].1, 1, "the hole's own body");
    let mut second = open(dir.path(), cache.path());
    assert_eq!(hashes(&mut second), want);
    let r = second.rebuilt(&id("p")).unwrap();
    assert_eq!(names(&r.built), ["s"], "only the sketch is solved again");
    assert_eq!(names(&r.disk), ["plate", "h", "row"]);
    assert!(r.tools.is_empty());
    // Another build's cache is another key space.
    let mut other = Session::open(dir.path())
        .unwrap()
        .with_disk(Some(Arc::new(Disk::open(cache.path(), [4; 32]))));
    assert_eq!(hashes(&mut other), want);
    let r = other.rebuilt(&id("p")).unwrap();
    assert_eq!(names(&r.built), ["plate", "s", "h", "row"]);
    // No disk at all: the same again.
    let mut none = Session::open(dir.path()).unwrap();
    assert_eq!(hashes(&mut none), want);
}

#[test]
fn a_tools_own_body_comes_from_the_cache_when_its_target_changes() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut first = project(dir.path(), cache.path());
    hashes(&mut first);
    // Another session widens the plate: the hole acts on a new plate, but
    // its own body is the one the first session built.
    let mut second = open(dir.path(), cache.path());
    must(
        &mut second,
        "feature.edit",
        json!({"part": "p", "id": "plate", "params": {"x": 50}}),
    );
    let got = hashes(&mut second);
    let r = second.rebuilt(&id("p")).unwrap();
    // The edit solved the sketch already, to keep it current.
    assert_eq!(names(&r.built), ["plate", "h", "row"]);
    assert!(
        r.tools.is_empty(),
        "the hole's own body came from the cache"
    );
    let mut none = Session::open(dir.path()).unwrap();
    assert_eq!(hashes(&mut none), got);
    assert_eq!(names(&none.rebuilt(&id("p")).unwrap().tools), ["h"]);
}

#[test]
fn a_corrupt_entry_is_a_miss_and_is_written_again() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut first = project(dir.path(), cache.path());
    let want = hashes(&mut first);
    let mut files = Vec::new();
    for e in walk(cache.path()) {
        let mut b = std::fs::read(&e).unwrap();
        let last = b.len() - 1;
        b[last] ^= 0x55;
        std::fs::write(&e, b).unwrap();
        files.push(e);
    }
    assert_eq!(files.len(), 4);
    let mut second = open(dir.path(), cache.path());
    assert_eq!(hashes(&mut second), want);
    let r = second.rebuilt(&id("p")).unwrap();
    assert!(r.disk.is_empty());
    assert_eq!(names(&r.built), ["plate", "s", "h", "row"]);
    let disk = Disk::open(cache.path(), [3; 32]);
    assert_eq!(disk.stats().entries, 4, "written again");
    let mut third = open(dir.path(), cache.path());
    assert_eq!(hashes(&mut third), want);
    assert_eq!(
        names(&third.rebuilt(&id("p")).unwrap().disk),
        ["plate", "h", "row"]
    );
}

#[test]
fn trimming_only_costs_rebuilds() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut first = project(dir.path(), cache.path());
    let want = hashes(&mut first);
    let disk = Disk::open(cache.path(), [3; 32]);
    let before = disk.stats();
    assert!(disk.trim(before.bytes / 2) > 0);
    assert!(disk.stats().bytes <= before.bytes / 2);
    let mut second = open(dir.path(), cache.path());
    assert_eq!(hashes(&mut second), want);
    assert_eq!(disk.clear().entries, 4, "every entry written back");
    let mut third = open(dir.path(), cache.path());
    assert_eq!(hashes(&mut third), want);
    assert!(third.rebuilt(&id("p")).unwrap().disk.is_empty());
}

/// Every entry file under `root`.
fn walk(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                todo.push(e.path());
            } else {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}
