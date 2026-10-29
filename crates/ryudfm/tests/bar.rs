//! Design 213's bars, release only (its own steps in full.yml): the
//! densest pinned board (`lpddr4-test-board`, about 340,000 features) read
//! from its archive and every layer's shapes built, from nothing, on all
//! cores, in under 2 s; and through a warm daemon, `inspect` in under
//! 100 ms and a 1600-pixel render in under 500 ms, each one CLI call.
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
