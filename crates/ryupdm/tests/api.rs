//! ryupdm's public API as a service links it: every function driven from a
//! bare repository at a pinned commit, with no work tree, through files in
//! memory. The commit guard needs Git's index, so it runs in the clone the
//! repository was made from; merge validation reads a merge's sides from
//! the bare repository's commits.

use ryucad_doc::doc::canonical;
use ryucad_doc::doc::id::{Id, ProjectId};
use ryucad_doc::doc::project::Project;
use ryucad_doc::doc::source::Folder;
use ryupdm::check::{self, History};
use ryupdm::config::Config;
use ryupdm::history::{self, Releases};
use ryupdm::props::Pdm;
use ryupdm::record::{BuiltWith, Record, Release, ReleasedBy, revising};
use ryupdm::state;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const PROJECT: &str = "0123456789abcdef0123456789abcdef";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn put(dir: &Path, rel: &str, v: &Value) {
    let path = rel.split('/').fold(dir.to_path_buf(), |p, s| p.join(s));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, canonical::to_string(v)).unwrap();
}

fn commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

/// The release of `p` as the files in `dir` have it, labelled A.
fn release_a(dir: &Path) -> Record {
    release(dir, "A", None)
}

/// The release of `p` as the files in `dir` have it.
fn release(dir: &Path, label: &str, supersedes: Option<&str>) -> Record {
    let src = Folder(dir);
    let p = Id::parse("p", "part").unwrap();
    let config = Config::read(&src).unwrap();
    Record::Release(Release {
        project: ProjectId::parse(PROJECT).unwrap(),
        part: p.clone(),
        label: label.into(),
        content: ryupdm::content::hash(&src, &p).unwrap(),
        properties: revising(&config, &ryupdm::props::read(&src, &p).unwrap()),
        results: BTreeMap::new(),
        built_with: BuiltWith {
            ryucad: "0".into(),
            ryukei: "0".into(),
            commit: "c".into(),
        },
        released_by: ReleasedBy {
            name: None,
            email: None,
            participant: "agent".into(),
        },
        supersedes: supersedes.map(str::to_owned),
        note: None,
    })
}

#[test]
fn the_library_reads_a_bare_repository_at_a_pinned_commit() {
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    git(w, &["init", "-q", "-b", "main"]);
    put(
        w,
        "cad/ryucad.json",
        &json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project",
                "project": PROJECT}),
    );
    put(
        w,
        "cad/parts/p/part.json",
        &json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
    );
    put(w, "cad/parts/p/features/a.json", &json!({"x": 1}));
    put(
        w,
        "cad/parts/p/properties.json",
        &json!({"format": 1, "kind": "ryupdm.properties", "number": "100"}),
    );
    commit(w, "start");
    git(w, &["switch", "-q", "-c", "propose"]);
    let a = release_a(&w.join("cad"));
    put(w, &format!("cad/{}", a.path()), &a.to_json());
    commit(w, "propose p A");
    git(w, &["switch", "-q", "main"]);
    git(w, &["merge", "-q", "--no-ff", "-m", "merge", "propose"]);
    let official = git(w, &["rev-parse", "HEAD"]);
    git(w, &["tag", "ryupdm/p/A", &official]);
    // A branch renumbers p and proposes it as B.
    git(w, &["switch", "-q", "-c", "renumber"]);
    put(
        w,
        "cad/parts/p/properties.json",
        &json!({"format": 1, "kind": "ryupdm.properties", "number": "101"}),
    );
    let b = release(&w.join("cad"), "B", Some("A"));
    put(w, &format!("cad/{}", b.path()), &b.to_json());
    commit(w, "propose p B");
    git(w, &["switch", "-q", "main"]);
    // Work goes on after the pinned commit.
    put(w, "cad/parts/p/features/a.json", &json!({"x": 2}));
    commit(w, "later");

    let bare = tempfile::tempdir().unwrap();
    git(
        bare.path(),
        &["clone", "-q", "--bare", w.to_str().unwrap(), "."],
    );
    let repo = gix::open(bare.path()).unwrap();
    assert!(repo.workdir().is_none());
    let src = history::files(&repo, &official, "cad/").unwrap();
    let p = Id::parse("p", "part").unwrap();
    let Record::Release(ra) = &a else {
        unreachable!()
    };

    // content::hash
    assert_eq!(ryupdm::content::hash(&src, &p).unwrap(), ra.content);
    // Properties judged across the project.
    let project = Project::load_from(Path::new("/pinned"), &src).unwrap();
    assert!(Pdm::read(&src, &project).faults().is_empty());
    // history::Releases::read
    let config = Config::read(&src).unwrap();
    let releases = Releases::read(&repo, "cad/", &config).unwrap();
    assert_eq!(releases.official["releases/p/A.json"].commit, official);
    // state::derive, at the pinned commit and at the tip.
    assert_eq!(state::derive(&src, &p, &releases).text(), "released A");
    let tip = history::files(&repo, "main", "cad/").unwrap();
    assert_eq!(state::derive(&tip, &p, &releases).text(), "A modified");
    // check::run
    let h = History {
        repo: &repo,
        prefix: "cad/",
        releases: &releases,
    };
    let found = check::run(&src, Some(h), None).unwrap();
    assert!(found.is_empty(), "{found:?}");
    // merge::validate: main's work merged with the branch's proposal.
    let theirs = history::files(&repo, "renumber", "cad/").unwrap();
    let mut merged = tip.clone();
    for path in ["parts/p/properties.json", "releases/p/B.json"] {
        merged.insert(path.to_owned(), theirs[path].clone());
    }
    let broke = ryupdm::merge::validate(&src, &tip, &theirs, &merged, &releases);
    assert_eq!(broke.len(), 1, "{broke:?}");
    assert_eq!(
        (
            broke[0].code.as_str(),
            broke[0].cause.as_str(),
            broke[0].withdraw
        ),
        ("release_stale", "ours", true)
    );
    assert_eq!(
        broke[0].files,
        ["releases/p/B.json", "parts/p/features/a.json"]
    );

    // check::staged needs an index: the clone it was made from.
    put(w, "cad/parts/p/features/a.json", &json!({"x": 3}));
    git(w, &["add", "-A"]);
    let work_repo = gix::open(w).unwrap();
    assert!(check::staged(&work_repo, "cad/").unwrap().is_empty());
    let mut changed = a.to_json();
    changed["note"] = json!("an afterthought");
    put(w, &format!("cad/{}", a.path()), &changed);
    git(w, &["add", "-A"]);
    let found = check::staged(&work_repo, "cad/").unwrap();
    assert_eq!(found[0].code, "release_modified");
    assert_eq!(found[0].to_json()["source"], "release");
}
