//! Design 157's bars, release only (its own step in full.yml): a design
//! of 10,000 component occurrences and about 20,000 nets loads, flattens
//! and runs ERC from nothing in under a second, and a write and the next
//! read through a warm daemon take under 50 ms. With `RYUEDA_BARS=1` the
//! bars are asserted as they are, else ten times looser (a shared CI
//! runner is no measuring instrument). Run with `--nocapture` to see the
//! times. Design 629's bars too: a 10,000-occurrence BOM through a warm
//! daemon in 50 ms, `release propose --all` of 10,000 library components
//! in 10 s, `release list` over them in 1 s, and where-used over 10
//! checkouts in 1 s. Design 732's bars: a generated board of 1,000 parts
//! and 2,000 tracks checked from nothing in 1 s, and a track write with
//! its DRC delta through a warm daemon in 250 ms (50 ms on the breakout).

#[path = "support/board_gen.rs"]
mod board_gen;
#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;
#[path = "support/generated.rs"]
mod generated;
#[path = "support/parts.rs"]
mod parts;
#[path = "support/rp2040.rs"]
mod rp2040;

use assert_cmd::Command;
use std::path::Path;
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

/// Design 518's bars: on the 10,000-occurrence design in a repository,
/// `diff --netlist` from HEAD (read from Git's objects) within 1 s, and
/// `merge status` of a clean merge (two branches renaming a net each)
/// within 2 s, the session already open as a daemon's is.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn a_netlist_diff_and_a_clean_merges_status_are_within_their_bars() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    generated::write(&board, 100, 100);
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&board)
            .env("GIT_CONFIG_GLOBAL", dir.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .args([
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "base"]);
    let rename = |path: &str, name: &str| {
        let at = board.join(path);
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&at).unwrap()).unwrap();
        v["name"] = serde_json::json!(name);
        std::fs::write(&at, ryueda::canonical::to_string(&v)).unwrap();
    };
    // diff --netlist: a link renamed in the working files.
    rename("blocks/top/nets/l1.json", "LINK1_W");
    let mut s = ryueda::session::Session::open(&board).unwrap();
    let opts = ryueda::diff::Options {
        netlist: true,
        ..Default::default()
    };
    let start = Instant::now();
    let d = ryueda::diff::between(&mut s, "HEAD", "working", &opts).unwrap();
    let diffed = start.elapsed();
    assert!(d.to_string().contains("LINK1_W"));
    // A clean merge: theirs renames a link, ours a channel's spare net.
    git(&["checkout", "-q", "--", "."]);
    git(&["checkout", "-q", "-b", "theirs"]);
    rename("blocks/top/nets/l2.json", "LINK2_T");
    git(&["commit", "-q", "-a", "-m", "theirs"]);
    git(&["checkout", "-q", "main"]);
    rename("blocks/ch/nets/s0.json", "SPARE0_O");
    git(&["commit", "-q", "-a", "-m", "ours"]);
    git(&["merge", "--no-commit", "-q", "theirs"]);
    let mut s = ryueda::session::Session::open(&board).unwrap();
    let start = Instant::now();
    let v = ryueda::validate::status(&mut s).unwrap();
    let status = start.elapsed();
    println!(
        "10,000 occurrences: diff --netlist from HEAD in {diffed:?}; a clean merge's status in {status:?} ({} warnings, {} already failing)",
        v["warnings"].as_array().map_or(0, Vec::len),
        v["already_failing"].as_array().map_or(0, Vec::len)
    );
    assert_eq!(v["clean"], true, "{}", v["conflicts"]);
    assert!(diffed < bar(Duration::from_secs(1)), "{diffed:?}");
    assert!(status < bar(Duration::from_secs(2)), "{status:?}");
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

/// Design 519's bar: a 500-part block's sheet drawn again within 150 ms
/// of the write that edited one of its values, through a warm daemon
/// (ten times looser without RYUEDA_BARS=1). Unmet (plan 3.B): laying
/// the 500 parts out again takes 3.5 s of the 3.7 s, the engine's cost
/// whatever reads the block (#802, incremental lay-out of a big block).
#[test]
#[ignore = "unmet until #802: cargo test -p ryueda --release --test bar an_edited_sheet -- --ignored"]
fn an_edited_sheet_is_drawn_again_within_its_bar() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    // One channel of 500 resistors: block `ch`.
    generated::write(&board, 1, 500);
    let run = dir.path().join("run");
    let ryueda = |args: &[&str]| {
        let mut cmd = Command::cargo_bin("ryueda").unwrap();
        cmd.current_dir(&board)
            .env("RYUEDA_RUNTIME_DIR", &run)
            .env("RYUEDA_CONFIG_DIR", dir.path().join("config"))
            .env("RYUEDA_IDLE_TIMEOUT", "120")
            .env_remove("RYUEDA_NO_DAEMON");
        for v in ryueda_parts::distributor::env_vars() {
            cmd.env_remove(v);
        }
        let out = cmd.args(args).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true, "{args:?}: {v}");
        v["result"].clone()
    };
    let start = Instant::now();
    ryueda(&["layout", "--block", "ch"]);
    println!("500 parts laid out in {:?}", start.elapsed());
    let before = ryueda(&["scene", "--block", "ch"]);
    let sheet = before["sheet"].as_str().unwrap().to_owned();
    let part = before["owners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["kind"] == "component")
        .unwrap()["ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let start = Instant::now();
    ryueda(&["component", "edit", &part, "--value", "22 kOhm"]);
    let wrote = start.elapsed();
    let start = Instant::now();
    let after = ryueda(&["scene", "--block", "ch", "--sheet", &sheet]);
    let drawn = start.elapsed();
    ryueda(&["daemon", "stop"]);
    let hash = |v: &serde_json::Value| {
        v["owners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["ref"] == part.as_str())
            .unwrap()["hash"]
            .clone()
    };
    assert_ne!(hash(&after), hash(&before), "{part}'s drawing moved");
    println!(
        "{part} edited in {wrote:?}; its sheet ({} owners) drawn again in {drawn:?}",
        after["owners"].as_array().unwrap().len()
    );
    assert!(drawn < bar(Duration::from_millis(150)), "{drawn:?}");
}

/// `git` in `dir` without anyone's config, a fixed identity and dates.
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", dir.join("..").join("no-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
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
}

/// `ryueda` in `board` through a warm daemon run from `run` (config under
/// `top`, no distributor keys), its result; it must succeed.
fn warm<'a>(
    board: &'a Path,
    run: &'a Path,
    top: &'a Path,
) -> impl Fn(&[&str]) -> serde_json::Value + 'a {
    move |args: &[&str]| {
        let mut cmd = Command::cargo_bin("ryueda").unwrap();
        cmd.current_dir(board)
            .env("RYUEDA_RUNTIME_DIR", run)
            .env("RYUEDA_CONFIG_DIR", top.join("config"))
            .env("RYUEDA_IDLE_TIMEOUT", "120")
            .env_remove("RYUEDA_NO_DAEMON");
        for v in ryueda_parts::distributor::env_vars() {
            cmd.env_remove(v);
        }
        let out = cmd.args(args).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true, "{args:?}: {v}");
        v["result"].clone()
    }
}

/// Design 688's bar: a compare scene of the RP2040's largest sheet
/// against HEAD through a warm daemon, under 100 ms with the revision's
/// lay-out kept (another sheet of the block asked first: the working
/// block is laid out again, the revision's isn't), under 50 ms kept
/// whole. The times include the CLI's start.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn a_compare_scene_is_within_its_bar() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("rp2040");
    std::fs::create_dir_all(&board).unwrap();
    let run = dir.path().join("run");
    let ryueda = warm(&board, &run, dir.path());
    ryueda(&["init"]);
    for args in rp2040::script() {
        ryueda(&args.iter().map(String::as_str).collect::<Vec<_>>());
    }
    ryueda(&["layout"]);
    git(&board, &["init", "-q"]);
    git(&board, &["add", "."]);
    git(&board, &["commit", "-q", "-m", "rp2040"]);
    let first = ryueda(&["scene", "--block", "mitayi", "--compare", "HEAD"]);
    let sheets: Vec<String> = first["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["sheet"].as_str().map(str::to_owned))
        .collect();
    let largest = sheets
        .iter()
        .max_by_key(|s| {
            ryueda(&["scene", "--block", "mitayi", "--sheet", s])["owners"]
                .as_array()
                .unwrap()
                .len()
        })
        .unwrap()
        .clone();
    let other = sheets.iter().find(|s| **s != largest).unwrap().clone();
    ryueda(&[
        "scene",
        "--block",
        "mitayi",
        "--sheet",
        &other,
        "--compare",
        "HEAD",
    ]);
    let t = Instant::now();
    ryueda(&[
        "scene",
        "--block",
        "mitayi",
        "--sheet",
        &largest,
        "--compare",
        "HEAD",
    ]);
    let kept_lay_out = t.elapsed();
    let t = Instant::now();
    ryueda(&[
        "scene",
        "--block",
        "mitayi",
        "--sheet",
        &largest,
        "--compare",
        "HEAD",
    ]);
    let kept_whole = t.elapsed();
    ryueda(&["daemon", "stop"]);
    println!(
        "rp2040 {largest} compared with HEAD: revision kept {kept_lay_out:?}, scene kept {kept_whole:?}"
    );
    assert!(
        kept_lay_out < bar(Duration::from_millis(100)),
        "{kept_lay_out:?}"
    );
    assert!(
        kept_whole < bar(Duration::from_millis(50)),
        "{kept_whole:?}"
    );
}

/// Design 688's bar, unmet with #802: a 500-part block's compare scene
/// drawn again within 150 ms of a value edit, through a warm daemon (ten
/// times looser without RYUEDA_BARS=1). Laying the 500 parts out again
/// is the engine's cost, whatever reads the block (#802).
#[test]
#[ignore = "unmet until #802: cargo test -p ryueda --release --test bar a_redraw_while_comparing -- --ignored"]
fn a_redraw_while_comparing_is_within_its_bar() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    generated::write(&board, 1, 500);
    let run = dir.path().join("run");
    let ryueda = warm(&board, &run, dir.path());
    ryueda(&["layout", "--block", "ch"]);
    git(&board, &["init", "-q"]);
    git(&board, &["add", "."]);
    git(&board, &["commit", "-q", "-m", "five hundred"]);
    let before = ryueda(&["scene", "--block", "ch", "--compare", "HEAD"]);
    let sheet = before["sheet"].as_str().unwrap().to_owned();
    let part = before["owners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["kind"] == "component")
        .unwrap()["ref"]
        .as_str()
        .unwrap()
        .to_owned();
    ryueda(&["component", "edit", &part, "--value", "22 kOhm"]);
    let start = Instant::now();
    let after = ryueda(&[
        "scene",
        "--block",
        "ch",
        "--sheet",
        &sheet,
        "--compare",
        "HEAD",
    ]);
    let drawn = start.elapsed();
    ryueda(&["daemon", "stop"]);
    let mark = after["owners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["ref"] == part.as_str())
        .unwrap()["mark"]
        .clone();
    assert_eq!(mark, "changed", "{part} is marked");
    println!("{part} edited; its compare sheet drawn again in {drawn:?}");
    assert!(drawn < bar(Duration::from_millis(150)), "{drawn:?}");
}

/// Design 688's bar: the Changes tab's reads on the 10,000-occurrence
/// project after one value edit, `diff --layout` and `diff --netlist`
/// each within 2 s through a warm daemon.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn the_changes_reads_are_within_their_bar() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    generated::write(&board, 100, 100);
    let run = dir.path().join("run");
    let ryueda = warm(&board, &run, dir.path());
    git(&board, &["init", "-q"]);
    git(&board, &["add", "."]);
    git(&board, &["commit", "-q", "-m", "ten thousand"]);
    ryueda(&["component", "edit", "ch/r0", "--value", "22 kOhm"]);
    for flag in ["--layout", "--netlist"] {
        let t = Instant::now();
        let d = ryueda(&["diff", flag]);
        let took = t.elapsed();
        assert!(d.to_string().contains("22 kOhm"), "{flag}");
        println!("diff {flag} over 10,000 occurrences: {took:?}");
        assert!(took < bar(Duration::from_secs(2)), "{flag}: {took:?}");
    }
    ryueda(&["daemon", "stop"]);
}

/// Design 629's bars: the BOM of the 10,000-occurrence design through a
/// warm daemon within 50 ms; `release propose --all` of 10,000 library
/// components within 10 s and `release list` over them within 1 s; and
/// where-used over 10 checkouts within 1 s.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn boms_releases_and_where_used_are_within_their_bars() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let at = |cwd: &std::path::Path, args: &[&str]| -> serde_json::Value {
        let mut cmd = Command::cargo_bin("ryueda").unwrap();
        cmd.current_dir(cwd)
            .env("RYUEDA_RUNTIME_DIR", &run)
            .env("RYUEDA_CONFIG_DIR", dir.path().join("config"))
            .env("RYUEDA_IDLE_TIMEOUT", "120")
            .env_remove("RYUEDA_NO_DAEMON");
        for v in ryueda_parts::distributor::env_vars() {
            cmd.env_remove(v);
        }
        let out = cmd.args(args).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true, "{args:?}: {v}");
        v["result"].clone()
    };
    // The BOM, warm.
    let board = dir.path().join("board");
    generated::write(&board, 100, 100);
    at(&board, &["bom"]);
    let mut worst = Duration::ZERO;
    for _ in 0..5 {
        let start = Instant::now();
        at(&board, &["bom"]);
        worst = worst.max(start.elapsed());
    }
    println!("a 10,000-occurrence BOM through a warm daemon: {worst:?} at worst of five");
    assert!(worst < bar(Duration::from_millis(50)), "{worst:?}");
    // 10,000 components released at once, then listed.
    let repo = dir.path().join("repo");
    let lib = repo.join("lib");
    generated::library(&lib, 10_000);
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q", "-b", "main"]);
    at(&lib, &["pdm", "init"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.name=b",
        "-c",
        "user.email=b@example.com",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "lib",
    ]);
    let start = Instant::now();
    let v = at(&lib, &["release", "propose", "--all"]);
    let proposed = start.elapsed();
    println!("release propose --all of 10,000 components: {proposed:?}");
    assert_eq!(
        v["proposed"].as_array().unwrap().len(),
        10_000,
        "{}",
        v["refused"]
    );
    assert!(proposed < bar(Duration::from_secs(10)), "{proposed:?}");
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.name=b",
        "-c",
        "user.email=b@example.com",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "A",
    ]);
    at(&lib, &["release", "list"]);
    let start = Instant::now();
    at(&lib, &["release", "list"]);
    let listed = start.elapsed();
    println!("release list over 10,000 released components: {listed:?}");
    assert!(listed < bar(Duration::from_secs(1)), "{listed:?}");
    // Where-used over 10 checkouts, each a 1,000-occurrence design
    // placing the library's c00000 at working by its project ID.
    // A folder holding both, as a workspace of checkouts does, so the
    // library resolves from each (`resolve`'s fence is the nearest folder
    // with `.git`).
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    let others = dir.path().join("others");
    for k in 0..10 {
        let b = others.join(format!("b{k}"));
        generated::write(&b, 10, 100);
        generated::place_from(
            &b,
            "lib",
            "../../repo/lib",
            "fedcba9876543210fedcba9876543210",
            "c00000",
        );
    }
    let o = others.to_str().unwrap().to_owned();
    at(&lib, &["where-used", "c00000", "--in", &o]);
    let start = Instant::now();
    let v = at(&lib, &["where-used", "c00000", "--in", &o]);
    let searched = start.elapsed();
    println!("where-used over 10 checkouts: {searched:?}");
    assert_eq!(v["working"].as_array().unwrap().len(), 10, "{v}");
    assert!(searched < bar(Duration::from_secs(1)), "{searched:?}");
    for cwd in [&board, &lib] {
        let _ = Command::cargo_bin("ryueda")
            .unwrap()
            .current_dir(cwd)
            .env("RYUEDA_RUNTIME_DIR", &run)
            .args(["daemon", "stop"])
            .output();
    }
}

/// Design 732's bars: the generated board (1,000 parts, 2,000 pads,
/// 2,000 tracks) loaded, shaped, connected and checked from nothing in
/// 1 s; a track write answering its DRC delta through a warm daemon in
/// 250 ms. The times include the CLI's start for the write.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn a_generated_board_checks_and_answers_a_write_within_its_bars() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    std::fs::create_dir_all(&board).unwrap();
    board_gen::write(&board, &board_gen::Spec::bar());
    git(&board, &["init", "-q"]);
    let id = ryueda::id::Id::parse("main", "board").unwrap();
    let t = Instant::now();
    let p = ryueda::project::Project::load(&board).unwrap();
    let c = ryueda::board::drc::check(&p, &p.boards[&id]).unwrap();
    let cold = t.elapsed();
    println!(
        "generated board: {} pads, {} tracks: loaded, shaped, connected and checked in {cold:?}",
        c.shapes.pads.len(),
        c.shapes.tracks.len()
    );
    assert!(
        c.findings.is_empty(),
        "{:?}",
        &c.findings[..c.findings.len().min(5)]
    );
    assert!(cold < bar(Duration::from_secs(1)), "{cold:?}");
    let run = dir.path().join("run");
    let ryueda = warm(&board, &run, dir.path());
    ryueda(&["board", "drc", "main"]);
    let net = ryueda(&["board", "show", "main", "--pads"])["pads"][0]["net"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut worst = Duration::ZERO;
    for i in 0..5 {
        let y = format!("{}", 1 + i);
        let t = Instant::now();
        let v = ryueda(&[
            "board",
            "track",
            "add",
            "main",
            "--net",
            &net,
            "--layer",
            "bottom",
            "--path",
            &format!("1,{y}"),
            &format!("2,{y}"),
            "--id",
            &format!("x{i}"),
        ]);
        let took = t.elapsed();
        assert_eq!(v["drc"]["errors"], 0, "{v}");
        worst = worst.max(took);
    }
    println!("a track write with its delta through a warm daemon: worst {worst:?}");
    assert!(worst < bar(Duration::from_millis(250)), "{worst:?}");
    ryueda(&["daemon", "stop"]);
}

/// Design 732's bar: a track write on the placed breakout through a warm
/// daemon, with its DRC delta, in 50 ms.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test bar"
)]
fn a_breakout_track_write_answers_its_delta_within_its_bar() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    std::fs::create_dir_all(&board).unwrap();
    git(&board, &["init", "-q"]);
    let run = dir.path().join("run");
    let ryueda = warm(&board, &run, dir.path());
    ryueda(&["init"]);
    for (path, text) in breakout::footprints() {
        let at = board.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script() {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ryueda(&args);
    }
    ryueda(&[
        "board", "create", "Main", "--design", "board", "--rect", "26,20", "--id", "main",
    ]);
    for (r, xy, rot) in [
        ("U1", "8,10", "0"),
        ("C1", "8,14", "90"),
        ("U2", "18,10", "0"),
        ("C2", "15,6", "0"),
        ("C3", "21,6", "0"),
        ("R1", "12,15", "90"),
        ("R2", "14,15", "90"),
        ("J1", "3,10", "0"),
    ] {
        ryueda(&["board", "place", "main", r, "--at", xy, "--rotation", rot]);
    }
    let net = ryueda(&["board", "show", "main", "--pads"])["pads"][0]["net"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut worst = Duration::ZERO;
    for i in 0..5 {
        let y = format!("{}", 1 + i);
        let t = Instant::now();
        let v = ryueda(&[
            "board",
            "track",
            "add",
            "main",
            "--net",
            &net,
            "--layer",
            "bottom",
            "--path",
            &format!("22,{y}"),
            &format!("24,{y}"),
            "--id",
            &format!("x{i}"),
        ]);
        worst = worst.max(t.elapsed());
        assert!(v["drc"].get("added").is_some(), "{v}");
    }
    println!("a breakout track write with its delta through a warm daemon: worst {worst:?}");
    assert!(worst < bar(Duration::from_millis(50)), "{worst:?}");
    ryueda(&["daemon", "stop"]);
}
