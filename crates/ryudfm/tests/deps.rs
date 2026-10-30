//! ryudfm builds on the shared crates and no kernel or other product: its
//! board model is `ryu-board`'s, which ryueda converts its boards into.

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
                    "ryukei",
                    "ryukei-testkit",
                    "ryucad",
                    "ryucad-doc",
                    "ryueda",
                    "ryustep",
                    "ryu-part21",
                    "ryupdm",
                    "ryumate",
                    "ryusketch"
                ]
                .contains(&name),
                "{section} names {name}"
            );
        }
    }
}

#[test]
fn archives_are_read_without_c() {
    let deps = names(MANIFEST, "[dependencies]");
    assert!(deps.contains(&"flate2") && deps.contains(&"zip") && deps.contains(&"tar"));
    // flate2's default backend is miniz_oxide; zip takes only flate2's
    // deflate, never bzip2, zstd or lzma, which are C.
    assert!(MANIFEST.contains(
        r#"zip = { version = "9", default-features = false, features = ["deflate-flate2"] }"#
    ));
    assert!(!MANIFEST.contains("zlib"));
}

#[test]
fn hashes_without_assembly() {
    // BLAKE3 names every input; its `pure` feature builds no C or
    // assembly on any platform.
    assert!(MANIFEST.contains(r#"blake3 = { version = "1", features = ["pure"] }"#));
}
