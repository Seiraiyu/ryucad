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
