//! The CLI against a stand-in for the service's ledger (ryuplm 2.B,
//! design 936): `ryu_review::testing::Ledger`, reached through
//! RYUPLM_REVIEW_URL, from a catalogue in a clone whose origin is
//! `acme/widgets` on GitHub. "Offline" is http://127.0.0.1:9, where
//! nothing listens.

use assert_cmd::Command;
use ryu_review::testing::Ledger;
use serde_json::{Value, json};
use std::path::PathBuf;

const OFFLINE: &str = "http://127.0.0.1:9";

struct Cat {
    _dir: tempfile::TempDir,
    root: PathBuf,
    run: PathBuf,
}

fn ryuplm(c: &Cat, url: &str) -> Command {
    let mut cmd = Command::cargo_bin("ryuplm").unwrap();
    cmd.current_dir(&c.root)
        .env("RYUPLM_RUNTIME_DIR", &c.run)
        .env("RYUPLM_IDLE_TIMEOUT", "60")
        .env("RYUPLM_CONFIG_DIR", c.run.with_file_name("config"))
        .env("RYUPLM_REVIEW_URL", url)
        .env("RYUPLM_REVIEW_TOKEN", "t")
        .env_remove("RYUPLM_NO_DAEMON")
        .env_remove("RYUPLM_PARTICIPANT");
    cmd
}

fn run(c: &Cat, url: &str, args: &[&str]) -> (i32, Value) {
    let out = ryuplm(c, url).args(args).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let v = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    (out.status.code().unwrap(), v)
}

fn ok(c: &Cat, url: &str, args: &[&str]) -> Value {
    let (code, v) = run(c, url, args);
    assert_eq!(code, 0, "ryuplm {args:?}: {v}");
    v
}

/// A catalogue with the `mech` scheme, its ledger on when `ledger`, in a
/// clone whose origin is `acme/widgets` on GitHub.
fn catalogue(ledger: bool) -> Cat {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("cat")).unwrap();
    let c = Cat {
        root: dir.path().join("cat").canonicalize().unwrap(),
        run: dir.path().join("run"),
        _dir: dir,
    };
    for args in [
        &["init", "-q"][..],
        &["remote", "add", "origin", "https://github.com/acme/widgets.git"],
    ] {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&c.root)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }
    ok(&c, OFFLINE, &["init"]);
    ok(
        &c,
        OFFLINE,
        &["settings", "edit", "--scheme", "mech", "--pattern", "MECH-\\d{5}", "--next-template", "MECH-{seq:5}"],
    );
    if ledger {
        ok(&c, OFFLINE, &["settings", "edit", "--ledger", "on"]);
    }
    c
}

#[test]
fn item_create_next_reserves_from_the_ledger_through_the_daemon_and_in_process() {
    let c = catalogue(true);
    let (fake, state) = Ledger::start("acme/widgets");
    state.lock().unwrap().sequences.insert("MECH-{seq:5}".into(), 42);
    let v = ok(&c, &fake.url, &["item", "create", "--next", "--name", "Bracket"]);
    assert_eq!(v["result"]["item"]["number"], "MECH-00042");
    assert_eq!(
        v["result"]["ledger"],
        json!({"reserved": true, "number": "MECH-00042", "expires_at": ryu_review::testing::EXPIRES})
    );
    let v = ok(&c, &fake.url, &["--no-daemon", "item", "create", "--next"]);
    assert_eq!(v["result"]["item"]["number"], "MECH-00043");
    assert_eq!(state.lock().unwrap().reserved.len(), 2);
    {
        let seen = fake.seen.lock().unwrap();
        assert!(seen.iter().all(|s| s.path == "/api/v1/repos/acme/widgets/numbers/next"));
        assert_eq!(seen[0].auth.as_deref(), Some("Bearer t"));
    }
    ok(&c, &fake.url, &["daemon", "stop"]);
}

#[test]
fn offline_the_local_number_with_a_warning() {
    let c = catalogue(true);
    let v = ok(&c, OFFLINE, &["item", "create", "--next"]);
    assert_eq!(v["result"]["item"]["number"], "MECH-00001");
    assert_eq!(
        (&v["result"]["ledger"]["reserved"], &v["result"]["ledger"]["why"]),
        (&json!(false), &json!("review_unreachable"))
    );
    assert!(
        v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().starts_with("ledger_unavailable: ")),
        "{v}"
    );
    ok(&c, OFFLINE, &["daemon", "stop"]);
}

#[test]
fn without_the_ledger_no_call_is_made() {
    let c = catalogue(false);
    let (fake, _) = Ledger::start("acme/widgets");
    let v = ok(&c, &fake.url, &["item", "create", "--next"]);
    assert_eq!(v["result"]["item"]["number"], "MECH-00001");
    assert!(v["result"].get("ledger").is_none());
    assert!(fake.seen.lock().unwrap().is_empty());
    ok(&c, &fake.url, &["daemon", "stop"]);
}

#[test]
fn number_commands() {
    let c = catalogue(true);
    let (fake, state) = Ledger::start("acme/widgets");
    let u = fake.url.as_str();
    let v = ok(&c, u, &["number", "next", "--count", "3"]);
    assert_eq!(v["result"]["numbers"], json!(["MECH-00001", "MECH-00002", "MECH-00003"]));
    assert_eq!(v["result"]["repo"], "acme/widgets");
    ok(&c, u, &["number", "reserve", "100-0042 rev", "100-0043", "--days", "7"]);
    assert!(fake.seen.lock().unwrap().last().unwrap().body.contains("\"days\":7"));
    let v = ok(&c, u, &["number", "who", "100-0042 rev"]);
    assert_eq!(v["result"]["holders"][0]["kind"], "reserved");
    let v = ok(&c, u, &["number", "list"]);
    assert_eq!(v["result"]["reservations"].as_array().unwrap().len(), 5);
    // Held for another catalogue's item: refused, naming the holder.
    state.lock().unwrap().bound.insert(
        "B-1".into(),
        ("fedcba9876543210fedcba9876543210".into(), "k3v9q2m8x4c1p".into()),
    );
    let (code, v) = run(&c, u, &["number", "reserve", "B-1"]);
    assert_eq!((code, v["error"]["code"].as_str()), (1, Some("part_number_taken")), "{v}");
    assert_eq!(v["error"]["details"]["service"]["taken"][0]["holders"][0]["item"], "k3v9q2m8x4c1p");
    // Offline: nothing local to fall back to.
    let (code, v) = run(&c, OFFLINE, &["number", "who", "B-1"]);
    assert_eq!((code, v["error"]["code"].as_str()), (4, Some("ledger_unavailable")), "{v}");
    let (code, _) = run(&c, u, &["number", "next", "--count", "0"]);
    assert_eq!(code, 2);
    ok(&c, u, &["daemon", "stop"]);
}

#[test]
fn number_commands_need_the_ledger_on() {
    let c = catalogue(false);
    let (fake, _) = Ledger::start("acme/widgets");
    let (code, v) = run(&c, &fake.url, &["number", "list"]);
    assert_eq!(
        (code, v["error"]["code"].as_str(), v["error"]["details"]["why"].as_str()),
        (4, Some("ledger_unavailable"), Some("ledger_off"))
    );
    assert!(fake.seen.lock().unwrap().is_empty());
    ok(&c, &fake.url, &["daemon", "stop"]);
}
