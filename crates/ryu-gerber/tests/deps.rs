//! ryu-gerber knows `ryu-board` and nothing else: no product, no kernel
//! (design 732). ryudfm's stage 6 adds its readers here.

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
fn ryu_gerber_depends_on_ryu_board_alone() {
    let deps = names(include_str!("../Cargo.toml"), "[dependencies]");
    assert_eq!(deps, vec!["ryu-board".to_owned()]);
}
