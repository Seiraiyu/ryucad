//! Design 213's bars, release only (its own steps in full.yml): the
//! densest pinned board (`lpddr4-test-board`, about 340,000 features) read
//! from its archive and every layer's shapes built, from nothing, on all
//! cores, in under 2 s; and through a warm daemon, `inspect` in under
//! 100 ms and a 1600-pixel render in under 500 ms, each one CLI call;
//! and design 245's copper bars on the same board: connectivity and the
//! netlist compare under 1 s, every gap under 0.2 mm under 2 s, and a
//! trace's edit with its queries under 1 ms (only on the pinned board).
//!
//! The pinned board is `scripts/fetch-dfm-corpus.sh`'s export, in
//! `target/dfm-corpus` (or `$RYUDFM_CORPUS`), which only the devcontainer
//! can make. Where it hasn't been fetched, a seeded job of the same size
//! stands in (minimal-clean with ten layers of 34,000 seeded features
//! each), unless `RYUDFM_REQUIRE_CORPUS=1`, which the devcontainer job
//! sets. With `RYUDFM_BARS=1` the bars are asserted as they are, else ten
//! times looser (a shared CI runner is no measuring instrument). Run with
//! `--nocapture` to see the times.

use assert_cmd::Command;
use ryudfm::odb::{ReadOptions, read_path};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The bar, or ten times it on a machine not set aside for measuring.
fn bar(d: Duration) -> Duration {
    if std::env::var("RYUDFM_BARS").is_ok_and(|v| v == "1") {
        d
    } else {
        d * 10
    }
}

/// The densest pinned board's archive, if it has been fetched.
fn pinned() -> Option<PathBuf> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir: PathBuf = std::env::var_os("RYUDFM_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| here.join("../../target/dfm-corpus"));
    let zip = dir.join("lpddr4-test-board/lpddr4-test-board.zip");
    zip.is_file().then_some(zip)
}

/// minimal-clean with ten of its layers' features replaced by 34,000
/// seeded ones each, as a gzipped tar in `dir`.
fn synthetic(dir: &Path) -> PathBuf {
    use ryu_board::model::{Features, Symbol};
    use ryu_board::symbol::StandardSymbol;
    use ryu_board::testing::{Lcg, seeded_features};
    use ryudfm::testing::{folder, symbol_name, tgz, write_features};
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = folder(&here.join("tests/corpus/minimal-clean"));
    let mut g = Lcg::new(0x6261_7273);
    let mut replaced = 0;
    for (path, bytes) in &mut files {
        let lower = path.to_ascii_lowercase();
        if replaced < 10 && lower.starts_with("steps/pcb/layers/") && lower.ends_with("/features") {
            let f = seeded_features(&mut g, 64, 34_000, 25_000_000);
            // ODB++ names no trapezoid (ryueda's pad): a round one instead.
            let symbols = f
                .symbols()
                .iter()
                .map(|s| match s {
                    Symbol::Standard(st) if symbol_name(st).is_none() => {
                        Symbol::Standard(StandardSymbol::Round { d: 200_000 })
                    }
                    s => s.clone(),
                })
                .collect();
            let f = Features::new(symbols, f.features().to_vec());
            *bytes = write_features(&f).into_bytes();
            replaced += 1;
        }
    }
    assert_eq!(replaced, 10);
    let path = dir.join("synthetic.tgz");
    std::fs::write(&path, tgz(&files)).unwrap();
    path
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryudfm --release --test bar"
)]
fn the_densest_board_reads_and_draws_within_the_bars() {
    let dir = tempfile::tempdir().unwrap();
    let job = match pinned() {
        Some(p) => p,
        None => {
            assert!(
                std::env::var("RYUDFM_REQUIRE_CORPUS").as_deref() != Ok("1"),
                "no lpddr4-test-board: run scripts/fetch-dfm-corpus.sh"
            );
            println!("the pinned corpus isn't fetched: a seeded job of the same size stands in");
            synthetic(dir.path())
        }
    };
    let start = Instant::now();
    let b = read_path(&job, &ReadOptions::default()).unwrap();
    let read = start.elapsed();
    let layers: Vec<usize> = (0..b.layers.len()).collect();
    b.build_shapes(b.board_step, &layers);
    let cold = start.elapsed();
    let features: usize = b.steps[b.board_step]
        .layers
        .iter()
        .map(|f| f.features().len())
        .sum();
    println!(
        "{}: {features} features on {} layers, read in {read:?}, every shape built by {cold:?} on {} threads",
        job.file_name().unwrap().to_string_lossy(),
        b.layers.len(),
        rayon::current_num_threads()
    );
    assert!(features > 300_000, "{features}");
    assert!(cold < bar(Duration::from_secs(2)), "{cold:?}");
    drop(b);

    // Through a warm daemon, as an agent sees it: one CLI call each.
    let project = dir.path().join("p");
    std::fs::create_dir_all(&project).unwrap();
    let run = dir.path().join("run");
    let ryudfm = |args: &[&str]| {
        let out = Command::cargo_bin("ryudfm")
            .unwrap()
            .current_dir(&project)
            .env("RYUDFM_RUNTIME_DIR", &run)
            .env("RYUDFM_CONFIG_DIR", dir.path().join("config"))
            .env("RYUDFM_IDLE_TIMEOUT", "60")
            .env_remove("RYUDFM_NO_DAEMON")
            .args(args)
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true, "{args:?}: {v}");
    };
    ryudfm(&["init"]);
    ryudfm(&["import", job.to_str().unwrap(), "--board", "dense"]);
    // Warm: the daemon read the board and built its shapes once.
    ryudfm(&["inspect", "dense"]);
    ryudfm(&["render", "dense", "--out", "warm.png"]);
    let worst = |args: &[&str]| {
        (0..5)
            .map(|_| {
                let start = Instant::now();
                ryudfm(args);
                start.elapsed()
            })
            .max()
            .unwrap()
    };
    let inspect = worst(&["inspect", "dense"]);
    let render = worst(&["render", "dense", "--out", "top.png"]);
    println!(
        "through a warm daemon, the worst of five: inspect {inspect:?}, a 1600-pixel render {render:?}"
    );
    let _ = Command::cargo_bin("ryudfm")
        .unwrap()
        .current_dir(&project)
        .env("RYUDFM_RUNTIME_DIR", &run)
        .args(["daemon", "stop"])
        .output();
    assert!(inspect < bar(Duration::from_millis(100)), "{inspect:?}");
    assert!(render < bar(Duration::from_millis(500)), "{render:?}");
}

/// The CPU this runs on, for the record.
fn machine() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    format!(
        "{cpu}, {} threads available, {} used",
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        rayon::current_num_threads()
    )
}

/// Design 245's bars, on lpddr4-test-board only: a seeded stand-in has no
/// board's copper (its features overlap at random), so without the
/// pinned board this says so and passes, unless RYUDFM_REQUIRE_CORPUS=1.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryudfm --release --test bar"
)]
fn the_densest_board_s_copper_within_the_bars() {
    use rayon::prelude::*;
    use ryu_copper::connect::Node;
    use ryudfm::copper::CopperBoard;
    use std::sync::Arc;
    let Some(job) = pinned() else {
        assert!(
            std::env::var("RYUDFM_REQUIRE_CORPUS").as_deref() != Ok("1"),
            "no lpddr4-test-board: run scripts/fetch-dfm-corpus.sh"
        );
        println!("the pinned corpus isn't fetched: the copper bars need lpddr4-test-board itself");
        return;
    };
    println!("{}", machine());
    let b = read_path(&job, &ReadOptions::default()).unwrap();
    let all: Vec<usize> = (0..b.layers.len()).collect();
    b.build_shapes(b.board_step, &all);
    let b = Arc::new(b);

    let start = Instant::now();
    let mut cb = CopperBoard::new(Arc::clone(&b));
    let connect = start.elapsed();

    let start = Instant::now();
    let gaps: usize = (0..cb.built.copper.len())
        .into_par_iter()
        .map(|l| cb.copper.clearance(l, 200_000).len())
        .sum();
    let clearance = start.elapsed();

    let islands = cb.copper.islands();
    let largest = islands
        .iter()
        .max_by(|x, y| x.pieces.len().cmp(&y.pieces.len()).then(y.id.cmp(&x.id)))
        .unwrap();
    let ground = *largest
        .items
        .iter()
        .find(|k| cb.is_trace(k))
        .expect("a trace on the largest island");
    let signal = islands
        .iter()
        .filter(|i| i.pieces.len() <= 10)
        .flat_map(|i| i.items.iter())
        .copied()
        .find(|k| cb.is_trace(k))
        .expect("a signal trace");
    let mut edits = Vec::new();
    for (what, k) in [("signal", signal), ("ground", ground)] {
        let (mut out, mut back) = (Duration::ZERO, Duration::ZERO);
        let resolves = cb.copper.resolves();
        for _ in 0..5 {
            let touching = cb.copper.contacts(&Node::Piece(k, 0));
            let start = Instant::now();
            let it = cb.copper.remove_item(&k).unwrap();
            // What it touched: are its neighbours still joined?
            let _: Vec<_> = touching.iter().map(|n| cb.copper.island_of(n)).collect();
            out = out.max(start.elapsed());
            let start = Instant::now();
            cb.copper.insert_item(k, it);
            // Its clearance: every item within 0.2 mm, with the exact
            // distance; and its island.
            let body = cb.copper.bodies().body(&k).unwrap().clone();
            let _: Vec<_> = cb
                .copper
                .bodies()
                .near(&k, 200_000)
                .iter()
                .filter_map(|n| {
                    ryu_copper::measure::distance(&body, cb.copper.bodies().body(&n.key)?)
                })
                .collect();
            let _ = cb.copper.island_of(&Node::Piece(k, 0));
            back = back.max(start.elapsed());
        }
        println!(
            "a {what} trace: out with its queries {out:?}, back with its queries {back:?} (the worst of five; {} booleans)",
            cb.copper.resolves() - resolves
        );
        edits.push((what, out.max(back)));
    }
    println!(
        "{}: {} items on {} copper layers, {} islands; connectivity and the compare {connect:?}; {gaps} gaps under 0.2 mm in {clearance:?}",
        job.file_name().unwrap().to_string_lossy(),
        cb.copper.items().count(),
        cb.built.copper.len(),
        islands.len(),
    );
    assert!(connect < bar(Duration::from_secs(1)), "{connect:?}");
    assert!(clearance < bar(Duration::from_secs(2)), "{clearance:?}");
    for (what, t) in edits {
        assert!(
            t < bar(Duration::from_millis(1)),
            "a {what} trace's edit: {t:?}"
        );
    }
}
