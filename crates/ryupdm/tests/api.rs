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
    git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
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
        project: None,
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
        project: None,
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
/// An annotated tag whose message is exactly `message`.
fn annotated(dir: &Path, name: &str, commit: &str, message: &str) {
    let file = dir.join(".git/TAG_MSG_TEST");
    std::fs::write(&file, message).unwrap();
    git(
        dir,
        &[
            "tag",
            "-a",
            "--cleanup=verbatim",
            "-F",
            file.to_str().unwrap(),
            name,
            commit,
        ],
    );
}

/// The service's attestation of the record at `rel` official at `commit`,
/// approved by alice, proposed by dana. The record is hashed as committed,
/// as the service and a clone hash it: a work tree's copy may differ, as
/// Git with `core.autocrlf` checks it out with CRLF (#289).
fn attested(
    dir: &Path,
    rel: &str,
    part: &str,
    label: &str,
    kind: ryupdm::record::Kind,
    commit: &str,
    outcome: ryupdm::attest::Outcome,
) -> String {
    attested_with(dir, rel, part, label, kind, commit, outcome, |_| {})
}

/// [`attested`], changed by `change` before it is signed.
#[allow(clippy::too_many_arguments)]
fn attested_with(
    dir: &Path,
    rel: &str,
    part: &str,
    label: &str,
    kind: ryupdm::record::Kind,
    commit: &str,
    outcome: ryupdm::attest::Outcome,
    change: impl Fn(&mut ryupdm::attest::Attestation),
) -> String {
    use ryupdm::attest::*;
    let out = Command::new("git")
        .args(["cat-file", "blob", &format!("{commit}:{rel}")])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "no {rel} at {commit}");
    let bytes = out.stdout;
    let rules = vec![ryupdm::approvals::Rule {
        parts: vec!["*".into()],
        count: 1,
        from: vec!["alice".into()],
    }];
    let approved = outcome == Outcome::Approved;
    let a = Attestation {
        approvals: if approved {
            vec![Approval {
                at: "2026-11-02T09:14:03Z".into(),
                commit: commit.into(),
                github: User {
                    id: 1,
                    login: "alice".into(),
                },
                review: 1,
                via: vec![],
            }]
        } else {
            vec![]
        },
        commit: commit.into(),
        label: label.into(),
        missing: if approved {
            vec![]
        } else {
            vec![ryupdm::approvals::Unmet {
                rule: rules[0].clone(),
                needed: 1,
            }]
        },
        outcome,
        part: Id::parse(part, "part").unwrap(),
        project: ProjectId::parse(PROJECT).unwrap(),
        proposed_by: User {
            id: 2,
            login: "dana".into(),
        },
        pull: approved.then(|| Pull {
            number: 1,
            repo: "acme/widgets".into(),
        }),
        record: blake3::hash(&bytes).to_hex().to_string(),
        record_kind: kind,
        rules,
        rules_at: git(dir, &["rev-parse", &format!("{commit}^")]),
        signed_at: "2026-11-02T09:20:11Z".into(),
    };
    let mut a = a;
    change(&mut a);
    sign(&a, &TEST_SEED)
}

fn statuses(dir: &Path) -> BTreeMap<String, String> {
    let repo = gix::open(dir).unwrap();
    let src = Folder(dir);
    let r = Releases::read(&repo, "", &Config::read(&src).unwrap()).unwrap();
    r.approvals
        .iter()
        .map(|(p, a)| {
            (
                p.clone(),
                a.verdict
                    .as_ref()
                    .map_or("moved", |v| v.status())
                    .to_owned(),
            )
        })
        .collect()
}

#[test]
fn approvals_verify_offline_in_any_clone() {
    use ryupdm::attest::{Outcome, TEST_SEED, hex, public_key, test_key_id};
    use ryupdm::record::Kind;
    for how in ["merge", "squash", "rebase"] {
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        git(w, &["init", "-q", "-b", "main"]);
        put(
            w,
            "ryucad.json",
            &json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project", "project": PROJECT}),
        );
        put(
            w,
            "parts/p/part.json",
            &json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
        );
        put(w, "parts/p/features/a.json", &json!({"x": 1}));
        put(
            w,
            "parts/p/properties.json",
            &json!({"format": 1, "kind": "ryupdm.properties", "number": "100"}),
        );
        put(
            w,
            "ryupdm.json",
            &json!({"approvals": {"keys": {test_key_id(): hex(&public_key(&TEST_SEED))},
             "rules": [{"count": 1, "from": ["alice"], "parts": ["*"]}]}, "format": 2, "kind": "ryupdm.config"}),
        );
        commit(w, "start, with rules");
        git(w, &["switch", "-q", "-c", "propose"]);
        let a = release_a(w);
        put(w, &a.path(), &a.to_json());
        commit(w, "propose p A");
        git(w, &["switch", "-q", "main"]);
        commit(w, "main moves on");
        match how {
            "merge" => {
                git(w, &["merge", "-q", "--no-ff", "-m", "merge", "propose"]);
            }
            "squash" => {
                git(w, &["merge", "-q", "--squash", "propose"]);
                commit(w, "squashed");
            }
            _ => {
                git(w, &["switch", "-q", "propose"]);
                git(w, &["rebase", "-q", "main"]);
                git(w, &["switch", "-q", "main"]);
                git(w, &["merge", "-q", "--ff-only", "propose"]);
            }
        }
        let official = git(w, &["rev-parse", "HEAD"]);
        let rel = "releases/p/A.json";
        assert_eq!(statuses(w)[rel], "missing", "{how}");
        annotated(
            w,
            "ryupdm/p/A",
            &official,
            &attested(
                w,
                rel,
                "p",
                "A",
                Kind::Release,
                &official,
                Outcome::Approved,
            ),
        );
        assert_eq!(statuses(w)[rel], "approved", "{how}");

        // A bare clone verifies it too, with no service and no work tree;
        // one that fetched no tags is told it's missing, never failed.
        let bare = tempfile::tempdir().unwrap();
        git(
            bare.path(),
            &["clone", "-q", "--bare", w.to_str().unwrap(), "."],
        );
        assert_eq!(statuses(bare.path())[rel], "approved", "{how}");
        let notags = tempfile::tempdir().unwrap();
        git(
            notags.path(),
            &["clone", "-q", "--no-tags", w.to_str().unwrap(), "."],
        );
        assert_eq!(statuses(notags.path())[rel], "missing", "{how}");
    }
}

#[test]
fn forged_moved_lightweight_and_unapproved_tags_are_told_apart() {
    use ryupdm::attest::{Outcome, TEST_SEED, hex, public_key, test_key_id};
    use ryupdm::record::Kind;
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    git(w, &["init", "-q", "-b", "main"]);
    put(
        w,
        "ryucad.json",
        &json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project", "project": PROJECT}),
    );
    put(
        w,
        "parts/p/part.json",
        &json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
    );
    put(w, "parts/p/features/a.json", &json!({"x": 1}));
    put(
        w,
        "parts/p/properties.json",
        &json!({"format": 1, "kind": "ryupdm.properties", "number": "100"}),
    );
    commit(w, "start, no rules");
    // A record without rules is judged as before.
    let a = release_a(w);
    put(w, &a.path(), &a.to_json());
    commit(w, "release A, no rules");
    assert!(statuses(w).is_empty());
    // Rules arrive; B arrives after them.
    put(
        w,
        "ryupdm.json",
        &json!({"approvals": {"keys": {test_key_id(): hex(&public_key(&TEST_SEED))},
         "rules": [{"count": 1, "from": ["alice"], "parts": ["*"]}]}, "format": 2, "kind": "ryupdm.config"}),
    );
    commit(w, "rules");
    put(w, "parts/p/features/a.json", &json!({"x": 2}));
    let b = release(w, "B", Some("A"));
    put(w, &b.path(), &b.to_json());
    let official = commit(w, "release B");
    let rel = "releases/p/B.json";
    let check = |message: String| {
        git(w, &["tag", "-d", "ryupdm/p/B"]);
        annotated(w, "ryupdm/p/B", &official, &message);
        statuses(w)[rel].clone()
    };
    annotated(
        w,
        "ryupdm/p/B",
        &official,
        &attested(
            w,
            rel,
            "p",
            "B",
            Kind::Release,
            &official,
            Outcome::Unapproved,
        ),
    );
    assert_eq!(statuses(w)[rel], "unapproved");
    // Forged: someone edits the approved message without the key.
    let forged = attested(
        w,
        rel,
        "p",
        "B",
        Kind::Release,
        &official,
        Outcome::Approved,
    )
    .replacen("\"alice\"", "\"bob\"", 1);
    assert_eq!(check(forged), "invalid");
    // Lightweight.
    git(w, &["tag", "-d", "ryupdm/p/B"]);
    git(w, &["tag", "ryupdm/p/B", &official]);
    assert_eq!(statuses(w)[rel], "invalid");
    // At another commit: not judged.
    let later = commit(w, "later");
    git(w, &["tag", "-f", "ryupdm/p/B", &later]);
    assert_eq!(statuses(w)[rel], "moved");
    // Obsolete under rules, approved.
    git(w, &["tag", "-d", "ryupdm/p/B"]);
    let o = Record::Obsolete(Obsolete {
        project: ProjectId::parse(PROJECT).unwrap(),
        part: Id::parse("p", "part").unwrap(),
        label: "B".into(),
        released_by: ReleasedBy {
            name: None,
            email: None,
            participant: "agent".into(),
        },
        note: None,
    });
    put(w, &o.path(), &o.to_json());
    let obsolete = commit(w, "obsolete B");
    let orel = "releases/p/B.obsolete.json";
    annotated(
        w,
        "ryupdm/p/B.obsolete",
        &obsolete,
        &attested(
            w,
            orel,
            "p",
            "B",
            Kind::Obsolete,
            &obsolete,
            Outcome::Approved,
        ),
    );
    assert_eq!(statuses(w)[orel], "approved");
}

#[test]
fn teams_and_revoked_keys_verify_offline_in_a_clone() {
    use ryupdm::attest::{Outcome, TEST_SEED, hex, public_key, test_key_id};
    use ryupdm::record::Kind;
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    git(w, &["init", "-q", "-b", "main"]);
    put(
        w,
        "ryucad.json",
        &json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project", "project": PROJECT}),
    );
    put(
        w,
        "parts/p/part.json",
        &json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
    );
    put(w, "parts/p/features/a.json", &json!({"x": 1}));
    put(
        w,
        "parts/p/properties.json",
        &json!({"format": 1, "kind": "ryupdm.properties", "number": "100"}),
    );
    let team_rules = json!([{"count": 1, "from": ["@acme/quality"], "parts": ["*"]}]);
    let key = hex(&public_key(&TEST_SEED));
    put(
        w,
        "ryupdm.json",
        &json!({"approvals": {"keys": {test_key_id(): key}, "rules": team_rules},
                "format": 3, "kind": "ryupdm.config"}),
    );
    commit(w, "start, a team's rule");
    let a = release_a(w);
    put(w, &a.path(), &a.to_json());
    let official = commit(w, "release p A");
    let rel = "releases/p/A.json";
    let through_team = |a: &mut ryupdm::attest::Attestation| {
        a.rules[0].from = vec!["@acme/quality".into()];
        a.approvals[0].github.login = "quinn".into();
        a.approvals[0].via = vec!["@acme/quality".into()];
    };
    annotated(
        w,
        "ryupdm/p/A",
        &official,
        &attested_with(
            w,
            rel,
            "p",
            "A",
            Kind::Release,
            &official,
            Outcome::Approved,
            through_team,
        ),
    );
    assert_eq!(statuses(w)[rel], "approved");
    let bare = tempfile::tempdir().unwrap();
    git(
        bare.path(),
        &["clone", "-q", "--bare", w.to_str().unwrap(), "."],
    );
    assert_eq!(statuses(bare.path())[rel], "approved");

    // Revoked later at the tip: A's attestation, made before, proves
    // nothing now, though its rules were read where the key was trusted.
    put(
        w,
        "ryupdm.json",
        &json!({"approvals": {"keys": {test_key_id(): {"key": key, "revoked": "2026-11-10"}},
                              "rules": team_rules},
                "format": 3, "kind": "ryupdm.config"}),
    );
    commit(w, "revoke the test key");
    assert_eq!(statuses(w)[rel], "revoked");
    let src = Folder(w);
    let repo = gix::open(w).unwrap();
    let releases = Releases::read(&repo, "", &Config::read(&src).unwrap()).unwrap();
    let h = History {
        repo: &repo,
        prefix: "",
        releases: &releases,
    };
    let found = check::run(&src, Some(h), None).unwrap();
    let f = found
        .iter()
        .find(|f| f.code.contains("attestation"))
        .expect("a finding");
    assert_eq!(
        (
            f.code,
            f.details["key"].as_str(),
            f.details["revoked"].as_str()
        ),
        (
            "release_attestation_revoked",
            Some(&*test_key_id()),
            Some("2026-11-10")
        )
    );
}

/// Two projects in one repository, each with a part `p` released as A
/// (#835): each reads only its own tags, `ryupdm/<folder>/p/A`, and an
/// old-form tag `ryupdm/p/A` counts only where it points at the record's
/// official commit and attests the record's project.
#[test]
fn two_projects_with_a_part_of_the_same_name_tag_apart() {
    use ryupdm::attest::Outcome;
    use ryupdm::history::Tag;
    use ryupdm::record::Kind;
    const OTHER: &str = "fedcba9876543210fedcba9876543210";
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    git(w, &["init", "-q", "-b", "main"]);
    for (folder, id) in [("a", PROJECT), ("b", OTHER)] {
        put(
            w,
            &format!("{folder}/ryucad.json"),
            &json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project", "project": id}),
        );
        put(
            w,
            &format!("{folder}/parts/p/part.json"),
            &json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
        );
        put(
            w,
            &format!("{folder}/parts/p/features/a.json"),
            &json!({"x": folder}),
        );
        put(
            w,
            &format!("{folder}/parts/p/properties.json"),
            &json!({"format": 1, "kind": "ryupdm.properties", "number": "100"}),
        );
    }
    commit(w, "start");
    let mut officials = BTreeMap::new();
    for (folder, id) in [("a", PROJECT), ("b", OTHER)] {
        let Record::Release(mut r) = release_a(&w.join(folder)) else {
            unreachable!()
        };
        r.project = ProjectId::parse(id).unwrap();
        let r = Record::Release(r);
        put(w, &format!("{folder}/{}", r.path()), &r.to_json());
        officials.insert(folder, commit(w, &format!("release {folder} p A")));
    }
    let (oa, ob) = (officials["a"].clone(), officials["b"].clone());
    let p = Id::parse("p", "part").unwrap();
    let read = |prefix: &str| {
        let repo = gix::open(w).unwrap();
        Releases::read(&repo, prefix, &Config::default()).unwrap()
    };
    let findings = |prefix: &str| {
        let repo = gix::open(w).unwrap();
        let releases = read(prefix);
        let src = history::files(&repo, "HEAD", prefix).unwrap();
        let h = History {
            repo: &repo,
            prefix,
            releases: &releases,
        };
        check::run(&src, Some(h), None)
            .unwrap()
            .iter()
            .map(|f| f.to_json())
            .map(|v| {
                (
                    v["code"].as_str().unwrap().to_owned(),
                    v["details"]["tag"].clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let tag_a = history::tag_name("a/", &p, "A");
    let tag_b = history::tag_name("b/", &p, "A");
    assert_eq!(tag_a, "ryupdm/a/p/A");
    assert_eq!(tag_b, "ryupdm/b/p/A");
    assert_eq!(read("a/").tag(&p, "A", &oa), Tag::Missing);
    assert_eq!(read("b/").tag(&p, "A", &ob), Tag::Missing);

    // Both tagged: both verify, and neither sees the other's as orphaned.
    git(w, &["tag", &tag_a, &oa]);
    git(w, &["tag", &tag_b, &ob]);
    for (prefix, official) in [("a/", &oa), ("b/", &ob)] {
        let r = read(prefix);
        assert_eq!(r.tag(&p, "A", official), Tag::Ok, "{prefix}");
        assert!(r.orphaned_tags().is_empty(), "{prefix}");
        assert_eq!(
            r.name_of(&p, "A", Kind::Release),
            format!("ryupdm/{prefix}p/A")
        );
        assert!(
            findings(prefix).is_empty(),
            "{prefix}: {:?}",
            findings(prefix)
        );
    }
    // A root project in the same repository ignores them: they have a
    // folder's segments.
    assert!(read("").tags.is_empty());

    // An old-form tag (ryucad before 0.9.1) made for a: a prefers its
    // new-form tag, and b reads it as not its own, missing, never moved.
    git(w, &["tag", "ryupdm/p/A", &oa]);
    git(w, &["tag", "-d", &tag_b]);
    assert_eq!(read("a/").tag(&p, "A", &oa), Tag::Ok);
    assert_eq!(read("a/").name_of(&p, "A", Kind::Release), tag_a);
    assert_eq!(read("b/").tag(&p, "A", &ob), Tag::Missing);
    assert!(read("b/").orphaned_tags().is_empty());
    assert_eq!(
        findings("b/"),
        [("release_tag_missing".to_owned(), json!(tag_b))]
    );
    // Without its new-form tag, a accepts the old one: it is at a's
    // official commit.
    git(w, &["tag", "-d", &tag_a]);
    let r = read("a/");
    assert_eq!(r.tag(&p, "A", &oa), Tag::Ok);
    assert_eq!(r.name_of(&p, "A", Kind::Release), "ryupdm/p/A");
    assert!(findings("a/").is_empty(), "{:?}", findings("a/"));
    // The root project reads that one as its own, as before 0.9.1.
    assert_eq!(read("").tags.keys().collect::<Vec<_>>(), ["p/A"]);

    // An annotated old-form tag at b's official commit attesting another
    // project isn't b's; one attesting b's project is.
    git(w, &["tag", "-d", "ryupdm/p/A"]);
    let rel = "b/releases/p/A.json";
    let foreign = attested(w, rel, "p", "A", Kind::Release, &ob, Outcome::Approved);
    annotated(w, "ryupdm/p/A", &ob, &foreign);
    assert_eq!(read("b/").tag(&p, "A", &ob), Tag::Missing);
    assert_eq!(read("a/").tag(&p, "A", &oa), Tag::Missing);
    git(w, &["tag", "-d", "ryupdm/p/A"]);
    let own = attested_with(
        w,
        rel,
        "p",
        "A",
        Kind::Release,
        &ob,
        Outcome::Approved,
        |a| a.project = ProjectId::parse(OTHER).unwrap(),
    );
    annotated(w, "ryupdm/p/A", &ob, &own);
    assert_eq!(read("b/").tag(&p, "A", &ob), Tag::Ok);
    assert_eq!(read("b/").name_of(&p, "A", Kind::Release), "ryupdm/p/A");

    // Package tags carry the folder too, and an old-form one counts on the
    // same terms: at the release's official commit.
    let package = ryupdm::package::attest::tag_name("b/", &p, "A");
    assert_eq!(package, "ryupdm/b/p/A.package");
    git(w, &["tag", &package, &ob]);
    assert!(read("b/").tags.contains_key("p/A.package"));
    assert!(!read("a/").tags.contains_key("p/A.package"));
    assert!(read("b/").orphaned_tags().is_empty());
    git(w, &["tag", "ryupdm/p/A.package", &oa]);
    let r = read("a/");
    assert_eq!(r.name("p/A.package"), "ryupdm/p/A.package");
    assert!(r.tags.contains_key("p/A.package"));
    assert!(!r.legacy.contains("p/A") || r.tags.contains_key("p/A"));
    assert_eq!(read("b/").name("p/A.package"), package);
}
