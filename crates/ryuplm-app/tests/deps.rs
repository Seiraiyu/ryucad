//! `ryuplm-app` is ryuplm's window (design 938, The window): it reaches
//! ryuplm through `proto`, `client` and `Plm`, built without `cad` (the
//! daemon it talks to does the reading), and never ryucad, ryueda, the
//! kernel or another product's window.

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

const NEVER: [&str; 11] = [
    "ryukei",
    "ryusketch",
    "ryucad",
    "ryucad-doc",
    "ryucad-app",
    "ryueda",
    "ryueda-app",
    "ryusch",
    "ryustep",
    "ryudfm",
    "ryupdm",
];

#[test]
fn depends_on_ryuplm_and_the_shell_and_never_on_another_product() {
    let deps = names(MANIFEST, "[dependencies]");
    for want in ["eframe", "egui_tiles", "ryu-app", "ryu-session", "ryuplm"] {
        assert!(deps.contains(&want), "{want} missing: {deps:?}");
    }
    for section in [
        "[dependencies]",
        "[target.'cfg(all(unix, not(target_os = \"macos\")))'.dependencies]",
        "[dev-dependencies]",
    ] {
        for name in names(MANIFEST, section) {
            assert!(!NEVER.contains(&name), "{section} names {name}");
        }
    }
    let line = MANIFEST
        .lines()
        .find(|l| l.starts_with("ryuplm ="))
        .expect("ryuplm in [dependencies]");
    assert!(
        line.contains("default-features = false"),
        "ryuplm without cad: {line}"
    );
}
