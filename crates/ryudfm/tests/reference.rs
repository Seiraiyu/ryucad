//! The committed boards that have KiCad's own outputs in
//! `reference/<name>/` (`pos.csv`, `netlist.d356`) agree with them.

use std::path::Path;

use ryudfm::odb::{ReadOptions, read_path};
use ryudfm::testing::{compare_with_kicad, manifest};

#[test]
fn boards_agree_with_their_references() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let declared = manifest(&std::fs::read_to_string(corpus.join("manifest.txt")).unwrap());
    let mut compared = 0;
    for d in declared.iter().filter(|d| !d.pinned) {
        let r = corpus.join("reference").join(&d.name);
        if !r.is_dir() {
            continue;
        }
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
        let b = read_path(&corpus.join(job), &options).unwrap();
        let pos = std::fs::read_to_string(r.join("pos.csv")).unwrap();
        let d356 = std::fs::read_to_string(r.join("netlist.d356")).unwrap();
        let diff = compare_with_kicad(&b, &pos, &d356);
        assert!(diff.is_empty(), "{}:\n{}", d.name, diff.join("\n"));
        compared += 1;
    }
    assert!(compared >= 2);
}
