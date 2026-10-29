//! Agents on branches: worktrees of one project, each with its own daemon,
//! sharing one disk cache, made, listed and removed through the CLI over
//! real repositories.

use assert_cmd::Command;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// A repository holding a project at its root, on `main`, with a
/// committed box; its home, runtime folder and disk cache are the test's
/// own, and every worktree shares the cache.
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
        let root = root.canonicalize().unwrap();
        let r = Repo { dir, root };
        r.git(&r.root, &["init", "-q", "-b", "main"]);
        r.ok(&r.root, &["init"]);
        r.ok(&r.root, &["part", "create", "p"]);
        r.ok(
            &r.root,
            &[
                "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20",
                "--param", "z=5",
            ],
        );
        r.git(&r.root, &["add", "-A"]);
        r.git(&r.root, &["commit", "-q", "-m", "a box"]);
        r
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).into_owned()
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

    /// Runs ryucad, which must fail with `error`.
    fn fails(&self, dir: &Path, args: &[&str], error: &str) -> Value {
        let (code, v) = self.run(dir, true, args);
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

/// A worktree's entry in `worktree list`.
fn entry<'a>(list: &'a Value, path: &Path) -> &'a Value {
    list["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["path"] == json!(path))
        .unwrap_or_else(|| panic!("{path:?} not in {list}"))
}

#[test]
fn worktrees_are_added_listed_and_removed() {
    let r = Repo::new();
    // This worktree's rebuild fills the machine's cache.
    r.ok(&r.root, &["rebuild"]);
    let v = r.ok(
        &r.root,
        &["--as", "agent-b", "worktree", "add", "../arm", "-b", "arm"],
    );
    let arm = r.dir.path().join("arm").canonicalize().unwrap();
    assert_eq!(v["path"], json!(arm));
    assert_eq!(v["branch"], "arm");
    assert_eq!(v["as"], "agent-b");
    // Warm from the shared cache: nothing to build.
    assert_eq!(v["warm"]["built"], 0, "{v}");
    let l = r.ok(&r.root, &["worktree", "list"]);
    assert_eq!(l["branch"], "main");
    let (main, other) = (entry(&l, &r.root), entry(&l, &arm));
    assert_eq!(
        (main["current"].clone(), main["main"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(other["current"], false);
    assert_eq!(other["as"], "agent-b");
    assert_eq!(
        (other["ahead"].clone(), other["behind"].clone()),
        (json!(0), json!(0))
    );
    assert_eq!(other["dirty"], false);
    // No daemon is running there, and listing starts none.
    assert_eq!(other["daemon"], Value::Null);
    let l = r.ok(&r.root, &["worktree", "list"]);
    assert_eq!(entry(&l, &arm)["daemon"], Value::Null);

    // An agent works there through its daemon, uncommitted.
    let (code, v) = r.run(
        &arm,
        false,
        &[
            "--as", "agent-b", "feature", "edit", "p", "b", "--param", "x=12",
        ],
    );
    assert_eq!(code, 0, "{v}");
    let l = r.ok(&r.root, &["worktree", "list"]);
    let other = entry(&l, &arm);
    assert_eq!(
        (other["dirty"].clone(), other["changes"].clone()),
        (json!(true), json!(1))
    );
    let d = &other["daemon"];
    assert_eq!(d["running"], true, "{other}");
    let names: Vec<&str> = d["participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["agent-b"]);
    assert_eq!(d["last_activity"]["method"], "feature.edit");
    assert_eq!(d["last_activity"]["participant"]["name"], "agent-b");
    let state = other["state"].clone();
    // Through this worktree's daemon, it is the one answering for itself.
    let (code, l) = r.run(&r.root, false, &["worktree", "list"]);
    assert_eq!(code, 0, "{l}");
    let me = entry(&l["result"], &r.root);
    assert_eq!(me["daemon"]["participants"][0]["name"], "agent");
    assert_eq!(entry(&l["result"], &arm)["state"], state);

    // Committed there, it is a commit ahead.
    r.git(&arm, &["commit", "-qam", "x is 12"]);
    let l = r.ok(&r.root, &["worktree", "list"]);
    let other = entry(&l, &arm);
    assert_eq!(
        (other["ahead"].clone(), other["dirty"].clone()),
        (json!(1), json!(false))
    );
    // The state is of the files, not the commit.
    assert_eq!(other["state"], state);

    // Removing it stops its daemon; with changes it is refused.
    r.ok(&arm, &["feature", "edit", "p", "b", "--param", "x=13"]);
    let e = r.fails(&r.root, &["worktree", "remove", "../arm"], "worktree_dirty");
    assert_eq!(e["details"]["changes"].as_array().unwrap().len(), 1);
    let v = r.ok(&r.root, &["worktree", "remove", "../arm", "--force"]);
    assert_eq!(v["daemon_stopped"], true);
    assert!(!arm.exists());
    let l = r.ok(&r.root, &["worktree", "list"]);
    assert_eq!(l["worktrees"].as_array().unwrap().len(), 1);
    // Neither this worktree nor a folder that isn't one goes.
    r.fails(&r.root, &["worktree", "remove", "."], "invalid_param");
    r.fails(
        &r.root,
        &["worktree", "remove", "../home"],
        "worktree_not_found",
    );
    // A worktree needs a new folder.
    r.fails(&r.root, &["worktree", "add", "../home"], "invalid_param");
}

#[test]
fn a_worktree_says_when_the_drivers_are_missing() {
    let r = Repo::new();
    // `init` in a repository installed them; a clone wouldn't have them.
    r.git(&r.root, &["config", "--unset", "merge.ryucad.driver"]);
    let (code, v) = r.run(&r.root, true, &["worktree", "add", "../plain", "--no-warm"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"]["branch"], Value::Null);
    assert_eq!(v["result"]["warm"], Value::Null);
    let w = v["warnings"].as_array().unwrap();
    assert!(w[0].as_str().unwrap().contains("ryucad git install"), "{v}");
    // Once installed, every worktree shares them.
    r.ok(&r.root, &["git", "install"]);
    let (_, v) = r.run(
        &r.root,
        true,
        &["worktree", "add", "../set-up", "--no-warm"],
    );
    assert_eq!(v["warnings"], json!([]), "{v}");
}

#[test]
fn another_worktree_is_compared_with_and_diffed_against() {
    let r = Repo::new();
    r.ok(
        &r.root,
        &["worktree", "add", "../arm", "-b", "arm", "--no-warm"],
    );
    let arm = r.dir.path().join("arm").canonicalize().unwrap();
    // An agent's uncommitted edit there.
    r.ok(&arm, &["feature", "edit", "p", "b", "--param", "x=12"]);
    // Named relative to where the command runs.
    let d = r.ok(&r.root, &["diff", "--to", "worktree:../arm", "--model"]);
    assert_eq!(d["to"]["worktree"], json!(arm));
    assert_eq!(d["to"]["branch"], "arm");
    assert_eq!(d["parts"]["p"]["features"]["b"]["status"], "changed");
    assert!(d["parts"]["p"]["features"]["b"]["model"].is_object(), "{d}");
    let from_arm = r.ok(&arm, &["diff", "--from", "worktree:../main"]);
    assert_eq!(from_arm["from"]["branch"], "main");

    // A compare through the daemon names it by its project folder.
    let (code, v) = r.run(&r.root, false, &["compare", "--worktree", "../arm/"]);
    assert_eq!(code, 0, "{v}");
    let rev = format!("worktree:{}", arm.display());
    assert_eq!(v["result"]["compare"], json!({"rev": rev, "commit": null}));
    let me = v["result"]["participant"].as_str().unwrap().to_owned();
    let (_, v) = r.run(&r.root, false, &["compare", "--of", &me]);
    assert_eq!(v["result"]["compare"]["rev"], json!(rev));
    // Its renders and meshes read that worktree's files.
    let (code, v) = r.run(&r.root, false, &["rebuild", "--rev", &rev]);
    assert_eq!(code, 0, "{v}");

    // When the other agent writes, the comparing participant hears of it.
    let watcher = {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("ryucad"));
        cmd.args([
            "watch",
            "--topic",
            "model",
            "--count",
            "1",
            "--timeout",
            "20",
        ])
        .current_dir(&r.root)
        .env("HOME", r.home())
        .env("RYUCAD_RUNTIME_DIR", r.dir.path().join("run"))
        .env("RYUCAD_CONFIG_DIR", r.dir.path().join("config"))
        .env("RYUCAD_CACHE_DIR", r.dir.path().join("cache"))
        .env("RYUCAD_IDLE_TIMEOUT", "60")
        .stdout(std::process::Stdio::piped());
        cmd.spawn().unwrap()
    };
    std::thread::sleep(std::time::Duration::from_millis(1500));
    r.ok(&arm, &["feature", "edit", "p", "b", "--param", "x=14"]);
    let out = watcher.wait_with_output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let n = &v["result"]["notifications"][0];
    assert_eq!(n["method"], "compare.changed", "{v}");
    assert_eq!(n["params"]["rev"], json!(rev));

    // Not a worktree of this repository.
    let (code, v) = r.run(&r.root, false, &["compare", "--worktree", "../home"]);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("worktree_not_found"))
    );
    r.fails(
        &r.root,
        &["diff", "--to", "worktree:../home"],
        "worktree_not_found",
    );
}

#[test]
fn eight_daemons_share_one_disk_cache() {
    // Eight worktrees, each with its own daemon, rebuilding one project at
    // once over one cache: every entry is written whole (a temporary file
    // renamed) and read whole, so every daemon gets the same bodies.
    let r = Repo::new();
    for i in 0..4 {
        let id = format!("s{i}");
        let radius = format!("radius={}", i + 1);
        r.ok(
            &r.root,
            &[
                "feature", "add", "p", "sphere", "--id", &id, "--param", &radius,
            ],
        );
    }
    r.git(&r.root, &["add", "-A"]);
    r.git(&r.root, &["commit", "-qm", "spheres"]);
    let folders: Vec<PathBuf> = (0..8)
        .map(|i| {
            let name = format!("../w{i}");
            r.ok(&r.root, &["worktree", "add", &name, "--no-warm"]);
            r.dir.path().join(format!("w{i}"))
        })
        .collect();
    let answers: Vec<Value> = std::thread::scope(|s| {
        let runs: Vec<_> = folders
            .iter()
            .map(|f| s.spawn(|| r.run(f, false, &["rebuild"])))
            .collect();
        runs.into_iter()
            .map(|h| {
                let (code, v) = h.join().unwrap();
                assert_eq!(code, 0, "{v}");
                v["result"]["parts"][0]["hash"].clone()
            })
            .collect()
    });
    assert!(
        answers.iter().all(|h| *h == answers[0] && h.is_string()),
        "{answers:?}"
    );
    // The cache they filled is sound: a fresh process rebuilds from it.
    let v = r.ok(&r.root, &["cache", "warm"]);
    assert_eq!(v["built"], 0, "{v}");
    let again = r.ok(&r.root, &["rebuild"]);
    assert_eq!(again["parts"][0]["hash"], answers[0]);
}
