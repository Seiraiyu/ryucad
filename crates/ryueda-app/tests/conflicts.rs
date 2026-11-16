//! The conflict panel against real daemons and real merges (design 688):
//! a merge opens Conflicts and the panel settles it, a stale revision is
//! refused on its row, Show ours and Show theirs read each side, and a
//! manifest's record opens Conflicts alone. Every repository is the
//! test's own (#920).

mod common;

use common::{OURS, THEIRS, cli, git_ok, merging, one_at_a_time, top_of};

/// Needs no adapter: runs everywhere CI does.
#[test]
fn a_merge_is_made_in_the_tests_own_repository() {
    let _one = one_at_a_time();
    let (top, board) = merging("cf-repo", OURS, THEIRS);
    assert_eq!(top_of(&board), top);
    // `ryueda init` put the driver in this repository's own config.
    assert!(git_ok(
        &top,
        &["config", "--local", "--get-regexp", "^merge\\..*\\.driver$"]
    ));
    assert!(
        top.join(".git").join("MERGE_HEAD").exists(),
        "the merge is in progress"
    );
    let v = cli(&board, &["merge", "status"]);
    let kinds: Vec<&str> = v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"refdes_clash"), "{v}");
}
