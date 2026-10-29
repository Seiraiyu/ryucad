//! Design 157's bars, release only (its own step in full.yml): a design
//! of 10,000 component occurrences and about 20,000 nets loads, flattens
//! and runs ERC from nothing in under a second, and a write and the next
//! read through a warm daemon take under 50 ms. With `RYUEDA_BARS=1` the
//! bars are asserted as they are, else ten times looser (a shared CI
//! runner is no measuring instrument). Run with `--nocapture` to see the
//! times.

#[path = "support/generated.rs"]
mod generated;

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
        let out = Command::cargo_bin("ryueda")
            .unwrap()
            .current_dir(&board)
            .env("RYUEDA_RUNTIME_DIR", &run)
            .env("RYUEDA_CONFIG_DIR", dir.path().join("config"))
            .env("RYUEDA_IDLE_TIMEOUT", "60")
            .env_remove("RYUEDA_NO_DAEMON")
            .args(args)
            .output()
            .unwrap();
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
