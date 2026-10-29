//! The disk cache beneath sessions: a session opened over a warm cache
//! builds no body and gives the same hashes as the one that filled it;
//! a join, cut or hole's own body comes from it when only what it acts
//! on changed; a corrupt entry is a miss, rebuilt and written again;
//! trimming only costs rebuilds; and a shared cache server fills an empty
//! cache on another machine, in a batch a depth.

mod fake_cache;

use ryucad::cache::remote::{Options, Remote};
use ryucad::cache::{Disk, Identity};
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

/// A session whose disk cache, in `cache`, has `fake` behind it, both
/// keyed by a shared identity.
fn shared(root: &Path, cache: &Path, fake: &fake_cache::Fake) -> Session {
    let remote = Remote::new(Options {
        url: fake.url.clone(),
        token: Some(fake_cache::WRITE.to_owned()),
        write: true,
        timeout: std::time::Duration::from_secs(5),
        backoff: std::time::Duration::from_millis(100),
        identity: [3; 32],
    });
    let disk = Disk::open_as(
        cache,
        Identity {
            bytes: [3; 32],
            shared: true,
        },
    )
    .with_remote(Some(remote));
    Session::open(root).unwrap().with_disk(Some(Arc::new(disk)))
}

fn flushed(s: &Session) {
    let remote = s.disk().unwrap().remote().unwrap();
    assert!(remote.flush(std::time::Duration::from_secs(30)));
}

#[test]
fn a_remote_fills_an_empty_cache_a_depth_at_a_time() {
    let fake = fake_cache::Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let (scratch, a, b) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    drop(project(dir.path(), scratch.path()));
    // One machine builds and meshes, and its cache sends it all behind.
    let mut first = shared(dir.path(), a.path(), &fake);
    let want = hashes(&mut first);
    must(&mut first, "mesh.get", json!({"part": "p"}));
    flushed(&first);
    let held = fake.held();
    assert_eq!(
        held,
        3 + 1 + 1,
        "three results, the hole's own body, a mesh"
    );
    // Its misses were asked for a depth at a time; only the hole's own
    // body (read when its result misses) one by one.
    assert_eq!(fake.count("GET", "/v1/features/"), 0);
    assert_eq!(fake.count("GET", "/v1/meshes/"), 0);
    assert_eq!(fake.count("GET", "/v1/tools/"), 1);
    let gets = fake.count("POST", "/v1/get");
    // Another, empty, builds nothing and asks in batches.
    let mut second = shared(dir.path(), b.path(), &fake);
    assert_eq!(hashes(&mut second), want);
    let r = second.rebuilt(&id("p")).unwrap();
    assert_eq!(names(&r.built), ["s"], "only the sketch is solved again");
    assert_eq!(names(&r.disk), ["plate", "h", "row"]);
    must(&mut second, "mesh.get", json!({"part": "p"}));
    assert_eq!(
        fake.count("GET", "/v1/features/"),
        0,
        "batched, never one by one"
    );
    assert_eq!(fake.count("GET", "/v1/meshes/"), 0);
    assert_eq!(fake.count("GET", "/v1/tools/"), 1, "its result came whole");
    assert!(
        fake.count("POST", "/v1/get") >= gets + 2,
        "the depths and the mesh"
    );
    flushed(&second);
    assert_eq!(fake.held(), held, "it had nothing new to send");
    assert_eq!(fake.count("PUT", "/v1/"), held);
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

/// `ryucad <args>` from `root` through its daemon, with `cache` as the
/// disk cache and `run` holding the daemons' sockets: the parsed answer.
fn cli(root: &Path, run: &Path, cache: &Path, args: &[&str]) -> Value {
    let v = cli_with(root, run, cache, &[], args);
    assert_eq!(v["ok"], true, "{args:?}: {v}");
    v["result"].clone()
}

/// [`cli`] with more of the environment set: the whole envelope.
fn cli_with(root: &Path, run: &Path, cache: &Path, env: &[(&str, &str)], args: &[&str]) -> Value {
    let out = std::process::Command::new(assert_cmd::cargo::cargo_bin("ryucad"))
        .args(args)
        .current_dir(root)
        .env("RYUCAD_RUNTIME_DIR", run)
        .env("RYUCAD_CONFIG_DIR", run.with_file_name("config"))
        .env("RYUCAD_CACHE_DIR", cache)
        .env("RYUCAD_IDLE_TIMEOUT", "60")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("{args:?}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// A release's identity, claimed for the development build under test.
const RELEASE: &str = "0707070707070707070707070707070707070707070707070707070707070707";

/// A project at `root` with part `p`: a plate and a cylinder cut from it
/// (a feature entry each, the cut's own body a tool entry).
fn plate(root: &Path, run: &Path, cache: &Path) {
    std::fs::create_dir_all(root).unwrap();
    let c = |args: &[&str]| cli(root, run, cache, args);
    c(&["init"]);
    c(&["part", "create", "p"]);
    c(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=20", "--param", "y=10", "--param",
        "z=5",
    ]);
    c(&[
        "feature",
        "add",
        "p",
        "cylinder",
        "--id",
        "c",
        "--operation",
        "cut",
        "--target",
        "b",
        "--param",
        "radius=2",
        "--param",
        "height=20",
        "--origin",
        "5,5,-5",
    ]);
}

/// The time each entry was last read or written, by path.
fn times(cache: &Path) -> std::collections::BTreeMap<std::path::PathBuf, std::time::SystemTime> {
    walk(cache)
        .into_iter()
        .map(|p| {
            let t = std::fs::metadata(&p).unwrap().modified().unwrap();
            (p, t)
        })
        .collect()
}

#[test]
fn two_daemons_share_one_disk_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (run, cache) = (dir.path().join("run"), dir.path().join("cache"));
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    plate(&a, &run, &cache);
    plate(&b, &run, &cache);
    // The first daemon builds and meshes the part, filling the cache.
    let built = cli(&a, &run, &cache, &["rebuild", "p"]);
    cli(&a, &run, &cache, &["mesh", "p"]);
    let filled = cli(&a, &run, &cache, &["cache", "stats"]);
    let kinds = |s: &Value| {
        ["features", "tools", "meshes"].map(|k| s["kinds"][k]["entries"].as_u64().unwrap())
    };
    assert_eq!(kinds(&filled), [2, 1, 1], "{filled}");
    // Their times put back an hour, so a read shows.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for p in times(&cache).keys() {
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(old).unwrap();
    }
    // The second daemon, over another project with the same part, reads
    // them rather than writing its own: the same entries, all read, and
    // the same results.
    let again = cli(&b, &run, &cache, &["rebuild", "p"]);
    cli(&b, &run, &cache, &["mesh", "p"]);
    assert_eq!(again["parts"][0]["hash"], built["parts"][0]["hash"]);
    let shared = cli(&b, &run, &cache, &["cache", "stats"]);
    assert_eq!(kinds(&shared), [2, 1, 1], "nothing new written: {shared}");
    let read = times(&cache)
        .into_iter()
        .filter(|(p, t)| *t > old && !p.to_string_lossy().contains("/tools/"))
        .count();
    // The cut's own body is read only when what it cuts changes.
    assert_eq!(read, 3, "both features and the mesh were read");
    for root in [&a, &b] {
        cli(root, &run, &cache, &["daemon", "stop"]);
    }
}

#[test]
fn daemons_writing_at_once_leave_sound_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (run, cache) = (dir.path().join("run"), dir.path().join("cache"));
    let roots: Vec<std::path::PathBuf> = (0..4).map(|i| dir.path().join(format!("p{i}"))).collect();
    // Each project made with a cache of its own, so nothing is written to
    // the shared one before the race.
    for r in &roots {
        plate(r, &run, &dir.path().join("setup"));
        cli(r, &run, &dir.path().join("setup"), &["daemon", "stop"]);
    }
    let answers: Vec<Value> = std::thread::scope(|s| {
        let jobs: Vec<_> = roots
            .iter()
            .map(|r| {
                let (run, cache) = (&run, &cache);
                s.spawn(move || {
                    let v = cli(r, run, cache, &["rebuild", "p"]);
                    cli(r, run, cache, &["mesh", "p"]);
                    v
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    for a in &answers {
        assert_eq!(a["parts"][0]["hash"], answers[0]["parts"][0]["hash"]);
    }
    // One entry per result, every one sound, whichever daemon won.
    let disk = Disk::open(&cache, [0; 32]);
    assert_eq!(disk.check(), (4, 0));
    assert_eq!(disk.stats().entries, 4);
    for r in &roots {
        cli(r, &run, &cache, &["daemon", "stop"]);
    }
}

/// Two parts with the same box: each body is named for its own part and
/// feature, though their inputs are alike, so neither may be the other's
/// from the disk.
#[test]
fn alike_features_of_different_parts_keep_their_own_names() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    init(dir.path(), None).unwrap();
    let mut s = open(dir.path(), cache.path());
    for (part, f) in [("p", "b"), ("q", "b"), ("q", "c")] {
        if !s.project().parts.contains_key(&id(part)) {
            must(&mut s, "part.create", json!({"part": part}));
        }
        must(
            &mut s,
            "feature.add",
            json!({"part": part, "type": "box", "id": f, "params": {"x": 1, "y": 2, "z": 3}}),
        );
    }
    let v = must(&mut s, "check", json!({}));
    assert_eq!(v["passed"], true, "{v}");
    let hash = |s: &mut Session, part: &str, f: &str| {
        let v = must(s, "rebuild", json!({"part": part}));
        v["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == f)
            .unwrap()["hash"]
            .clone()
    };
    let (pb, qb, qc) = (
        hash(&mut s, "p", "b"),
        hash(&mut s, "q", "b"),
        hash(&mut s, "q", "c"),
    );
    assert_ne!(pb, qb);
    assert_ne!(qb, qc);
    // A cold session over the warm cache agrees.
    let mut cold = open(dir.path(), cache.path());
    assert_eq!(hash(&mut cold, "q", "c"), qc);
    assert_eq!(hash(&mut cold, "q", "b"), qb);
}

#[test]
fn the_shared_cache_is_set_and_reported_from_the_cli() {
    let fake = fake_cache::Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let (run, cache, a) = (
        dir.path().join("run"),
        dir.path().join("cache"),
        dir.path().join("a"),
    );
    plate(&a, &run, &cache);
    let none = cli(&a, &run, &cache, &["cache", "stats"]);
    assert_eq!(none["remote"], json!({"enabled": false}));
    assert_eq!(none["shared"], false);
    cli(
        &a,
        &run,
        &cache,
        &["config", "set", "cache.remote", &fake.url],
    );
    // A development build never uses it, and says why.
    let dev = cli(&a, &run, &cache, &["cache", "stats"]);
    assert_eq!(dev["remote"]["enabled"], false, "{dev}");
    assert_eq!(dev["remote"]["url"], fake.url.as_str());
    assert!(
        dev["remote"]["note"]
            .as_str()
            .unwrap()
            .contains("development build")
    );
    cli(&a, &run, &cache, &["daemon", "stop"]);
    // A release's daemon reads through and writes behind it.
    let env = [
        ("RYUCAD_CACHE_IDENTITY", RELEASE),
        ("RYUCAD_CACHE_TOKEN", fake_cache::WRITE),
    ];
    for no_daemon in ["0", "1"] {
        let env = [env[0], env[1], ("RYUCAD_NO_DAEMON", no_daemon)];
        let s = cli_with(&a, &run, &cache, &env, &["cache", "stats"]);
        let r = &s["result"]["remote"];
        assert_eq!(r["enabled"], true, "{s}");
        assert_eq!(
            (&r["url"], &r["token"], &r["write"]),
            (&json!(fake.url), &json!(true), &json!(true))
        );
        assert_eq!(s["result"]["shared"], true);
    }
    cli_with(&a, &run, &cache, &env, &["rebuild", "p"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while fake.held() < 3 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        fake.held(),
        3,
        "two features and the cut's own body, sent behind"
    );
    let s = cli_with(&a, &run, &cache, &env, &["cache", "stats"]);
    assert_eq!(s["result"]["remote"]["puts"], 3, "{s}");
    // Reading only.
    cli_with(
        &a,
        &run,
        &cache,
        &env,
        &["config", "set", "cache.remote_write", "false"],
    );
    let s = cli_with(&a, &run, &cache, &env, &["cache", "stats"]);
    assert_eq!(s["result"]["remote"]["write"], false, "{s}");
    // None again, without restarting the daemon.
    cli_with(
        &a,
        &run,
        &cache,
        &env,
        &["config", "set", "cache.remote", "\"\""],
    );
    let s = cli_with(&a, &run, &cache, &env, &["cache", "stats"]);
    assert_eq!(s["result"]["remote"], json!({"enabled": false}), "{s}");
    cli(&a, &run, &cache, &["daemon", "stop"]);
}

#[test]
fn a_token_is_stored_checked_and_removed_from_the_cli() {
    let fake = fake_cache::Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let (run, cache, a) = (
        dir.path().join("run"),
        dir.path().join("cache"),
        dir.path().join("a"),
    );
    plate(&a, &run, &cache);
    // Its daemon is a development build's: the next is a release's.
    cli(&a, &run, &cache, &["daemon", "stop"]);
    let env = [("RYUCAD_CACHE_IDENTITY", RELEASE)];
    let c = |args: &[&str]| {
        let v = cli_with(&a, &run, &cache, &env, args);
        assert_eq!(v["ok"], true, "{args:?}: {v}");
        v["result"].clone()
    };
    c(&["config", "set", "cache.remote", &fake.url]);
    let token = run.with_file_name("config").join("cache-token");
    let r = c(&["cache", "login", "--token", fake_cache::READ]);
    assert_eq!(r["token"], "file");
    assert_eq!(r["server"], json!({"write": false, "entries": 0}), "{r}");
    assert_eq!(r["remote"]["token"], true);
    assert_eq!(std::fs::read_to_string(&token).unwrap(), "read-token\n");
    // From standard input, as a person types it.
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("ryucad"))
        .args(["cache", "login"])
        .current_dir(&a)
        .env("RYUCAD_RUNTIME_DIR", &run)
        .env("RYUCAD_CONFIG_DIR", run.with_file_name("config"))
        .env("RYUCAD_CACHE_DIR", &cache)
        .env("RYUCAD_CACHE_IDENTITY", RELEASE)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    writeln!(child.stdin.take().unwrap(), "{}", fake_cache::WRITE).unwrap();
    let out: Value = serde_json::from_slice(&child.wait_with_output().unwrap().stdout).unwrap();
    assert_eq!(out["result"]["server"]["write"], true, "{out}");
    // A token the server doesn't know is stored, and said so.
    let r = c(&["cache", "login", "--token", "nobody"]);
    assert_eq!(r["server"]["error"]["code"], "cache_remote_refused", "{r}");
    let bad = cli_with(
        &a,
        &run,
        &cache,
        &env,
        &["cache", "login", "--token", "two words"],
    );
    assert_eq!(bad["error"]["code"], "invalid_param", "{bad}");
    let r = c(&["cache", "logout"]);
    assert_eq!(r["token"], Value::Null);
    assert!(!token.exists());
    // The variable wins, and the answer says so.
    let env = [
        ("RYUCAD_CACHE_IDENTITY", RELEASE),
        ("RYUCAD_CACHE_TOKEN", fake_cache::WRITE),
        ("RYUCAD_NO_DAEMON", "1"),
    ];
    let v = cli_with(
        &a,
        &run,
        &cache,
        &env,
        &["cache", "login", "--token", fake_cache::READ],
    );
    assert_eq!(v["result"]["overridden_by"], "RYUCAD_CACHE_TOKEN", "{v}");
    assert_eq!(v["result"]["server"]["write"], true);
    // Logging out removes the stored token, and says the variable's goes on.
    let v = cli_with(&a, &run, &cache, &env, &["cache", "logout"]);
    assert_eq!(v["result"]["removed"], true, "{v}");
    assert_eq!(v["result"]["overridden_by"], "RYUCAD_CACHE_TOKEN", "{v}");
    assert!(!token.exists());
    cli(&a, &run, &cache, &["daemon", "stop"]);
}

#[test]
fn a_release_pushes_what_the_shared_cache_lacks() {
    let fake = fake_cache::Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let (run, cache, a) = (
        dir.path().join("run"),
        dir.path().join("cache"),
        dir.path().join("a"),
    );
    // Built as a release with no shared cache set: nothing sent.
    let release = [
        ("RYUCAD_CACHE_IDENTITY", RELEASE),
        ("RYUCAD_NO_DAEMON", "1"),
    ];
    std::fs::create_dir_all(&a).unwrap();
    for args in [&["init"][..], &["part", "create", "p"]] {
        cli_with(&a, &run, &cache, &release, args);
    }
    let add = [
        "feature", "add", "p", "box", "--id", "b", "--param", "x=20", "--param", "y=10", "--param",
        "z=5",
    ];
    cli_with(&a, &run, &cache, &release, &add);
    cli_with(&a, &run, &cache, &release, &["mesh", "p"]);
    // A development build's entries of the same project are its own.
    cli(&a, &run, &cache, &["rebuild", "p"]);
    cli(&a, &run, &cache, &["daemon", "stop"]);
    let push = |env: &[(&str, &str)], args: &[&str]| {
        let mut all = vec!["cache", "push"];
        all.extend_from_slice(args);
        cli_with(&a, &run, &cache, env, &all)
    };
    let none = push(&release, &[]);
    assert_eq!(none["error"]["code"], "invalid_param", "{none}");
    cli(
        &a,
        &run,
        &cache,
        &["config", "set", "cache.remote", &fake.url],
    );
    let dev = push(&[("RYUCAD_CACHE_TOKEN", fake_cache::WRITE)], &[]);
    assert_eq!(dev["error"]["code"], "invalid_param", "{dev}");
    let read = [
        ("RYUCAD_CACHE_IDENTITY", RELEASE),
        ("RYUCAD_CACHE_TOKEN", fake_cache::READ),
    ];
    assert_eq!(push(&read, &[])["error"]["code"], "cache_remote_refused");
    let write = [
        ("RYUCAD_CACHE_IDENTITY", RELEASE),
        ("RYUCAD_CACHE_TOKEN", fake_cache::WRITE),
    ];
    let meshes = push(&write, &["--kind", "meshes"]);
    assert_eq!(meshes["result"]["pushed"], 1, "{meshes}");
    assert_eq!(fake.held(), 1);
    let all = push(&write, &[]);
    let r = &all["result"];
    assert_eq!(r["pushed"], 1, "the box; the mesh was there: {all}");
    assert_eq!(
        r["kinds"]["meshes"],
        json!({"local": 1, "held": 1, "pushed": 0, "bytes": 0})
    );
    assert_eq!(
        r["kinds"]["features"]["local"], 1,
        "not the development build's"
    );
    assert_eq!(
        push(&write, &[])["result"]["pushed"],
        0,
        "nothing left to send"
    );
    // A server that isn't there.
    fake.with(|s| s.fail = Some(502));
    let down = push(&write, &[]);
    assert_eq!(down["error"]["code"], "cache_remote_unreachable", "{down}");
    let bad = push(&write, &["--kind", "bodies"]);
    assert_eq!(bad["error"]["code"], "usage");
}

#[test]
fn a_warm_fetches_what_the_team_built() {
    let fake = fake_cache::Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    let (ca, cb) = (dir.path().join("cache-a"), dir.path().join("cache-b"));
    plate(&a, &run, &ca);
    plate(&b, &run, &cb);
    cli(&a, &run, &ca, &["config", "set", "cache.remote", &fake.url]);
    // Their daemons are a development build's: the next are a release's.
    for root in [&a, &b] {
        cli(root, &run, &ca, &["daemon", "stop"]);
    }
    let as_release = |root: &Path, cache: &Path, token: &str, args: &[&str]| {
        let env = [
            ("RYUCAD_CACHE_IDENTITY", RELEASE),
            ("RYUCAD_CACHE_TOKEN", token),
        ];
        let v = cli_with(root, &run, cache, &env, args);
        assert_eq!(v["ok"], true, "{args:?}: {v}");
        v["result"].clone()
    };
    // The first machine warms from an empty shared cache: it builds.
    let first = as_release(&a, &ca, fake_cache::WRITE, &["cache", "warm"]);
    assert_eq!(
        (&first["parts"], &first["fetched"]),
        (&json!(1), &json!(0)),
        "{first}"
    );
    assert_eq!(first["built"], 3, "two features and the cut's own body");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while fake.held() < 3 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(fake.held(), 3, "{first}");
    // The second, reading only, builds nothing.
    let second = as_release(&b, &cb, fake_cache::READ, &["cache", "warm", "p"]);
    assert_eq!(
        (&second["built"], &second["fetched"]),
        (&json!(0), &json!(2)),
        "{second}"
    );
    let again = as_release(&b, &cb, fake_cache::READ, &["cache", "warm", "p"]);
    assert_eq!((&again["built"], &again["fetched"]), (&json!(0), &json!(0)));
    let missing = cli_with(&b, &run, &cb, &[], &["cache", "warm", "nope"]);
    assert_eq!(missing["error"]["code"], "not_found", "{missing}");
    for (root, cache) in [(&a, &ca), (&b, &cb)] {
        cli(root, &run, cache, &["daemon", "stop"]);
    }
}

/// Design 192's scenario on two machines (two disk caches): one fills the
/// shared cache, the other, empty, rebuilds and meshes writing nothing of
/// its own and gets the same hashes; through daemons and in process alike
/// (a process run in place sends what it built before it exits).
#[test]
fn a_second_machine_builds_nothing_the_first_shared() {
    for no_daemon in ["0", "1"] {
        let fake = fake_cache::Fake::start();
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        let (ca, cb) = (dir.path().join("cache-a"), dir.path().join("cache-b"));
        plate(&a, &run, &dir.path().join("scratch"));
        plate(&b, &run, &dir.path().join("scratch"));
        cli(&a, &run, &ca, &["config", "set", "cache.remote", &fake.url]);
        for root in [&a, &b] {
            cli(root, &run, &ca, &["daemon", "stop"]);
        }
        let release = |root: &Path, cache: &Path, token: &str, args: &[&str]| {
            let env = [
                ("RYUCAD_CACHE_IDENTITY", RELEASE),
                ("RYUCAD_CACHE_TOKEN", token),
                ("RYUCAD_NO_DAEMON", no_daemon),
            ];
            let v = cli_with(root, &run, cache, &env, args);
            assert_eq!(v["ok"], true, "{args:?}: {v}");
            v["result"].clone()
        };
        let built = release(&a, &ca, fake_cache::WRITE, &["rebuild", "p"]);
        let mesh = release(&a, &ca, fake_cache::WRITE, &["mesh", "p"]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while fake.held() < 4 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(
            fake.held(),
            4,
            "two features, the cut's body and a mesh ({no_daemon})"
        );
        let puts = fake.count("PUT", "/v1/");
        let again = release(&b, &cb, fake_cache::WRITE, &["rebuild", "p"]);
        assert_eq!(again["parts"][0]["hash"], built["parts"][0]["hash"]);
        let mesh_b = release(&b, &cb, fake_cache::WRITE, &["mesh", "p"]);
        assert_eq!(mesh_b, mesh, "the same mesh");
        let s = release(&b, &cb, fake_cache::WRITE, &["cache", "stats"]);
        assert_eq!(
            fake.count("PUT", "/v1/"),
            puts,
            "nothing sent: nothing built ({no_daemon})"
        );
        assert_eq!(s["entries"], 3, "what it read, kept: {s}");
        // A development build never asks.
        let seen = fake.with(|s| s.seen.len());
        cli_with(
            &b,
            &run,
            &dir.path().join("dev"),
            &[("RYUCAD_NO_DAEMON", "1")],
            &["rebuild", "p"],
        );
        assert_eq!(fake.with(|s| s.seen.len()), seen);
        for (root, cache) in [(&a, &ca), (&b, &cb)] {
            cli(root, &run, cache, &["daemon", "stop"]);
        }
    }
}

#[test]
fn a_request_in_process_doesnt_wait_for_a_remote_that_is_down() {
    let fake = fake_cache::Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let root = dir.path().join("p");
    plate(&root, &run, &dir.path().join("scratch"));
    cli(
        &root,
        &run,
        &dir.path().join("scratch"),
        &["daemon", "stop"],
    );
    // Nothing listens on the first; the second refuses the token.
    let cases = [
        ("http://127.0.0.1:1", fake_cache::WRITE),
        (fake.url.as_str(), "not-a-token"),
    ];
    for (i, (url, token)) in cases.into_iter().enumerate() {
        let cache = dir.path().join(format!("cache-{i}"));
        let env = [
            ("RYUCAD_CACHE_IDENTITY", RELEASE),
            ("RYUCAD_CACHE_TOKEN", token),
            ("RYUCAD_NO_DAEMON", "1"),
        ];
        let v = cli_with(
            &root,
            &run,
            &cache,
            &env,
            &["config", "set", "cache.remote", url],
        );
        assert_eq!(v["ok"], true, "{v}");
        let start = std::time::Instant::now();
        let v = cli_with(&root, &run, &cache, &env, &["rebuild", "p"]);
        let took = start.elapsed();
        assert_eq!(v["ok"], true, "{url}: {v}");
        assert!(
            took < std::time::Duration::from_secs(10),
            "{url}: a cold rebuild took {took:?}"
        );
    }
}
