//! Design 364's bars for references (ryucad 21.D). With `RYUCAD_BARS=1`
//! they are asserted as recorded, else ten times looser.

use serde_json::json;
use std::time::Instant;

fn slack() -> f64 {
    if std::env::var("RYUCAD_BARS").as_deref() == Ok("1") {
        1.0
    } else {
        10.0
    }
}

fn call(s: &mut ryucad::session::Session, m: &str, p: serde_json::Value) -> serde_json::Value {
    let req = ryucad::proto::Request::from_rpc(m, Some(p)).unwrap();
    ryucad::server::handle(s, req).unwrap_or_else(|e| panic!("{m}: {e}"))
}

/// Every body of the committed corpus, imported as references through
/// ryucad, in process, on one thread: under a second (design 364).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: a bar")]
fn the_committed_corpus_imports_as_references_under_a_second() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryustep/tests/corpus");
    let mut files: Vec<_> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "stp" || x == "step"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let start = Instant::now();
    let mut bodies = 0;
    for f in &files {
        let at = Instant::now();
        let v = pool.install(|| call(&mut s, "import", json!({"path": f.canonicalize().unwrap()})));
        for p in v["parts"].as_array().unwrap() {
            for feat in p["features"].as_array().into_iter().flatten() {
                assert!(feat.get("error").is_none(), "{f:?}: {feat}");
                bodies += 1;
            }
        }
        println!(
            "  {}: {:.3} s",
            f.file_name().unwrap().to_string_lossy(),
            at.elapsed().as_secs_f64()
        );
    }
    let took = start.elapsed().as_secs_f64();
    println!(
        "reference_bar: {bodies} bodies of {} files in {took:.3} s, one thread",
        files.len()
    );
    assert!(took < 1.0 * slack(), "{took:.3} s");
}

/// Runs the `ryucad` binary in `dir` with private runtime, config and
/// cache folders, and gives its envelope, refusing a failure.
fn ryucad(dir: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_ryucad"))
        .current_dir(dir)
        .env("RYUCAD_RUNTIME_DIR", dir.join("run"))
        .env("RYUCAD_CONFIG_DIR", dir.join("config"))
        .env("RYUCAD_CACHE_DIR", dir.join("cache"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The owner's NUCLEO board (#437, #499), from the owner's `step-files`
/// set (never committed: `RYUCAD_NUCLEO` names the file): imported
/// flattened as references, then a cold daemon's first rebuild of its
/// part over an empty disk cache, under 30 s (design 364).
#[test]
#[ignore = "the owner's file: RYUCAD_NUCLEO=<path>"]
fn the_nucleo_board_flattens_and_a_cold_daemon_rebuilds_it_under_30_s() {
    let file = std::env::var("RYUCAD_NUCLEO").expect("RYUCAD_NUCLEO");
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().join("project");
    std::fs::create_dir(&dir).unwrap();
    ryucad(&dir, &["init"]);
    let t = Instant::now();
    let v = ryucad(
        &dir,
        &[
            "--no-daemon",
            "import",
            &file,
            "--flatten",
            "--part",
            "board",
        ],
    );
    let import = t.elapsed().as_secs_f64();
    let n = v["result"]["parts"][0]["features"]
        .as_array()
        .unwrap()
        .len();
    std::fs::remove_dir_all(dir.join("cache")).ok();
    let t = Instant::now();
    let r = ryucad(&dir, &["rebuild", "board"]);
    let cold = t.elapsed().as_secs_f64();
    ryucad(&dir, &["daemon", "stop"]);
    let failed = r["result"]["parts"][0]["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["ok"] == false)
        .count();
    println!(
        "nucleo: {n} bodies flattened in {import:.1} s; a cold daemon's first rebuild {cold:.1} s, {failed} failed"
    );
    assert!(cold < 30.0 * slack(), "{cold:.1} s");
}

/// The pinned Switchwire (`scripts/fetch-corpus.sh`), imported whole as
/// references through a daemon: under 5 minutes (design 364).
/// `RYUCAD_REQUIRE_CORPUS=1` fails where it isn't fetched.
#[test]
#[ignore = "the pinned Voron Switchwire, after scripts/fetch-corpus.sh"]
fn the_switchwire_imports_as_references_under_5_minutes() {
    let voron = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus/voron");
    let file = voron.read_dir().ok().and_then(|d| {
        d.map(|e| e.unwrap().path()).find(|p| {
            let n = p.to_string_lossy().to_lowercase();
            n.contains("switchwire") && (n.ends_with(".step") || n.ends_with(".stp"))
        })
    });
    let Some(file) = file else {
        assert!(
            std::env::var("RYUCAD_REQUIRE_CORPUS").as_deref() != Ok("1"),
            "the Switchwire isn't fetched"
        );
        return;
    };
    let file = file.canonicalize().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().join("project");
    std::fs::create_dir(&dir).unwrap();
    ryucad(&dir, &["init"]);
    let t = Instant::now();
    let v = ryucad(&dir, &["import", &file.to_string_lossy()]);
    let took = t.elapsed().as_secs_f64();
    ryucad(&dir, &["daemon", "stop"]);
    let parts = v["result"]["parts"].as_array().unwrap();
    let features = parts
        .iter()
        .flat_map(|p| p["features"].as_array().into_iter().flatten());
    let (mut bodies, mut refused) = (0, 0);
    for f in features {
        bodies += 1;
        if f.get("error").is_some() {
            refused += 1;
        }
    }
    println!(
        "switchwire: {} parts, {bodies} bodies, {refused} refused, in {took:.1} s",
        parts.len()
    );
    assert!(took < 300.0 * slack(), "{took:.1} s");
}
