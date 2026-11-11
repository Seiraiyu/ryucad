//! A reference follows a face a later feature split (design 447, issue
//! #156): a slot lengthened across a bore splits the bore's face, and a
//! revolute mate on it holds on the first piece, both pieces lying on the
//! bore's cylinder, with a `reference_followed` warning naming both
//! (#271 named them as `candidates` of a failure before).

use assert_cmd::cargo::cargo_bin;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

thread_local! {
    /// Where a test's daemon runs, when it has one; else in process.
    static RUN: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn r(dir: &Path, args: &[&str]) -> Value {
    let mut c = Command::new(cargo_bin("ryucad"));
    c.args(args)
        .current_dir(dir)
        .env("RYUCAD_CACHE_DIR", dir.join(".cache"))
        .env("RYUCAD_CONFIG_DIR", dir.join(".config"));
    match RUN.with(|r| r.borrow().clone()) {
        Some(run) => c
            .env("RYUCAD_RUNTIME_DIR", run)
            .env_remove("RYUCAD_NO_DAEMON"),
        None => c.env("RYUCAD_NO_DAEMON", "1"),
    };
    let out = c.output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn face(dir: &Path, part: &str, pick: impl Fn(&Value) -> bool) -> Vec<String> {
    r(dir, &["inspect", part])["result"]["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| pick(f))
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_mate_on_a_split_bore_follows_its_first_piece() {
    let dir = tempfile::tempdir().unwrap();
    split_bore(dir.path());
    // check passes, and says the mate now rides on a piece.
    let c = r(dir.path(), &["check"]);
    assert_eq!(c["result"]["passed"], true, "{c}");
    let found = c["result"]["findings"].as_array().unwrap();
    assert!(
        found
            .iter()
            .any(|f| f["code"] == "reference_followed" && f["severity"] == "warning"),
        "{c}"
    );
}

/// Through a daemon, which built the bore before the slot split it, the
/// mate follows the same way: the session's memory of the name explains,
/// never decides.
#[test]
fn through_a_daemon_the_mate_follows_the_split_face_the_same_way() {
    let (dir, run) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    RUN.with(|r| *r.borrow_mut() = Some(run.path().to_owned()));
    split_bore(dir.path());
    let _ = r(dir.path(), &["daemon", "stop"]);
}

/// A bore, a mate on its face, and the slot lengthened across it: the
/// mate's report, which builds and names both pieces as what it follows.
fn split_bore(d: &Path) -> Value {
    for a in [
        &["init", "--unit", "mm"][..],
        &["part", "create", "p"],
        &[
            "feature", "add", "p", "box", "--id", "blk", "--param", "x=60", "--param", "y=40",
            "--param", "z=20",
        ],
        &[
            "feature",
            "add",
            "p",
            "cylinder",
            "--id",
            "bore",
            "--param",
            "radius=5",
            "--param",
            "height=40",
            "--origin",
            "20,20,-10",
            "--operation",
            "cut",
            "--target",
            "blk",
        ],
        &[
            "feature",
            "add",
            "p",
            "box",
            "--id",
            "slot",
            "--param",
            "x=4",
            "--param",
            "y=4",
            "--param",
            "z=40",
            "--origin",
            "18,30,-10",
            "--operation",
            "cut",
            "--target",
            "blk",
        ],
        &["part", "create", "pin"],
        &[
            "feature",
            "add",
            "pin",
            "cylinder",
            "--id",
            "shank",
            "--param",
            "radius=5",
            "--param",
            "height=30",
        ],
    ] {
        assert_eq!(r(d, a)["ok"], true, "{a:?}");
    }
    let is_bore =
        |f: &Value| f["surface"]["type"] == "cylinder" && f["origin"]["feature"] == "bore";
    let bore = face(d, "p", is_bore).remove(0);
    let side = face(d, "pin", |f| f["surface"]["type"] == "cylinder").remove(0);
    for a in [
        &["part", "create", "sub", "--assembly"][..],
        &[
            "feature", "add", "sub", "instance", "--id", "base", "--of", "p", "--fixed",
        ],
        &[
            "feature", "add", "sub", "instance", "--id", "pin1", "--of", "pin", "--origin",
            "20,20,-5",
        ],
    ] {
        assert_eq!(r(d, a)["ok"], true, "{a:?}");
    }
    let a = format!("sub/base/blk/{bore}");
    let b = format!("sub/pin1/shank/{side}");
    assert_eq!(
        r(
            d,
            &[
                "feature", "add", "sub", "mate", "--id", "j_bore", "--form", "revolute", "--a", &a,
                "--b", &b
            ]
        )["ok"],
        true
    );
    // The slot now runs across the bore: its face splits in two.
    assert_eq!(
        r(
            d,
            &[
                "feature",
                "edit",
                "p",
                "slot",
                "--param",
                "y=24",
                "--origin",
                "18,10,-10"
            ]
        )["ok"],
        true
    );
    let mut pieces = face(d, "p", is_bore);
    pieces.sort();
    assert_eq!(pieces.len(), 2);
    let report = r(d, &["rebuild", "sub"]);
    assert_eq!(report["ok"], true, "{report}");
    let j = report["result"]["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "j_bore")
        .unwrap()
        .clone();
    assert_eq!(j["ok"], true, "{j}");
    let want: Vec<Value> = pieces
        .iter()
        .map(|n| Value::from(format!("sub/base/blk/{n}")))
        .collect();
    let w = &j["warnings"][0];
    assert_eq!(w["code"], "reference_followed", "{j}");
    assert_eq!(w["role"], "a", "{j}");
    assert_eq!(w["reference"], Value::from(a), "{j}");
    assert_eq!(w["pieces"], Value::from(want), "{j}");
    j
}

#[test]
fn a_sketch_on_a_split_face_follows_its_first_piece() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for a in [
        &["init", "--unit", "mm"][..],
        &["part", "create", "p"],
        &[
            "feature", "add", "p", "box", "--id", "blk", "--param", "x=60", "--param", "y=40",
            "--param", "z=20",
        ],
        &[
            "feature",
            "add",
            "p",
            "box",
            "--id",
            "slot",
            "--param",
            "x=4",
            "--param",
            "y=4",
            "--param",
            "z=40",
            "--origin",
            "18,30,-10",
            "--operation",
            "cut",
            "--target",
            "blk",
        ],
    ] {
        assert_eq!(r(d, a)["ok"], true, "{a:?}");
    }
    // The block's top face, which the slot doesn't reach yet.
    let top = face(d, "p", |f| {
        f["surface"]["type"] == "plane"
            && f["origin"]["feature"] == "blk"
            && f["surface"]["frame"]["origin"][2].as_f64() == Some(0.02)
    })
    .remove(0);
    assert_eq!(
        r(
            d,
            &[
                "feature",
                "add",
                "p",
                "sketch",
                "--id",
                "s",
                "--face",
                &format!("p/blk/{top}")
            ]
        )["ok"],
        true
    );
    // Lengthened right across the block, the slot cuts the top in two.
    assert_eq!(
        r(
            d,
            &[
                "feature",
                "edit",
                "p",
                "slot",
                "--param",
                "y=60",
                "--origin",
                "18,-10,-10"
            ]
        )["ok"],
        true
    );
    let s = r(d, &["rebuild", "p"])["result"]["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "s")
        .unwrap()
        .clone();
    assert_eq!(s["ok"], true, "{s}");
    assert_eq!(s["warnings"][0]["code"], "reference_followed", "{s}");
    assert_eq!(s["warnings"][0]["by"], "slot", "{s}");
    assert_eq!(
        s["warnings"][0]["pieces"].as_array().map(Vec::len),
        Some(2),
        "{s}"
    );
}
