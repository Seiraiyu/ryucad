//! Our copper against KiCad's own DRC of the same boards at a fixed
//! clearance of 0.2 mm (design 245): every board of `pinned.txt`, the
//! committed ones from the committed archives and the fetched ones from
//! `scripts/fetch-dfm-corpus.sh`'s exports, beside the `drc.json` the
//! script writes. Nets open and pairs of nets shorted agree, and so do
//! pairs of nets closer than 0.2 mm on a layer but those within
//! `TOLERANCE` of it (KiCad's arc error and its reports' precision), which
//! are printed. Release builds in the devcontainer only:
//!
//! ```sh
//! sh scripts/fetch-dfm-corpus.sh
//! cargo test -p ryudfm --release --test kicad_drc -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ryudfm::copper::CopperBoard;
use ryudfm::odb::{ReadOptions, read_path};
use ryudfm::testing::{compare_with_drc, manifest};

#[test]
#[ignore = "needs scripts/fetch-dfm-corpus.sh's DRC reports: run in the devcontainer"]
fn copper_agrees_with_kicad_s_drc() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir: PathBuf = std::env::var_os("RYUDFM_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| here.join("../../target/dfm-corpus"));
    let pinned = std::fs::read_to_string(here.join("tests/corpus/pinned.txt")).unwrap();
    let declared =
        manifest(&std::fs::read_to_string(here.join("tests/corpus/manifest.txt")).unwrap());
    let mut problems = Vec::new();
    let mut boards = 0;
    for line in pinned
        .lines()
        .filter(|l| l.starts_with("fetch ") || l.starts_with("commit "))
    {
        let mut w = line.split_whitespace();
        let (kind, name) = (w.next().unwrap(), w.next().unwrap());
        let job = if kind == "commit" {
            let d = declared.iter().find(|d| d.name == name).expect("declared");
            here.join("tests/corpus")
                .join(&d.archive.as_ref().expect("an archive").0)
        } else {
            dir.join(name).join(format!("{name}.zip"))
        };
        let report = dir.join(name).join("drc.json");
        let drc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).unwrap_or_else(|e| {
                panic!("{}: {e}: run scripts/fetch-dfm-corpus.sh", report.display())
            }))
            .unwrap();
        // What KiCad wrote, for the record: its violations by type, and a
        // few clearance ones (their descriptions are what we parse).
        let mut types = std::collections::BTreeMap::<String, usize>::new();
        for v in drc["violations"].as_array().into_iter().flatten() {
            *types
                .entry(v["type"].as_str().unwrap_or("?").to_string())
                .or_default() += 1;
        }
        println!(
            "{name}: KiCad's violations {types:?}, {} unconnected",
            drc["unconnected_items"].as_array().map_or(0, Vec::len)
        );
        for v in drc["violations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v["type"] == "clearance")
            .take(3)
        {
            println!("  {v}");
        }
        let cb = CopperBoard::new(Arc::new(read_path(&job, &ReadOptions::default()).unwrap()));
        let found = compare_with_drc(&cb, &drc, 200_000);
        println!("{name}: {} disagreement(s)", found.len());
        problems.extend(found.into_iter().map(|p| format!("{name}: {p}")));
        boards += 1;
    }
    assert_eq!(boards, 4);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
