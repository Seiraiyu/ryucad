//! `ryueda review` against a stand-in service: the branch's pull request
//! found, status and threads read, a comment pinned to a thread anchor,
//! sheets downloaded, Merge in asked, refusals as review codes.

#[path = "support/breakout.rs"]
mod breakout;
#[allow(dead_code)]
#[path = "support/draw.rs"]
mod draw;
#[path = "support/gitrepo.rs"]
mod gitrepo;
use gitrepo::*;

use ryu_review::testing::{Fake, json};
use serde_json::{Value, json as j};
use std::path::Path;

fn review(board: &Path, run: &Path, url: &str, args: &[&str]) -> (i32, Value) {
    let mut all = vec!["review"];
    all.extend_from_slice(args);
    let out = ryueda_cmd(board, run, false, &all)
        .env("RYUEDA_REVIEW_URL", url)
        .env("RYUEDA_REVIEW_TOKEN", "tok")
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

#[test]
fn review_against_a_fake_service() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    git(&top, &["checkout", "-q", "-b", "feature/r1"]);
    git(
        &top,
        &["remote", "add", "origin", "https://github.com/acme/boards.git"],
    );
    let fake = Fake::start(vec![
        json(
            "GET",
            "/api/v1/repos/acme/boards/pulls?branch=feature%2Fr1",
            200,
            j!({"pulls": [{"number": 4}]}),
        ),
        json(
            "GET",
            "/api/v1/repos/acme/boards/pulls/4",
            200,
            j!({
                "checks": {"ryueda / check": "success"},
                "renders": [
                    {"project": "board", "block": "breakout", "sheet": "s1", "side": "before",
                     "url": "/api/v1/artifacts/b"},
                    {"project": "board", "block": "breakout", "sheet": "s1", "side": "after",
                     "url": "/api/v1/artifacts/a"}]}),
        ),
        (
            "GET",
            "/api/v1/artifacts/b".to_owned(),
            200,
            "image/png",
            vec![1],
        ),
        (
            "GET",
            "/api/v1/artifacts/a".to_owned(),
            200,
            "image/png",
            vec![2],
        ),
        json(
            "GET",
            "/api/v1/repos/acme/boards/pulls/4/threads?state=open",
            200,
            j!({"threads": []}),
        ),
        json(
            "POST",
            "/api/v1/repos/acme/boards/pulls/4/threads",
            201,
            j!({"id": "t1", "ref": "board:breakout/r1"}),
        ),
        json(
            "POST",
            "/api/v1/repos/acme/boards/pulls/4/merge-in",
            409,
            j!({"error": {"code": "merge_unavailable", "message": "conflicts",
                          "details": {"reason": "conflicts"}}}),
        ),
    ]);
    let (code, v) = review(&board, &run, &fake.url, &["status"]);
    assert_eq!(
        (code, &v["result"]["pull"], &v["result"]["repo"]),
        (0, &j!(4), &j!("acme/boards")),
        "{v}"
    );
    let (code, v) = review(&board, &run, &fake.url, &["threads", "--open"]);
    assert_eq!(code, 0, "{v}");
    let (code, v) = review(
        &board,
        &run,
        &fake.url,
        &["comment", "board:breakout/r1", "why 10k?"],
    );
    assert_eq!((code, &v["result"]["id"]), (0, &j!("t1")), "{v}");
    let (code, v) = review(&board, &run, &fake.url, &["renders", "--out", "shots"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        std::fs::read(board.join("shots/board-breakout-s1-after.png")).unwrap(),
        vec![2]
    );
    let (code, v) = review(&board, &run, &fake.url, &["update"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &j!("review_merge_unavailable")),
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
        j!({"ref": "board:breakout/r1", "body": "why 10k?"})
    );
}
