//! What the hosted review service reads from ryueda (design 667, plan
//! 8.A): every answer's schema, `version`'s probe, `diff`'s sheets,
//! framed renders, `git attributes` and `resolve`, both ways where a
//! request (in process and through the daemon).

#[path = "support/breakout.rs"]
mod breakout;
#[allow(dead_code)]
#[path = "support/draw.rs"]
mod draw;
#[path = "support/gitrepo.rs"]
mod gitrepo;
use gitrepo::*;

use serde_json::{Value, json};
use std::path::Path;

/// `ryueda` that must succeed: its result.
fn ok(dir: &Path, run: &Path, daemon: bool, args: &[&str]) -> Value {
    let (code, v) = ryueda(dir, run, daemon, args);
    assert_eq!(code, 0, "{args:?}: {v}");
    v["result"].clone()
}

#[test]
fn version_says_the_schema_of_its_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    std::fs::create_dir_all(dir.path().join("p")).unwrap();
    let v = ok(&dir.path().join("p"), &run, false, &["version"]);
    assert_eq!(v["schema_outputs"], 1, "{v}");
    assert_eq!(v["schema_outputs"], json!(ryueda::OUTPUT_SCHEMA));
}
