//! `ryuplm review` against a stand-in service: the branch's pull request
//! found, status and threads read, a comment pinned to an item reference,
//! approvals, Merge in asked, refusals as review codes.

use assert_cmd::Command;
use ryu_review::testing::{Fake, json};
use serde_json::{Value, json as j};
use std::path::Path;

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
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

fn ryuplm(root: &Path, url: &str, args: &[&str]) -> (i32, Value) {
    let out = Command::cargo_bin("ryuplm")
        .unwrap()
        .current_dir(root)
        .env("RYUPLM_NO_DAEMON", "1")
        .env("RYUPLM_CONFIG_DIR", root.with_file_name("config"))
        .env("RYUPLM_REVIEW_URL", url)
        .env("RYUPLM_REVIEW_TOKEN", "tok")
        .env("RYUPLM_ID_SEED", "review")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

/// A catalogue with one item, committed on a branch of a clone whose
/// origin is a GitHub repository. Its folder and the item's ID.
fn catalogue(dir: &Path) -> (std::path::PathBuf, String) {
    let root = dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let (code, v) = ryuplm(&root, "http://127.0.0.1:9", &["init"]);
    assert_eq!(code, 0, "{v}");
    let (code, v) = ryuplm(&root, "http://127.0.0.1:9", &["item", "create", "KIT-1"]);
    assert_eq!(code, 0, "{v}");
    let id = v["result"]["item"]["id"].as_str().unwrap().to_owned();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "a kit"]);
    git(&root, &["checkout", "-q", "-b", "feature/kit"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/items.git",
        ],
    );
    (root, id)
}

#[test]
fn review_against_a_fake_service() {
    let dir = tempfile::tempdir().unwrap();
    let (root, id) = catalogue(dir.path());
    let reference = format!("item/{id}");
    let fake = Fake::start(vec![
        json(
            "GET",
            "/api/v1/repos/acme/items/pulls?branch=feature%2Fkit",
            200,
            j!({"pulls": [{"number": 4}]}),
        ),
        json(
            "GET",
            "/api/v1/repos/acme/items/pulls/4",
            200,
            j!({"checks": {"ryuplm / check": "success"}}),
        ),
        json(
            "GET",
            "/api/v1/repos/acme/items/pulls/4/threads?state=open",
            200,
            j!({"threads": []}),
        ),
        json(
            "POST",
            "/api/v1/repos/acme/items/pulls/4/threads",
            201,
            j!({"id": "t1", "ref": reference}),
        ),
        json("POST", "/api/v1/threads/t1/comments", 201, j!({"id": "c2"})),
        json(
            "POST",
            "/api/v1/threads/t1/resolve",
            200,
            j!({"id": "t1", "state": "resolved"}),
        ),
        json(
            "POST",
            "/api/v1/threads/t1/reopen",
            200,
            j!({"id": "t1", "state": "open"}),
        ),
        json(
            "GET",
            "/api/v1/repos/acme/items/pulls/4/approvals",
            200,
            j!({"records": []}),
        ),
        json(
            "POST",
            "/api/v1/repos/acme/items/pulls/4/merge-in",
            409,
            j!({"error": {"code": "merge_unavailable", "message": "conflicts",
                          "details": {"reason": "conflicts"}}}),
        ),
    ]);
    let review = |args: &[&str]| {
        let mut all = vec!["review"];
        all.extend_from_slice(args);
        ryuplm(&root, &fake.url, &all)
    };
    let (code, v) = review(&["status"]);
    assert_eq!(
        (code, &v["result"]["pull"], &v["result"]["repo"]),
        (0, &j!(4), &j!("acme/items")),
        "{v}"
    );
    assert_eq!(v["result"]["checks"]["ryuplm / check"], "success");
    let (code, v) = review(&["threads", "--open"]);
    assert_eq!(code, 0, "{v}");
    let (code, v) = review(&["comment", &reference, "why two?"]);
    assert_eq!((code, &v["result"]["id"]), (0, &j!("t1")), "{v}");
    for args in [
        vec!["reply", "t1", "because"],
        vec!["resolve", "t1"],
        vec!["reopen", "t1"],
        vec!["approvals"],
    ] {
        let (code, v) = review(&args);
        assert_eq!(code, 0, "{args:?}: {v}");
    }
    let (code, v) = review(&["update"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &j!("review_merge_unavailable")),
        "{v}"
    );
    // A reference that isn't one is refused here, before the service.
    let (code, v) = review(&["comment", "sheet:1", "x"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &j!("invalid_param")),
        "{v}"
    );
    let seen = fake.seen.lock().unwrap();
    assert!(seen.iter().all(|s| s.auth.as_deref() == Some("Bearer tok")));
    let posted = seen
        .iter()
        .find(|s| s.method == "POST" && s.path.ends_with("/threads"))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&posted.body).unwrap(),
        j!({"ref": reference, "body": "why two?"})
    );
    assert!(!seen.iter().any(|s| s.body.contains("sheet:1")));
}

#[test]
fn review_needs_a_github_clone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (code, v) = ryuplm(&root, "http://127.0.0.1:9", &["review", "status"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &j!("not_a_git_repo")),
        "{v}"
    );
}

#[test]
fn where_used_across_the_organization_asks_the_service() {
    let dir = tempfile::tempdir().unwrap();
    let (root, id) = catalogue(dir.path());
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("ryuplm.json")).unwrap()).unwrap();
    let project = manifest["project"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/where-used?project={project}&item={id}");
    let fake = Fake::start(vec![json(
        "GET",
        &path,
        200,
        j!({"schema": 1, "item": id, "working": [{"parent": "0000000000009", "repo": "acme/kits"}], "released": []}),
    )]);
    // By number: the item is found here, and asked for by its ID.
    let (code, v) = ryuplm(&root, &fake.url, &["where-used", "KIT-1", "--org"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"]["working"][0]["repo"], "acme/kits");
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen[0].path, path);
    assert_eq!(seen[0].auth.as_deref(), Some("Bearer tok"));
    drop(seen);
    // Another schema is refused, never half read.
    let two = Fake::start(vec![json("GET", &path, 200, j!({"schema": 2}))]);
    let (code, v) = ryuplm(&root, &two.url, &["where-used", &id, "--org"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &j!("review_failed")),
        "{v}"
    );
    // --org asks the service only: the local flags don't go with it.
    let (code, v) = ryuplm(
        &root,
        &fake.url,
        &["where-used", "KIT-1", "--org", "--working"],
    );
    assert_eq!((code, &v["error"]["code"]), (2, &j!("usage")), "{v}");
}
