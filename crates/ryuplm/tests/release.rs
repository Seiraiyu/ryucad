//! Items released, listed, made obsolete and checked through ryupdm, in a
//! real Git repository, in process.

use ryuplm::proto::Request;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn ask(root: &Path, req: Request) -> ryuplm::error::Result<Value> {
    ryuplm::server::handle_safely(&mut None, None, Some("agent"), root, req).map(|a| a.result)
}

#[test]
fn an_item_is_released_and_made_obsolete() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    ryuplm::server::init(&root, None).unwrap();
    let made = ask(
        &root,
        Request::ItemCreate {
            fields: serde_json::from_value(json!({"name": "Label"})).unwrap(),
            number: Some("LBL-1".into()),
            next: None,
            expected_revision: None,
        },
    )
    .unwrap();
    let id = made["item"]["id"].as_str().unwrap().to_owned();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "an item"]);
    // Records committed on main are official at once (ryueda's tests
    // propose on a branch for the same reason): propose on a branch.
    git(&root, &["checkout", "-q", "-b", "rel"]);
    let dry = ask(
        &root,
        Request::ReleasePropose {
            items: vec![id.clone()],
            all: false,
            dry_run: true,
            note: None,
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(dry["items"][0]["label"], "A");
    assert!(!root.join("releases").exists());
    let p = ask(
        &root,
        Request::ReleasePropose {
            items: vec!["LBL-1".into()],
            all: false,
            dry_run: false,
            note: Some("first".into()),
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(
        p["proposed"][0]["record"]["built_with"]["product"],
        "ryuplm"
    );
    assert_eq!(p["proposed"][0]["record"]["properties"]["number"], "LBL-1");
    assert_eq!(p["proposed"][0]["state"]["state"], "in_work");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "release LBL-1 A"]);
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["merge", "-q", "--no-ff", "rel", "-m", "merge"]);
    let listed = ask(
        &root,
        Request::ReleaseList {
            item: Some(id.clone()),
        },
    )
    .unwrap();
    assert_eq!(listed["schema"], 1);
    assert_eq!(listed["items"][0]["state"], "released");
    ask(&root, Request::ReleaseCheck {}).unwrap();
    let synced = ask(&root, Request::ReleaseSync {}).unwrap();
    assert!(
        synced.to_string().contains(&format!("ryupdm/{id}/A")),
        "{synced}"
    );
    // A change makes it modified; obsolete needs the latest label.
    ask(
        &root,
        Request::ItemEdit {
            item: id.clone(),
            fields: serde_json::from_value(json!({"description": "x"})).unwrap(),
            unset: vec![],
            expected_revision: None,
        },
    )
    .unwrap();
    let shown = ask(&root, Request::ItemShow { item: id.clone() }).unwrap();
    assert_eq!(shown["state"]["state"], "modified");
    let wrong = ask(
        &root,
        Request::ReleaseObsolete {
            item: id.clone(),
            label: "B".into(),
            note: None,
            expected_revision: None,
        },
    )
    .unwrap_err();
    assert_ne!(wrong.code, ryuplm::error::Code::Internal);
    ask(
        &root,
        Request::ReleaseObsolete {
            item: id.clone(),
            label: "A".into(),
            note: None,
            expected_revision: None,
        },
    )
    .unwrap();
    assert!(
        root.join(format!("releases/{id}/A.obsolete.json"))
            .is_file()
    );
}

#[test]
fn an_item_missing_a_required_property_is_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    ryuplm::server::init(&root, None).unwrap();
    ask(
        &root,
        Request::SettingsEdit {
            change: serde_json::from_value(json!({
        "property": "finish", "declaration": {"type": "string", "required": true}}))
            .unwrap(),
            expected_revision: None,
        },
    )
    .unwrap();
    ask(
        &root,
        Request::ItemCreate {
            fields: Default::default(),
            number: Some("P-1".into()),
            next: None,
            expected_revision: None,
        },
    )
    .unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "x"]);
    git(&root, &["checkout", "-q", "-b", "rel"]);
    let e = ask(
        &root,
        Request::ReleasePropose {
            items: vec!["P-1".into()],
            all: false,
            dry_run: false,
            note: None,
            expected_revision: None,
        },
    )
    .unwrap_err();
    assert_eq!(e.code, ryuplm::error::Code::ReleaseNotReady);
    assert!(e.message.contains("finish"), "{}", e.message);
}
#[test]
fn diff_names_items_added_removed_and_changed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    ryuplm::server::init(&root, None).unwrap();
    let a = ask(
        &root,
        Request::ItemCreate {
            fields: Default::default(),
            number: Some("A-1".into()),
            next: None,
            expected_revision: None,
        },
    )
    .unwrap();
    let b = ask(
        &root,
        Request::ItemCreate {
            fields: Default::default(),
            number: Some("B-1".into()),
            next: None,
            expected_revision: None,
        },
    )
    .unwrap();
    let (a, b) = (
        a["item"]["id"].as_str().unwrap().to_owned(),
        b["item"]["id"].as_str().unwrap().to_owned(),
    );
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "two"]);
    ask(
        &root,
        Request::ItemDelete {
            item: b.clone(),
            expected_revision: None,
        },
    )
    .unwrap();
    ask(
        &root,
        Request::ItemRenumber {
            item: a.clone(),
            number: Some("A-2".into()),
            next: None,
            expected_revision: None,
        },
    )
    .unwrap();
    let c = ask(
        &root,
        Request::ItemCreate {
            fields: Default::default(),
            number: Some("C-1".into()),
            next: None,
            expected_revision: None,
        },
    )
    .unwrap();
    let d = ask(
        &root,
        Request::Diff {
            from: None,
            to: None,
        },
    )
    .unwrap();
    assert_eq!(
        d["added"],
        json!([{"id": c["item"]["id"], "number": "C-1"}])
    );
    assert_eq!(d["removed"], json!([{"id": b, "number": "B-1"}]));
    assert_eq!(d["changed"][0]["id"], json!(a));
    assert_eq!(d["changed"][0]["removed"], json!(["number = A-1"]));
    assert_eq!(
        d["changed"][0]["added"],
        json!(["number = A-2", "previous_numbers = A-1"])
    );
}

#[test]
fn next_skips_a_number_a_record_holds_after_its_item_is_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    ryuplm::server::init(&root, None).unwrap();
    ask(
        &root,
        Request::SettingsEdit {
            change: serde_json::from_value(json!({
                "scheme": "mech", "pattern": "MECH-\\d{5}", "next_template": "MECH-{seq:5}"}))
            .unwrap(),
            expected_revision: None,
        },
    )
    .unwrap();
    let next = |root: &Path| {
        ask(
            root,
            Request::ItemCreate {
                fields: Default::default(),
                number: None,
                next: Some(String::new()),
                expected_revision: None,
            },
        )
        .unwrap()["item"]["number"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(next(&root), "MECH-00001");
    assert_eq!(next(&root), "MECH-00002");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "two items"]);
    // The highest is released, then deleted: its record still holds it.
    git(&root, &["checkout", "-q", "-b", "rel"]);
    ask(
        &root,
        Request::ReleasePropose {
            items: vec!["MECH-00002".into()],
            all: false,
            dry_run: false,
            note: None,
            expected_revision: None,
        },
    )
    .unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "release MECH-00002 A"]);
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["merge", "-q", "--no-ff", "rel", "-m", "merge"]);
    ask(
        &root,
        Request::ItemDelete {
            item: "MECH-00002".into(),
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(next(&root), "MECH-00003");
}

#[test]
fn a_first_release_continues_a_carried_legacy_label() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    ryuplm::server::init(&root, None).unwrap();
    for n in ["MECH-1", "MECH-2"] {
        ask(
            &root,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some(n.into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
    }
    std::fs::create_dir_all(root.join("imports")).unwrap();
    std::fs::write(
        root.join("imports/hist.json"),
        ryuplm::canonical::to_string(&json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "csv"}, "key": "PN", "fields": {"number": {"from": "PN"}},
            "numbers": "keep", "history": {"revision": "Rev"}})),
    )
    .unwrap();
    let csv = root.join("hist.csv");
    std::fs::write(&csv, "PN,Rev\nMECH-1,A\nMECH-1,B\nMECH-1,C\nMECH-2,X3\n").unwrap();
    let run = ask(
        &root,
        Request::ImportRun {
            name: "hist".into(),
            file: csv.display().to_string(),
            dry_run: false,
            prefer: None,
            limit: None,
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(run["adopted"]["count"], 2, "{run}");
    git(&root, &["add", "-A"]);
    git(
        &root,
        &["commit", "-q", "-m", "items and their legacy history"],
    );
    git(&root, &["checkout", "-q", "-b", "rel"]);
    let propose = |item: &str, dry_run: bool| {
        ask(
            &root,
            Request::ReleasePropose {
                items: vec![item.into()],
                all: false,
                dry_run,
                note: None,
                expected_revision: None,
            },
        )
        .unwrap()
    };
    assert_eq!(propose("MECH-1", true)["items"][0]["label"], "D");
    assert_eq!(propose("MECH-2", true)["items"][0]["label"], "A");
    let p = propose("MECH-1", false);
    assert_eq!(p["proposed"][0]["label"], "D");
    assert!(
        p["proposed"][0]["record"]
            .get("supersedes")
            .is_none_or(Value::is_null)
    );
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "MECH-1 at D"]);
    let report = match ask(&root, Request::Check { within: vec![] }) {
        Ok(v) => v,
        Err(e) => e.details["report"].clone(),
    };
    let codes: Vec<&str> = report["faults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(!codes.contains(&"release_sequence"), "{report}");
    assert_eq!(
        codes
            .iter()
            .filter(|c| **c == "item_history_label_foreign")
            .count(),
        1,
        "MECH-2's X3, not MECH-1's C: {report}"
    );
}
