//! Switching branches through the CLI and its daemon, over real
//! repositories (design 447): listed, refused while project files are
//! uncommitted or another worktree has the branch, switched, logged once
//! and never undone.

use assert_cmd::Command;
use ryucad::doc::project::plain;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// A repository holding a project at its root on `main`, with a committed
/// box, and branch `arm` one commit ahead with the box 15 mm long; its
/// home, runtime folder and cache are the test's own.
struct Repo {
    dir: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        let root = root.canonicalize().map(plain).unwrap();
        let r = Repo { dir, root };
        let root = r.root.clone();
        r.git(&root, &["init", "-q", "-b", "main"]);
        r.ok(&root, &["init"]);
        r.ok(&root, &["part", "create", "p"]);
        r.ok(
            &root,
            &[
                "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20",
                "--param", "z=5",
            ],
        );
        r.git(&root, &["add", "-A"]);
        r.git(&root, &["commit", "-q", "-m", "a box"]);
        r.git(&root, &["switch", "-q", "-c", "arm"]);
        r.ok(&root, &["feature", "edit", "p", "b", "--param", "x=15"]);
        r.git(&root, &["commit", "-q", "-am", "longer"]);
        r.git(&root, &["switch", "-q", "main"]);
        r
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env("HOME", self.home())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_GLOBAL")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Runs ryucad in `dir`, through its daemon unless `direct`.
    fn run(&self, dir: &Path, direct: bool, args: &[&str]) -> (i32, Value) {
        let mut cmd = Command::cargo_bin("ryucad").unwrap();
        cmd.args(args)
            .current_dir(dir)
            .env("HOME", self.home())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env("RYUCAD_RUNTIME_DIR", self.dir.path().join("run"))
            .env("RYUCAD_CONFIG_DIR", self.dir.path().join("config"))
            .env("RYUCAD_CACHE_DIR", self.dir.path().join("cache"))
            .env("RYUCAD_IDLE_TIMEOUT", "60");
        if direct {
            cmd.env("RYUCAD_NO_DAEMON", "1");
        }
        let out = cmd.output().unwrap();
        let v: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
        (out.status.code().unwrap(), v)
    }

    /// Runs ryucad in process, which must succeed; its result.
    fn ok(&self, dir: &Path, args: &[&str]) -> Value {
        let (code, v) = self.run(dir, true, args);
        assert_eq!(code, 0, "ryucad {args:?}: {v}");
        v["result"].clone()
    }

    /// Runs ryucad through the daemon, which must succeed; its result.
    fn live(&self, args: &[&str]) -> Value {
        let (code, v) = self.run(&self.root, false, args);
        assert_eq!(code, 0, "ryucad {args:?}: {v}");
        v["result"].clone()
    }

    /// Runs ryucad through the daemon, which must fail with `error`.
    fn refused(&self, args: &[&str], error: &str) -> Value {
        let (code, v) = self.run(&self.root, false, args);
        assert_eq!((code, v["error"]["code"].as_str()), (1, Some(error)), "{v}");
        v["error"].clone()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = Command::cargo_bin("ryucad")
            .unwrap()
            .args(["daemon", "stop", "--all"])
            .env("RYUCAD_RUNTIME_DIR", self.dir.path().join("run"))
            .output();
    }
}

/// The row of `branch list` named `name`.
fn row<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == name)
        .unwrap_or_else(|| panic!("{name} not in {list}"))
}

#[test]
fn an_agent_switches_branches_through_its_daemon_both_ways() {
    let r = Repo::new();
    let list = r.live(&["branch", "list"]);
    assert_eq!(list["current"], "main");
    assert_eq!(
        (&row(&list, "arm")["ahead"], &row(&list, "arm")["behind"]),
        (&json!(1), &json!(0))
    );
    let project = list["project"].as_str().unwrap().to_owned();
    // An uncommitted change through the daemon: refused, the file named.
    r.live(&["feature", "edit", "p", "b", "--param", "x=12"]);
    let e = r.refused(&["switch", "arm"], "checkout_dirty");
    assert_eq!(e["details"]["files"], json!(["parts/p/features/b.json"]));
    // Committed in Git, the switch goes through, guarded by the project
    // revision the list gave.
    r.git(&r.root, &["commit", "-q", "-am", "shorter"]);
    let a = r.live(&["switch", "arm", "--expect", &project]);
    assert_eq!(
        (&a["from"], &a["to"], &a["switched"]),
        (&json!("main"), &json!("arm"), &json!(true))
    );
    let shown = r.live(&["feature", "show", "p", "b"]);
    assert_eq!(shown["params"]["x"]["value"], "15 mm");
    // The log has it, by the agent; undo never takes it, and the edit
    // before it moved with the files.
    let log = r.live(&["activity"]);
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (&last["kind"], &last["method"]),
        (&json!("switch"), &json!("branch.switch"))
    );
    assert_eq!(last["params"], json!({"from": "main", "to": "arm"}));
    assert_eq!(a["entry"], last["seq"]);
    let seq = last["seq"].to_string();
    r.refused(&["undo", "--entry", &seq], "nothing_to_undo");
    r.refused(&["undo"], "undo_conflict");
    // Another worktree's branch: refused, its folder named.
    let wt = r.dir.path().join("wt");
    r.ok(
        &r.root,
        &[
            "worktree",
            "add",
            wt.to_str().unwrap(),
            "-b",
            "leg",
            "--no-warm",
        ],
    );
    let e = r.refused(&["switch", "leg"], "branch_in_worktree");
    assert_eq!(
        e["details"]["worktree"],
        json!(plain(wt.canonicalize().unwrap()))
    );
    // An unknown branch, and back to main.
    r.refused(&["switch", "nope"], "rev_not_found");
    let back = r.live(&["switch", "main"]);
    assert_eq!(
        (&back["from"], &back["to"]),
        (&json!("arm"), &json!("main"))
    );
    let shown = r.live(&["feature", "show", "p", "b"]);
    assert_eq!(shown["params"]["x"]["value"], "12 mm");
}
