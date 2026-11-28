//! The pinned KiCad corpus, release only (its own job in full.yml):
//! KiCad 9.0.9's symbol, footprint and 3D model libraries, fetched by
//! `scripts/fetch-kicad.sh` into `target/kicad` (or
//! `$RYUEDA_KICAD_CORPUS`) and never committed. Every item imports or is
//! a typed refusal, and no fewer import than `tests/kicad/pinned.txt`
//! records: raise a count when a change imports more, never lower one.
//! Without the corpus the test says so and passes, unless
//! `RYUEDA_REQUIRE_KICAD=1` (full.yml sets it). Run with `--nocapture`
//! for the counts and times.

use ryueda::error::Code;
use ryueda::kicad::import::{Options, import};
use ryueda::session::Session;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The corpus folder, or `None` when it hasn't been fetched.
fn corpus() -> Option<PathBuf> {
    let dir = std::env::var_os("RYUEDA_KICAD_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/kicad"));
    if dir.join("kicad-symbols-9.0.9").is_dir() {
        return Some(dir);
    }
    assert!(
        !std::env::var("RYUEDA_REQUIRE_KICAD").is_ok_and(|v| v == "1"),
        "no KiCad corpus in {}: run scripts/fetch-kicad.sh",
        dir.display()
    );
    println!(
        "no KiCad corpus in {}; run scripts/fetch-kicad.sh",
        dir.display()
    );
    None
}

/// `imports <what> <at least>` from pinned.txt.
fn floors() -> BTreeMap<String, usize> {
    include_str!("kicad/pinned.txt")
        .lines()
        .filter_map(|l| l.strip_prefix("imports "))
        .filter_map(|l| l.split_once(' '))
        .map(|(k, n)| (k.to_owned(), n.trim().parse().unwrap()))
        .collect()
}

/// A new project in a folder of its own.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    // Its own repository, so the install lands there and never in one
    // above the temp folder (#1004).
    assert!(
        ryu_git::git_env::isolated()
            .args(["init", "-q"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    ryueda::server::init(dir.path(), None, false).unwrap();
    let s = Session::open(dir.path()).unwrap();
    (dir, s)
}

/// The codes a refused item may carry: a KiCad file or item ryueda
/// can't read, or what ryueda's checks refuse.
fn typed(code: &str) -> bool {
    code == "kicad_unreadable" || code == "not_found" || code.starts_with("library_")
}

fn sorted(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    v.sort();
    v
}

/// Counts what an answer added (or found unchanged) and refused.
fn count(r: &Value, into: &mut BTreeMap<String, usize>) {
    for kind in ["symbols", "footprints", "components"] {
        let t = &r[kind];
        *into.entry(kind.to_owned()).or_default() +=
            t["added"].as_array().unwrap().len() + t["unchanged"].as_u64().unwrap() as usize;
    }
    *into.entry("models".to_owned()).or_default() += r["models"]["added"].as_array().unwrap().len();
    for x in r["refused"].as_array().unwrap() {
        let code = x["error"]["code"].as_str().unwrap();
        assert!(typed(code), "{x}");
        *into
            .entry(format!("refused {} {code}", x["kind"].as_str().unwrap()))
            .or_default() += 1;
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryueda --release --test kicad_corpus"
)]
fn the_pinned_kicad_libraries_import_or_are_refused_by_type() {
    let Some(dir) = corpus() else { return };
    let (symbols, footprints, models) = (
        dir.join("kicad-symbols-9.0.9"),
        dir.join("kicad-footprints-9.0.9"),
        dir.join("kicad-packages3D-9.0.9"),
    );
    let mut got = BTreeMap::new();

    // Each footprint library with its models, in a project of its own so
    // the disk holds one library's models at a time.
    let start = Instant::now();
    for pretty in sorted(&footprints, "pretty") {
        let (_dir, mut s) = project();
        let opts = Options {
            paths: vec![pretty.clone()],
            models: vec![models.clone()],
            ..Options::default()
        };
        let r = import(&mut s, &opts, None).unwrap_or_else(|e| panic!("{}: {e}", pretty.display()));
        count(&r, &mut got);
    }
    let fp_time = start.elapsed();

    // Every symbol library at once, the footprints they name looked up.
    let start = Instant::now();
    let (_dir, mut s) = project();
    let opts = Options {
        paths: sorted(&symbols, "kicad_sym"),
        footprints: vec![footprints.clone()],
        ..Options::default()
    };
    let r = import(&mut s, &opts, None).unwrap();
    let sym_time = start.elapsed();
    let mut sym = BTreeMap::new();
    count(&r, &mut sym);
    for k in ["symbols", "components"] {
        got.insert(k.to_owned(), sym[k]);
    }
    for (k, v) in sym
        .iter()
        .filter(|(k, _)| k.starts_with("refused symbol") || k.starts_with("refused libcomp"))
    {
        got.insert(k.clone(), *v);
    }
    let power = r["notices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["code"] == serde_json::json!(Code::ImportPowerSkipped))
        .map(|n| n["details"]["symbols"].as_array().unwrap().len())
        .sum::<usize>();
    println!("KiCad 9.0.9: {got:?}; {power} power symbols skipped");
    println!("footprint libraries with models in {fp_time:?}, symbol libraries in {sym_time:?}");
    for (k, floor) in floors() {
        let n = got.get(&k).copied().unwrap_or(0);
        assert!(n >= floor, "{k}: {n} imported, pinned.txt records {floor}");
    }
}

/// KiCad's own USB-C receptacle (ryueda 7.F's review), from the pinned
/// corpus: it imports, and `lib check` passes with its stacked lands
/// (A4 on B9 and A9 on B4 for VBUS, A1 on B12 and A12 on B1 for GND) as
/// `footprint_pads_overlap` warnings, never errors. Read from the fetch,
/// not committed: no KiCad library file is (the owner's decision of
/// 2026-10-28, `docs/claude/ryueda.md`). The default build's
/// `a_usb_c_receptacles_stacked_lands_dont_fail_lib_check` holds the rule
/// on a file of our own.
#[test]
fn kicads_usb_c_receptacle_passes_lib_check_with_overlap_warnings() {
    let Some(dir) = corpus() else { return };
    let file = dir.join(
        "kicad-footprints-9.0.9/Connector_USB.pretty/USB_C_Receptacle_GCT_USB4105-xx-A_16P_TopMnt_Horizontal.kicad_mod",
    );
    let (_dir, mut s) = project();
    let opts = Options {
        paths: vec![file],
        ..Options::default()
    };
    let r = import(&mut s, &opts, None).unwrap();
    assert_eq!(r["refused"], serde_json::json!([]), "{r}");
    let list = ryueda::libcmd::footprint_list(s.project(), None).unwrap();
    let id = list["footprints"][0]["id"].as_str().unwrap().to_owned();
    let listing = ryueda::symlint::Listing {
        summary: false,
        codes: Vec::new(),
    };
    let a = ryueda::symlint::library_check(s.project(), None, None, Some(&id), &listing)
        .unwrap()
        .into_json()
        .expect("lib check passes: no errors");
    assert_eq!(a.result["errors"], 0, "{}", a.result);
    let found: Vec<&str> = a.result["footprints"][0]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    let overlaps = found
        .iter()
        .filter(|c| **c == "footprint_pads_overlap")
        .count();
    assert_eq!(overlaps, 4, "the four stacked pairs: {found:?}");
}
