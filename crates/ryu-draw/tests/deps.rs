//! `ryu-draw` is product-neutral: it depends on no kernel, no product and
//! no `serde`, so every product's drawings can build on it.

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

#[test]
fn depends_on_the_math_the_pdf_crates_and_nothing_else() {
    let manifest = include_str!("../Cargo.toml");
    assert_eq!(
        names(manifest, "[dependencies]"),
        [
            "ryu-math",
            "pdf-writer",
            "subsetter",
            "ttf-parser",
            "miniz_oxide",
            "blake3"
        ]
    );
}
