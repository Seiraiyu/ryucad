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
use ryupdm::record::{BuiltWith, Obsolete, Record, Release, ReleasedBy, revising};
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

/// The release of `part` as the files in `dir` have it.
fn release_of(dir: &Path, part: &str, label: &str, supersedes: Option<&str>) -> Record {
    let src = Folder(dir);
    let p = Id::parse(part, "part").unwrap();
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

/// Commits `part`'s release `label` of the files in `w`'s `cad/`.
fn released(w: &Path, part: &str, label: &str, supersedes: Option<&str>) -> String {
    let r = release_of(&w.join("cad"), part, label, supersedes);
    put(w, &format!("cad/{}", r.path()), &r.to_json());
    commit(w, &format!("release {part} {label}"))
}

#[test]
fn boms_and_where_used_read_released_assemblies_at_their_commits() {
    use ryucad_doc::doc::assembly::Rev;
    use ryupdm::structure::{Cache, Reader, Repo};
    use ryupdm::where_used::{Query, where_used};
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
        "cad/parts/pin/part.json",
        &json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
    );
    let pin = |w: &Path, len: &str, number: &str| {
        put(
            w,
            "cad/parts/pin/features/a.json",
            &json!({"format": 1, "kind": "ryucad.feature", "type": "cylinder",
                    "radius": "2", "height": len}),
        );
        put(
            w,
            "cad/parts/pin/properties.json",
            &json!({"format": 1, "kind": "ryupdm.properties", "number": number}),
        );
    };
    let asm = |w: &Path, label: &str, n: usize| {
        let ids: Vec<String> = (1..=n).map(|i| format!("p{i}")).collect();
        put(
            w,
            "cad/parts/asm/part.json",
            &json!({"features": ids, "format": 2, "kind": "ryucad.part",
                    "part_kind": "assembly"}),
        );
        for id in &ids {
            put(
                w,
                &format!("cad/parts/asm/features/{id}.json"),
                &json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                        "part": "pin", "rev": {"label": label},
                        "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            );
        }
        put(
            w,
            "cad/parts/asm/properties.json",
            &json!({"format": 1, "kind": "ryupdm.properties", "number": "900"}),
        );
    };
    pin(w, "10", "100");
    commit(w, "pin");
    let pin_a = released(w, "pin", "A", None);
    asm(w, "A", 2);
    commit(w, "asm");
    let asm_a = released(w, "asm", "A", None);
    pin(w, "12", "100");
    commit(w, "longer pin");
    released(w, "pin", "B", Some("A"));
    asm(w, "B", 3);
    commit(w, "asm on pin B");
    let asm_b = released(w, "asm", "B", Some("A"));
    // Work goes on: the working assembly places four pins at B.
    asm(w, "B", 4);
    commit(w, "a fourth pin");
    // asm's latest release, B, is made obsolete: still read where it was
    // released.
    let gone = Record::Obsolete(Obsolete {
        project: ProjectId::parse(PROJECT).unwrap(),
        part: Id::parse("asm", "part").unwrap(),
        label: "B".into(),
        released_by: ReleasedBy {
            name: None,
            email: None,
            participant: "agent".into(),
        },
        note: None,
    });
    put(w, &format!("cad/{}", gone.path()), &gone.to_json());
    commit(w, "asm B obsolete");

    let bare = tempfile::tempdir().unwrap();
    git(
        bare.path(),
        &["clone", "-q", "--bare", w.to_str().unwrap(), "."],
    );
    let repo = gix::open(bare.path()).unwrap();
    let src = history::files(&repo, "main", "cad/").unwrap();
    let project = Project::load_from(Path::new("/tip"), &src).unwrap();
    let config = Config::read(&src).unwrap();
    let releases = Releases::read(&repo, "cad/", &config).unwrap();
    let mut cache = Cache::new();
    let mut reader = Reader::new(
        &src,
        &project,
        Some(Repo {
            git: &repo,
            prefix: "cad/",
        }),
        Some(&releases),
        &mut cache,
    );
    let id = |s: &str| Id::parse(s, "part").unwrap();
    // The BOM of asm's release A, read where it became official: two pins
    // at A, read where pin A did.
    let root = ryupdm::bom::tree(&mut reader, &id("asm"), &Rev::Label("A".into()), None).unwrap();
    assert_eq!(
        root.node.as_ref().unwrap().commit.as_deref(),
        Some(asm_a.as_str())
    );
    let pins = &root.children[0];
    assert_eq!((pins.count, &pins.rev), (2, &Rev::Label("A".into())));
    assert_eq!(
        pins.node.as_ref().unwrap().commit.as_deref(),
        Some(pin_a.as_str())
    );
    assert!(!root.node.as_ref().unwrap().obsolete);
    // Release B's record is obsolete, and its BOM says so of the root alone.
    let b = ryupdm::bom::tree(&mut reader, &id("asm"), &Rev::Label("B".into()), None).unwrap();
    let v = ryupdm::bom::row_json(&b, &BTreeMap::new());
    assert_eq!(
        (&v["obsolete"], &v["commit"]),
        (&json!(true), &json!(asm_b))
    );
    assert!(v["children"][0].get("obsolete").is_none());
    // Where pin is used: the working assembly, and both releases.
    let q = Query {
        part: id("pin"),
        rev: None,
        direct: false,
    };
    let v = where_used(&mut reader, &q, &config);
    assert_eq!(v["working"][0]["count"], 4);
    assert_eq!(v["working"][0]["uses"][0]["rev"], json!({"label": "B"}));
    let released: Vec<(Value, Value, Value)> = v["released"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["label"].clone(), r["state"].clone(), r["count"].clone()))
        .collect();
    assert_eq!(
        released,
        [
            (json!("A"), json!("superseded"), json!(2)),
            (json!("B"), json!("obsolete"), json!(3)),
        ]
    );
    assert_eq!(v["released"][1]["commit"], asm_b.as_str());
    // Only pin A: asm's release A.
    let q = Query {
        rev: Some(Rev::Label("A".into())),
        ..q
    };
    let v = where_used(&mut reader, &q, &config);
    assert_eq!(v["working"], json!([]));
    assert_eq!(v["released"].as_array().unwrap().len(), 1);
}

/// Design 180's bar: where-used over 100 official releases of an
/// assembly answers in under a second cold, and in under 10 ms with the
/// parts at their commits already read. Release builds only.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build")]
fn where_used_over_a_hundred_released_assemblies_is_quick() {
    use ryucad_doc::doc::assembly::Rev;
    use ryupdm::structure::{Cache, Reader, Repo};
    use ryupdm::where_used::{Query, where_used};
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
        "cad/parts/pin/part.json",
        &json!({"features": [], "format": 1, "kind": "ryucad.part"}),
    );
    put(
        w,
        "cad/parts/pin/properties.json",
        &json!({"format": 1, "kind": "ryupdm.properties", "number": "100"}),
    );
    commit(w, "pin");
    released(w, "pin", "A", None);
    let scheme = ryupdm::config::Scheme::Letters;
    let mut last: Option<String> = None;
    for n in 0..100u64 {
        let ids: Vec<String> = (0..=n % 7).map(|i| format!("p{i}")).collect();
        put(
            w,
            "cad/parts/asm/part.json",
            &json!({"features": ids, "format": 2, "kind": "ryucad.part",
                    "part_kind": "assembly"}),
        );
        for id in &ids {
            put(
                w,
                &format!("cad/parts/asm/features/{id}.json"),
                &json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                        "part": "pin", "rev": {"label": "A"},
                        "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            );
        }
        put(
            w,
            "cad/parts/asm/properties.json",
            &json!({"format": 1, "kind": "ryupdm.properties", "number": "900"}),
        );
        commit(w, &format!("asm {n}"));
        let label = ryupdm::label::label(scheme, n + 1);
        released(w, "asm", &label, last.as_deref());
        last = Some(label);
    }
    let repo = gix::open(w).unwrap();
    let src = history::files(&repo, "main", "cad/").unwrap();
    let project = Project::load_from(Path::new("/tip"), &src).unwrap();
    let config = Config::read(&src).unwrap();
    let releases = Releases::read(&repo, "cad/", &config).unwrap();
    let mut cache = Cache::new();
    let q = Query {
        part: Id::parse("pin", "part").unwrap(),
        rev: Some(Rev::Label("A".into())),
        direct: false,
    };
    let ask = |cache: &mut Cache| {
        let mut reader = Reader::new(
            &src,
            &project,
            Some(Repo {
                git: &repo,
                prefix: "cad/",
            }),
            Some(&releases),
            cache,
        );
        let t = std::time::Instant::now();
        let v = where_used(&mut reader, &q, &config);
        (t.elapsed().as_secs_f64() * 1e3, v)
    };
    let (cold, v) = ask(&mut cache);
    let (warm, _) = ask(&mut cache);
    assert_eq!(v["released"].as_array().unwrap().len(), 100);
    assert_eq!(v["released"][99]["state"], "released");
    assert_eq!(v["released"][0]["state"], "superseded");
    println!("where-used over 100 released assemblies: cold {cold:.1} ms, read {warm:.1} ms");
    let slack = if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    };
    assert!(cold < 1000.0 * slack, "{cold} ms");
    assert!(warm < 10.0 * slack, "{warm} ms");
}
