//! Design 72's bars for assemblies at scale: 1,000 pins of 19 sizes on a
//! fixed plate (1,001 occurrences of 20 parts), each held by one of
//! 1,000 mates (fastened, revolute, revolute driven to 30°). The
//! assembly's rebuild over parts already built solves in under 0.1 s
//! when its poses hold and under 1 s from poses jostled about a degree
//! and a tenth of a millimetre off; and a daemon started cold over a warm
//! disk cache answers `rebuild` for it in under 2 s, writing nothing new.
//! Each prints the machine it ran on. Release builds only. The project is
//! `tests/bar_project`'s, which `benches/assembly.rs` builds too.
//!
//! The bars are design 72's on the machine they are recorded on: with
//! `RYUCAD_BARS=1` they are asserted as they are. CI's runners differ
//! from run to run, so there they are asserted ten times looser, which
//! still catches a cost that grows out of hand.

mod bar_project;

use assert_cmd::Command;
use bar_project::{id, jostle, machine, project, project_over_dependency};
use ryucad::session::Session;
use serde_json::Value;
use std::path::Path;
use std::time::Instant;

/// How much slack the bars get: none where they are being recorded
/// (`RYUCAD_BARS` set), else ten times.
fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

/// The best of three runs of the assembly's rebuild over its parts as
/// `s` has them built, in milliseconds, and the last result's warnings.
fn assembly_ms(s: &mut Session) -> (f64, Vec<Value>) {
    let part = s.project().part(&id("asm")).unwrap().clone();
    let mut fastest = f64::INFINITY;
    let mut warnings = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        let r = s.rebuilt_as(part.clone()).unwrap();
        fastest = fastest.min(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(
            r.failed(),
            0,
            "{:?}",
            r.features.iter().find(|(_, r)| r.is_err())
        );
        assert!(r.solve.as_ref().unwrap().report.converged);
        warnings = r.warnings(&id("asm"));
    }
    (fastest, warnings)
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_thousand_mates_solve_held_and_jostled() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = project(dir.path());
    let built = s.rebuilt(&id("asm")).unwrap();
    assert_eq!(
        built.warnings(&id("asm")),
        Vec::<Value>::new(),
        "the poses hold"
    );
    let (held, warnings) = assembly_ms(&mut s);
    assert!(warnings.is_empty(), "{warnings:?}");
    jostle(dir.path());
    s.refresh().unwrap();
    let (jostled, warnings) = assembly_ms(&mut s);
    assert!(
        warnings.iter().any(|w| w["code"] == "assembly_stale"),
        "{warnings:?}"
    );
    println!(
        "1,000 mates over 1,001 occurrences of 20 parts, on {}: held {held:.1} ms, jostled \
         {jostled:.1} ms",
        machine()
    );
    assert!(held < 100.0 * slack(), "held: {held:.1} ms");
    assert!(jostled < 1000.0 * slack(), "jostled: {jostled:.1} ms");
}

/// Every file under `dir`, for counting what a cache holds.
fn files(dir: &Path) -> usize {
    let mut n = 0;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                todo.push(e.path());
            } else {
                n += 1;
            }
        }
    }
    n
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_cold_daemon_over_a_warm_cache_answers_within_two_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    std::fs::create_dir(&root).unwrap();
    drop(project(&root));
    let (run, cache) = (dir.path().join("run"), dir.path().join("cache"));
    let ryucad = |args: &[&str]| -> Value {
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .current_dir(&root)
            .args(args)
            .env("RYUCAD_RUNTIME_DIR", &run)
            .env("RYUCAD_CONFIG_DIR", dir.path().join("config"))
            .env("RYUCAD_CACHE_DIR", &cache)
            .env("RYUCAD_IDLE_TIMEOUT", "60")
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("{args:?}: {}", String::from_utf8_lossy(&out.stderr)))
    };
    // A daemon fills the cache, and stops.
    let t = Instant::now();
    let first = ryucad(&["rebuild", "asm"]);
    let cold = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(first["ok"], true, "{first}");
    ryucad(&["daemon", "stop"]);
    let entries = files(&cache);
    assert!(entries >= 20, "every part's body: {entries}");
    // A new one, over the warm cache.
    let t = Instant::now();
    let again = ryucad(&["rebuild", "asm"]);
    let warm = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(
        again["result"]["parts"][0]["hash"],
        first["result"]["parts"][0]["hash"]
    );
    assert_eq!(files(&cache), entries, "nothing built, so nothing written");
    // What a request that builds nothing costs with 2,000 feature files to
    // check for freshness (issue #25): the best of five.
    let mut request = f64::INFINITY;
    for _ in 0..5 {
        let t = Instant::now();
        assert_eq!(ryucad(&["part", "list"])["ok"], true);
        request = request.min(t.elapsed().as_secs_f64() * 1e3);
    }
    ryucad(&["daemon", "stop"]);
    println!(
        "a cold daemon's rebuild of 1,001 occurrences, on {}: {cold:.0} ms filling the cache, \
         {warm:.0} ms over it; then {request:.0} ms a `part list`",
        machine()
    );
    assert!(warm < 2000.0 * slack(), "warm: {warm:.0} ms");
}

/// The best of five runs of `f` on `s`, in milliseconds, and its last
/// answer.
fn best_ms(s: &mut Session, f: impl Fn(&mut Session) -> Value) -> (f64, Value) {
    let mut fastest = f64::INFINITY;
    let mut out = Value::Null;
    for _ in 0..5 {
        let t = Instant::now();
        out = f(s);
        fastest = fastest.min(t.elapsed().as_secs_f64() * 1e3);
    }
    (fastest, out)
}

/// Design 180's bars: the assembly's flat BOM, and where a pin is used,
/// each in under 50 ms, read from its 2,021 files with nothing rebuilt
/// (the methods themselves: a request's freshness check over the files
/// is design 72's).
#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_thousand_pins_give_their_bom_and_where_used_within_fifty_ms() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = project(dir.path());
    let (bom, v) = best_ms(&mut s, |s| {
        let ask = ryucad::bom::Ask {
            part: Some("asm"),
            rev: None,
            flat: true,
            depth: None,
            format: None,
        };
        ryucad::bom::get(s, &ask).unwrap()
    });
    let total: u64 = v["flat"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["quantity"]["count"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 1001);
    let (used, v) = best_ms(&mut s, |s| {
        ryucad::bom::where_used(
            s,
            &ryucad::bom::WhereUsed {
                part: "pin0",
                label: None,
                working: false,
                direct: false,
                within: &[],
                no_dependents: true,
            },
        )
        .unwrap()
    });
    assert_eq!(v["working"][0]["assembly"], "asm");
    println!(
        "BOM of 1,001 occurrences of 20 parts, on {}: flat {bom:.1} ms, where-used {used:.1} ms",
        machine()
    );
    assert!(bom < 50.0 * slack(), "{bom} ms");
    assert!(used < 50.0 * slack(), "{used} ms");
}

/// Design 233's bar: the same assembly with its 20 parts released in a
/// dependency rebuilds over parts already built within 1.25 times the
/// assembly whose parts are its own.
#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_thousand_pins_from_a_dependency_rebuild_as_fast_as_their_own() {
    let dir = tempfile::tempdir().unwrap();
    let mut own = project(&dir.path().join("own"));
    own.rebuilt(&id("asm")).unwrap();
    let (local, _) = assembly_ms(&mut own);
    let mut s = project_over_dependency(&dir.path().join("dep"));
    let built = s.rebuilt(&id("asm")).unwrap();
    assert_eq!(built.failed(), 0);
    assert_eq!(
        built.warnings(&id("asm")),
        Vec::<Value>::new(),
        "the poses hold"
    );
    let (foreign, _) = assembly_ms(&mut s);
    println!(
        "1,000 mates over 1,001 occurrences of 20 parts, on {}: their own {local:.1} ms, \
         from a dependency {foreign:.1} ms",
        machine()
    );
    assert!(
        foreign < 1.25 * local * slack(),
        "from a dependency {foreign:.1} ms against {local:.1} ms"
    );
}
