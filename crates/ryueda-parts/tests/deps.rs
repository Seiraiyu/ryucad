//! `ryueda-parts` is part data alone: it depends on `ryu-doc` for its
//! quantities and errors, and on nothing of ryueda's, ryucad's or a
//! kernel's, so 6.B's and 6.C's adapters and a hosted service can link it.

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
fn depends_on_ryu_doc_and_nothing_of_a_product() {
    assert_eq!(
        names(MANIFEST, "[dependencies]"),
        [
            "blake3",
            "flate2",
            "rayon",
            "ryu-doc",
            "serde",
            "serde_json"
        ]
    );
    for section in ["[dependencies]", "[dev-dependencies]"] {
        for name in names(MANIFEST, section) {
            assert!(
                ![
                    "ryukei",
                    "ryucad",
                    "ryucad-doc",
                    "ryueda",
                    "ryudfm",
                    "ryustep",
                    "ryu-session",
                    "ryusch",
                    "ureq",
                ]
                .contains(&name),
                "{section} names {name}"
            );
        }
    }
}
