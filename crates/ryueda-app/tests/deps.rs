//! `ryueda-app` is ryueda's window (design 519, The app's home): it
//! reaches ryueda through `proto` and `client`, and never ryucad, the
//! kernel, or the schematic engine (the app draws the daemon's scene, so a
//! window and `ryueda render` can never disagree).

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
fn depends_on_ryueda_and_the_shell_and_never_on_ryucad_or_the_engine() {
    let deps = names(MANIFEST, "[dependencies]");
    for want in ["eframe", "ryu-app", "ryueda"] {
        assert!(deps.contains(&want), "{want} missing: {deps:?}");
    }
    for section in [
        "[dependencies]",
        "[target.'cfg(all(unix, not(target_os = \"macos\")))'.dependencies]",
    ] {
        for name in names(MANIFEST, section) {
            assert!(
                ![
                    "ryukei",
                    "ryucad",
                    "ryucad-doc",
                    "ryucad-app",
                    "ryusch",
                    "ryustep",
                    "ryupdm",
                    "ryudfm"
                ]
                .contains(&name),
                "{section} names {name}"
            );
        }
    }
    // The tests alone may take the engine, and only for the fixtures they
    // share with ryueda's own tests (`tests/support/draw.rs` measures pin
    // names with its font): never ryucad or the kernel.
    for name in names(MANIFEST, "[dev-dependencies]") {
        assert!(
            ![
                "ryukei",
                "ryucad",
                "ryucad-doc",
                "ryucad-app",
                "ryustep",
                "ryupdm",
                "ryudfm"
            ]
            .contains(&name),
            "[dev-dependencies] names {name}"
        );
    }
}
