//! `reference_unresolved` names what the lost face became (issue #271):
//! a slot lengthened across a bore splits the bore's face, and a mate on it
//! is told both pieces.

use assert_cmd::cargo::cargo_bin;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn r(dir: &Path, args: &[&str]) -> Value {
    let out = Command::new(cargo_bin("ryucad"))
        .args(args)
        .current_dir(dir)
        .env("RYUCAD_NO_DAEMON", "1")
        .env("RYUCAD_CACHE_DIR", dir.join(".cache"))
        .env("RYUCAD_CONFIG_DIR", dir.join(".config"))
        .output()
        .unwrap();
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
fn a_split_face_s_pieces_are_the_mate_s_candidates() {
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
    let j = report["error"]["details"]["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "j_bore")
        .unwrap()
        .clone();
    assert_eq!(j["error"]["code"], "reference_unresolved", "{j}");
    let want: Vec<Value> = pieces
        .iter()
        .map(|n| Value::from(format!("sub/base/blk/{n}")))
        .collect();
    assert_eq!(
        j["error"]["details"]["candidates"],
        Value::from(want),
        "{j}"
    );
}

#[test]
fn a_part_reference_to_a_split_face_names_its_pieces() {
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
    let s = r(d, &["rebuild", "p"])["error"]["details"]["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "s")
        .unwrap()
        .clone();
    assert_eq!(s["error"]["code"], "reference_unresolved", "{s}");
    assert_eq!(
        s["error"]["details"]["candidates"].as_array().map(Vec::len),
        Some(2),
        "{s}"
    );
}
