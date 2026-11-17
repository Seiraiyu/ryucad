//! The committed boards' copper (question 1017 of plan 1.C): each builds
//! to its `copper` line in `manifest.txt`, and where `reference/<name>/`
//! holds KiCad's IPC-D-356 netlist, every net of it lies on one island of
//! ours and no island of ours holds two. Release only (connectivity of
//! KiCad's zones takes a while in a debug build); full.yml's release
//! suites run it. `UPDATE_MANIFEST=1` writes the lines read now; review
//! the diff.

use std::path::Path;
use std::sync::Arc;

use ryudfm::copper::CopperBoard;
use ryudfm::odb::{ReadOptions, read_path};
use ryudfm::testing::{check_copper, compare_islands_with_d356, copper_summary, manifest};

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: cargo test -p ryudfm --release --test copper_corpus"
)]
fn committed_boards_copper() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let declared = manifest(&std::fs::read_to_string(corpus.join("manifest.txt")).unwrap());
    let mut got = Vec::new();
    let mut compared = 0;
    for d in declared.iter().filter(|d| !d.pinned) {
        let job = d
            .archive
            .as_ref()
            .map(|(a, _)| a)
            .or(d.folder.as_ref())
            .unwrap();
        let options = ReadOptions {
            step: d.step.clone(),
            ..ReadOptions::default()
        };
        let cb = CopperBoard::new(Arc::new(read_path(&corpus.join(job), &options).unwrap()));
        let line = copper_summary(&cb);
        println!("{}:\n{line}", d.name);
        got.push((d.name.clone(), line));
        let r = corpus.join("reference").join(&d.name);
        if r.is_dir() {
            let d356 = std::fs::read_to_string(r.join("netlist.d356")).unwrap();
            let diff = compare_islands_with_d356(&cb, &d356);
            assert!(diff.is_empty(), "{}:\n{}", d.name, diff.join("\n"));
            compared += 1;
        }
    }
    assert!(compared >= 2);
    let differing = check_copper(&corpus.join("manifest.txt"), &got);
    assert!(
        differing.is_empty(),
        "copper changed for {differing:?}: UPDATE_MANIFEST=1 and review the diff"
    );
}
