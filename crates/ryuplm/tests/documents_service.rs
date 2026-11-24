//! Service-stored documents through the binary (ryuplm 3.B): a catalogue
//! in a clone whose origin is acme/widgets, against
//! `ryu_review::testing::Store` reached through RYUPLM_REVIEW_URL, each
//! command in process and through the daemon. Each way has its own cache
//! (RYUPLM_CACHE_DIR), as two machines would.

use assert_cmd::Command;
use ryu_review::testing::Store;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Way {
    root: PathBuf,
    run: PathBuf,
    cache: PathBuf,
    daemon: bool,
}

fn ryuplm(w: &Way, url: &str) -> Command {
    let mut cmd = Command::cargo_bin("ryuplm").unwrap();
    cmd.current_dir(&w.root)
        .env("RYUPLM_RUNTIME_DIR", &w.run)
        .env("RYUPLM_IDLE_TIMEOUT", "60")
        .env("RYUPLM_CONFIG_DIR", w.run.with_file_name("config"))
        .env("RYUPLM_CACHE_DIR", &w.cache)
        .env("RYUPLM_REVIEW_URL", url)
        .env("RYUPLM_REVIEW_TOKEN", "t")
        .env("RYUPLM_ID_SEED", "docs")
        .env_remove("RYUPLM_PARTICIPANT");
    if w.daemon {
        cmd.env_remove("RYUPLM_NO_DAEMON");
    } else {
        cmd.env("RYUPLM_NO_DAEMON", "1");
    }
    cmd
}

fn run(w: &Way, url: &str, args: &[&str]) -> (i32, Value) {
    let out = ryuplm(w, url).args(args).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    (out.status.code().unwrap(), v)
}

fn way(top: &Path, name: &str, daemon: bool) -> Way {
    let root = top.join(name).join("cat");
    std::fs::create_dir_all(&root).unwrap();
    for args in [
        &["init", "-q"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
    }
    Way {
        root: root.canonicalize().unwrap(),
        run: top.join(name).join("run"),
        cache: top.join(name).join("cache"),
        daemon,
    }
}

#[test]
fn service_stored_documents_both_ways() {
    let top = tempfile::tempdir().unwrap();
    let (fake, store) = Store::start("acme/widgets");
    store.lock().unwrap().drive.insert(
        "f1".into(),
        vec![ryu_review::testing::Revision::file(
            "r1",
            "Plan.pdf",
            "application/pdf",
            b"plan one",
        )],
    );
    let mut answers = Vec::new();
    for (name, daemon) in [("direct", false), ("daemon", true)] {
        let w = way(top.path(), name, daemon);
        let url = fake.url.as_str();
        assert_eq!(run(&w, url, &["init"]).0, 0);
        std::fs::write(w.root.join("../spec.pdf"), b"%PDF service").unwrap();
        let (code, add) = run(
            &w,
            url,
            &[
                "document",
                "add",
                "../spec.pdf",
                "--number",
                "D-1",
                "--title",
                "Spec",
                "--storage",
                "service",
            ],
        );
        assert_eq!(code, 0, "{add}");
        assert_eq!(
            add["result"]["uploaded"].as_array().map(Vec::len),
            Some(if name == "direct" { 1 } else { 0 }),
            "the second way finds the bytes held: {add}"
        );
        // A fresh machine's cache: get fetches through the service.
        std::fs::remove_dir_all(&w.cache).unwrap();
        let (code, got) = run(&w, url, &["document", "get", "D-1", "--out", "../got.pdf"]);
        assert_eq!(code, 0, "{got}");
        assert_eq!(
            std::fs::read(w.root.join("../got.pdf")).unwrap(),
            b"%PDF service"
        );
        assert_eq!(
            run(&w, url, &["document", "move", "D-1", "--storage", "git"]).0,
            0
        );
        assert_eq!(run(&w, url, &["check"]).0, 0);
        assert_eq!(
            run(
                &w,
                url,
                &["document", "move", "D-1", "--storage", "service"]
            )
            .0,
            0
        );
        let (code, d) = run(
            &w,
            url,
            &["document", "add", "--drive", "f1", "--number", "D-2"],
        );
        assert_eq!(code, 0, "{d}");
        assert_eq!(d["result"]["document"]["pin"]["revision"], "r1");
        // Offline: the record is written, the upload is a warning.
        std::fs::write(w.root.join("../late.pdf"), b"late").unwrap();
        let (code, late) = run(
            &w,
            "http://127.0.0.1:9",
            &[
                "document",
                "add",
                "../late.pdf",
                "--number",
                "D-3",
                "--title",
                "Late",
                "--storage",
                "service",
            ],
        );
        assert_eq!(code, 0, "{late}");
        assert!(
            late["warnings"][0]
                .as_str()
                .unwrap()
                .starts_with("document_not_uploaded:"),
            "{late}"
        );
        let (code, up) = run(&w, url, &["document", "upload"]);
        assert_eq!(code, 0, "{up}");
        let (_, list) = run(&w, url, &["document", "list"]);
        answers.push(list["result"].clone());
        let _ = ryuplm(&w, url).args(["daemon", "stop"]).output();
    }
    assert_eq!(answers[0], answers[1], "in process and through the daemon");
    assert_eq!(
        answers[0]["documents"].as_array().map(Vec::len),
        Some(3),
        "{}",
        json!(answers)
    );
}
