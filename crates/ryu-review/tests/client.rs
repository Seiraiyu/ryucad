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

#[test]
fn a_handler_answers_each_request_from_what_it_asked() {
    let fake = ryu_review::testing::Fake::serve(|s| {
        let body = serde_json::json!({"path": s.path, "method": s.method}).to_string();
        (200, "application/json", body.into_bytes())
    });
    let c = ryu_review::Client::new(&fake.url, "t");
    let v = c.get("/x?y=1").unwrap();
    assert_eq!(
        (v["path"].as_str(), v["method"].as_str()),
        (Some("/api/v1/x?y=1"), Some("GET"))
    );
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
}

#[test]
fn the_fake_ledger_hands_out_reserves_and_answers() {
    use ryu_doc::error::Code;
    use ryu_review::testing::Ledger;
    use serde_json::json;
    let (fake, ledger) = Ledger::start("acme/widgets");
    let c = ryu_review::Client::new(&fake.url, "t");
    let cat = "0123456789abcdef0123456789abcdef";
    let other = "fedcba9876543210fedcba9876543210";
    let base = "/repos/acme/widgets/numbers";
    // Above the floor, then on from where the sequence is.
    let v = c
        .post(
            &format!("{base}/next"),
            &json!({"catalogue": cat, "template": "M-{seq:3}", "count": 2, "floor": 7}),
        )
        .unwrap();
    assert_eq!(
        (v["schema"].clone(), v["numbers"].clone()),
        (json!(1), json!(["M-008", "M-009"]))
    );
    let v = c
        .post(
            &format!("{base}/next"),
            &json!({"catalogue": other, "template": "M-{seq:3}"}),
        )
        .unwrap();
    assert_eq!(v["numbers"], json!(["M-010"]));
    // All or none: one number held for another catalogue refuses the call.
    let e = c
        .post(
            &format!("{base}/reserve"),
            &json!({"catalogue": other, "numbers": ["M-008", "X 1"]}),
        )
        .unwrap_err();
    assert_eq!(e.code, Code::PartNumberTaken);
    assert_eq!(e.details["service"]["taken"][0]["number"], "M-008");
    assert_eq!(
        e.details["service"]["taken"][0]["holders"][0]["catalogue"],
        cat
    );
    assert!(!ledger.lock().unwrap().reserved.contains_key("X 1"));
    c.post(
        &format!("{base}/reserve"),
        &json!({"catalogue": other, "numbers": ["X 1"]}),
    )
    .unwrap();
    let v = c
        .get(&format!("{base}/{}", ryu_review::escape("X 1")))
        .unwrap();
    assert_eq!(
        (v["number"].as_str(), v["holders"][0]["kind"].as_str()),
        (Some("X 1"), Some("reserved"))
    );
    let v = c.get(&format!("{base}?catalogue={other}")).unwrap();
    let listed: Vec<&str> = v["reservations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["number"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ["M-010", "X 1"]);
    assert_eq!(v["after"], serde_json::Value::Null);
    // A bound number is held for its item; a refusal answers every call.
    ledger
        .lock()
        .unwrap()
        .bound
        .insert("B-1".into(), (other.into(), "k3v9q2m8x4c1p".into()));
    let v = c.get(&format!("{base}/B-1")).unwrap();
    assert_eq!(
        (
            v["holders"][0]["kind"].as_str(),
            v["holders"][0]["item"].as_str()
        ),
        (Some("bound"), Some("k3v9q2m8x4c1p"))
    );
    ledger.lock().unwrap().refuse = Some((403, "not_in_beta".into()));
    assert_eq!(
        c.get(&format!("{base}/B-1")).unwrap_err().code,
        Code::ReviewNotInBeta
    );
    assert_eq!(ledger.lock().unwrap().next_calls, 2);
}
