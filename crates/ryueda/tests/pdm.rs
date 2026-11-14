//! PDM through Git (design 629): releases proposed, made official,
//! tagged and checked; merges that break PDM; the commit guard. In
//! process and through the daemon alike.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;
#[path = "support/gitrepo.rs"]
mod gitrepo;
use gitrepo::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// `ryueda args` in `dir`, which must succeed; its `result`.
fn ok(dir: &Path, run: &Path, daemon: bool, args: &[&str]) -> Value {
    let (code, v) = ryueda(dir, run, daemon, args);
    assert_eq!(code, 0, "{args:?}: {v}");
    v["result"].clone()
}

/// A library project `lib` with the breakout's library (its footprints,
/// symbols and components) in a fresh repository, every component
/// numbered and ryupdm set up, committed on `main`. Its parts' IDs, as
/// `release propose --all --dry-run` lists them.
fn library(top: &Path, run: &Path, daemon: bool) -> (PathBuf, Vec<String>) {
    git(top, &["init", "-q", "-b", "main"]);
    let lib = top.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    ok(&lib, run, daemon, &["init", "--library"]);
    for (path, text) in breakout::footprints() {
        let at = lib.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::library() {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ok(&lib, run, daemon, &args);
    }
    ok(&lib, run, daemon, &["pdm", "init"]);
    let dry = ok(
        &lib,
        run,
        daemon,
        &["release", "propose", "--all", "--dry-run"],
    );
    let parts: Vec<String> = dry["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["part"].as_str().unwrap().to_owned())
        .collect();
    for (i, p) in parts.iter().enumerate() {
        ok(
            &lib,
            run,
            daemon,
            &["props", "set", p, &format!("number=100-{:04}", i + 1)],
        );
    }
    git(top, &["add", "-A"]);
    git(top, &["commit", "-q", "-m", "start"]);
    (lib, parts)
}

/// The kinds `merge status` lists.
fn kinds(v: &Value) -> Vec<String> {
    v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["kind"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn a_merge_of_two_proposals_of_one_part_is_one_conflict_whole() {
    for daemon in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let (top, run) = (d.path().join("repo"), d.path().join("run"));
        std::fs::create_dir_all(&top).unwrap();
        let (lib, parts) = library(&top, &run, daemon);
        assert!(parts.contains(&"r_4k7".to_owned()), "{parts:?}");
        git(&top, &["checkout", "-q", "-b", "other"]);
        ok(
            &lib,
            &run,
            daemon,
            &["props", "set", "r_4k7", "description=one"],
        );
        ok(&lib, &run, daemon, &["release", "propose", "r_4k7"]);
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "A on other"]);
        git(&top, &["checkout", "-q", "main"]);
        ok(
            &lib,
            &run,
            daemon,
            &["props", "set", "r_4k7", "description=two"],
        );
        ok(&lib, &run, daemon, &["release", "propose", "r_4k7"]);
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "A on main"]);
        let _ = git_try(&top, &[], &["merge", "-q", "--no-edit", "other"]);
        let v = ok(&lib, &run, daemon, &["merge", "status"]);
        let k = kinds(&v);
        assert!(k.iter().any(|k| k == "value"), "{v}");
    }
}

#[test]
fn a_clean_merge_that_breaks_pdm_is_pdm_broke() {
    for daemon in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let (top, run) = (d.path().join("repo"), d.path().join("run"));
        std::fs::create_dir_all(&top).unwrap();
        let (lib, _) = library(&top, &run, daemon);
        // Each side alone is fine; merged, two parts share a number.
        git(&top, &["checkout", "-q", "-b", "other"]);
        ok(
            &lib,
            &run,
            daemon,
            &["props", "set", "r_4k7", "number=100-0099"],
        );
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "other"]);
        git(&top, &["checkout", "-q", "main"]);
        ok(
            &lib,
            &run,
            daemon,
            &["props", "set", "c_100n", "number=100-0099"],
        );
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "main"]);
        let (merged, why) = git_try(&top, &[], &["merge", "-q", "--no-commit", "other"]);
        assert!(merged, "{why}");
        let v = ok(&lib, &run, daemon, &["merge", "status"]);
        let broke: Vec<&Value> = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "pdm_broke")
            .collect();
        assert!(!broke.is_empty(), "{v}");
        assert!(
            broke
                .iter()
                .all(|c| c["error"]["code"] == "part_number_duplicate"),
            "{v}"
        );
        git(&top, &["merge", "--abort"]);
    }
}

#[test]
fn a_stale_proposal_a_merge_completes_is_withdrawn() {
    for daemon in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let (top, run) = (d.path().join("repo"), d.path().join("run"));
        std::fs::create_dir_all(&top).unwrap();
        let (lib, _) = library(&top, &run, daemon);
        // Ours proposes r_4k7; theirs changes it: merged, the proposal no
        // longer describes the part. On a work branch: main is the release
        // branch, where a record is official at once.
        git(&top, &["checkout", "-q", "-b", "work"]);
        git(&top, &["checkout", "-q", "-b", "other"]);
        ok(
            &lib,
            &run,
            daemon,
            &["libcomp", "edit", "r_4k7", "--value", "4k99"],
        );
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "other"]);
        git(&top, &["checkout", "-q", "work"]);
        ok(&lib, &run, daemon, &["release", "propose", "r_4k7"]);
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "proposed"]);
        let (merged, why) = git_try(&top, &[], &["merge", "-q", "--no-commit", "other"]);
        assert!(merged, "{why}");
        let v = ok(&lib, &run, daemon, &["merge", "status"]);
        let Some(c) = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["kind"] == "pdm_broke" && c["repairs"][0]["id"] == "withdraw")
            .cloned()
        else {
            panic!("no withdraw: {v}");
        };
        ok(
            &lib,
            &run,
            daemon,
            &[
                "merge",
                "resolve",
                c["id"].as_str().unwrap(),
                "--repair",
                "withdraw",
            ],
        );
        assert!(!lib.join("releases/r_4k7/A.json").exists());
        let v = ok(&lib, &run, daemon, &["merge", "status"]);
        assert!(!kinds(&v).iter().any(|k| k == "pdm_broke"), "{v}");
        git(&top, &["merge", "--abort"]);
    }
}

#[test]
fn the_hook_refuses_a_commit_that_makes_a_proposal_stale() {
    let d = tempfile::tempdir().unwrap();
    let (top, run) = (d.path().join("repo"), d.path().join("run"));
    std::fs::create_dir_all(&top).unwrap();
    let (lib, _) = library(&top, &run, false);
    ok(&lib, &run, false, &["git", "install"]);
    // On a work branch: main is the release branch.
    git(&top, &["checkout", "-q", "-b", "work"]);
    ok(&lib, &run, false, &["release", "propose", "r_4k7"]);
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "propose A"]);
    // The component changes; its proposal no longer describes it.
    ok(
        &lib,
        &run,
        false,
        &["libcomp", "edit", "r_4k7", "--value", "4k99"],
    );
    git(&top, &["add", "-A"]);
    let (passed, stderr) = git_try(&top, &[], &["commit", "-q", "-m", "edit"]);
    assert!(!passed, "the hook refuses it");
    assert!(stderr.contains("release_stale"), "{stderr}");
    // A commit that changes nothing a record describes goes through.
    git(&top, &["reset", "-q", "--hard"]);
    std::fs::write(top.join("README"), "notes\n").unwrap();
    git(&top, &["add", "-A"]);
    let (passed, stderr) = git_try(&top, &[], &["commit", "-q", "-m", "notes"]);
    assert!(passed, "{stderr}");
}

#[test]
fn watchers_hear_of_release_states() {
    let d = tempfile::tempdir().unwrap();
    let (top, run) = (d.path().join("repo"), d.path().join("run"));
    std::fs::create_dir_all(&top).unwrap();
    let (lib, _) = library(&top, &run, true);
    // On a work branch: main is the release branch.
    git(&top, &["checkout", "-q", "-b", "work"]);
    let watch = ryueda_cmd(
        &lib,
        &run,
        true,
        &[
            "--as",
            "watcher",
            "watch",
            "--topic",
            "pdm",
            "--timeout",
            "20",
        ],
    );
    let waiting = std::thread::spawn(move || {
        let mut watch = watch;
        watch.output().unwrap()
    });
    let attached = || {
        let v = ok(&lib, &run, true, &["participants"]);
        v["participants"]
            .as_array()
            .is_some_and(|l| l.iter().any(|x| x["name"] == "watcher"))
    };
    for _ in 0..200 {
        if attached() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // Nothing says when the watcher has subscribed: propose and undo
    // until it has heard one change.
    let mut proposed = false;
    while !waiting.is_finished() {
        let args: &[&str] = match proposed {
            false => &["release", "propose", "r_4k7"],
            true => &["undo"],
        };
        ok(&lib, &run, true, args);
        proposed = !proposed;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = waiting.join().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let note = &v["result"]["notifications"][0];
    assert_eq!(note["method"], "pdm.changed", "{v}");
    let part = &note["params"]["parts"][0];
    assert_eq!(part["part"], "r_4k7", "{v}");
    assert_eq!(part["state"]["state"], "in_work", "{v}");
    ok(&lib, &run, true, &["daemon", "stop"]);
}
