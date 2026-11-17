//! ryuplm stands on the shared crates and ryupdm's core, never on the
//! kernel or another product (design 734): a catalogue needs no CAD.

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
fn depends_on_no_kernel_and_no_other_product() {
    for section in ["[dependencies]", "[dev-dependencies]"] {
        for name in names(MANIFEST, section) {
            assert!(
                ![
                    "ryukei", "ryukei-testkit", "ryucad", "ryucad-doc", "ryueda", "ryudfm",
                    "ryu-board", "ryu-copper", "ryustep", "ryu-part21", "ryumate", "ryusketch",
                ]
                .contains(&name),
                "{section} names {name}"
            );
        }
    }
}

#[test]
fn takes_ryupdms_core_only() {
    assert!(MANIFEST.contains(r#"ryupdm = { path = "../ryupdm", default-features = false }"#));
    for name in ["ryu-doc", "ryu-session", "ryu-git", "ryupdm", "ryueda-parts"] {
        assert!(names(MANIFEST, "[dependencies]").contains(&name), "{name}");
    }
}
