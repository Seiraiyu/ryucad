//! Design 734's bars, release only (a step in full.yml): 100,000 items
//! loaded by a cold daemon in 2 s; check over them in 1 s; a 100,000-row
//! first import in 20 s; re-importing it unchanged in 5 s, writing
//! nothing; where-used across 10 checkouts of 10,000 items in 1 s. With
//! `RYUPLM_BARS=1` the bars are asserted as they are, else ten times
//! looser (a shared CI runner is no measuring instrument). Run with
//! `--nocapture` to see the times.

use assert_cmd::Command;
use ryuplm::proto::Request;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

fn bar(d: Duration) -> Duration {
    if std::env::var("RYUPLM_BARS").is_ok_and(|v| v == "1") {
        d
    } else {
        d * 10
    }
}

fn within(what: &str, took: Duration, limit: Duration) {
    println!("{what}: {took:?} (bar {:?})", bar(limit));
    assert!(
        took <= bar(limit),
        "{what} took {took:?}, over {:?}",
        bar(limit)
    );
}

/// `n` items written to `root` as a catalogue: `ryuplm init`'s manifest,
/// then each seeded item's file as it is. The seed shares thirteen numbers
/// out among three in seven items, so at 100,000 items about 43,000 of
/// them sit in groups of 3,300: each `part_number_duplicate` names ten of
/// its group and counts the rest (#989), which `check` must judge too.
fn write_catalogue(root: &Path, seed: &str, n: usize) {
    ryuplm::server::init(root, None).unwrap();
    for (p, b) in ryuplm::testing::catalogue(seed, n) {
        if p == ryuplm::catalogue::MANIFEST {
            continue; // keep init's own project ID
        }
        let at = root.join(&p);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, b).unwrap();
    }
}

fn ask(slot: &mut Option<ryuplm::session::Session>, root: &Path, r: Request) -> Value {
    ryuplm::server::handle_safely(slot, None, None, root, r)
        .unwrap()
        .result
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the bars time a release build"
)]
fn a_hundred_thousand_items() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("big");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    write_catalogue(&root, "bar", 100_000);
    let run = d.path().join("run");
    // A cold daemon: started, the catalogue read, the answer printed.
    let t = Instant::now();
    let out = Command::cargo_bin("ryuplm")
        .unwrap()
        .current_dir(&root)
        .env("RYUPLM_RUNTIME_DIR", &run)
        .env("RYUPLM_CONFIG_DIR", d.path().join("config"))
        .env_remove("RYUPLM_NO_DAEMON")
        .arg("info")
        .output()
        .unwrap();
    within(
        "cold daemon load of 100,000 items",
        t.elapsed(),
        Duration::from_secs(2),
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["result"]["items"], 100_000, "{v}");
    let _ = Command::cargo_bin("ryuplm")
        .unwrap()
        .current_dir(&root)
        .env("RYUPLM_RUNTIME_DIR", &run)
        .args(["daemon", "stop"])
        .output();
    // check, warm.
    let mut slot = None;
    ask(&mut slot, &root, Request::ProjectInfo {});
    let t = Instant::now();
    let _ = ryuplm::server::handle_safely(
        &mut slot,
        None,
        None,
        &root,
        Request::Check { within: vec![] },
    );
    within(
        "check of 100,000 items",
        t.elapsed(),
        Duration::from_secs(1),
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the bars time a release build"
)]
fn a_hundred_thousand_row_import() {
    // Unseeded: a seeded ID retries every earlier draw first, so seeding
    // 100,000 new items is quadratic in hashes; the bar times the import,
    // not the test's seed.
    // SAFETY: no test in this file sets the seed.
    unsafe { std::env::remove_var("RYUPLM_ID_SEED") };
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    ryuplm::server::init(&root, None).unwrap();
    let mut slot = None;
    ask(
        &mut slot,
        &root,
        Request::SettingsEdit {
            change: serde_json::from_value(json!({"class": "part"})).unwrap(),
            expected_revision: None,
        },
    );
    let mut csv = String::from("ID,Number,Name,Description,Maker,MPN,Note\n");
    for k in 0..100_000 {
        csv += &format!(
            "{k},P-{k:06},Part {k},\"Part number {k}, made up\",Maker {},M{k},n{}\n",
            k % 50,
            k % 7
        );
    }
    let src = root.join("master.csv");
    std::fs::write(&src, &csv).unwrap();
    std::fs::create_dir_all(root.join("imports")).unwrap();
    std::fs::write(
        root.join("imports/erp.json"),
        ryuplm::canonical::to_string(&json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {"number": {"from": "Number"}, "name": {"from": "Name"}, "description": {"from": "Description"},
                "aml.0.manufacturer": {"from": "Maker"}, "aml.0.mpn": {"from": "MPN"}}, "class": "part"})),
    )
    .unwrap();
    let file = src.display().to_string();
    let req = || Request::ImportRun {
        name: "erp".into(),
        file: file.clone(),
        dry_run: false,
        prefer: None,
        limit: Some(10),
        expected_revision: None,
    };
    let t = Instant::now();
    let first = ask(&mut slot, &root, req());
    within(
        "first import of 100,000 rows",
        t.elapsed(),
        Duration::from_secs(20),
    );
    assert_eq!(first["created"]["count"], 100_000);
    // Re-import, cold: a fresh session, as a new process would have.
    let mut cold = None;
    let t = Instant::now();
    let again = ask(&mut cold, &root, req());
    within(
        "unchanged re-import of 100,000 rows",
        t.elapsed(),
        Duration::from_secs(5),
    );
    assert_eq!(
        (again["unchanged"].clone(), again["written"].clone()),
        (json!(100_000), json!(0))
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the bars time a release build"
)]
fn where_used_across_ten_checkouts() {
    let d = tempfile::tempdir().unwrap();
    let top = d.path().canonicalize().unwrap();
    let here = top.join("c0");
    for k in 0..10 {
        let c = top.join(format!("c{k}"));
        std::fs::create_dir_all(&c).unwrap();
        write_catalogue(&c, &format!("bar{k}"), 10_000);
    }
    let mut slot = None;
    let item = ask(
        &mut slot,
        &here,
        Request::ItemList {
            number: None,
            class: None,
            text: None,
            limit: Some(1),
        },
    )["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let t = Instant::now();
    let v = ask(
        &mut slot,
        &here,
        Request::BomWhereUsed {
            item,
            within: vec![top.display().to_string()],
            no_dependents: true,
            working: false,
        },
    );
    within(
        "where-used across 10 checkouts of 10,000 items",
        t.elapsed(),
        Duration::from_secs(1),
    );
    assert_eq!(
        v["searched"].as_array().unwrap().len(),
        9,
        "{}",
        v["problems"]
    );
}
