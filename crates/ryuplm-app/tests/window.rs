//! The window against real daemons (design 938, Testing): the list over
//! the seeded catalogue, a search, a pick read by an agent, an agent's
//! show adopted, a finding's Show, Undo, an import arriving as one round,
//! and a lost daemon found again.

mod common;

use common::{agent, catalogue, cli, daemon, gpu, one_at_a_time};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ryuplm_app::ui::app::App;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

fn window(root: &Path) -> Harness<'static, App> {
    let root = root.to_owned();
    Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .wgpu()
        .build_eframe(move |cc| App::open(&cc.egui_ctx, &root, daemon()))
}

/// Steps frames until `done` holds, 30 s at most.
fn frames(h: &mut Harness<'static, App>, done: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !done(h.state()) {
        assert!(Instant::now() < end, "the window never got there");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn loaded(h: &mut Harness<'static, App>, n: usize) {
    frames(h, |a| {
        a.opened()
            .is_some_and(|w| w.rows.total == Some(n) && !w.rows.rows.is_empty())
    });
}

/// The first row's item: `(id, number, name)`.
fn first(a: &App) -> (String, String, String) {
    let r = a.opened().unwrap().rows.get(0).unwrap();
    (
        r.id.clone(),
        r.number.clone(),
        r.name.clone().unwrap_or_default(),
    )
}

#[test]
fn the_list_reads_the_seeded_catalogue_and_searches() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = catalogue("list", 300);
    let mut h = window(&root);
    loaded(&mut h, 300);
    // Only the rows in view and those ahead were read.
    assert!(h.state().opened().unwrap().rows.rows.len() < 300);
    let (_, number, _) = first(h.state());
    let w = h.state_mut().opened_mut().unwrap();
    w.set_filter(ryuplm_app::core::rows::Filter {
        text: Some(number.clone()),
        ..Default::default()
    });
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| {
            w.rows.get(0).is_some_and(|r| r.number == number) && w.rows.total < Some(300)
        })
    });
    // The Navigator's counts came in.
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.counts.get("count:all") == Some(&300))
    });
}

#[test]
fn a_pick_is_read_by_the_agent() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = catalogue("pick", 300);
    let mut h = window(&root);
    loaded(&mut h, 300);
    h.step();
    let (id, number, name) = first(h.state());
    h.get_by_label(format!("{number} {name}").trim()).click();
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.selection == [format!("item/{id}")])
    });
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        h.step();
        let got = agent(&root, "selection.get", json!({}));
        if got["selection"] == json!([format!("item/{id}")]) {
            assert_eq!(got["objects"][0]["number"], number.as_str());
            break;
        }
        assert!(Instant::now() < end, "the agent never saw the pick: {got}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The pick opened the item.
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.card.value.as_ref())
            .is_some_and(|c| c.id == id)
    });
}

#[test]
fn an_agent_s_show_is_adopted() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = catalogue("show", 300);
    let mut h = window(&root);
    loaded(&mut h, 300);
    // An own line, added by the agent (the seeded catalogue has none).
    let (id, child) = {
        let w = h.state().opened().unwrap();
        (
            w.rows.get(0).unwrap().id.clone(),
            w.rows.get(1).unwrap().id.clone(),
        )
    };
    let added = agent(
        &root,
        "bom.add",
        json!({"item": id, "child": child, "quantity": "2"}),
    );
    let line = (
        id.clone(),
        added["line"].as_str().expect("a line").to_owned(),
    );
    let (id, l) = line;
    let a = agent(
        &root,
        "view.show",
        json!({"ref": format!("item/{id}/line/{l}")}),
    );
    assert_eq!(a["shown"].as_array().unwrap().len(), 1, "{a}");
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| {
            w.item.as_deref() == Some(id.as_str())
                && w.tab.as_deref() == Some("bom")
                && w.selection == [format!("item/{id}/line/{l}")]
        })
    });
    assert!(ryu_app::shell::is_open(&h.state().tree, "bom"));
    // With Follow what agents show off, the next show isn't adopted.
    h.state_mut().opened_mut().unwrap().follow_shows = false;
    let other = h.state().opened().unwrap().rows.get(5).unwrap().id.clone();
    agent(&root, "view.show", json!({"ref": format!("item/{other}")}));
    for _ in 0..20 {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        h.state().opened().unwrap().item.as_deref(),
        Some(id.as_str())
    );
}

#[test]
fn a_finding_s_show_selects_what_it_names() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = catalogue("finding", 300);
    let mut h = window(&root);
    loaded(&mut h, 300);
    let a = h.state_mut();
    ryu_app::shell::open(&ryuplm_app::ui::tiles::SPEC, &mut a.tree, "findings");
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.findings.value.as_ref())
            .is_some_and(|f| f.iter().any(|f| f.reference().is_some()))
    });
    h.step();
    let want = h
        .state()
        .opened()
        .unwrap()
        .findings
        .value
        .as_ref()
        .unwrap()
        .iter()
        .find_map(|f| f.reference())
        .unwrap();
    let k = h
        .state()
        .opened()
        .unwrap()
        .findings
        .value
        .as_ref()
        .unwrap()
        .iter()
        .position(|f| f.reference().is_some())
        .unwrap();
    h.query_all_by_label("Show").nth(k).unwrap().click();
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| w.selection == [want.clone()])
    });
}

#[test]
fn undo_takes_the_newest_change() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = catalogue("undo", 300);
    let mut h = window(&root);
    loaded(&mut h, 300);
    let (id, _, name) = first(h.state());
    let a = h.state_mut();
    ryu_app::shell::open(&ryuplm_app::ui::tiles::SPEC, &mut a.tree, "activity");
    agent(
        &root,
        "item.edit",
        json!({"item": id, "name": "Renamed by the agent"}),
    );
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.activity.value.as_ref())
            .is_some_and(|e| e.iter().any(|e| e.undo))
    });
    h.step();
    h.get_by_label("Undo agent edited").click();
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.step();
        let v = cli(&root, &["item", "show", &id]);
        if v["item"]["name"].as_str().unwrap_or_default() == name {
            break;
        }
        assert!(Instant::now() < end, "never undone: {v}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_import_arriving_as_one_change_is_one_round() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = catalogue("import", 300);
    agent(&root, "settings.edit", json!({"class": "part"}));
    let mut csv = String::from("ID,Number,Name\n");
    // 2,000 rows: the window's rule holds for any size, and seeded IDs
    // (RYUPLM_ID_SEED) make a large import slow to seed (tests/bar.rs).
    for k in 0..2_000 {
        csv += &format!("{k},P-{k:06},Part {k}\n");
    }
    let src = root.join("master.csv");
    std::fs::write(&src, &csv).unwrap();
    std::fs::create_dir_all(root.join("imports")).unwrap();
    std::fs::write(
        root.join("imports/erp.json"),
        ryuplm::canonical::to_string(&json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "csv"}, "key": "ID",
            "fields": {"number": {"from": "Number"}, "name": {"from": "Name"}}, "class": "part"})),
    )
    .unwrap();
    let mut h = window(&root);
    loaded(&mut h, 300);
    for t in ["findings", "changes", "imports", "activity"] {
        let a = h.state_mut();
        ryu_app::shell::open(&ryuplm_app::ui::tiles::SPEC, &mut a.tree, t);
        h.step();
    }
    frames(&mut h, |a| a.opened().is_some_and(|w| w.idle()));
    let before = h.state().opened().unwrap().asks.clone();
    agent(
        &root,
        "import.run",
        json!({"name": "erp", "file": src.display().to_string()}),
    );
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| w.counts.get("count:all") == Some(&2_300))
    });
    frames(&mut h, |a| a.opened().is_some_and(|w| w.idle()));
    let after = &h.state().opened().unwrap().asks;
    for (k, n) in after {
        let was = before.get(k).copied().unwrap_or(0);
        assert!(
            n - was <= 2,
            "{k} was asked {} times for one import",
            n - was
        );
    }
    assert_eq!(h.state().opened().unwrap().rows.total, Some(2_300));
}

#[test]
fn a_lost_daemon_is_found_again() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = catalogue("lost", 300);
    let mut h = window(&root);
    loaded(&mut h, 300);
    let out = std::process::Command::new(common::ryuplm_exe())
        .current_dir(&root)
        .args(["daemon", "stop"])
        .output()
        .unwrap();
    assert!(out.status.success());
    frames(&mut h, |a| a.opened().is_some_and(|w| w.lost));
    frames(&mut h, |a| {
        a.opened()
            .is_some_and(|w| !w.lost && w.rows.total == Some(300))
    });
}

#[test]
fn a_document_opens_through_the_opener() {
    let _one = one_at_a_time();
    if !gpu() {
        return;
    }
    let root = common::write("documents", ryuplm::testing::documents("app", 300, 20));
    // A Git-stored document whose bytes read, and an item naming it.
    let listed = cli(&root, &["document", "list"]);
    let (doc, item, size) = listed["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|d| {
            let id = d["id"].as_str()?;
            let s = cli(&root, &["document", "show", id]);
            (s["document"]["storage"] == "git" && s["faults"].as_array()?.is_empty())
                .then(|| {
                    Some((
                        id.to_owned(),
                        s["items"][0]["id"].as_str()?.to_owned(),
                        s["document"]["file"]["size"].as_u64()?,
                    ))
                })
                .flatten()
        })
        .expect("a readable Git-stored document named by an item");
    #[cfg(unix)]
    let said = {
        use std::os::unix::fs::PermissionsExt;
        let said = common::shared().join("opened");
        let _ = std::fs::remove_file(&said);
        let opener = common::shared().join("opener");
        std::fs::write(
            &opener,
            format!("#!/bin/sh\necho \"$1\" > '{}'\n", said.display()),
        )
        .unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: one test at a time; the window reads it when it opens bytes.
        unsafe { std::env::set_var("RYUPLM_OPENER", &opener) };
        said
    };
    let mut h = window(&root);
    frames(&mut h, |a| {
        a.opened().is_some_and(|w| w.rows.total.is_some())
    });
    let a = h.state_mut();
    a.opened_mut().unwrap().open_item(Some(item.clone()));
    ryu_app::shell::open(&ryuplm_app::ui::tiles::SPEC, &mut a.tree, "documents");
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.doc_cards.get(&doc))
            .is_some_and(|g| g.value.is_some())
    });
    h.step();
    let number = h.state().opened().unwrap().doc_cards[&doc]
        .value
        .as_ref()
        .unwrap()
        .number
        .clone();
    h.get_by_label(&format!("Open {number}")).click();
    // The opener is a shell script: its bytes are checked on Unix only.
    #[cfg(not(unix))]
    let _ = size;
    #[cfg(unix)]
    {
        let end = Instant::now() + Duration::from_secs(10);
        while !said.exists() {
            h.step();
            assert!(Instant::now() < end, "the opener never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
        let path = std::fs::read_to_string(&said).unwrap();
        assert_eq!(std::fs::metadata(path.trim()).unwrap().len(), size);
    }
    // Every document: the list, searched.
    let w = h.state_mut().opened_mut().unwrap();
    w.set_docs(true, &number);
    frames(&mut h, |a| {
        a.opened()
            .and_then(|w| w.documents.value.as_ref())
            .is_some_and(|d| d.iter().any(|d| d.number == number))
    });
}
