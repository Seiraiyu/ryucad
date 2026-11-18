//! ryueda's dependencies: the board model and the copper engine, never the
//! kernel, STEP, ryucad or ryudfm (design 732).

fn names(toml: &str, section: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == section;
            continue;
        }
        if inside
            && !t.starts_with('#')
            && let Some((name, _)) = t.split_once('=')
        {
            out.push(name.trim().to_owned());
        }
    }
    out
}

#[test]
fn boards_build_on_ryu_board_and_ryu_copper_and_never_on_a_kernel_or_product() {
    let toml = include_str!("../Cargo.toml");
    let deps = names(toml, "[dependencies]");
    for want in ["ryu-board", "ryu-copper", "ryu-gerber"] {
        assert!(
            deps.iter().any(|d| d == want),
            "ryueda needs {want}: {deps:?}"
        );
    }
    for never in [
        "ryukei",
        "ryustep",
        "ryucad",
        "ryucad-doc",
        "ryudfm",
        "ryuformats",
    ] {
        assert!(
            !deps.iter().any(|d| d == never),
            "ryueda must not depend on {never}"
        );
    }
}
