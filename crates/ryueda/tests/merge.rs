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
        let whole = ok(&board, &run, daemon, &["merge", "status"]);
        assert!(
            whole["already_failing"].is_array() && whole["warnings"].is_array(),
            "{whole}"
        );
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

/// Runs each command in `board`, which must succeed.
fn script(board: &Path, run: &Path, cmds: &[Vec<&str>]) {
    for args in cmds {
        ok(board, run, false, args);
    }
}

/// The breakout committed with a spare library component R 10k, then
/// branch `theirs` and `main` (ours) each running their commands, and
/// `git merge --no-commit theirs` begun (`--no-commit`, so the hook never
/// decides whether the merge stops).
fn branches(
    top: &Path,
    run: &Path,
    ours: &[Vec<&str>],
    theirs: &[Vec<&str>],
) -> std::path::PathBuf {
    let board = breakout_in_git(top, run);
    script(
        &board,
        run,
        &[vec![
            "libcomp",
            "create",
            "R 10k",
            "--symbol",
            "r",
            "--prefix",
            "R",
            "--value",
            "10k",
            "--footprint",
            "r_0402",
            "--mpn",
            "BASE",
        ]],
    );
    commit_all(top, "a spare part");
    git(top, &["checkout", "-q", "-b", "theirs"]);
    script(&board, run, theirs);
    commit_all(top, "theirs");
    git(top, &["checkout", "-q", "main"]);
    script(&board, run, ours);
    commit_all(top, "ours");
    let _ = git_try(top, &[], &["merge", "--no-commit", "-q", "theirs"]);
    board
}

/// Settles every item by its first repair (a block's areas together),
/// until the status is clean or an item has none; answers the kinds met,
/// in the order met.
fn settle_all(board: &Path, run: &Path, daemon: bool) -> Vec<String> {
    let mut met = Vec::new();
    for _ in 0..8 {
        let list = items(board, run, daemon);
        let Some(c) = list
            .iter()
            .find(|c| c["repairs"].as_array().is_some_and(|r| !r.is_empty()))
        else {
            assert!(list.is_empty(), "items with no repair left: {list:?}");
            return met;
        };
        met.push(c["kind"].as_str().unwrap().to_owned());
        let repair = c["repairs"][0]["id"].as_str().unwrap();
        let ids: Vec<String> = match c["with"].as_array() {
            Some(all) => all.iter().map(|x| x.as_str().unwrap().to_owned()).collect(),
            None => vec![c["id"].as_str().unwrap().to_owned()],
        };
        let mut args = vec!["merge", "resolve"];
        args.extend(ids.iter().map(String::as_str));
        args.extend(["--repair", repair]);
        ok(board, run, daemon, &args);
    }
    panic!(
        "not settled in eight rounds: {:?}",
        items(board, run, daemon)
    );
}

#[test]
fn designators_names_and_joined_nets_settle_by_their_first_repairs() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let ours = [
            vec![
                "component",
                "add",
                "breakout",
                "--of",
                "r_10k",
                "--id",
                "r8",
                "--refdes",
                "R5",
            ],
            vec!["connect", "breakout/r8/1", "rail:3V3"],
            vec!["connect", "breakout/r8/2", "rail:GND"],
            vec!["net", "add", "breakout", "AUX", "--id", "aux_o"],
            vec!["net", "join", "breakout/SDA", "SCL"],
        ];
        let theirs = [
            vec![
                "component",
                "add",
                "breakout",
                "--of",
                "r_10k",
                "--id",
                "r9",
                "--refdes",
                "R5",
            ],
            vec!["net", "add", "breakout", "AUX", "--id", "aux_t"],
            vec!["connect", "breakout/r9/1", "AUX"],
            vec!["connect", "breakout/r9/2", "SDA"],
        ];
        let board = branches(&top, &run, &ours, &theirs);
        let list = items(&board, &run, daemon);
        let kinds: std::collections::BTreeSet<&str> =
            list.iter().filter_map(|c| c["kind"].as_str()).collect();
        for k in ["refdes_clash", "name_clash", "delete_edit"] {
            assert!(kinds.contains(k), "{k}: {list:?}");
        }
        // Staged as they are, the shared hook refuses the commit.
        git(&top, &["add", "-A"]);
        let (committed, said) = git_try(&top, &[], &["commit", "-q", "-m", "merge"]);
        assert!(!committed && said.contains("refdes:board/R5"), "{said}");
        // Through the daemon, one resolution undone and done again.
        if daemon {
            let c = list.iter().find(|c| c["kind"] == "refdes_clash").unwrap();
            ok(
                &board,
                &run,
                true,
                &[
                    "merge",
                    "resolve",
                    c["id"].as_str().unwrap(),
                    "--repair",
                    "renumber",
                ],
            );
            assert!(
                items(&board, &run, true)
                    .iter()
                    .all(|x| x["kind"] != "refdes_clash")
            );
            ok(&board, &run, true, &["undo"]);
            assert!(
                items(&board, &run, true)
                    .iter()
                    .any(|x| x["kind"] == "refdes_clash")
            );
        }
        settle_all(&board, &run, daemon);
        // Ours' R5 stands, theirs' R9 took another; theirs' AUX joined
        // ours'; R9's pin on SDA followed SDA into SCL.
        let read = |p: &str| -> Value {
            serde_json::from_slice(&std::fs::read(board.join(p)).unwrap()).unwrap()
        };
        let r8 = read("blocks/breakout/components/r8.json");
        let r9 = read("blocks/breakout/components/r9.json");
        assert_eq!(r8["refdes"], "R5");
        assert_ne!(r9["refdes"], "R5");
        assert_eq!(
            (r9["pins"]["1"].clone(), r9["pins"]["2"].clone()),
            (json!("aux_o"), json!("scl")),
            "{r9}"
        );
        assert!(!board.join("blocks/breakout/nets/aux_t.json").exists());
        // The repaired merge commits.
        git(&top, &["add", "-A"]);
        let (committed, said) = git_try(&top, &[], &["commit", "-q", "-m", "merge"]);
        assert!(committed, "{said}");
        if daemon {
            let _ = ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

#[test]
fn a_library_component_both_sides_made_is_renumbered_with_its_users() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let make = |mpn: &'static str| {
        vec![
            "libcomp",
            "create",
            "R 22k",
            "--symbol",
            "r",
            "--prefix",
            "R",
            "--value",
            "22k",
            "--footprint",
            "r_0402",
            "--mpn",
            mpn,
        ]
    };
    let board = branches(
        &top,
        &run,
        &[make("OURS")],
        &[
            make("THEIRS"),
            vec![
                "component",
                "add",
                "breakout",
                "--of",
                "r_22k",
                "--id",
                "r7",
                "--refdes",
                "R7",
            ],
        ],
    );
    let list = items(&board, &run, false);
    let c = list
        .iter()
        .find(|c| c["id"] == "library/components/r_22k.json#")
        .unwrap_or_else(|| panic!("the collision: {list:?}"));
    assert_eq!(c["kind"], "id_collision");
    assert_eq!(c["repairs"][0]["to"], "r_22k-2");
    ok(
        &board,
        &run,
        false,
        &[
            "merge",
            "resolve",
            "library/components/r_22k.json#",
            "--repair",
            "renumber",
        ],
    );
    let read = |p: &str| -> Value {
        serde_json::from_slice(&std::fs::read(board.join(p)).unwrap()).unwrap()
    };
    assert_eq!(read("library/components/r_22k.json")["mpn"], "OURS");
    assert_eq!(read("library/components/r_22k-2.json")["mpn"], "THEIRS");
    assert_eq!(
        read("blocks/breakout/components/r7.json")["of"],
        json!({"id": "r_22k-2"})
    );
}

/// Both branches draw a symbol `vreg` and make a library component `vreg`
/// of it, so the symbol and the component collide together: once both
/// are renumbered, theirs' component names theirs' symbol, whether one
/// `merge resolve` renumbers both or the symbol's comes first. (The
/// component's alone first is refused: its users would name a symbol
/// whose record is still unsettled.)
#[test]
fn a_symbol_and_its_library_component_renumbered_together_stay_wired() {
    let make = |pins: &'static str, mpn: &'static str| {
        vec![
            vec![
                "symbol",
                "create",
                "vreg",
                "--template",
                "ic",
                "--pins",
                pins,
            ],
            vec![
                "libcomp",
                "create",
                "VREG",
                "--symbol",
                "vreg",
                "--prefix",
                "U",
                "--value",
                "vreg",
                "--footprint",
                "r_0402",
                "--mpn",
                mpn,
            ],
        ]
    };
    let sym = "library/symbols/vreg.json#";
    let comp = "library/components/vreg.json#";
    for batches in [vec![vec![sym, comp]], vec![vec![sym], vec![comp]]] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = branches(
            &top,
            &run,
            &make(
                r#"[{"number":"1","name":"A","type":"passive"},{"number":"2","name":"B","type":"passive"}]"#,
                "OURS",
            ),
            &make(
                r#"[{"number":"1","name":"IN","type":"passive"},{"number":"2","name":"OUT","type":"passive"}]"#,
                "THEIRS",
            ),
        );
        let list = items(&board, &run, false);
        for id in [sym, comp] {
            let c = list
                .iter()
                .find(|c| c["id"] == id)
                .unwrap_or_else(|| panic!("{id}: {list:?}"));
            assert_eq!(c["kind"], "id_collision");
            assert_eq!(c["repairs"][0]["to"], "vreg-2");
        }
        for batch in &batches {
            let mut args = vec!["merge", "resolve"];
            args.extend(batch.iter().copied());
            args.extend(["--repair", "renumber"]);
            ok(&board, &run, false, &args);
        }
        let read = |p: &str| -> Value {
            serde_json::from_slice(&std::fs::read(board.join(p)).unwrap()).unwrap()
        };
        let (ours, theirs) = (
            read("library/components/vreg.json"),
            read("library/components/vreg-2.json"),
        );
        assert_eq!(
            (&ours["mpn"], &ours["symbol"]),
            (&json!("OURS"), &json!({"id": "vreg"})),
            "{batches:?}"
        );
        assert_eq!(
            (&theirs["mpn"], &theirs["symbol"]),
            (&json!("THEIRS"), &json!({"id": "vreg-2"})),
            "{batches:?}"
        );
        assert_ne!(
            read("library/symbols/vreg.json"),
            read("library/symbols/vreg-2.json")
        );
    }
}

#[test]
fn a_library_component_deleted_under_a_placement_is_restored() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = branches(
        &top,
        &run,
        &[vec!["libcomp", "delete", "r_10k"]],
        &[vec![
            "component",
            "add",
            "breakout",
            "--of",
            "r_10k",
            "--id",
            "r9",
            "--refdes",
            "R9",
        ]],
    );
    let list = items(&board, &run, false);
    let c = list
        .iter()
        .find(|c| c["kind"] == "delete_edit")
        .unwrap_or_else(|| panic!("the deleted libcomp: {list:?}"));
    assert_eq!(c["id"], "blocks/breakout/components/r9.json#/of");
    assert_eq!(c["repairs"][0]["id"], "restore");
    ok(
        &board,
        &run,
        false,
        &[
            "merge",
            "resolve",
            c["id"].as_str().unwrap(),
            "--repair",
            "restore",
        ],
    );
    assert!(board.join("library/components/r_10k.json").exists());
}

#[test]
fn what_the_merge_broke_blocks_the_commit_until_fixed() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    // A class in the base; theirs deletes it, ours gives SDA it.
    let board = breakout_in_git(&top, &run);
    script(
        &board,
        &run,
        &[vec!["class", "add", "Fast", "--id", "fast"]],
    );
    commit_all(&top, "a class");
    git(&top, &["checkout", "-q", "-b", "theirs"]);
    script(&board, &run, &[vec!["class", "delete", "fast"]]);
    commit_all(&top, "theirs");
    git(&top, &["checkout", "-q", "main"]);
    script(
        &board,
        &run,
        &[vec!["net", "edit", "breakout/SDA", "--class", "Fast"]],
    );
    commit_all(&top, "ours");
    let _ = git_try(&top, &[], &["merge", "--no-commit", "-q", "theirs"]);
    let list = items(&board, &run, false);
    let c = list
        .iter()
        .find(|c| c["kind"] == "merge_faults")
        .unwrap_or_else(|| panic!("what the merge broke: {list:?}"));
    assert_eq!(c["id"], "faults:blocks/breakout/nets/sda.json#/class");
    assert_eq!(c["cause"], "both");
    git(&top, &["add", "-A"]);
    let (committed, _) = git_try(&top, &[], &["commit", "-q", "-m", "merge"]);
    assert!(!committed);
    // Fixed by an edit: the class taken off SDA.
    ok(
        &board,
        &run,
        false,
        &["net", "edit", "breakout/SDA", "--class", ""],
    );
    assert!(items(&board, &run, false).is_empty());
    git(&top, &["add", "-A"]);
    let (committed, said) = git_try(&top, &[], &["commit", "-q", "-m", "merge"]);
    assert!(committed, "{said}");
}
