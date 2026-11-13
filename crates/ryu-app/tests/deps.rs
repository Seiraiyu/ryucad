//! `ryu-app` is the shell every ryu product's window shares (design 519,
//! The app's home): it names no product and never reaches the kernel, so
//! ryueda's window builds without ryucad, and ryucad's without ryueda.

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
fn depends_on_no_product_and_no_kernel() {
    assert_eq!(
        names(MANIFEST, "[dependencies]"),
        ["egui", "egui_tiles", "ryu-doc", "ryu-math", "ryu-session", "serde_json"]
    );
    for section in ["[dependencies]", "[dev-dependencies]"] {
        for name in names(MANIFEST, section) {
            assert!(
                ![
                    "ryukei", "ryucad", "ryucad-doc", "ryucad-app", "ryueda", "ryusch",
                    "ryudfm", "ryu-board", "ryupdm", "ryustep", "eframe",
                ]
                .contains(&name),
                "{section} names {name}"
            );
        }
    }
}
