//! Design 734's bars, release only (a step in full.yml): 100,000 items
//! loaded by a cold daemon in 2 s; check over them in 1 s; a 100,000-row
//! first import in 20 s; re-importing it unchanged in 5 s, writing
//! nothing; where-used across 10 checkouts of 10,000 items in 1 s.
//! Design 935's: 100,000 JSON records and 100,000 XML records each read
//! into a table in 2 s, and imported first within the CSV's 20 s; a
//! history of 500,000 rows (100,000 items, five revisions each)
//! imported onto an existing master in 45 s, and again unchanged in
//! 10 s. Design 936's: a 100,000-row first import on a ledger catalogue,
//! its numbers in one request to the ledger's stand-in, within the same
//! 20 s, and 100,000 numbers asked and read in 2 s. Design 937's: 10,000
//! documents (640 MiB) beside them loaded by a cold daemon in 2.5 s; a
//! cold check hashing them in 3 s and a warm one in 1 s; document
//! where-used across 10 checkouts in 1 s. With
//! `RYUPLM_BARS=1` the bars are asserted as they are, else ten times
//! looser (a shared CI runner is no measuring instrument). Run with
//! `--nocapture` to see the times.

use assert_cmd::Command;
use ryuplm::proto::Request;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// Whether the bars are strict (RYUPLM_BARS=1, a quiet machine).
fn strict() -> bool {
    std::env::var("RYUPLM_BARS").is_ok_and(|v| v == "1")
}

fn bar(d: Duration) -> Duration {
    if strict() { d } else { d * 10 }
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
        Request::Check {
            within: vec![],
            against: None,
        },
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
        numbers: Default::default(),
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
fn a_bom_of_two_hundred_thousand_children() {
    // One top record holding every line, as a nested BOM exported from one
    // assembly is (design 935): twice the flat records, read within their
    // 2 s. A reader that is quadratic in a parent's children takes about
    // 7 s here.
    use ryuplm::import::mapping::Mapping;
    let mut xml = String::from("<Bom><Item pn=\"TOP\"><Uses>");
    for k in 0..200_000 {
        xml += &format!(
            "<Item pn=\"P-{k:06}\" qty=\"{}\"><Name>Part {k}</Name></Item>",
            1 + k % 3
        );
    }
    xml += "</Uses></Item></Bom>";
    let m = Mapping::from_json(
        "bom",
        &json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "xml", "records": "/Bom/Item", "children": "Uses/Item"},
            "key": "@pn", "fields": {"number": {"from": "@pn"}, "name": {"from": "Name"}},
            "bom": {"level": "#level", "quantity": "@qty"}}),
    )
    .unwrap();
    let t = Instant::now();
    let table = ryuplm::import::read_table(xml.as_bytes(), &m.reader).unwrap();
    within(
        "a BOM of 200,000 xml children read",
        t.elapsed(),
        Duration::from_secs(2),
    );
    assert_eq!(table.rows.len(), 200_001);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the bars time a release build"
)]
fn a_history_of_half_a_million_rows() {
    // Unseeded, as the CSV import's bar is.
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
    let mut master = String::from("ID,Number,Name\n");
    let mut history = String::from("Number,Rev,Released,Lifecycle,ECO,By\n");
    for k in 0..100_000 {
        master += &format!("{k},P-{k:06},Part {k}\n");
        for (r, label) in ["A", "B", "C", "D", "E"].into_iter().enumerate() {
            history += &format!(
                "P-{k:06},{label},{:02}/{:02}/{},{},ECO-{k}-{r},Author {}\n",
                1 + (k + r) % 28,
                1 + r,
                2015 + r,
                if r == 4 && k % 3 == 0 {
                    "In Work"
                } else {
                    "Released"
                },
                k % 9
            );
        }
    }
    std::fs::create_dir_all(root.join("imports")).unwrap();
    std::fs::write(
        root.join("imports/erp.json"),
        ryuplm::canonical::to_string(&json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "csv"}, "key": "ID",
            "fields": {"number": {"from": "Number"}, "name": {"from": "Name"}}, "class": "part"})),
    )
    .unwrap();
    std::fs::write(
        root.join("imports/history.json"),
        ryuplm::canonical::to_string(&json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "csv"}, "key": "Number", "fields": {"number": {"from": "Number"}},
            "numbers": "keep",
            "history": {"revision": "Rev", "date": {"from": "Released", "date": ["DD/MM/YYYY"]},
                "state": "Lifecycle", "change": "ECO", "author": "By",
                "released": {"Lifecycle": "Released"}}})),
    )
    .unwrap();
    std::fs::write(root.join("master.csv"), &master).unwrap();
    std::fs::write(root.join("history.csv"), &history).unwrap();
    let run = |slot: &mut Option<ryuplm::session::Session>, name: &str, file: &str| {
        ask(
            slot,
            &root,
            Request::ImportRun {
                name: name.into(),
                file: root.join(file).display().to_string(),
                dry_run: false,
                prefer: None,
                limit: Some(10),
                numbers: Default::default(),
                expected_revision: None,
            },
        )
    };
    assert_eq!(
        run(&mut slot, "erp", "master.csv")["created"]["count"],
        100_000
    );
    let t = Instant::now();
    let first = run(&mut slot, "history", "history.csv");
    within(
        "a history of 500,000 rows imported onto 100,000 items",
        t.elapsed(),
        Duration::from_secs(45),
    );
    assert_eq!(first["adopted"]["count"], 100_000);
    // Again, cold: a fresh session, as a new process would have.
    let mut cold = None;
    let t = Instant::now();
    let again = run(&mut cold, "history", "history.csv");
    within(
        "the same history imported again",
        t.elapsed(),
        Duration::from_secs(10),
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
fn a_hundred_thousand_tree_records() {
    use ryuplm::import::mapping::{Mapping, ReaderKind};
    // Unseeded, as the CSV import's bar is.
    // SAFETY: no test in this file sets the seed.
    unsafe { std::env::remove_var("RYUPLM_ID_SEED") };
    let mut csv = String::from("ID,Number,Name,Description,Maker,MPN,Note\n");
    for k in 0..100_000 {
        csv += &format!(
            "{k},P-{k:06},Part {k},\"Part number {k}, made up\",Maker {},M{k},n{}\n",
            k % 50,
            k % 7
        );
    }
    let erp = Mapping::from_json(
        "erp",
        &json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {"number": {"from": "Number"}, "name": {"from": "Name"}, "description": {"from": "Description"},
                "aml.0.manufacturer": {"from": "Maker"}, "aml.0.mpn": {"from": "MPN"}}, "class": "part"}),
    )
    .unwrap();
    let (json_text, xml_text) = ryuplm::testing::csv_as_trees(&csv);
    for (kind, ext, bytes) in [
        (ReaderKind::Json, "json", json_text.into_bytes()),
        (ReaderKind::Xml, "xml", xml_text.into_bytes()),
    ] {
        let m = ryuplm::testing::tree_mapping(&erp, kind);
        let t = Instant::now();
        let table = ryuplm::import::read_table(&bytes, &m.reader).unwrap();
        within(
            &format!("100,000 {ext} records read"),
            t.elapsed(),
            Duration::from_secs(2),
        );
        assert_eq!(table.rows.len(), 100_000);
        #[cfg(target_os = "linux")]
        if let Ok(s) = std::fs::read_to_string("/proc/self/status")
            && let Some(l) = s.lines().find(|l| l.starts_with("VmHWM"))
        {
            println!(
                "after the {ext} read, the process's peak memory: {}",
                l.trim_start_matches("VmHWM:").trim()
            );
        }
        drop(table);
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
        let src = root.join(format!("master.{ext}"));
        std::fs::write(&src, &bytes).unwrap();
        std::fs::create_dir_all(root.join("imports")).unwrap();
        std::fs::write(root.join("imports/erp.json"), m.to_text()).unwrap();
        let t = Instant::now();
        let first = ask(
            &mut slot,
            &root,
            Request::ImportRun {
                name: "erp".into(),
                file: src.display().to_string(),
                dry_run: false,
                prefer: None,
                limit: Some(10),
                numbers: Default::default(),
                expected_revision: None,
            },
        );
        within(
            &format!("first import of 100,000 {ext} records"),
            t.elapsed(),
            Duration::from_secs(20),
        );
        assert_eq!(first["created"]["count"], 100_000);
    }
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
            state: None,
            make_buy: None,
            faults: None,
            source: None,
            sort: None,
            offset: None,
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

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the bars time a release build"
)]
fn the_index_and_list_windows_of_a_hundred_thousand_items() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("big");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    write_catalogue(&root, "bar", 100_000);
    let mut slot = None;
    ask(&mut slot, &root, Request::ProjectInfo {});
    // The index, warm: judged, joined and written.
    let out = d.path().join("index.jsonl");
    let t = Instant::now();
    let v = ask(
        &mut slot,
        &root,
        Request::Index {
            out: out.display().to_string(),
            rev: None,
            within: vec![],
        },
    );
    within(
        "index of 100,000 items",
        t.elapsed(),
        Duration::from_secs(10),
    );
    assert_eq!(v["items"], 100_000, "{v}");
    assert_eq!(
        std::fs::read_to_string(&out).unwrap().lines().count(),
        100_002
    );
    // The first list builds the listing (states, faults counted): not
    // the bar, but said.
    let list = |slot: &mut Option<ryuplm::session::Session>, ask_: Value| {
        ask(
            slot,
            &root,
            Request::from_rpc("item.list", Some(ask_)).unwrap(),
        )
    };
    let t = Instant::now();
    let first = list(&mut slot, json!({"limit": 200}));
    println!("the listing built and its first window: {:?}", t.elapsed());
    assert_eq!(
        (first["total"].clone(), first["count"].clone()),
        (json!(100_000), json!(200))
    );
    // Each sort's first window sorts once; said, not barred.
    for sort in ["number", "name", "class"] {
        let t = Instant::now();
        list(&mut slot, json!({"sort": sort, "limit": 200}));
        println!("first window by {sort}: {:?}", t.elapsed());
    }
    // The bar: any window of 200, any sort and filter, from a warm daemon.
    let windows = [
        json!({"limit": 200, "offset": 50_000}),
        json!({"sort": "number", "limit": 200, "offset": 99_800}),
        json!({"sort": "name", "limit": 200, "offset": 10_000, "state": "in_work"}),
        json!({"sort": "class", "limit": 200, "text": "bracket"}),
        json!({"sort": "number", "limit": 200, "faults": "warnings"}),
        json!({"limit": 200, "make_buy": "buy", "text": "m3"}),
    ];
    let mut worst = Duration::ZERO;
    for w in &windows {
        let t = Instant::now();
        let v = list(&mut slot, w.clone());
        let took = t.elapsed();
        println!("window {w}: {took:?}, {} of {}", v["count"], v["total"]);
        worst = worst.max(took);
    }
    // A recorded miss (#1018): about 180 ms of every warm request is the
    // session's freshness scan (ryu-session's `fresh::Stamps` statting
    // 100,000 files), the window itself about 1 ms. Strict runs
    // (RYUPLM_BARS=1) still assert 20 ms, so they show the miss; the loose
    // run (full.yml) would sit just above the measured time and go red at
    // random, so it says the time instead. The fix is a later designed
    // plan: every product shares the freshness check.
    let what = "a 200-row window of 100,000 items, warm";
    if strict() {
        within(what, worst, Duration::from_millis(20));
    } else {
        println!(
            "{what}: {worst:?} (bar 20ms, a recorded miss: #1018; asserted only with RYUPLM_BARS=1)"
        );
    }
}

/// Design 936's client bar: a 100,000-row first import on a ledger
/// catalogue takes its numbers in one `next` request (to the stand-in),
/// within the first import's 20 s; the request alone, 100,000 numbers
/// asked and read, in 2 s.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the bars time a release build"
)]
fn a_hundred_thousand_numbers_from_the_ledger() {
    // SAFETY: no test in this file sets the seed.
    unsafe { std::env::remove_var("RYUPLM_ID_SEED") };
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    ryuplm::server::init(&root, None).unwrap();
    let mut slot = None;
    for change in [
        json!({"scheme": "mech", "pattern": "MECH-\\d{6}", "next_template": "MECH-{seq:6}"}),
        json!({"ledger": true}),
    ] {
        ask(
            &mut slot,
            &root,
            Request::SettingsEdit {
                change: serde_json::from_value(change).unwrap(),
                expected_revision: None,
            },
        );
    }
    let mut csv = String::from("ID,Name\n");
    for k in 0..100_000 {
        csv += &format!("{k},Part {k}\n");
    }
    let src = root.join("master.csv");
    std::fs::write(&src, &csv).unwrap();
    std::fs::create_dir_all(root.join("imports")).unwrap();
    std::fs::write(
        root.join("imports/erp.json"),
        ryuplm::canonical::to_string(&json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "csv"}, "key": "ID", "fields": {"name": {"from": "Name"}},
            "numbers": "assign", "scheme": "mech"})),
    )
    .unwrap();
    let (fake, state) = ryu_review::testing::Ledger::start("acme/widgets");
    let url = fake.url.clone();
    let open = move || -> ryuplm::error::Result<ryuplm::ledger::Ledger> {
        Ok(ryuplm::ledger::Ledger::at(&url, "t", "acme/widgets"))
    };
    let mut send = |r: Request| {
        ryuplm::server::handle_safely(&mut slot, None, None, &root, r).map(|a| a.result)
    };
    let req = Request::ImportRun {
        name: "erp".into(),
        file: src.display().to_string(),
        dry_run: false,
        prefer: None,
        limit: Some(10),
        expected_revision: None,
        numbers: Default::default(),
    };
    let t = Instant::now();
    let v = ryuplm::ledger::import_run(&mut send, &open, req, &mut Vec::new()).unwrap();
    within(
        "first import of 100,000 rows, numbers from the ledger",
        t.elapsed(),
        Duration::from_secs(20),
    );
    assert_eq!(
        (&v["created"]["count"], &v["ledger"]["requests"]),
        (&json!(100_000), &json!(1))
    );
    assert_eq!(state.lock().unwrap().next_calls, 1);
    // The request alone: 100,000 more numbers asked and read.
    let l = ryuplm::ledger::Ledger::at(&fake.url, "t", "acme/widgets");
    let t = Instant::now();
    let more = l
        .next(
            "0123456789abcdef0123456789abcdef",
            "MECH-{seq:6}",
            100_000,
            100_000,
            None,
        )
        .unwrap();
    within(
        "100,000 numbers asked of the ledger",
        t.elapsed(),
        Duration::from_secs(2),
    );
    assert_eq!(more["numbers"][0], "MECH-100001");
}

/// `m` documents of `size` bytes written into the catalogue at `root`,
/// numbered `DOC-<k>`; the first `named` items each naming the first
/// document.
fn write_documents(root: &Path, m: usize, size: usize, named: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(m);
    let mut seeded = ryuplm::ids::Seeded::new("bar.documents");
    for k in 0..m {
        let id = seeded.next().unwrap();
        let mut bytes = vec![0u8; size];
        let mut h = blake3::Hasher::new();
        h.update(&(k as u64).to_le_bytes());
        h.finalize_xof().fill(&mut bytes);
        let d = ryuplm::document::Document::new(
            &format!("DOC-{k:05}"),
            "Spec",
            ryuplm::document::File::of("spec.pdf", &bytes, None).unwrap(),
        );
        for (p, b) in [
            (ryuplm::ids::document_path(&id), d.to_text().into_bytes()),
            (ryuplm::ids::bytes_path(&id, "spec.pdf"), bytes),
        ] {
            let at = root.join(p);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(at, b).unwrap();
        }
        ids.push(id.to_string());
    }
    let items: Vec<std::path::PathBuf> = walk_items(root).into_iter().take(named).collect();
    for p in items {
        let id = ryuplm::ids::item_of(&rel(root, &p)).unwrap();
        let mut item = ryuplm::item::Item::read(&id, &std::fs::read(&p).unwrap()).unwrap();
        item.documents
            .push(ryuplm::document::DocRef::parse(&ids[0]).unwrap());
        std::fs::write(&p, item.to_text()).unwrap();
    }
    ids
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the bars time a release build"
)]
fn ten_thousand_documents() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("big");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    write_catalogue(&root, "bar", 100_000);
    write_documents(&root, 10_000, 64 << 10, 0);
    let run = d.path().join("run");
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
        "cold daemon load of 100,000 items and 10,000 documents (640 MiB)",
        t.elapsed(),
        Duration::from_millis(2500),
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["result"]["documents"], 10_000, "{v}");
    let _ = Command::cargo_bin("ryuplm")
        .unwrap()
        .current_dir(&root)
        .env("RYUPLM_RUNTIME_DIR", &run)
        .args(["daemon", "stop"])
        .output();
    // A cold check: a fresh process's session, every file hashed.
    let mut slot = None;
    let t = Instant::now();
    ask(&mut slot, &root, Request::ProjectInfo {});
    let _ = ryuplm::server::handle_safely(
        &mut slot,
        None,
        None,
        &root,
        Request::Check {
            within: vec![],
            against: None,
        },
    );
    within(
        "cold check hashing 10,000 documents",
        t.elapsed(),
        Duration::from_secs(3),
    );
    // Warm: nothing hashed again.
    let t = Instant::now();
    let _ = ryuplm::server::handle_safely(
        &mut slot,
        None,
        None,
        &root,
        Request::Check {
            within: vec![],
            against: None,
        },
    );
    within(
        "warm check of 100,000 items and 10,000 documents",
        t.elapsed(),
        Duration::from_secs(1),
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the bars time a release build"
)]
fn document_where_used_across_ten_checkouts() {
    let d = tempfile::tempdir().unwrap();
    let top = d.path().canonicalize().unwrap();
    let here = top.join("c0");
    std::fs::create_dir_all(&here).unwrap();
    write_catalogue(&here, "bar0", 10_000);
    let doc = write_documents(&here, 1, 1024, 100).remove(0);
    let project =
        ryuplm::catalogue::Manifest::read(&std::fs::read(here.join("ryuplm.json")).unwrap())
            .unwrap()
            .project;
    for k in 1..10 {
        let c = top.join(format!("c{k}"));
        std::fs::create_dir_all(&c).unwrap();
        write_catalogue(&c, &format!("bar{k}"), 10_000);
        // Each names c0's document from 100 of its items.
        for p in walk_items(&c).into_iter().take(100) {
            let id = ryuplm::ids::item_of(&rel(&c, &p)).unwrap();
            let mut item = ryuplm::item::Item::read(&id, &std::fs::read(&p).unwrap()).unwrap();
            item.documents
                .push(ryuplm::document::DocRef::parse(&format!("{project}:{doc}")).unwrap());
            std::fs::write(&p, item.to_text()).unwrap();
        }
    }
    let mut slot = None;
    ask(&mut slot, &here, Request::ProjectInfo {});
    let t = Instant::now();
    let v = ask(
        &mut slot,
        &here,
        Request::DocumentWhereUsed {
            document: doc,
            within: vec![top.display().to_string()],
            no_dependents: true,
            working: false,
        },
    );
    within(
        "document where-used across 10 checkouts of 10,000 items",
        t.elapsed(),
        Duration::from_secs(1),
    );
    assert_eq!(
        v["working"].as_array().unwrap().len(),
        1000,
        "{}",
        v["problems"]
    );
}

fn walk_items(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for shard in std::fs::read_dir(root.join("items")).unwrap().flatten() {
        for f in std::fs::read_dir(shard.path()).unwrap().flatten() {
            out.push(f.path());
        }
    }
    out.sort();
    out
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}
