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
use serde_json::{Value, json};
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

/// What `git args` printed in `dir`, trimmed.
fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = git_command(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// The service's signed tag on the record at `rel` (relative to the
/// repository), official at `commit`, as ryucad's tests make it, the
/// project at `prefix`.
fn attest(top: &Path, prefix: &str, rel: &str, commit: &str) {
    use ryupdm::attest::*;
    use ryupdm::record::{Kind, Record};
    let bytes = std::fs::read(top.join(rel)).unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let record = Record::from_json_in(&v, Kind::Release, "ryueda").unwrap();
    let rules = vec![ryupdm::approvals::Rule {
        parts: vec!["*".into()],
        count: 1,
        from: vec!["alice".into(), "bob".into()],
    }];
    let a = Attestation {
        approvals: vec![Approval {
            at: "2026-11-02T09:14:03Z".into(),
            commit: commit.into(),
            github: User {
                id: 1,
                login: "alice".into(),
            },
            review: 1,
            via: vec![],
        }],
        commit: commit.into(),
        label: record.label().into(),
        missing: vec![],
        outcome: Outcome::Approved,
        part: record.part().clone(),
        project: record.project().clone(),
        proposed_by: User {
            id: 2,
            login: "dana".into(),
        },
        pull: Some(Pull {
            number: 1,
            repo: "acme/widgets".into(),
        }),
        record: blake3::hash(&bytes).to_hex().to_string(),
        record_kind: Kind::Release,
        rules,
        rules_at: git_out(top, &["rev-parse", &format!("{commit}^")]),
        signed_at: "2026-11-02T09:20:11Z".into(),
    };
    let name = ryupdm::history::tag_name_of(prefix, record.part(), record.label(), Kind::Release);
    let file = top.join(".git/ATTEST_MSG");
    std::fs::write(&file, sign(&a, &TEST_SEED)).unwrap();
    git(
        top,
        &[
            "tag",
            "-a",
            "--cleanup=verbatim",
            "-F",
            file.to_str().unwrap(),
            &name,
            commit,
        ],
    );
}

#[test]
fn components_release_with_all_become_official_tag_and_go_modified() {
    for daemon in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let (top, run) = (d.path().join("repo"), d.path().join("run"));
        std::fs::create_dir_all(&top).unwrap();
        let (lib, parts) = library(&top, &run, daemon);
        let v = ok(
            &lib,
            &run,
            daemon,
            &["release", "propose", "--all", "--note", "first"],
        );
        assert_eq!(v["proposed"].as_array().unwrap().len(), parts.len(), "{v}");
        assert_eq!(v["refused"], json!([]), "{v}");
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "release A"]);
        let v = ok(&lib, &run, daemon, &["release", "list"]);
        assert!(
            v["records"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["status"] == "official"),
            "{v}"
        );
        let v = ok(&lib, &run, daemon, &["release", "sync"]);
        assert_eq!(v["created"].as_array().unwrap().len(), parts.len(), "{v}");
        let tags = git_out(&top, &["tag", "-l"]);
        assert!(tags.contains("ryupdm/lib/r_4k7/A"), "{tags}");
        // The resistor's symbol changes: the resistor is modified, the rest
        // stay released, and --all proposes B of the resistor alone.
        let sym = lib.join("library/symbols/r.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&sym).unwrap()).unwrap();
        v["name"] = json!("R_new");
        std::fs::write(&sym, ryueda::canonical::to_string(&v)).unwrap();
        let v = ok(&lib, &run, daemon, &["release", "list", "r_4k7"]);
        assert_eq!(v["parts"][0]["state"], "modified", "{v}");
        let v = ok(&lib, &run, daemon, &["release", "propose", "--all"]);
        let labels: Vec<(&str, &str)> = v["proposed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["part"].as_str().unwrap(), p["label"].as_str().unwrap()))
            .collect();
        assert_eq!(labels, [("r_4k7", "B")], "{v}");
        let v = ok(&lib, &run, daemon, &["release", "check"]);
        assert_eq!(v["passed"], true, "{v}");
        if daemon {
            ok(&lib, &run, true, &["daemon", "stop"]);
        }
    }
}

#[test]
fn approvals_are_verified_offline_from_the_services_tags() {
    use ryupdm::attest::{TEST_SEED, hex, public_key, test_key_id};
    let d = tempfile::tempdir().unwrap();
    let (top, run) = (d.path().join("repo"), d.path().join("run"));
    std::fs::create_dir_all(&top).unwrap();
    let (lib, _) = library(&top, &run, false);
    let approvals = json!({"keys": {test_key_id(): hex(&public_key(&TEST_SEED))},
                           "rules": [{"count": 1, "from": ["alice", "bob"], "parts": ["*"]}]});
    ok(
        &lib,
        &run,
        false,
        &["pdm", "configure", "--approvals", &approvals.to_string()],
    );
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "rules"]);
    let v = ok(&lib, &run, false, &["release", "propose", "r_4k7"]);
    assert_eq!(v["proposed"][0]["approval"]["required"], true, "{v}");
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "A"]);
    let official = git_out(&top, &["rev-parse", "HEAD"]);
    let v = ok(&lib, &run, false, &["release", "sync"]);
    assert_eq!(v["left_to_service"].as_array().unwrap().len(), 1, "{v}");
    attest(&top, "lib/", "lib/releases/r_4k7/A.json", &official);
    let v = ok(&lib, &run, false, &["release", "list"]);
    let a = &v["records"][0]["approval"];
    assert_eq!(
        (a["status"].as_str(), a["approvers"][0]["login"].as_str()),
        (Some("approved"), Some("alice")),
        "{v}"
    );
    assert_eq!(a["key"], test_key_id());
}

#[test]
fn a_design_releases_with_its_results() {
    for daemon in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let (top, run) = (d.path().join("repo"), d.path().join("run"));
        std::fs::create_dir_all(&top).unwrap();
        let board = breakout_in_git(&top, &run);
        git(&top, &["branch", "-M", "main"]);
        ok(&board, &run, daemon, &["pdm", "init"]);
        ok(
            &board,
            &run,
            daemon,
            &["props", "set", "board", "number=200-0001"],
        );
        let dry = ok(
            &board,
            &run,
            daemon,
            &["release", "propose", "board", "--dry-run"],
        );
        let problems = dry["parts"][0]["problems"].as_array().unwrap().clone();
        // Every component without a maker part number gets one.
        let list = ok(&board, &run, daemon, &["libcomp", "list"]);
        for c in list["libcomps"].as_array().unwrap() {
            if c["mpn"].as_str().unwrap_or_default().is_empty() {
                let id = c["id"].as_str().unwrap();
                ok(
                    &board,
                    &run,
                    daemon,
                    &[
                        "libcomp",
                        "edit",
                        id,
                        "--manufacturer",
                        "Yageo",
                        "--mpn",
                        &format!("TEST-{id}"),
                    ],
                );
            }
        }
        let dry = ok(
            &board,
            &run,
            daemon,
            &["release", "propose", "board", "--dry-run"],
        );
        assert_eq!(
            dry["parts"][0]["ready"], true,
            "before: {problems:?}, now: {dry}"
        );
        let v = ok(&board, &run, daemon, &["release", "propose", "board"]);
        let rec = &v["proposed"][0]["record"];
        assert!(
            rec["results"]["netlist"].is_string() && rec["results"]["erc"].is_string(),
            "{v}"
        );
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "board A"]);
        let v = ok(&board, &run, daemon, &["release", "check", "--results"]);
        assert_eq!(v["passed"], true, "{v}");
        if daemon {
            ok(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

/// The breakout (`board`) beside `lib` in one repository, naming `lib` as
/// `passives` and placing `comp` from it instead of from its own library.
fn board_placing(top: &Path, run: &Path, daemon: bool, comp: &str) -> PathBuf {
    let board = breakout_in_git(top, run);
    ok(&board, run, daemon, &["lib", "add", "passives", "../lib"]);
    place_from_passives(&board, comp);
    git(top, &["add", "-A"]);
    git(top, &["commit", "-q", "-m", "board places passives"]);
    board
}

/// Every component of `board` placing its own `comp` places `passives`'s.
fn place_from_passives(board: &Path, comp: &str) {
    let mut moved = false;
    for b in std::fs::read_dir(board.join("blocks")).unwrap() {
        let dir = b.unwrap().path().join("components");
        for f in std::fs::read_dir(&dir).into_iter().flatten() {
            let f = f.unwrap().path();
            let mut v: Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
            if v["of"]["id"] == comp && v["of"].get("library").is_none() {
                v["of"]["library"] = json!("passives");
                std::fs::write(
                    &f,
                    format!("{}\n", serde_json::to_string_pretty(&v).unwrap()),
                )
                .unwrap();
                moved = true;
            }
        }
    }
    assert!(moved, "the breakout places {comp}");
}

/// Edits the description of the first footprint `comp` lists in `lib`,
/// so the component releases again.
fn edit_footprint_of(lib: &Path, comp: &str, text: &str) {
    let c: Value = serde_json::from_slice(
        &std::fs::read(lib.join(format!("library/components/{comp}.json"))).unwrap(),
    )
    .unwrap();
    let fp = c["footprints"][0]["footprint"]["id"].as_str().unwrap();
    let at = lib.join(format!("library/footprints/{fp}.json"));
    let mut v: Value = serde_json::from_slice(&std::fs::read(&at).unwrap()).unwrap();
    v["description"] = json!(text);
    std::fs::write(
        &at,
        format!("{}\n", serde_json::to_string_pretty(&v).unwrap()),
    )
    .unwrap();
}

#[test]
fn a_design_places_a_sibling_librarys_components_by_label() {
    for daemon in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let (top, run) = (d.path().join("repo"), d.path().join("run"));
        std::fs::create_dir_all(&top).unwrap();
        let (lib, parts) = library(&top, &run, daemon);
        let comp = parts.iter().find(|p| p.starts_with("r_")).unwrap().clone();
        ok(&lib, &run, daemon, &["release", "propose", "--all"]);
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "library A"]);
        let board = board_placing(&top, &run, daemon, &comp);
        let dry = ok(
            &board,
            &run,
            daemon,
            &["release", "propose", "board", "--dry-run"],
        );
        assert!(dry.to_string().contains("component_not_released"), "{dry}");
        let target = format!("passives/{comp}");
        let v = ok(&board, &run, daemon, &["lib", "pin", &target, "--latest"]);
        assert_eq!(v["pinned"][0]["to"], "A", "{v}");
        let dry = ok(
            &board,
            &run,
            daemon,
            &["release", "propose", "board", "--dry-run"],
        );
        assert!(!dry.to_string().contains("component_not_released"), "{dry}");
        // The library moves on: the footprint edited, its parts released at B.
        edit_footprint_of(&lib, &comp, "edited");
        ok(&lib, &run, daemon, &["release", "propose", "--all"]);
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", "library B"]);
        let parts_v = ok(&board, &run, daemon, &["lib", "parts", "passives"]);
        assert!(parts_v.to_string().contains("\"superseded\""), "{parts_v}");
        let o = ok(&board, &run, daemon, &["lib", "outdated"]);
        assert_eq!(o["outdated"][0]["latest"], "B", "{o}");
        let (_, check) = ryueda(&board, &run, daemon, &["check"]);
        assert!(check.to_string().contains("component_outdated"), "{check}");
        let v = ok(&board, &run, daemon, &["lib", "pin", &target, "--latest"]);
        assert_eq!(
            (
                v["pinned"][0]["from"].as_str(),
                v["pinned"][0]["to"].as_str()
            ),
            (Some("A"), Some("B")),
            "{v}"
        );
        // A label that isn't official: refused, nothing written.
        let (code, v) = ryueda(
            &board,
            &run,
            daemon,
            &["lib", "pin", &target, "--label", "Z"],
        );
        assert_ne!(code, 0);
        assert_eq!(v["error"]["code"], "label_not_found", "{v}");
        // Back to working: allowed for a library in this repository.
        ok(&board, &run, daemon, &["lib", "pin", &target, "--working"]);
        if daemon {
            ok(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

#[test]
fn a_library_in_another_repository_is_fetched_pinned_updated_and_refused_when_rewritten() {
    for daemon in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let run = d.path().join("run");
        let libs = d.path().join("libs");
        std::fs::create_dir_all(&libs).unwrap();
        let (lib, parts) = library(&libs, &run, daemon);
        let comp = parts.iter().find(|p| p.starts_with("r_")).unwrap().clone();
        ok(&lib, &run, daemon, &["release", "propose", "--all"]);
        git(&libs, &["add", "-A"]);
        git(&libs, &["commit", "-q", "-m", "library A"]);
        let top = d.path().join("boards");
        std::fs::create_dir_all(&top).unwrap();
        let board = breakout_in_git(&top, &run);
        let url = format!("file://{}", libs.display());
        let v = ok(
            &board,
            &run,
            daemon,
            &["lib", "add", "passives", "--url", &url, "--path", "lib"],
        );
        assert!(v["library"]["project"].is_string(), "{v}");
        // At working: library_foreign_working; pinned: locked.
        let target = format!("passives/{comp}");
        let (code, v) = ryueda(&board, &run, daemon, &["lib", "pin", &target, "--working"]);
        assert_ne!(code, 0);
        assert_eq!(v["error"]["code"], "library_foreign_working", "{v}");
        ok(&board, &run, daemon, &["lib", "pin", &target, "--latest"]);
        assert!(board.join("ryueda.lock").exists());
        place_from_passives(&board, &comp);
        let (_, check) = ryueda(&board, &run, daemon, &["check"]);
        let report = match check.get("result") {
            Some(r) => r.clone(),
            None => check["error"]["details"]["report"].clone(),
        };
        assert!(
            report.is_object() && report.get("deps").is_none(),
            "{check}"
        );
        // The library releases B; fetched, outdated, updated.
        edit_footprint_of(&lib, &comp, "edited");
        ok(&lib, &run, daemon, &["release", "propose", "--all"]);
        git(&libs, &["add", "-A"]);
        git(&libs, &["commit", "-q", "-m", "library B"]);
        let f = ok(&board, &run, daemon, &["lib", "fetch"]);
        assert_eq!(f["fetched"][0]["branches"][0]["changed"], true, "{f}");
        let o = ok(&board, &run, daemon, &["lib", "outdated"]);
        assert_eq!(o["outdated"][0]["latest"], "B", "{o}");
        let dry = ok(&board, &run, daemon, &["lib", "update", "--dry-run"]);
        assert_eq!(dry["moved"][0]["to"], "B", "{dry}");
        let v = ok(&board, &run, daemon, &["lib", "update"]);
        assert_eq!(v["lock_changed"], true, "{v}");
        let diff = ok(&board, &run, daemon, &["diff"]);
        assert!(
            diff.get("pins").is_some() && diff.get("deps").is_some(),
            "{diff}"
        );
        let u = ok(&board, &run, daemon, &["lib", "uses"]);
        assert_eq!(u["working"][0]["places"][0]["alias"], "passives", "{u}");
        assert_eq!(u["working"][0]["places"][0]["label"], "B", "{u}");
        // The library's history rewritten: B released again at another
        // commit. The pin is refused until updated.
        git(&libs, &["reset", "-q", "--hard", "HEAD~1"]);
        edit_footprint_of(&lib, &comp, "edited again");
        ok(&lib, &run, daemon, &["release", "propose", "--all"]);
        git(&libs, &["add", "-A"]);
        git(&libs, &["commit", "-q", "-m", "library B again"]);
        ok(&board, &run, daemon, &["lib", "fetch"]);
        let (_, check) = ryueda(&board, &run, daemon, &["check"]);
        assert!(
            check.to_string().contains("dependency_rewritten"),
            "{check}"
        );
        // Updating is the decision: the lock moves to what it has now.
        let v = ok(&board, &run, daemon, &["lib", "update"]);
        assert_eq!(v["lock_changed"], true, "{v}");
        let (_, check) = ryueda(&board, &run, daemon, &["check"]);
        assert!(
            !check.to_string().contains("dependency_rewritten"),
            "{check}"
        );
        if daemon {
            ok(&board, &run, true, &["daemon", "stop"]);
        }
    }
}
