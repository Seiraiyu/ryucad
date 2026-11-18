//! Our copper against KiCad's own DRC of the same boards at a fixed
//! clearance of 0.2 mm (design 245): every board of `pinned.txt`, the
//! committed ones from the committed archives and the fetched ones from
//! `scripts/fetch-dfm-corpus.sh`'s exports, beside the `drc.json` the
//! script writes. Nets open and pairs of nets shorted agree, and so do
//! pairs of nets closer than 0.2 mm on a layer but those within
//! `TOLERANCE` of it (KiCad's arc error and its reports' precision), which
//! are printed. KiCad's report is cut at `KICAD_REPORT_CAP` violations of
//! a type, so on a board that reaches it only KiCad's gaps are checked
//! against ours; a board whose CAD netlist lies off its copper (#933) has
//! its opens and shorts printed, not compared; a gap KiCad gives as 0 is a
//! contact (copper of no net on a net's), one island for us. Gaps to a
//! plated hole's wall on layers where it has no pad are ours too
//! (`Copper::hole_clearance`, #946), as KiCad reports them. A board in
//! `KNOWN_DRC_CASES` has its disagreements printed under its issue.
//! Release builds in the devcontainer only:
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

/// Boards whose disagreements with KiCad's DRC are printed rather than
/// asserted, each with the issue that holds them.
const KNOWN_DRC_CASES: &[(&str, &str)] = &[];

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
        if let Some((_, issue)) = KNOWN_DRC_CASES.iter().find(|(n, _)| *n == name) {
            println!("{name}: recorded under {issue}, not asserted");
            for p in &found {
                println!("  {p}");
            }
        } else {
            problems.extend(found.into_iter().map(|p| format!("{name}: {p}")));
        }
        boards += 1;
    }
    assert_eq!(boards, 4);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The comparison's rules on a committed board against reports made here:
/// KiCad's names (spaces for ODB++'s `_`) and layers (a via on a span), a
/// cut report, and a CAD netlist that lies off the copper (#933).
#[test]
fn the_rules_on_made_reports() {
    use ryudfm::testing::KICAD_REPORT_CAP;
    use serde_json::json;
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let job = here.join("tests/corpus/vna-se-probe.zip");
    let cb = CopperBoard::new(Arc::new(read_path(&job, &ReadOptions::default()).unwrap()));
    // vna-se-probe's islands are 3.5 mm apart at least: every pair under
    // 20 mm is a gap.
    let below = 20_000_000;
    let gaps: Vec<_> = (0..cb.built.copper.len())
        .flat_map(|l| cb.gaps(l, below).iter().cloned().collect::<Vec<_>>())
        .collect();
    assert!(gaps.len() >= 2, "{}", gaps.len());
    let violation = |g: &ryu_copper::connect::Gap<ryudfm::copper::Key>| {
        let net = |id| {
            cb.claims()
                .get(id)
                .and_then(|c| c.first().cloned())
                .unwrap()
                .replace('_', " ")
        };
        let layer = cb
            .layer_name(g.layer)
            .replace("cu", "Cu")
            .replace("f.", "F.")
            .replace("b.", "B.")
            .replace("in", "In");
        json!({
            "type": "clearance",
            "description": "Clearance violation (rule 'ryudfm clearance' clearance 20.0000 mm; actual 1.0000 mm)",
            "items": [
                {"description": format!("Via [{}] on F.Cu - B.Cu", net(&g.islands.0))},
                {"description": format!("Track [{}] on {layer}, length 1.0 mm", net(&g.islands.1))},
            ],
        })
    };
    // KiCad says GND is open: vna's CAD netlist judged nothing, so it isn't
    // compared.
    let unconnected = json!([{"items": [{"description": "Pad 1 [GND] of J1 on F.Cu"}]}]);
    // Every gap KiCad lists is ours, but not all of ours are listed: a
    // disagreement unless the report is cut.
    let one = json!({"violations": [violation(&gaps[0])], "unconnected_items": unconnected});
    let found = compare_with_drc(&cb, &one, below);
    assert!(
        !found.is_empty()
            && found
                .iter()
                .all(|f| f.contains("for ryudfm, not for KiCad")),
        "{found:?}"
    );
    let cut = json!({
        "violations": vec![violation(&gaps[0]); KICAD_REPORT_CAP],
        "unconnected_items": unconnected,
    });
    assert_eq!(compare_with_drc(&cb, &cut, below), Vec::<String>::new());
    // Copper of no net on a net's, given as 0: a contact, one island for
    // us, not a disagreement.
    let mut contact = violation(&gaps[0]);
    contact["description"] = json!(
        "Clearance violation (rule 'ryudfm clearance' clearance 0.2000 mm; actual 0.0000 mm)"
    );
    contact["items"][0]["description"] = json!("Zone [<no net>] on F.Cu");
    let mut v = vec![violation(&gaps[0]); KICAD_REPORT_CAP];
    v.push(contact);
    assert_eq!(
        compare_with_drc(&cb, &json!({"violations": v}), below),
        Vec::<String>::new()
    );
    // A gap only KiCad lists is a disagreement, cut or not.
    let mut extra = violation(&gaps[0]);
    extra["items"][1]["description"] = json!("Track [NOT A NET] on F.Cu, length 1.0 mm");
    let mut v = vec![violation(&gaps[0]); KICAD_REPORT_CAP];
    v.push(extra);
    let found = compare_with_drc(&cb, &json!({"violations": v}), below);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("for KiCad, not for ryudfm"), "{found:?}");
}
