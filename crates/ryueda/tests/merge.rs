//! ryueda's merges through Git itself (design 518): the driver merging
//! by value, `merge status` and `merge resolve` both ways (in process and
//! through the daemon), the shared hook refusing an unsettled commit,
//! layouts merged at status time, and the steps Git merges in (merge,
//! rebase, cherry-pick).

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;
#[path = "support/gitrepo.rs"]
mod gitrepo;

use gitrepo::*;
use serde_json::{Value, json};
use std::path::Path;

/// `ryueda` that must succeed: its result.
fn ok(dir: &Path, run: &Path, daemon: bool, args: &[&str]) -> Value {
    let (code, v) = ryueda(dir, run, daemon, args);
    assert_eq!(code, 0, "{args:?}: {v}");
    v["result"].clone()
}

/// The items of `merge status`.
fn items(dir: &Path, run: &Path, daemon: bool) -> Vec<Value> {
    ok(dir, run, daemon, &["merge", "status"])["conflicts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Commits everything in `top`.
fn commit_all(top: &Path, message: &str) {
    git(top, &["add", "-A"]);
    git(top, &["commit", "-q", "-m", message]);
}

/// The breakout laid out and committed on `main`, then a branch `theirs`
/// adding C97 on SCL and changing R1's value, and `main` adding C98 on
/// SCL: both sides' new capacitors take the same free space.
fn laid_out_branches(top: &Path, run: &Path) -> std::path::PathBuf {
    let board = breakout_in_git(top, run);
    ok(&board, run, false, &["layout"]);
    commit_all(top, "laid out");
    git(top, &["checkout", "-q", "-b", "theirs"]);
    for args in [
        vec![
            "component",
            "add",
            "breakout",
            "--of",
            "c_100n",
            "--id",
            "c97",
            "--refdes",
            "C97",
        ],
        vec!["connect", "breakout/c97/1", "SCL"],
        vec!["component", "edit", "breakout/r1", "--value", "47k"],
    ] {
        ok(&board, run, false, &args);
    }
    commit_all(top, "theirs");
    git(top, &["checkout", "-q", "main"]);
    for args in [
        vec![
            "component",
            "add",
            "breakout",
            "--of",
            "c_100n",
            "--id",
            "c98",
            "--refdes",
            "C98",
        ],
        vec!["connect", "breakout/c98/1", "SCL"],
    ] {
        ok(&board, run, false, &args);
    }
    commit_all(top, "ours");
    board
}

#[test]
fn layouts_merge_at_status_time_and_relayout_settles_them() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = laid_out_branches(&top, &run);
        let (merged, _) = git_try(&top, &[], &["merge", "-q", "theirs"]);
        assert!(!merged, "both sides changed the layout");
        // The connectivity merged by value: each side's files, R1 theirs.
        let r1: Value = serde_json::from_slice(
            &std::fs::read(board.join("blocks/breakout/components/r1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(r1["value"], "47k", "{r1}");
        let list = items(&board, &run, daemon);
        let areas: Vec<&Value> = list
            .iter()
            .filter(|c| c["kind"] == "layout_conflict")
            .collect();
        assert!(!areas.is_empty(), "{list:?}");
        assert_eq!(areas[0]["id"], "breakout#area:1");
        assert_eq!(areas[0]["repairs"][0]["id"], "relayout");
        let refs = areas
            .iter()
            .flat_map(|a| a["concerns"]["refs"].as_array().unwrap().clone())
            .collect::<Vec<_>>();
        assert!(
            refs.contains(&json!("breakout/c97")) && refs.contains(&json!("breakout/c98")),
            "{refs:?}"
        );
        // Staged with its record, the shared hook refuses the commit.
        git(&top, &["add", "-A"]);
        let (committed, said) = git_try(&top, &[], &["commit", "-q", "-m", "merge"]);
        assert!(!committed);
        assert!(said.contains("breakout#area:1"), "{said}");
        // One area alone is refused when the block has more.
        let ids: Vec<String> = areas[0]["with"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect();
        if ids.len() > 1 {
            let (code, v) = ryueda(
                &board,
                &run,
                daemon,
                &["merge", "resolve", &ids[0], "--repair", "relayout"],
            );
            assert_eq!(
                (code, v["error"]["code"].clone()),
                (1, json!("invalid_param")),
                "{v}"
            );
        }
        let mut args = vec!["merge", "resolve"];
        args.extend(ids.iter().map(String::as_str));
        args.extend(["--repair", "relayout"]);
        let v = ok(&board, &run, daemon, &args);
        assert_eq!(
            v["changes"],
            json!([{"path": "blocks/breakout/layout.json", "change": "written"}])
        );
        assert_eq!(v["status"]["clean"], true, "{v}");
        if daemon {
            // One logged change: undo brings the record back, and redo settles it.
            ok(&board, &run, true, &["undo"]);
            assert!(
                items(&board, &run, true)
                    .iter()
                    .any(|c| c["kind"] == "layout_conflict")
            );
            ok(&board, &run, true, &["redo"]);
            assert_eq!(ok(&board, &run, true, &["merge", "status"])["clean"], true);
        }
        commit_all(&top, "merged");
        // What was committed is current: laying out again changes nothing.
        ok(&board, &run, daemon, &["layout"]);
        let out = git_command(&top)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            out.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        if daemon {
            ok(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

/// The breakout committed on `main`, then `theirs` setting R1 to 47k and
/// tagging it, and `main` setting it to 10k and adding a net: R1's value
/// is the one conflict.
fn value_branches(top: &Path, run: &Path) -> std::path::PathBuf {
    let board = breakout_in_git(top, run);
    git(top, &["checkout", "-q", "-b", "theirs"]);
    ok(
        &board,
        run,
        false,
        &["component", "edit", "breakout/r1", "--value", "47k"],
    );
    ok(
        &board,
        run,
        false,
        &["tag", "R1", "--block", "breakout", "--add", "critical"],
    );
    commit_all(top, "theirs");
    git(top, &["checkout", "-q", "main"]);
    ok(
        &board,
        run,
        false,
        &["component", "edit", "breakout/r1", "--value", "10k"],
    );
    ok(
        &board,
        run,
        false,
        &["net", "add", "breakout", "INT", "--id", "int"],
    );
    commit_all(top, "ours");
    board
}

const R1: &str = "blocks/breakout/components/r1.json";

#[test]
fn a_value_conflict_is_recorded_listed_refused_and_settled() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = value_branches(&top, &run);
        let (merged, _) = git_try(&top, &[], &["merge", "-q", "theirs"]);
        assert!(!merged);
        // The driver merged R1 by value: theirs' tag in, the value recorded.
        let v: Value = serde_json::from_slice(&std::fs::read(board.join(R1)).unwrap()).unwrap();
        assert_eq!(v["tags"], json!(["critical"]));
        assert_eq!(v["value"], "10k", "ours' in place");
        assert_eq!(v["conflicts"]["/value"]["theirs"], "47k");
        assert!(
            board.join("blocks/breakout/nets/int.json").is_file(),
            "ours' net"
        );
        let list = items(&board, &run, daemon);
        assert_eq!(list.len(), 1, "{list:?}");
        let id = format!("{R1}#/value");
        assert_eq!(list[0]["id"], id.as_str());
        assert_eq!(
            list[0]["concerns"],
            json!({"block": "breakout", "refs": ["breakout/r1"]})
        );
        // Loading the file fails until it is settled; check says so.
        let (code, c) = ryueda(&board, &run, daemon, &["check"]);
        assert_eq!(code, 1, "{c}");
        // Staged with its record, the hook refuses the commit.
        git(&top, &["add", "-A"]);
        let (committed, said) = git_try(&top, &[], &["commit", "-q", "-m", "merge"]);
        assert!(!committed && said.contains(&id), "{said}");
        // Settled by theirs; unstaged until added; then committed.
        let r = ok(&board, &run, daemon, &["merge", "resolve", &id, "--theirs"]);
        assert_eq!(r["status"]["clean"], true);
        assert_eq!(r["status"]["unstaged"], json!([R1]));
        let (committed, said) = git_try(&top, &[], &["commit", "-q", "-m", "merge"]);
        assert!(
            !committed && said.contains("differ from what would be committed"),
            "{said}"
        );
        commit_all(&top, "merge");
        let v: Value = serde_json::from_slice(&std::fs::read(board.join(R1)).unwrap()).unwrap();
        assert_eq!(
            (v["value"].clone(), v["format"].clone()),
            (json!("47k"), json!(2))
        );
        assert_eq!(
            ryueda(&board, &run, daemon, &["merge", "status"]).1["error"]["code"],
            "not_merging"
        );
        if daemon {
            ok(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

#[test]
fn rebase_and_cherry_pick_stop_on_a_record_and_go_on_once_settled() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = value_branches(&top, &run);
    let id = format!("{R1}#/value");
    // Rebase theirs onto main: its commit stops on R1.
    git(&top, &["checkout", "-q", "theirs"]);
    let (rebased, _) = git_try(&top, &[], &["rebase", "-q", "main"]);
    assert!(!rebased);
    let st = ok(&board, &run, false, &["merge", "status"]);
    assert_eq!(st["step"], "rebase");
    ok(&board, &run, false, &["merge", "resolve", &id, "--ours"]);
    git(&top, &["add", "-A"]);
    let (done, said) = git_try(&top, &["core.editor=true"], &["rebase", "--continue"]);
    assert!(done, "{said}");
    // Cherry-pick main's commit onto a branch from the base.
    git(&top, &["checkout", "-q", "-b", "pick", "HEAD~2"]);
    ok(
        &board,
        &run,
        false,
        &["component", "edit", "breakout/r1", "--value", "22k"],
    );
    commit_all(&top, "pick base");
    let (picked, _) = git_try(&top, &[], &["cherry-pick", "main"]);
    assert!(!picked);
    assert_eq!(
        ok(&board, &run, false, &["merge", "status"])["step"],
        "cherry-pick"
    );
    ok(
        &board,
        &run,
        false,
        &["merge", "resolve", &id, "--value", "33k"],
    );
    git(&top, &["add", "-A"]);
    let (done, said) = git_try(&top, &["core.editor=true"], &["cherry-pick", "--continue"]);
    assert!(done, "{said}");
    let v: Value = serde_json::from_slice(&std::fs::read(board.join(R1)).unwrap()).unwrap();
    assert_eq!(v["value"], "33k");
}

#[test]
fn crlf_checkouts_merge_by_value_too() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    git(&top, &["checkout", "-q", "-b", "theirs"]);
    ok(
        &board,
        &run,
        false,
        &["component", "edit", "breakout/r1", "--value", "47k"],
    );
    commit_all(&top, "theirs");
    git(&top, &["checkout", "-q", "main"]);
    ok(
        &board,
        &run,
        false,
        &["component", "edit", "breakout/r1", "--refdes", "R9"],
    );
    commit_all(&top, "ours");
    // Git for Windows' default: the attributes' eol=lf keeps files LF.
    let (merged, said) = git_try(
        &top,
        &["core.autocrlf=true"],
        &["merge", "-q", "theirs", "-m", "m"],
    );
    assert!(merged, "{said}");
    let text = std::fs::read_to_string(board.join(R1)).unwrap();
    assert!(!text.contains('\r'), "{text:?}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        (v["value"].clone(), v["refdes"].clone()),
        (json!("47k"), json!("R9"))
    );
}

#[cfg(unix)]
#[test]
fn ryucads_hook_line_and_ryuedas_share_the_hook() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    // A stand-in ryucad that notes each run and passes.
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let ryucad = bin.join("ryucad");
    std::fs::write(
        &ryucad,
        "#!/bin/sh\necho ran >> \"$(dirname \"$0\")/ran\"\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&ryucad, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(&top, &["init", "-q"]);
    // ryucad's hook as it wrote it before the shared one (ryu-git takes it in).
    let hooks = top.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for name in ["pre-merge-commit", "pre-commit"] {
        let p = hooks.join(name);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\n# ryucad: refuse a merge commit while a ryucad project's merge is unresolved (`ryucad git install`)\nexec '{}' merge status --hook\n",
                ryucad.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(top.join("board")).unwrap();
    ok(&top.join("board"), &run, false, &["init"]);
    let hook = std::fs::read_to_string(hooks.join("pre-commit")).unwrap();
    assert!(
        hook.contains("# ryucad") && hook.contains("# ryueda"),
        "{hook}"
    );
    // `init` writes no erc.json: the base has one, so both sides change it
    // (added on both, it would be one id_collision for the whole file).
    std::fs::write(
        top.join("board/erc.json"),
        "{\n  \"format\": 1,\n  \"kind\": \"ryueda.erc\",\n  \"severity\": {\n    \"pin_conflict\": \"warning\"\n  }\n}\n",
    )
    .unwrap();
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "init"]);
    assert!(bin.join("ran").is_file(), "ryucad's line ran");
    // A ryueda record left staged: ryueda's line refuses, after ryucad's ran.
    git(&top, &["checkout", "-q", "-b", "theirs"]);
    std::fs::write(top.join("board/erc.json"), "{\n  \"format\": 1,\n  \"kind\": \"ryueda.erc\",\n  \"severity\": {\n    \"pin_conflict\": \"off\"\n  }\n}\n").unwrap();
    commit_all(&top, "theirs");
    git(&top, &["checkout", "-q", "main"]);
    std::fs::write(top.join("board/erc.json"), "{\n  \"format\": 1,\n  \"kind\": \"ryueda.erc\",\n  \"severity\": {\n    \"pin_conflict\": \"error\"\n  }\n}\n").unwrap();
    commit_all(&top, "ours");
    let (merged, _) = git_try(&top, &[], &["merge", "-q", "theirs"]);
    assert!(!merged);
    git(&top, &["add", "-A"]);
    let (committed, said) = git_try(&top, &[], &["commit", "-q", "-m", "merge"]);
    assert!(
        !committed && said.contains("erc.json#/severity/pin_conflict"),
        "{said}"
    );
}

#[test]
fn a_merge_is_told_to_watchers() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = value_branches(&top, &run);
    let (merged, _) = git_try(&top, &[], &["merge", "-q", "theirs"]);
    assert!(!merged);
    let out = ryueda_cmd(
        &board,
        &run,
        true,
        &[
            "--as",
            "watcher",
            "watch",
            "--topic",
            "merge",
            "--timeout",
            "20",
        ],
    )
    .output()
    .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let note = &v["result"]["notifications"][0];
    assert_eq!(note["method"], "merge.changed", "{v}");
    assert_eq!(note["params"]["merging"], true);
    assert_eq!(
        note["params"]["status"]["conflicts"][0]["id"],
        format!("{R1}#/value")
    );
    ok(&board, &run, true, &["daemon", "stop"]);
}
