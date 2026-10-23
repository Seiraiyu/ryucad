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
            .env("RYUCAD_NO_DAEMON", "1")
            .env("RYUCAD_CONFIG_DIR", self.dir.path().join("config"))
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
