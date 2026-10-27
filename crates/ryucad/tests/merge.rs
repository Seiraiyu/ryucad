//! Merges through Git itself: real repositories, branches, and `git merge`,
//! `git rebase` and `git cherry-pick` calling ryucad's merge driver, and
//! the hooks ryucad installs refusing commits. Git's config is the test's
//! own (a home of its own, no system file), and the driver is the ryucad
//! binary these tests build.

use assert_cmd::Command;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

/// A repository holding a project, maybe below its root.
struct Repo {
    /// The folder of the test: the home, and every clone.
    dir: std::rc::Rc<tempfile::TempDir>,
    /// The work tree.
    top: PathBuf,
    /// The project folder.
    root: PathBuf,
}

impl Repo {
    /// A repository with a project at `sub` (`""` for the root) on branch
    /// `main`, Git told about ryucad's drivers by hand.
    fn new(sub: &str) -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().join("repo");
        let root = if sub.is_empty() {
            top.clone()
        } else {
            top.join(sub)
        };
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        let r = Repo {
            dir: std::rc::Rc::new(dir),
            top,
            root,
        };
        r.ryucad(&["init"]);
        r.git(&["init", "-q", "-b", "main"]);
        let bin = assert_cmd::cargo::cargo_bin("ryucad");
        let bin = bin.to_str().unwrap().replace('\\', "/");
        r.git(&["config", "merge.ryucad.name", "ryucad"]);
        r.git(&[
            "config",
            "merge.ryucad.driver",
            &format!("'{bin}' git merge-driver %O %A %B %P"),
        ]);
        r
    }

    /// A clone of this repository at `name` in the test's folder, set up
    /// with `ryucad git install` as its owner would set it up.
    fn clone_as(&self, name: &str) -> Repo {
        let top = self.dir.path().join(name);
        self.git(&[
            "clone",
            "-q",
            self.top.to_str().unwrap(),
            top.to_str().unwrap(),
        ]);
        let sub = self.root.strip_prefix(&self.top).unwrap();
        let r = Repo {
            dir: self.dir.clone(),
            root: top.join(sub),
            top,
        };
        r.ryucad(&["git", "install"]);
        r
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    /// Runs git in the work tree with a fixed identity and time; its exit
    /// code and output.
    fn git_status(&self, args: &[&str]) -> (i32, String) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args)
            .current_dir(&self.top)
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env("GIT_AUTHOR_DATE", "2026-10-21T12:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-10-21T12:00:00Z")
            .env("GIT_EDITOR", "true")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().unwrap_or(-1), text)
    }

    /// Runs git, which must succeed.
    fn git(&self, args: &[&str]) -> String {
        let (code, text) = self.git_status(args);
        assert_eq!(code, 0, "git {args:?}: {text}");
        text
    }

    /// Commits everything.
    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    /// Runs ryucad in the project, in process; its exit code and envelope.
    fn ryucad_status(&self, args: &[&str]) -> (i32, Value) {
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env("RYUCAD_NO_DAEMON", "1")
            .env("RYUCAD_CONFIG_DIR", self.dir.path().join("config"))
            .env("RYUCAD_CACHE_DIR", self.dir.path().join("cache"))
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
        (out.status.code().unwrap(), v)
    }

    /// Runs ryucad, which must succeed; its result.
    fn ryucad(&self, args: &[&str]) -> Value {
        let (code, v) = self.ryucad_status(args);
        assert_eq!(code, 0, "ryucad {args:?}: {v}");
        v["result"].clone()
    }

    /// A project file's JSON.
    fn json(&self, rel: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.root.join(rel)).unwrap()).unwrap()
    }

    /// Whether a project file is canonical.
    fn canonical(&self, rel: &str) -> bool {
        ryucad::doc::canonical::is_canonical(&fs::read_to_string(self.root.join(rel)).unwrap())
    }
}

/// A project with part `p`: a box `b` and a parameter `wall`.
fn boxed(sub: &str) -> Repo {
    let r = Repo::new(sub);
    r.ryucad(&["part", "create", "p"]);
    r.ryucad(&["param", "set", "p", "wall=3 mm"]);
    r.ryucad(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    r.commit("a box");
    r
}

const BOX: &str = "parts/p/features/b.json";

#[test]
fn disjoint_edits_merge_value_by_value() {
    let r = boxed("");
    r.git(&["checkout", "-q", "-b", "theirs"]);
    // Neighbouring lines: Git's own line merge would call this a conflict.
    r.ryucad(&["feature", "edit", "p", "b", "--param", "y=25"]);
    r.ryucad(&["param", "set", "p", "wall=4 mm"]);
    r.commit("theirs");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=12"]);
    r.commit("ours");
    r.git(&["merge", "-q", "--no-edit", "theirs"]);
    let b = r.json(BOX);
    assert_eq!(
        b["params"],
        json!({"x": "12 mm", "y": "25 mm", "z": "5 mm"})
    );
    assert!(r.canonical(BOX));
    assert_eq!(
        r.json("parts/p/parameters.json")["parameters"]["wall"],
        "4 mm"
    );
    r.ryucad(&["rebuild"]);
}

#[test]
fn features_added_on_both_branches_both_stay() {
    let r = boxed("");
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&[
        "feature", "add", "p", "cylinder", "--id", "t1", "--param", "radius=1", "--param",
        "height=2",
    ]);
    r.commit("theirs");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&[
        "feature", "add", "p", "sphere", "--id", "o1", "--param", "radius=1",
    ]);
    r.commit("ours");
    r.git(&["merge", "-q", "--no-edit", "theirs"]);
    // Both after the box: ours first.
    assert_eq!(
        r.json("parts/p/part.json")["features"],
        json!(["b", "o1", "t1"])
    );
    assert!(r.canonical("parts/p/part.json"));
    let built = r.ryucad(&["rebuild"]);
    assert_eq!(built["parts"][0]["features"].as_array().unwrap().len(), 3);
}

#[test]
fn a_value_changed_on_both_branches_is_left_as_a_conflict() {
    let r = boxed("");
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=15"]);
    r.commit("theirs");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=12"]);
    r.commit("ours");
    let (code, out) = r.git_status(&["merge", "--no-edit", "theirs"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("CONFLICT"), "{out}");
    // Git lists it as unmerged; the file is ours with the record.
    assert!(
        r.git(&["status", "--porcelain"])
            .contains(&format!("UU {BOX}"))
    );
    let b = r.json(BOX);
    assert_eq!(b["params"]["x"], "12 mm");
    assert_eq!(
        b["conflicts"],
        json!({"/params/x": {"base": "10 mm", "kind": "value", "ours": "12 mm", "theirs": "15 mm"}})
    );
    assert!(r.canonical(BOX));
    let (code, v) = r.ryucad_status(&["feature", "show", "p", "b"]);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("merge_conflict"))
    );
    let (_, v) = r.ryucad_status(&["doctor"]);
    assert_eq!(v["result"]["issues"][0]["code"], "merge_conflict");
}

#[test]
fn a_sketch_edited_on_both_sides_merges_per_entity() {
    let r = boxed("");
    r.ryucad(&[
        "feature", "add", "p", "sketch", "--id", "s", "--plane", "xy",
    ]);
    r.ryucad(&[
        "sketch",
        "edit",
        "p",
        "s",
        r#"[{"op": "add", "type": "point", "at": [0, 0]},
            {"op": "add", "type": "point", "at": [10, 0]},
            {"op": "add", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "type": "length", "a": "l1", "value": "10 mm"}]"#,
    ]);
    r.commit("a sketch");
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&[
        "sketch",
        "edit",
        "p",
        "s",
        r#"[{"op": "set", "id": "d1", "value": "12 mm"}]"#,
    ]);
    r.commit("theirs: longer");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&[
        "sketch",
        "edit",
        "p",
        "s",
        r#"[{"op": "add", "type": "horizontal", "a": "l1"}]"#,
    ]);
    r.commit("ours: level");
    r.git(&["merge", "-q", "--no-edit", "theirs"]);
    let s = r.json("parts/p/features/s.json");
    assert_eq!(s["constraints"]["d1"]["value"], "12 mm");
    assert_eq!(s["constraints"]["c1"]["type"], "horizontal");
    assert!(r.canonical("parts/p/features/s.json"));
    // The positions are each side's; the sketch reads, stale or not.
    r.ryucad(&["sketch", "get", "p", "s"]);
}

#[test]
fn git_follows_a_feature_it_thinks_was_renamed() {
    let r = boxed("");
    r.ryucad(&[
        "feature", "add", "p", "sphere", "--id", "c", "--param", "radius=1",
    ]);
    r.commit("a sphere");
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&["feature", "edit", "p", "c", "--param", "radius=2"]);
    r.commit("theirs: bigger");
    r.git(&["checkout", "-q", "main"]);
    // Ours replaces c with d, the same sphere. Git sees a rename (by
    // default; 2.47 even with merge.renames=false) and puts theirs' edit
    // on d.
    r.ryucad(&["feature", "delete", "p", "c"]);
    r.ryucad(&[
        "feature", "add", "p", "sphere", "--id", "d", "--param", "radius=1",
    ]);
    r.commit("ours: c is d now");
    r.git(&["merge", "-q", "--no-edit", "theirs"]);
    assert!(!r.root.join("parts/p/features/c.json").exists());
    assert_eq!(
        r.json("parts/p/features/d.json")["params"]["radius"],
        "2 mm"
    );
}

/// Git's version, as `(major, minor)`: `git version 2.50.1 (Apple
/// Git-155)` and `git version 2.56.0.windows.2` alike.
fn git_version() -> (u32, u32) {
    let out = std::process::Command::new("git")
        .arg("--version")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let number = text.split_whitespace().nth(2).unwrap_or_default();
    let mut parts = number.split('.').map(|p| p.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

#[test]
fn git_versions_read_as_numbers() {
    assert!(git_version() >= (2, 0), "{:?}", git_version());
}

#[test]
fn a_set_up_clone_merges_a_delete_and_an_add_as_themselves() {
    // Issue #44: a feature deleted and another added on one side looked
    // like a rename, so the merge lost the other side's edit and never
    // saw the two features colliding at one ID.
    //
    // Issue #75: Git before 2.50 ignores `merge.renames=false`, which is
    // all `ryucad git install` can do about it, and reports the rename;
    // there this passes with a note. CI (which sets `CI`) runs a newer Git
    // and must not skip it.
    let v = git_version();
    if v < (2, 50) {
        assert!(
            std::env::var_os("CI").is_none(),
            "Git {v:?} ignores merge.renames=false; CI needs Git 2.50 or newer"
        );
        eprintln!("note: Git {v:?} ignores merge.renames=false (2.50 honours it); skipped");
        return;
    }
    let r = boxed("");
    r.ryucad(&[
        "feature", "add", "p", "sphere", "--id", "knob", "--param", "radius=4",
    ]);
    r.commit("a knob");
    // `ryucad git install` turns merge.renames off in the clone.
    let c = r.clone_as("clone");
    c.git(&["checkout", "-q", "-b", "feat-b"]);
    c.ryucad(&[
        "feature", "add", "p", "box", "--id", "lug", "--param", "x=4", "--param", "y=4", "--param",
        "z=4",
    ]);
    c.ryucad(&["feature", "delete", "p", "knob"]);
    c.commit("feat-b: a lug for the knob");
    c.git(&["checkout", "-q", "main"]);
    c.ryucad(&[
        "feature",
        "add",
        "p",
        "torus",
        "--id",
        "lug",
        "--param",
        "major_radius=4",
        "--param",
        "minor_radius=1",
    ]);
    c.ryucad(&["feature", "edit", "p", "knob", "--param", "radius=5"]);
    c.commit("main: a lug and a bigger knob");
    let (code, text) = c.git_status(&["merge", "--no-edit", "feat-b"]);
    assert_ne!(code, 0, "{text}");
    assert!(!text.contains("rename"), "{text}");
    let v = c.ryucad(&["merge", "status"]);
    let kinds: Vec<(&str, &str)> = v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["kind"].as_str().unwrap(), c["files"][0].as_str().unwrap()))
        .collect();
    assert!(
        kinds.contains(&("modify_delete", "parts/p/features/knob.json")),
        "{v}"
    );
    assert!(
        kinds.contains(&("id_collision", "parts/p/features/lug.json")),
        "{v}"
    );
}

#[test]
fn rebase_and_cherry_pick_merge_through_the_driver() {
    let r = boxed("");
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "y=25"]);
    r.commit("theirs");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=12"]);
    r.commit("ours");
    r.git(&["branch", "ours"]);
    r.git(&["cherry-pick", "theirs"]);
    assert_eq!(
        r.json(BOX)["params"],
        json!({"x": "12 mm", "y": "25 mm", "z": "5 mm"})
    );
    r.git(&["checkout", "-q", "theirs"]);
    r.git(&["rebase", "-q", "ours"]);
    assert_eq!(
        r.json(BOX)["params"],
        json!({"x": "12 mm", "y": "25 mm", "z": "5 mm"})
    );
}

#[test]
fn a_project_below_the_repository_root_merges_too() {
    let r = boxed("cad");
    assert!(r.root.join(".gitattributes").is_file());
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "y=25"]);
    r.commit("theirs");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=12"]);
    r.commit("ours");
    r.git(&["merge", "-q", "--no-edit", "theirs"]);
    assert_eq!(
        r.json(BOX)["params"],
        json!({"x": "12 mm", "y": "25 mm", "z": "5 mm"})
    );
}

#[test]
fn the_hooks_refuse_a_merge_commit_until_it_is_settled() {
    let r = boxed("");
    let installed = r.ryucad(&["git", "install"]);
    assert_eq!(
        installed["hooks"]["pre-merge-commit"]["status"],
        "installed"
    );
    // Theirs adds a sphere sized by `wall`; ours deletes the parameter.
    // Git merges the files cleanly: only rebuilding shows the break.
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&[
        "feature",
        "add",
        "p",
        "sphere",
        "--id",
        "s",
        "--param",
        "radius=wall",
    ]);
    r.commit("theirs: a sphere as big as the wall");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["param", "delete", "p", "wall"]);
    r.commit("ours: no wall");
    let (code, out) = r.git_status(&["merge", "--no-edit", "theirs"]);
    assert_ne!(code, 0, "{out}");
    assert!(
        out.contains("ryucad: the merge leaves 1 conflict(s)"),
        "{out}"
    );
    assert!(
        out.contains("parts/p/features/s.json#merge_broke (merge_broke, ours)"),
        "{out}"
    );
    let v = r.ryucad(&["merge", "status"]);
    assert_eq!(
        v["conflicts"][0]["error"]["code"], "unknown_parameter",
        "{v}"
    );
    // The agent puts the parameter back; the commit goes through.
    r.ryucad(&["param", "set", "p", "wall=4 mm"]);
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "--no-edit"]);
    let parents = r.git(&["log", "-1", "--format=%P"]);
    assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
    // A merge Git stopped for conflicts is checked when it is committed.
    r.git(&["checkout", "-q", "-b", "later"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=15"]);
    r.commit("later: longer");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=12"]);
    r.commit("main: a bit longer");
    let (code, _) = r.git_status(&["merge", "--no-edit", "later"]);
    assert_eq!(code, 1);
    // Marked resolved without settling it: the commit is refused.
    r.git(&["add", "-A"]);
    let (code, out) = r.git_status(&["commit", "--no-edit"]);
    assert_ne!(code, 0, "{out}");
    assert!(
        out.contains("parts/p/features/b.json#/params/x (value, both)"),
        "{out}"
    );
    r.ryucad(&[
        "merge",
        "resolve",
        "parts/p/features/b.json#/params/x",
        "--theirs",
    ]);
    // Settled in the work tree but not staged: the index still holds the
    // record, and that is what would be committed.
    let (code, out) = r.git_status(&["commit", "--no-edit"]);
    assert_ne!(code, 0, "{out}");
    assert!(out.contains(&format!("  {BOX}")), "{out}");
    assert!(out.contains("git add"), "{out}");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "--no-edit"]);
    assert_eq!(r.json(BOX)["params"]["x"], "15 mm");
    // Outside a merge the check passes at once: ordinary commits pay nothing.
    r.ryucad(&["feature", "edit", "p", "b", "--param", "z=6"]);
    r.commit("an ordinary commit");
}

#[test]
fn the_commit_guard_keeps_release_records_true() {
    for sub in ["", "cad"] {
        let r = boxed(sub);
        r.ryucad(&["git", "install"]);
        r.ryucad(&["props", "set", "p", "number=100"]);
        r.commit("numbered");
        // A proposal on a branch commits.
        r.git(&["checkout", "-q", "-b", "propose"]);
        r.ryucad(&["release", "propose", "p", "--note", "first"]);
        r.commit("propose p A");
        // Editing the proposed part is refused: the record would be stale.
        r.ryucad(&["feature", "edit", "p", "b", "--param", "x=11"]);
        r.git(&["add", "-A"]);
        let (code, out) = r.git_status(&["commit", "-q", "-m", "edit after proposing"]);
        assert_ne!(code, 0, "{out}");
        assert!(out.contains("release_stale"), "{out}");
        assert!(out.contains("--no-verify"), "{out}");
        // Past the hook, check finds it.
        r.git(&["commit", "-q", "--no-verify", "-m", "edit anyway"]);
        let (code, v) = r.ryucad_status(&["check"]);
        assert_eq!(code, 1, "{v}");
        let codes: Vec<&str> = v["error"]["details"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["source"] == "release")
            .map(|f| f["code"].as_str().unwrap())
            .collect();
        assert_eq!(codes, ["release_stale"]);
        // Withdrawn and proposed again, it commits and merges to main.
        fs::remove_file(r.root.join("releases/p/A.json")).unwrap();
        r.ryucad(&["release", "propose", "p"]);
        r.commit("propose p A again");
        r.git(&["checkout", "-q", "main"]);
        r.git(&["merge", "-q", "--no-ff", "--no-edit", "propose"]);
        let listed = r.ryucad(&["release", "list"]);
        assert_eq!(listed["records"][0]["status"], "official", "{listed}");
        // On main the record is official: it may not change, the part may.
        let a = r.root.join("releases/p/A.json");
        let text = fs::read_to_string(&a).unwrap();
        fs::write(&a, text.replace("\"agent\"", "\"someone\"")).unwrap();
        r.git(&["add", "-A"]);
        let (code, out) = r.git_status(&["commit", "-q", "-m", "rewrite A"]);
        assert_ne!(code, 0, "{out}");
        assert!(out.contains("release_modified"), "{out}");
        r.git(&["checkout", "-q", "HEAD", "--", "."]);
        r.ryucad(&["feature", "edit", "p", "b", "--param", "x=12"]);
        r.commit("work towards B");
        let state = r.ryucad(&["props", "show", "p"]);
        assert_eq!(state["state"]["text"], "A modified");
    }
}

/// The record of `p`'s revision A, as listed.
fn record_a(r: &Repo) -> Value {
    r.ryucad(&["release", "list", "p"])["records"][0].clone()
}

#[test]
fn two_branches_proposing_one_label_conflict() {
    let r = boxed("");
    r.ryucad(&["props", "set", "p", "number=100"]);
    r.commit("numbered");
    r.git(&["checkout", "-q", "-b", "longer"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=15"]);
    r.ryucad(&["release", "propose", "p"]);
    r.commit("longer, proposed as A");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=12"]);
    r.ryucad(&["release", "propose", "p"]);
    r.commit("a bit longer, proposed as A");
    let (code, out) = r.git_status(&["merge", "--no-edit", "longer"]);
    assert_ne!(code, 0, "{out}");
    let v = r.ryucad(&["merge", "status"]);
    let files: Vec<&str> = v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["files"][0].as_str().unwrap())
        .collect();
    assert!(files.contains(&"releases/p/A.json"), "{v}");
}

#[test]
fn rewritten_history_moves_official_commits_and_sync_follows() {
    let r = boxed("");
    r.ryucad(&["props", "set", "p", "number=100"]);
    r.ryucad(&["release", "propose", "p"]);
    r.commit("release p A");
    r.ryucad(&["release", "sync"]);
    let before = record_a(&r);
    assert_eq!(before["tag"]["status"], "ok");
    r.git(&["commit", "-q", "--amend", "-m", "release p A, reworded"]);
    let after = record_a(&r);
    assert_ne!(after["commit"], before["commit"]);
    assert_eq!(after["tag"]["status"], "moved");
    assert_eq!(after["tag"]["points_at"], before["commit"]);
    let v = r.ryucad(&["release", "sync"]);
    assert_eq!(v["moved"][0]["to"], after["commit"]);
    assert_eq!(record_a(&r)["tag"]["status"], "ok");
}

#[test]
fn a_clone_reads_releases_from_origin_and_says_when_it_is_shallow() {
    let up = boxed("");
    up.ryucad(&["props", "set", "p", "number=100"]);
    up.commit("numbered");
    let down = up.clone_as("down");
    // Released upstream: official for the clone once it fetches, though
    // its own main hasn't the record yet.
    up.ryucad(&["release", "propose", "p"]);
    up.commit("release p A");
    let listed = down.ryucad(&["release", "list"]);
    assert_eq!(listed["branch"]["name"], "origin/main");
    assert_eq!(listed["records"], json!([]));
    down.git(&["pull", "-q", "--no-rebase"]);
    assert_eq!(record_a(&down)["status"], "official");
    // Proposed downstream and committed on its main, but not pushed: not
    // official, since origin/main decides.
    down.ryucad(&["feature", "edit", "p", "b", "--param", "x=11"]);
    down.ryucad(&["release", "propose", "p"]);
    down.commit("release p B");
    let v = down.ryucad(&["release", "list"]);
    let b = &v["records"][1];
    assert_eq!(
        (&b["label"], &b["status"]),
        (&json!("B"), &json!("proposed"))
    );
    // A shallow clone sees only so far back.
    let shallow = up.dir.path().join("shallow");
    let url = format!("file://{}", up.top.to_str().unwrap().replace('\\', "/"));
    up.git(&[
        "clone",
        "-q",
        "--depth",
        "1",
        &url,
        shallow.to_str().unwrap(),
    ]);
    let s = Repo {
        dir: up.dir.clone(),
        root: shallow.clone(),
        top: shallow,
    };
    let v = s.ryucad(&["release", "list"]);
    assert_eq!(
        (&v["shallow"], &v["records"][0]["shallow"]),
        (&json!(true), &json!(true))
    );
}

#[test]
fn a_pull_is_checked_as_a_merge() {
    let up = boxed("");
    let down = up.clone_as("down");
    // Upstream sizes a sphere by `wall`; downstream deletes the parameter.
    up.ryucad(&[
        "feature",
        "add",
        "p",
        "sphere",
        "--id",
        "s",
        "--param",
        "radius=wall",
    ]);
    up.commit("a sphere as big as the wall");
    down.ryucad(&["param", "delete", "p", "wall"]);
    down.commit("no wall");
    let (code, out) = down.git_status(&["pull", "--no-rebase", "--no-edit"]);
    assert_ne!(code, 0, "{out}");
    assert!(
        out.contains("s.json#merge_broke (merge_broke, ours)"),
        "{out}"
    );
}

#[test]
fn the_hook_refuses_what_a_merge_broke_in_pdm_until_it_is_settled() {
    let r = boxed("");
    r.ryucad(&["git", "install"]);
    r.ryucad(&["part", "create", "q"]);
    r.ryucad(&["part", "create", "s"]);
    r.ryucad(&["props", "set", "p", "number=100"]);
    r.commit("numbered");
    // Theirs proposes p and numbers q; ours makes p's box longer and gives
    // s q's number. Git merges every file cleanly.
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&["release", "propose", "p"]);
    r.ryucad(&["props", "set", "q", "number=300"]);
    r.commit("theirs: propose p A, number q");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["feature", "edit", "p", "b", "--param", "x=12"]);
    r.ryucad(&["props", "set", "s", "number=300"]);
    r.commit("ours: longer, number s");
    let (code, out) = r.git_status(&["merge", "--no-edit", "theirs"]);
    assert_ne!(code, 0, "{out}");
    let stale = "releases/p/A.json#pdm_broke/release_stale";
    let s = "parts/s/properties.json#pdm_broke/part_number_duplicate/number";
    for line in [
        format!("{stale} (pdm_broke, ours)"),
        "parts/q/properties.json#pdm_broke/part_number_duplicate/number (pdm_broke, ours)".into(),
        format!("{s} (pdm_broke, theirs)"),
    ] {
        assert!(out.contains(&line), "{line}: {out}");
    }
    // check sees the same, as release and doctor findings.
    let (code, v) = r.ryucad_status(&["check"]);
    assert_eq!(code, 1, "{v}");
    let codes: Vec<&str> = v["error"]["details"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"release_stale"), "{codes:?}");
    assert!(codes.contains(&"part_number_duplicate"), "{codes:?}");
    // The proposal is withdrawn and s renumbered; the merge commits.
    let v = r.ryucad(&["merge", "status"]);
    let repair = v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == stale)
        .map(|c| c["repairs"][0]["id"].as_str().unwrap().to_owned())
        .expect("the stale proposal");
    r.ryucad(&["merge", "resolve", stale, "--repair", &repair]);
    let v = r.ryucad(&["merge", "resolve", s, "--value", "\"301\""]);
    assert_eq!(v["status"]["clean"], true, "{v}");
    assert!(!r.root.join("releases/p/A.json").exists());
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "--no-edit"]);
    // The merged part is proposed again.
    let v = r.ryucad(&["release", "propose", "p"]);
    assert_eq!(v["record"]["label"], "A");
    assert_eq!(r.json("parts/s/properties.json")["number"], "301");
}

/// The ops for a 20 × 10 mm rectangle with its corner at the origin,
/// every point fixed, as points `p` and lines `l`.
fn rectangle(p: [&str; 4], l: [&str; 4]) -> Vec<Value> {
    let at = [[0, 0], [20, 0], [20, 10], [0, 10]];
    let mut ops = Vec::new();
    for (id, at) in p.iter().zip(at) {
        ops.push(json!({"op": "add", "id": id, "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": id}));
    }
    for k in 0..4 {
        ops.push(json!({"op": "add", "id": l[k], "type": "line",
                        "start": p[k], "end": p[(k + 1) % 4]}));
    }
    ops
}

/// The name of the edge of an `inspect` body between the vertices at `a`
/// and `b` (meters).
fn edge_at(body: &Value, a: [f64; 3], b: [f64; 3]) -> String {
    let at = |p: [f64; 3]| {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-9))
            .map(|v| v["name"].clone())
            .unwrap_or_else(|| panic!("no vertex at {p:?}"))
    };
    let (a, b) = (at(a), at(b));
    body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .expect("an edge from a to b")
}

/// The spike's base: the rectangle `s` extruded 5 mm as `e`, and a hole
/// `h` through it from a sketch `t` on its top, committed on `main`.
fn spike() -> Repo {
    let r = Repo::new("");
    r.ryucad(&["part", "create", "p"]);
    r.ryucad(&["feature", "add", "p", "sketch", "--id", "s"]);
    let ops = json!(rectangle(
        ["p1", "p2", "p3", "p4"],
        ["l1", "l2", "l3", "l4"]
    ))
    .to_string();
    r.ryucad(&["sketch", "edit", "p", "s", &ops]);
    r.ryucad(&[
        "feature", "add", "p", "extrude", "--id", "e", "--sketch", "s", "--length", "5",
    ]);
    r.ryucad(&[
        "feature", "add", "p", "sketch", "--id", "t", "--origin", "0,0,5",
    ]);
    let ops = r#"[{"op": "add", "type": "point", "at": [4, 6]}, {"op": "add", "type": "fix", "a": "p1"}]"#;
    r.ryucad(&["sketch", "edit", "p", "t", ops]);
    r.ryucad(&[
        "feature",
        "add",
        "p",
        "hole",
        "--id",
        "h",
        "--sketch",
        "t",
        "--diameter",
        "3",
        "--depth",
        "through_all",
    ]);
    r.commit("a drilled block");
    r
}

/// Ours (`main`) rounds the vertical edge at the rectangle's corner `p3`
/// by 1 mm as fillet `f`, and commits; returns that edge's name.
fn round_the_corner(r: &Repo) -> String {
    let e = r.ryucad(&["inspect", "p", "e"]);
    let corner = edge_at(&e["bodies"][0], [0.02, 0.01, 0.0], [0.02, 0.01, 0.005]);
    r.ryucad(&[
        "feature",
        "add",
        "p",
        "fillet",
        "--id",
        "f",
        "--radius",
        "1",
        "--edge",
        &format!("p/e/{corner}"),
    ]);
    r.commit("ours: the corner rounded");
    corner
}

#[test]
fn a_notch_merged_in_keeps_the_fillet_on_its_corner() {
    let r = spike();
    // Theirs notches the rectangle's bottom.
    r.git(&["checkout", "-q", "-b", "theirs"]);
    let mut ops = vec![json!({"op": "remove", "id": "l1"})];
    for (id, at) in [
        ("p5", [6, 0]),
        ("p6", [6, 3]),
        ("p7", [12, 3]),
        ("p8", [12, 0]),
    ] {
        ops.push(json!({"op": "add", "id": id, "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": id}));
    }
    for (id, a, b) in [
        ("l1", "p1", "p5"),
        ("l5", "p5", "p6"),
        ("l6", "p6", "p7"),
        ("l7", "p7", "p8"),
        ("l8", "p8", "p2"),
    ] {
        ops.push(json!({"op": "add", "id": id, "type": "line", "start": a, "end": b}));
    }
    r.ryucad(&["sketch", "edit", "p", "s", &json!(ops).to_string()]);
    r.commit("theirs: a notch");
    // Ours rounds the corner, against the names before the notch.
    r.git(&["checkout", "-q", "main"]);
    let corner = round_the_corner(&r);
    let fillet = |r: &Repo| {
        let v = r.ryucad(&["inspect", "p"]);
        v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["origin"]["feature"] == "f")
            .cloned()
            .expect("the fillet's face")
    };
    let before = fillet(&r);
    r.git(&["merge", "-q", "--no-edit", "theirs"]);
    // The merge has the notch and the fillet, on the same corner.
    assert_eq!(
        r.json("parts/p/features/s.json")["entities"]["l8"]["end"],
        "p2"
    );
    r.ryucad(&["check"]);
    let e = r.ryucad(&["inspect", "p", "e"]);
    assert_eq!(
        edge_at(&e["bodies"][0], [0.02, 0.01, 0.0], [0.02, 0.01, 0.005]),
        corner
    );
    let after = fillet(&r);
    assert_eq!(after["name"], before["name"]);
    assert_eq!(after["origin"], before["origin"]);
    assert_eq!(after["surface"], before["surface"]);
}

#[test]
fn a_redraw_merged_in_fails_the_fillet_and_is_blamed() {
    let r = spike();
    // Theirs draws the rectangle again, where it was, with new IDs.
    r.git(&["checkout", "-q", "-b", "theirs"]);
    let mut ops: Vec<Value> = ["l1", "l2", "l3", "l4", "p1", "p2", "p3", "p4"]
        .iter()
        .map(|id| json!({"op": "remove", "id": id}))
        .collect();
    ops.extend(rectangle(
        ["p5", "p6", "p7", "p8"],
        ["l5", "l6", "l7", "l8"],
    ));
    r.ryucad(&["sketch", "edit", "p", "s", &json!(ops).to_string()]);
    r.commit("theirs: the rectangle drawn again");
    r.git(&["checkout", "-q", "main"]);
    let corner = round_the_corner(&r);
    // Git merges the files cleanly; only rebuilding shows the break.
    r.git(&["merge", "-q", "--no-commit", "--no-ff", "theirs"]);
    let v = r.ryucad(&["merge", "status"]);
    let c: Vec<&Value> = v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "merge_broke")
        .collect();
    assert_eq!(c.len(), 1, "{v}");
    assert_eq!(c[0]["concerns"]["feature"], "f");
    assert_eq!(c[0]["cause"], "theirs", "{v}");
    let e = &c[0]["error"];
    assert_eq!(e["code"], "reference_unresolved", "{e}");
    assert_eq!(e["details"]["reference"], format!("p/e/{corner}"));
    assert_eq!(e["details"]["role"], "groups.0.edges.0");
    // What the name was, from the sides that built it, and that the
    // extrude no longer makes it.
    assert_eq!(
        e["details"]["was"],
        json!({"kind": "edge",
               "from": {"feature": "e", "sketch": "s", "entities": ["l2", "l3"]}})
    );
    assert_eq!(e["details"]["removed_by"], "e");
    // Nothing lands on another edge: the rebuild fails the fillet alone.
    let (code, v) = r.ryucad_status(&["rebuild", "p"]);
    assert_eq!(code, 1, "{v}");
    let failed: Vec<&Value> = v["error"]["details"]["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f.get("error").is_some())
        .collect();
    assert_eq!(failed.len(), 1, "{v}");
    assert_eq!(failed[0]["id"], "f");
}

/// The planar face of solid `solid` of `part` whose outward normal is `n`.
fn face_along(r: &Repo, part: &str, solid: &str, n: [f64; 3]) -> String {
    let v = r.ryucad(&["inspect", part, solid]);
    v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let z = &f["surface"]["frame"]["z"];
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && (0..3).all(|k| (z[k].as_f64().unwrap() * sign - n[k]).abs() < 1e-12)
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .expect("a face along n")
}

/// An assembly `frame`: a plate fixed, and a bolt stood on its top by a
/// revolute 20 mm along, its pose solved and stored.
fn hinged() -> Repo {
    let r = Repo::new("");
    r.ryucad(&["part", "create", "plate"]);
    r.ryucad(&[
        "feature", "add", "plate", "box", "--id", "slab", "--param", "x=100", "--param", "y=50",
        "--param", "z=5",
    ]);
    r.ryucad(&["part", "create", "bolt"]);
    r.ryucad(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    r.ryucad(&["part", "create", "frame", "--assembly"]);
    r.ryucad(&[
        "feature", "add", "frame", "instance", "--id", "base", "--of", "plate", "--fixed",
    ]);
    r.ryucad(&[
        "feature", "add", "frame", "instance", "--id", "pin", "--of", "bolt", "--origin",
        "50mm,0,0",
    ]);
    let top = face_along(&r, "plate", "slab", [0.0, 0.0, 1.0]);
    let foot = face_along(&r, "bolt", "shank", [0.0, 0.0, -1.0]);
    let v = r.ryucad(&[
        "feature",
        "add",
        "frame",
        "mate",
        "--id",
        "m1",
        "--form",
        "revolute",
        "--a",
        &format!("frame/base/slab/{top}"),
        "--a-offset",
        r#"{"origin": ["20 mm", "0 mm", "0 mm"]}"#,
        "--b",
        &format!("frame/pin/shank/{foot}"),
        "--b-offset",
        r#"{"flip": true}"#,
    ]);
    assert_eq!(v["assemblies"][0]["moved"], json!(["pin"]), "{v}");
    r.commit("a hinge");
    r
}

#[test]
fn a_merge_that_leaves_an_assembly_stale_says_so_and_repairs_it() {
    let r = hinged();
    // Theirs moves the hinge along; ours makes the plate thicker. Each
    // keeps the assembly current: the pin's pose is written both ways.
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&[
        "feature",
        "edit",
        "frame",
        "m1",
        "--a-offset",
        r#"{"origin": ["30 mm", "0 mm", "0 mm"]}"#,
    ]);
    r.commit("theirs: the hinge moved along");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["feature", "edit", "plate", "slab", "--param", "z=8"]);
    r.commit("ours: a thicker plate");
    // The pin's pose merges as solver state, ours: not where theirs'
    // hinge holds it.
    r.git(&["merge", "-q", "--no-commit", "--no-ff", "theirs"]);
    let v = r.ryucad(&["merge", "status"]);
    let c: Vec<&Value> = v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "assembly_stale")
        .collect();
    assert_eq!(c.len(), 1, "{v}");
    assert_eq!(c[0]["files"], json!(["parts/frame/features/pin.json"]));
    let id = c[0]["id"].as_str().unwrap().to_owned();
    let repair = c[0]["repairs"][0]["id"].as_str().unwrap().to_owned();
    r.ryucad(&["merge", "resolve", &id, "--repair", &repair]);
    let pin = r.json("parts/frame/features/pin.json");
    let o = &pin["pose"]["origin_m"];
    assert!((o[0].as_f64().unwrap() - 0.03).abs() < 1e-9, "{o}");
    assert!((o[2].as_f64().unwrap() - 0.008).abs() < 1e-9, "{o}");
    r.git(&["add", "-A"]);
    let v = r.ryucad(&["merge", "status"]);
    assert!(
        !v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "assembly_stale"),
        "{v}"
    );
    r.ryucad(&["check"]);
}

/// OCCT's box, `h` mm tall, written in the test's folder as an agent
/// would have it.
fn box_step(r: &Repo, h: u32) -> String {
    let path = format!(
        "{}/../ryustep/tests/data/occt/box_ap214.stp",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = fs::read_to_string(path)
        .unwrap()
        .replace(",30.))", &format!(",{h}.))"))
        .replace("(30.,0.)", &format!("({h}.,0.)"));
    let at = r.dir.path().join(format!("box{h}.step"));
    fs::write(&at, text).unwrap();
    at.to_str().unwrap().to_owned()
}

const BOX_STEP: &str = "imports/box30.step";
const BOX_IMPORT: &str = "parts/b/features/body.json";

/// The box imported on `main`, then imported again 50 mm tall on
/// `theirs` and 40 mm tall on `main`, and `theirs` merged in.
fn reimported_both_ways() -> Repo {
    let r = Repo::new("");
    r.ryucad(&["import", &box_step(&r, 30), "--part", "b"]);
    r.commit("a box");
    r.git(&["checkout", "-q", "-b", "theirs"]);
    r.ryucad(&["import", &box_step(&r, 50), "--update", BOX_STEP]);
    r.commit("50 mm");
    r.git(&["checkout", "-q", "main"]);
    r.ryucad(&["import", &box_step(&r, 40), "--update", BOX_STEP]);
    r.commit("40 mm");
    let (code, out) = r.git_status(&["merge", "--no-edit", "theirs"]);
    assert_eq!(code, 1, "{out}");
    r
}

/// `merge status`'s conflicts, by ID, with their kinds.
fn conflicts(r: &Repo) -> Vec<(String, String)> {
    let v = r.ryucad(&["merge", "status"]);
    v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["id"].as_str().unwrap().to_owned(),
                c["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn two_reimports_conflict_on_bytes_and_hashes_and_settle_either_way() {
    for side in ["--ours", "--theirs"] {
        let r = reimported_both_ways();
        // Git leaves the bytes unmerged; the driver, the hash.
        let porcelain = r.git(&["status", "--porcelain"]);
        assert!(porcelain.contains(&format!("UU {BOX_STEP}")), "{porcelain}");
        let mut found = conflicts(&r);
        found.sort();
        assert_eq!(
            found,
            [
                (format!("{BOX_STEP}#"), "merge_unparsed".to_owned()),
                (format!("{BOX_IMPORT}#/file/blake3"), "value".to_owned()),
            ]
        );
        let v = r.ryucad(&["merge", "status"]);
        let bytes = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == format!("{BOX_STEP}#"))
            .unwrap()
            .clone();
        assert!(bytes["ours"]["blake3"].is_string(), "{bytes}");
        // The bytes one way and the hash the other: the merge broke the
        // import, and says so.
        let other = if side == "--ours" {
            "--theirs"
        } else {
            "--ours"
        };
        r.ryucad(&["merge", "resolve", &format!("{BOX_STEP}#"), other]);
        r.ryucad(&[
            "merge",
            "resolve",
            &format!("{BOX_IMPORT}#/file/blake3"),
            side,
        ]);
        let broke: Vec<(String, String)> = conflicts(&r)
            .into_iter()
            .filter(|(_, k)| k == "merge_broke")
            .collect();
        assert_eq!(
            broke,
            [(
                format!("{BOX_IMPORT}#merge_broke"),
                "merge_broke".to_owned()
            )],
            "{side}"
        );
        let v = r.ryucad(&["merge", "status"]);
        let c = v["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["kind"] == "merge_broke")
            .unwrap()
            .clone();
        assert_eq!(c["error"]["code"], "import_file_changed", "{c}");
        // Both sides changed the bytes it reads.
        assert_eq!(c["cause"], "both", "{c}");
        assert!(
            c["files"].as_array().unwrap().contains(&json!(BOX_STEP)),
            "{c}"
        );
        // The bytes the same way as the hash: settled.
        r.git(&["checkout", side, "--", BOX_STEP]);
        r.git(&["add", "-A"]);
        assert_eq!(conflicts(&r), [], "{side}");
        let built = r.ryucad(&["rebuild", "b"]);
        assert_eq!(built["parts"][0]["failed"], 0, "{side}: {built}");
        let want = blake3::hash(&fs::read(r.root.join(BOX_STEP)).unwrap())
            .to_hex()
            .to_string();
        assert_eq!(r.json(BOX_IMPORT)["file"]["blake3"], want, "{side}");
        r.git(&["commit", "-q", "--no-edit"]);
    }
}
