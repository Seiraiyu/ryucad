//! step-hunt end to end: against a stand-in that fails each stage in turn,
//! and against the real ryucad over the committed corpus, whose findings
//! must be exactly what the ledger lists.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The workspace's root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Runs step-hunt with `args`, `env` set, and gives report.json.
fn hunt(args: &[&str], env: &[(&str, &str)], out: &Path) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_step-hunt"));
    cmd.args(args)
        .arg("--out")
        .arg(out)
        .arg("--root")
        .arg(root());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let o = cmd.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&std::fs::read(out.join("report.json")).unwrap()).unwrap()
}

/// A sources file of one OCCT box, run body by body and whole.
fn one_box(dir: &Path) -> PathBuf {
    let path = dir.join("sources.txt");
    let occt = root().join("crates/ryustep/tests/data/occt");
    std::fs::write(
        &path,
        format!(
            "source box {} box_ap214.stp LicenseRef-ryucad whole\n",
            occt.display()
        ),
    )
    .unwrap();
    path
}

/// The signatures report `v` found, sorted.
fn signatures(v: &Value) -> Vec<String> {
    let mut s: Vec<String> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["signature"].as_str().unwrap().to_owned())
        .collect();
    s.sort();
    s
}

#[test]
fn every_stage_fails_findably() {
    let fake = env!("CARGO_BIN_EXE_fake-ryucad");
    let cases = [
        ("", None),
        (
            "import:fail",
            Some("import/import_failed/faces_overlap/ADVANCED_BREP_SHAPE_REPRESENTATION"),
        ),
        ("check:fail", Some("check/check_failed/faces_intersect")),
        ("export:fail", Some("export/export_unrepresentable/pcurve")),
        ("reimport:counts", Some("reimport/counts")),
        ("volume:volume", Some("volume/volume")),
        ("fixpoint:bytes", Some("fixpoint/bytes")),
        ("import:crash", Some("import/crash/101")),
        ("whole_import:crash", Some("whole_import/crash/101")),
        ("export:hang", Some("export/timeout/faces<50/PLANE")),
    ];
    for (fault, want) in cases {
        let dir = tempfile::tempdir().unwrap();
        let sources = one_box(dir.path());
        let v = hunt(
            &[
                "run",
                "--ryucad",
                fake,
                "--sources",
                sources.to_str().unwrap(),
                "--timeout",
                "2",
                "--ledger",
                "/nonexistent",
            ],
            &[("STEP_HUNT_FAKE", fault)],
            &dir.path().join("out"),
        );
        assert_eq!(
            signatures(&v),
            want.into_iter().collect::<Vec<_>>(),
            "{fault}"
        );
        assert_eq!(v["cases"], 2, "a body and the file whole");
    }
}

#[test]
fn a_rerun_runs_only_what_failed() {
    let fake = env!("CARGO_BIN_EXE_fake-ryucad");
    let dir = tempfile::tempdir().unwrap();
    let sources = one_box(dir.path());
    let first = dir.path().join("first");
    let v = hunt(
        &[
            "run",
            "--ryucad",
            fake,
            "--sources",
            sources.to_str().unwrap(),
            "--ledger",
            "/nonexistent",
        ],
        &[("STEP_HUNT_FAKE", "check:fail")],
        &first,
    );
    assert_eq!(v["failed"], 1);
    let v = hunt(
        &[
            "rerun",
            first.join("report.json").to_str().unwrap(),
            "--ryucad",
            fake,
            "--sources",
            sources.to_str().unwrap(),
            "--ledger",
            "/nonexistent",
        ],
        &[],
        &dir.path().join("again"),
    );
    assert_eq!(
        (v["cases"].as_u64(), v["failed"].as_u64()),
        (Some(1), Some(0))
    );
}

/// The `ryucad` binary of this build's profile, built if needed:
/// `cargo test -p step-hunt` doesn't build other packages' binaries.
fn ryucad() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let bin = exe.parent().unwrap().parent().unwrap();
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "-p", "ryucad", "--bin", "ryucad"]);
    match bin.file_name().unwrap().to_str().unwrap() {
        "debug" => {}
        "release" => {
            cmd.arg("--release");
        }
        other => {
            cmd.args(["--profile", other]);
        }
    }
    assert!(cmd.status().unwrap().success(), "building ryucad");
    bin.join(format!("ryucad{}", std::env::consts::EXE_SUFFIX))
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn the_committed_corpus_finds_only_what_the_ledger_lists() {
    let ryucad = ryucad();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let v = hunt(
        &[
            "run",
            "--ryucad",
            ryucad.to_str().unwrap(),
            "--source",
            "committed",
            "--source",
            "occt",
            "--timeout",
            "600",
        ],
        &[],
        &out,
    );
    // Slow stages and timeouts depend on the machine: the report lists
    // them, the test judges only the rest.
    let md = std::fs::read_to_string(out.join("report.md")).unwrap();
    let new: u64 = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| g["timed"] == false)
        .map(|g| g["new"].as_u64().unwrap())
        .sum();
    assert_eq!(new, 0, "findings the ledger doesn't list:\n{md}");
    let fixed = v["fixed"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["timed"] == false)
        .count();
    assert_eq!(fixed, 0, "ledger lines that no longer fail:\n{md}");
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn one_job_and_four_find_the_same() {
    let ryucad = ryucad();
    let dir = tempfile::tempdir().unwrap();
    let strip = |mut v: Value| {
        for r in v["records"].as_array_mut().unwrap() {
            for s in r["stages"].as_array_mut().unwrap() {
                s["seconds"] = Value::Null;
            }
            if let Some(k) = r.get_mut("key").filter(|k| !k.is_null()) {
                k["seconds"] = Value::Null;
            }
            r["case"]["path"] = Value::Null;
        }
        v
    };
    let run = |jobs: &str, name: &str| {
        strip(hunt(
            &[
                "run",
                "--ryucad",
                ryucad.to_str().unwrap(),
                "--source",
                "occt",
                "--jobs",
                jobs,
                "--slow",
                "1000000",
            ],
            &[],
            &dir.path().join(name),
        ))
    };
    assert_eq!(run("1", "one"), run("4", "four"));
}
