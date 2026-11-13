//! `ryu-standards` is product-neutral data: it depends on no crate at
//! all, so ryucad, ryueda and anything after can share it, and it holds
//! no float, so every value is the same integer on every platform.

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
fn depends_on_nothing() {
    assert!(names(MANIFEST, "[dependencies]").is_empty());
    assert!(names(MANIFEST, "[dev-dependencies]").is_empty());
}

#[test]
fn holds_no_float() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut seen = 0;
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for word in ["f64", "f32", "libm"] {
                assert!(
                    !code.contains(word),
                    "{}:{}: {word} in ryu-standards",
                    path.display(),
                    n + 1
                );
            }
        }
        seen += 1;
    }
    assert!(seen >= 2, "read src/");
}
