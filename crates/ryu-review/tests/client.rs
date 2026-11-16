//! The client against the stand-in service.

use ryu_review::testing::{Fake, json};
use ryu_review::{Client, Product};
use serde_json::json as j;

#[test]
fn a_product_names_itself_and_finds_its_service() {
    let fake = Fake::start(vec![json("GET", "/api/v1/x", 200, j!({"ok": 1}))]);
    // A variable only this test sets: no machine's token or preference.
    static P: Product = Product {
        name: "ryutest",
        version: "1.2.3",
        url_var: "RYU_REVIEW_TEST_URL",
        token_var: "RYU_REVIEW_TEST_TOKEN",
        url_pref: || None,
    };
    // SAFETY: this test binary sets only its own variables, before any
    // thread reads them.
    unsafe {
        std::env::set_var("RYU_REVIEW_TEST_URL", &fake.url);
        std::env::set_var("RYU_REVIEW_TEST_TOKEN", "tok");
    }
    assert_eq!(P.client().unwrap().get("/x").unwrap(), j!({"ok": 1}));
    assert_eq!(
        fake.seen.lock().unwrap()[0].auth.as_deref(),
        Some("Bearer tok")
    );
    let c = Client::new(&fake.url, "tok");
    assert_eq!(c.get("/x").unwrap(), j!({"ok": 1}));
}

struct Toy;
impl ryu_git::Layout for Toy {
    const NAME: &'static str = "toy";
    const MANIFEST: &'static str = "toy.json";
    const ROOT_FILES: &'static [&'static str] = &["toy.json"];
    const FOLDERS: &'static [&'static str] = &[];
    fn project_file(rel: &str) -> bool {
        rel == "toy.json"
    }
}

/// `git` in `dir`, reading neither the user's config nor the system's.
fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .current_dir(dir)
        .env(
            "GIT_CONFIG_GLOBAL",
            std::env::temp_dir().join("ryu-review-no-gitconfig"),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn shared_actions_ask_the_service_about_the_clones_pull_request() {
    let dir = tempfile::tempdir().unwrap();
    let top = dir.path();
    git(top, &["init", "-q", "-b", "feature/arm"]);
    std::fs::write(top.join("toy.json"), "{}\n").unwrap();
    git(top, &["add", "."]);
    git(top, &["commit", "-q", "-m", "toy"]);
    git(
        top,
        &["remote", "add", "origin", "git@github.com:acme/widget.git"],
    );
    let fake = Fake::start(vec![
        json(
            "GET",
            "/api/v1/repos/acme/widget/pulls?branch=feature%2Farm",
            200,
            j!({"pulls": [{"number": 7}]}),
        ),
        json(
            "GET",
            "/api/v1/repos/acme/widget/pulls/7",
            200,
            j!({"renders": [
                {"project": "", "part": "p", "side": "before", "url": "/api/v1/artifacts/k1"},
                {"project": "eda/board", "block": "breakout", "sheet": "s1", "side": "after",
                 "format": "svg", "url": "/api/v1/artifacts/k2"}]}),
        ),
        (
            "GET",
            "/api/v1/artifacts/k1".to_owned(),
            200,
            "image/png",
            vec![1],
        ),
        (
            "GET",
            "/api/v1/artifacts/k2".to_owned(),
            200,
            "image/svg+xml",
            b"<svg/>".to_vec(),
        ),
        json(
            "POST",
            "/api/v1/repos/acme/widget/pulls/7/threads",
            201,
            j!({"id": "t1"}),
        ),
    ]);
    static P: Product = Product {
        name: "ryutest",
        version: "0",
        url_var: "RYU_REVIEW_TEST2_URL",
        token_var: "RYU_REVIEW_TEST2_TOKEN",
        url_pref: || None,
    };
    // SAFETY: only this binary's own variables, each test its own.
    unsafe {
        std::env::set_var("RYU_REVIEW_TEST2_URL", &fake.url);
        std::env::set_var("RYU_REVIEW_TEST2_TOKEN", "tok");
    }
    use ryu_review::Action;
    let v = ryu_review::run::<Toy>(top, &P, Action::Status { pr: None }).unwrap();
    assert_eq!(
        (&v["repo"], &v["pull"]),
        (&j!("acme/widget"), &j!(7)),
        "{v}"
    );
    let out = top.join("renders");
    let v = ryu_review::run::<Toy>(
        top,
        &P,
        Action::Renders {
            pr: Some(7),
            out: out.clone(),
        },
    )
    .unwrap();
    assert_eq!(v["files"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(
        std::fs::read(out.join("root-p-before.png")).unwrap(),
        vec![1]
    );
    assert_eq!(
        std::fs::read(out.join("eda_board-breakout-s1-after.svg")).unwrap(),
        b"<svg/>"
    );
    let v = ryu_review::run::<Toy>(
        top,
        &P,
        Action::Comment {
            reference: "board:breakout/r1".into(),
            text: "why 10k?".into(),
            pr: Some(7),
            commit: None,
        },
    )
    .unwrap();
    assert_eq!(v["id"], "t1");
    let seen = fake.seen.lock().unwrap();
    let posted: serde_json::Value = serde_json::from_str(&seen.last().unwrap().body).unwrap();
    assert_eq!(posted, j!({"ref": "board:breakout/r1", "body": "why 10k?"}));
}
