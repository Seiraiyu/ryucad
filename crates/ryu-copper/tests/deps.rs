//! `ryu-copper` is product-neutral: it depends on `ryu-board`, `ryu-math`
//! and `rayon` only, so ryueda and ryudfm can both build on it, and no
//! geometry crate decides one of its results.

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
fn depends_on_the_board_model_and_nothing_else() {
    assert_eq!(
        names(MANIFEST, "[dependencies]"),
        ["ryu-board", "ryu-math", "rayon"]
    );
    for section in ["[dependencies]", "[dev-dependencies]"] {
        for name in names(MANIFEST, section) {
            assert!(
                ![
                    "ryukei",
                    "ryukei-testkit",
                    "ryucad",
                    "ryucad-doc",
                    "ryueda",
                    "ryudfm",
                    "ryustep",
                    "ryu-part21",
                    "ryu-session",
                    "ryu-doc",
                ]
                .contains(&name),
                "{section} names {name}"
            );
        }
    }
}
