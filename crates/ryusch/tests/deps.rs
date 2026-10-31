//! `ryusch` takes plain inputs and gives plain outputs: it reads no
//! files, projects or sessions, and depends on no product and no kernel.

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
fn depends_on_no_product_and_no_files() {
    assert_eq!(names(MANIFEST, "[dependencies]"), ["serde", "serde_json"]);
    for section in ["[dependencies]", "[dev-dependencies]"] {
        for name in names(MANIFEST, section) {
            assert!(
                ![
                    "ryukei",
                    "ryucad",
                    "ryucad-doc",
                    "ryu-doc",
                    "ryueda",
                    "ryudfm",
                    "ryu-session",
                    "ryu-board",
                ]
                .contains(&name),
                "{section} names {name}"
            );
        }
    }
}
