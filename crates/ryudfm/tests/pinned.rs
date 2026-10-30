//! The pinned corpus, which `scripts/fetch-dfm-corpus.sh` fetches and
//! exports with the devcontainer's KiCad into `target/dfm-corpus` (or
//! `$RYUDFM_CORPUS`): each board reads to its block of `manifest.txt`, as
//! a folder and as a zip alike, and agrees with KiCad's positions and
//! netlist. Release builds in the devcontainer only:
//!
//! ```sh
//! sh scripts/fetch-dfm-corpus.sh
//! cargo test -p ryudfm --release --test pinned -- --ignored
//! ```

use std::path::{Path, PathBuf};

use ryudfm::odb::{ReadOptions, read_path};
use ryudfm::testing::{check_manifest, compare_with_kicad, manifest, summary};

#[test]
#[ignore = "needs scripts/fetch-dfm-corpus.sh's exports: run in the devcontainer"]
fn pinned_boards_read_to_their_manifest() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir: PathBuf = std::env::var_os("RYUDFM_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| here.join("../../target/dfm-corpus"));
    let pinned = std::fs::read_to_string(here.join("tests/corpus/pinned.txt")).unwrap();
    let fetched: Vec<&str> = pinned
        .lines()
        .filter(|l| l.starts_with("fetch "))
        .map(|l| l.split_whitespace().nth(1).unwrap())
        .collect();
    let declared =
        manifest(&std::fs::read_to_string(here.join("tests/corpus/manifest.txt")).unwrap());
    let names: Vec<&str> = declared
        .iter()
        .filter(|d| d.pinned)
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(
        names, fetched,
        "manifest.txt's pinned boards and pinned.txt's fetched ones differ"
    );
    let mut got = Vec::new();
    for name in fetched {
        let out = dir.join(name);
        assert!(
            out.join("odb").is_dir(),
            "no {}: run scripts/fetch-dfm-corpus.sh",
            out.display()
        );
        let a = read_path(&out.join("odb"), &ReadOptions::default()).unwrap();
        let mut b = read_path(&out.join(format!("{name}.zip")), &ReadOptions::default()).unwrap();
        b.source.created.clone_from(&a.source.created);
        b.source.saved.clone_from(&a.source.saved);
        assert!(a == b, "{name}: the folder and the zip read differently");
        let pos = std::fs::read_to_string(out.join("pos.csv")).unwrap();
        let d356 = std::fs::read_to_string(out.join("netlist.d356")).unwrap();
        let diff = compare_with_kicad(&a, &pos, &d356);
        assert!(diff.is_empty(), "{name}:\n{}", diff.join("\n"));
        got.push((name.to_string(), summary(&a)));
    }
    let differing = check_manifest(&here.join("tests/corpus/manifest.txt"), &got);
    assert!(
        differing.is_empty(),
        "counts changed for {differing:?}: UPDATE_MANIFEST=1 and review the diff"
    );
}
