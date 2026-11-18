//! The declared detail keys, printed from the table (design 698): a change
//! to what a code's details hold is reviewed here like any golden.
//! `UPDATE_GOLDEN=1 cargo test -p ryu-doc --test details` after a
//! deliberate change, then review the diff.

use ryu_doc::error::details;
use std::path::PathBuf;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

#[test]
fn the_details_golden() {
    let text = details::golden();
    let path = golden("details.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        want == text,
        "tests/golden/details.txt is out of step with the table: run \
         UPDATE_GOLDEN=1 cargo test -p ryu-doc --test details and review the diff"
    );
}
/// How many codes may be undeclared: the list's length when 33.E wrote
/// it. It only goes down. A new code declares its keys in
/// `crates/ryu-doc/src/error/details.rs`, never gets a line here, and
/// declaring an old one takes its line out and this down with it (design
/// 698, question 1014).
const MOST: usize = 365;

#[test]
fn the_undeclared_list_only_shrinks() {
    use ryu_doc::error::Code;
    let undeclared: Vec<String> = Code::ALL
        .iter()
        .filter(|c| c.keys().is_none())
        .map(|c| details::name(*c))
        .collect();
    let path = golden("undeclared.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let text: String = undeclared.iter().map(|c| format!("{c}\n")).collect();
        std::fs::write(&path, text).unwrap();
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let listed: Vec<String> = text.lines().map(str::to_owned).collect();
    let new: Vec<&String> = undeclared.iter().filter(|c| !listed.contains(c)).collect();
    assert!(
        new.is_empty(),
        "new codes declare their detail keys in crates/ryu-doc/src/error/details.rs: {new:?}"
    );
    let gone: Vec<&String> = listed.iter().filter(|c| !undeclared.contains(c)).collect();
    assert!(
        gone.is_empty(),
        "declared now: take their lines out of tests/golden/undeclared.txt and lower MOST: {gone:?}"
    );
    assert_eq!(listed, undeclared, "undeclared.txt is in Code::ALL's order");
    assert!(
        listed.len() <= MOST,
        "{} codes undeclared, at most {MOST}: the list only shrinks",
        listed.len()
    );
}
