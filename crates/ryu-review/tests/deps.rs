//! `ryu-review` is the review service's client for every ryu product:
//! it knows no product's files, so it depends on none.

/// The names in one `[section]` of the manifest.
fn names<'a>(toml: &'a str, section: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == section;
            continue;
        }
        if inside && !line.is_empty() && !line.starts_with('#') {
            out.push(line.split(['=', ' ']).next().unwrap_or(""));
        }
    }
    out
}

const MANIFEST: &str = include_str!("../Cargo.toml");

#[test]
fn depends_on_no_product() {
    assert_eq!(
        names(MANIFEST, "[dependencies]"),
        ["ryu-doc", "ryu-git", "serde_json", "ureq"]
    );
    for section in ["[dependencies]", "[dev-dependencies]"] {
        for name in names(MANIFEST, section) {
            assert!(
                !["ryukei", "ryucad", "ryucad-doc", "ryueda", "ryudfm", "ryupdm"].contains(&name),
                "{section} names {name}"
            );
        }
    }
}
