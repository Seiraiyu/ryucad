//! The owner's files never reach CI: no workflow runs `cox`, and the
//! sources `tests/hunt.rs` runs are committed ones.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn no_workflow_runs_cox() {
    let dir = root().join(".github/workflows");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        seen += 1;
        for (n, line) in text.lines().enumerate() {
            let at = format!("{}:{}: {line}", path.display(), n + 1);
            let words = line.split(|c: char| c.is_whitespace() || "\"'`;|&()$={}".contains(c));
            for w in words {
                assert!(
                    w != "cox" && !w.ends_with("/cox"),
                    "a workflow runs cox: {at}"
                );
            }
            assert!(!line.contains("cox-files:"), "{at}");
            assert!(!line.contains("owner-step"), "{at}");
            assert!(!line.contains("HUNT_COX"), "{at}");
        }
    }
    assert!(seen > 0, "no workflows read");
}

#[test]
fn hunt_rs_runs_no_cox_files_source() {
    let list = step_hunt::sources::parse(
        &std::fs::read_to_string(root().join("crates/step-hunt/sources.txt")).unwrap(),
    )
    .unwrap();
    let hunt = std::fs::read_to_string(root().join("crates/step-hunt/tests/hunt.rs")).unwrap();
    for s in list
        .iter()
        .filter(|s| s.root.starts_with(step_hunt::cox_files::PREFIX))
    {
        assert!(
            !hunt.contains(&format!("\"{}\"", s.name)),
            "tests/hunt.rs names the cox-files source {}",
            s.name
        );
    }
    assert!(!hunt.contains(step_hunt::cox_files::PREFIX));
}
