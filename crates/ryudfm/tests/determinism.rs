//! Bit-exact boards on every platform and CPU level:
//! `tests/golden/ryudfm.txt` holds the hashes of each committed board's
//! model and of every layer's shapes, for every board `manifest.txt`
//! declares but the pinned ones. After a deliberate change, or a board
//! added, regenerate with `UPDATE_GOLDEN=1 cargo test -p ryudfm --test
//! determinism` and review the diff. Plan 0.C adds the renders.

use std::path::Path;

use ryu_board::testing::Golden;
use ryudfm::odb::{ReadOptions, read_path};
use ryudfm::testing::manifest;

#[test]
fn boards_are_bit_exact() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = dir.join("tests/corpus");
    let mut golden = Golden::new(dir.join("tests/golden/ryudfm.txt"));
    let declared = manifest(&std::fs::read_to_string(corpus.join("manifest.txt")).unwrap());
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
        let b = read_path(&corpus.join(job), &options).unwrap();
        {
            // The model as read, before any shape is built (each feature's
            // shape slot is part of its debug text).
            let mut s = golden.section(&format!("{}.model", d.name));
            s.str(&format!("{b:?}"));
        }
        let mut s = golden.section(&format!("{}.shapes", d.name));
        for step in 0..b.steps.len() {
            for layer in 0..b.layers.len() {
                for shape in b.layer_shapes(step, layer) {
                    s.shape(shape);
                }
            }
        }
    }
    golden.finish();
}
