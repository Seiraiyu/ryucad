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
