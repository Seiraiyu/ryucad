//! Where-used across a second catalogue's checkout and its official
//! release, through the library (Git on PATH, as ryupdm's tests use it).

use ryuplm::proto::Request;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn ask(root: &Path, req: Request) -> Value {
    ryuplm::server::handle_safely(&mut None, None, None, root, req)
        .unwrap_or_else(|e| panic!("{e:?}"))
        .result
}

fn create(root: &Path, n: &str) -> String {
    let v = ask(root, Request::ItemCreate { fields: Default::default(), number: Some(n.into()), next: None, expected_revision: None });
    v["item"]["id"].as_str().unwrap().to_owned()
}

#[test]
fn where_used_reads_other_checkouts_and_their_releases() {
    // SAFETY: this test binary sets the seed once, before any ID.
    unsafe { std::env::set_var("RYUPLM_ID_SEED", "where") };
    let d = tempfile::tempdir().unwrap();
    let top = d.path().canonicalize().unwrap();
    let (parts, kits) = (top.join("parts"), top.join("kits"));
    for c in [&parts, &kits] {
        std::fs::create_dir_all(c).unwrap();
        ryuplm::server::init(c, None).unwrap();
    }
    git(&top, &["init", "-q", "-b", "main"]);
    let screw = create(&parts, "SCREW-1");
    let parts_id = ask(&parts, Request::ProjectInfo {})["project"].as_str().unwrap().to_owned();
    let kit = create(&kits, "KIT-1");
    ask(&kits, Request::BomAdd {
        item: "KIT-1".into(),
        fields: serde_json::from_value(json!({"child": format!("{parts_id}:{screw}"), "quantity": "8"})).unwrap(),
        expected_revision: None,
    });
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "catalogues"]);
    // KIT-1 released on a branch and merged: official, as tests/release.rs does it.
    git(&top, &["checkout", "-q", "-b", "rel"]);
    ask(&kits, Request::ReleasePropose { items: vec![kit.clone()], all: false, dry_run: false, note: None, expected_revision: None });
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "release KIT-1 A"]);
    git(&top, &["checkout", "-q", "main"]);
    git(&top, &["merge", "-q", "--no-ff", "rel", "-m", "merge"]);
    let v = ask(&parts, Request::BomWhereUsed { item: "SCREW-1".into(), within: vec![], no_dependents: true, working: false });
    assert_eq!(v["schema"], 1);
    let working = v["working"].as_array().unwrap();
    assert_eq!(working.len(), 1, "{v}");
    assert_eq!(working[0]["number"], "KIT-1");
    assert_eq!(working[0]["catalogue"], ask(&kits, Request::ProjectInfo {})["project"]);
    assert_eq!(working[0]["quantity"], "8");
    let released = v["released"].as_array().unwrap();
    assert_eq!(released.len(), 1, "{v}");
    assert_eq!(released[0]["release"], "A");
    assert_eq!(released[0]["state"], "released");
    let only = ask(&parts, Request::BomWhereUsed { item: "SCREW-1".into(), within: vec![], no_dependents: true, working: true });
    assert_eq!(only["released"], json!([]));
}
