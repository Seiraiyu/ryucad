//! Design 157's bars, release only (its own step in full.yml): a design
//! of 10,000 component occurrences and about 20,000 nets loads, flattens
//! and runs ERC from nothing in under a second, and a write and the next
//! read through a warm daemon take under 50 ms. With `RYUEDA_BARS=1` the
//! bars are asserted as they are, else ten times looser (a shared CI
//! runner is no measuring instrument). Run with `--nocapture` to see the
//! times.

#[path = "support/generated.rs"]
mod generated;
#[path = "support/parts.rs"]
mod parts;

use assert_cmd::Command;
use std::time::{Duration, Instant};

/// The bar, or ten times it on a machine not set aside for measuring.
fn bar(d: Duration) -> Duration {
    if std::env::var("RYUEDA_BARS").is_ok_and(|v| v == "1") {
        d
    } else {
        d * 10
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn ten_thousand_occurrences_flatten_and_check_within_the_bars() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    generated::write(&board, 100, 100);
    let design = ryueda::id::Id::parse("d", "design").unwrap();
    let start = Instant::now();
    let p = ryueda::project::Project::load(&board).unwrap();
    let loaded = start.elapsed();
    let n = ryueda::netlist::flatten(&p, &design).unwrap();
    let flattened = start.elapsed();
    let found = ryueda::erc::check_netlist(&p, &n);
    let cold = start.elapsed();
    println!(
        "{} occurrences, {} nets: loaded in {loaded:?}, flattened by {flattened:?}, ERC done by {cold:?} ({} findings)",
        n.occurrences.len(),
        n.nets.len(),
        found.len()
    );
    assert_eq!(n.occurrences.len(), 10_000);
    assert!(n.nets.len() >= 19_900, "{}", n.nets.len());
    assert!(cold < bar(Duration::from_secs(1)), "{cold:?}");

    // Through a warm daemon, as an agent sees it: one CLI call to write,
    // one to read what it wrote.
    let run = dir.path().join("run");
    let ryueda = |args: &[&str]| {
        let mut cmd = Command::cargo_bin("ryueda").unwrap();
        cmd.current_dir(&board)
            .env("RYUEDA_RUNTIME_DIR", &run)
            .env("RYUEDA_CONFIG_DIR", dir.path().join("config"))
            .env("RYUEDA_IDLE_TIMEOUT", "60")
            .env_remove("RYUEDA_NO_DAEMON");
        // No person's distributor keys or API, in the daemon it starts too.
        for v in ryueda_parts::distributor::env_vars() {
            cmd.env_remove(v);
        }
        let out = cmd.args(args).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true, "{args:?}: {v}");
    };
    // Warm: the daemon started, the project read, a write made.
    ryueda(&["block", "show", "top"]);
    ryueda(&["net", "rename", "top/l1", "LINK1_W"]);
    let mut worst = Duration::ZERO;
    for i in 0..5 {
        let start = Instant::now();
        ryueda(&["net", "rename", "top/l1", &format!("LINK1_{i}")]);
        ryueda(&["net", "show", "top/l1"]);
        worst = worst.max(start.elapsed());
    }
    println!("a write and the next read through a warm daemon: {worst:?} at worst of five");
    let _ = Command::cargo_bin("ryueda")
        .unwrap()
        .current_dir(&board)
        .env("RYUEDA_RUNTIME_DIR", &run)
        .args(["daemon", "stop"])
        .output();
    assert!(worst < bar(Duration::from_millis(50)), "{worst:?}");
}

/// Design 348's bar for plan 2.F: annotating a design of 10,000
/// component occurrences (every unlocked designator, its blocks laid out
/// in memory to order them) takes under a second, the project loaded.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn annotating_ten_thousand_occurrences_is_within_its_bar() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    generated::write(&board, 100, 100);
    let p = ryueda::project::Project::load(&board).unwrap();
    let start = Instant::now();
    let plan = ryueda::annotate::plan(
        &p,
        Some("d"),
        ryueda::annotate::Order::Down,
        ryueda::annotate::Reset::All,
    )
    .unwrap();
    let took = start.elapsed();
    println!(
        "annotate --reset all over 10,000 occurrences: {} changes in {took:?}",
        plan.changes.len()
    );
    assert!(took < bar(Duration::from_secs(1)), "{took:?}");
}

/// Design 347's bar: a search over 10,000 library components takes under
/// 50 ms on one thread (the project already loaded).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn a_search_of_ten_thousand_library_components_is_within_its_bar() {
    let dir = tempfile::tempdir().unwrap();
    parts::write(dir.path(), 10_000, false);
    let p = ryueda::project::Project::load(dir.path()).unwrap();
    let search = ryueda::parts::Search {
        category: Some("capacitor".into()),
        conditions: vec![
            "capacitance=100nF".into(),
            "voltage_rating>=16V".into(),
            "package=0402".into(),
        ],
        ..Default::default()
    };
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    one.install(|| ryueda::parts::search(&p, &search, &Default::default()).unwrap());
    let mut worst = Duration::ZERO;
    for _ in 0..5 {
        let start = Instant::now();
        let v = one.install(|| ryueda::parts::search(&p, &search, &Default::default()).unwrap());
        worst = worst.max(start.elapsed());
        assert!(!v["results"].as_array().unwrap().is_empty());
    }
    println!("a search over 10,000 library components on one thread: {worst:?} at worst of five");
    assert!(worst < bar(Duration::from_millis(50)), "{worst:?}");
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn the_jlc_catalogue_searches_under_its_bar() {
    // The catalogue's size: jlcparts' totalComponents as plan 387's live
    // run measured it (2026-11-06); RYUEDA_JLC_PARTS sets another.
    let n: usize = std::env::var("RYUEDA_JLC_PARTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(370_668);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("jlc");
    let recs = ryueda_parts::jlc::testing::seeded(n);
    ryueda_parts::jlc::index::write_next(&root, &recs, 1, "seeded").unwrap();
    ryueda_parts::jlc::index::swap(&root).unwrap();
    drop(recs);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    pool.install(|| {
        use ryueda_parts::source::Source;
        let jlc = ryueda_parts::jlc::Jlc::open(&root).unwrap().unwrap();
        let q = ryueda_parts::query::Query {
            category: Some(ryueda_parts::category::Category::Capacitor),
            conditions: ["capacitance=100nF", "voltage_rating>=16V", "package=0402"]
                .iter()
                .map(|c| ryueda_parts::query::Condition::parse(c).unwrap())
                .collect(),
            stock: Some(1000),
            basic: true,
            limit: 20,
        };
        let t = Instant::now();
        let first = jlc.search(&q, &Default::default()).unwrap();
        let load = t.elapsed();
        assert!(!first.is_empty(), "the seeded catalogue has 100 nF 0402 parts");
        let worst = (0..5)
            .map(|_| {
                let t = Instant::now();
                jlc.search(&q, &Default::default()).unwrap();
                t.elapsed()
            })
            .max()
            .unwrap();
        println!(
            "jlc search over {n} parts on one thread: first (loading capacitors) {load:?}, worst of five after {worst:?}"
        );
        assert!(worst <= bar(Duration::from_millis(500)), "{worst:?}");
    });
}
