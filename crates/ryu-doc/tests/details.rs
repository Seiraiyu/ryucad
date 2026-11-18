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
