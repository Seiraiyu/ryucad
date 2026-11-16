//! A board's copper through ryudfm: the layout from small jobs made for
//! the test, and what `check` says of them.

use std::path::Path;

use ryu_board::geom::Point;
use ryu_board::model::{Feature, FeatureKind, Features, Polarity, Symbol};
use ryu_board::symbol::StandardSymbol;
use ryudfm::testing::{folder, tgz, write_features};

/// minimal-clean with its top layer two round pads 10 mm apart, and a CAD
/// netlist of one net with a point on each: an open.
pub fn opened(dir: &Path) -> std::path::PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = folder(&here.join("tests/corpus/minimal-clean"));
    let pad = |x| {
        Feature::new(
            FeatureKind::Pad {
                symbol: 0,
                place: ryu_board::geom::Place::at(Point::new(x, 10_000_000)),
            },
            Polarity::Positive,
        )
    };
    let top = Features::new(
        vec![Symbol::Standard(StandardSymbol::Round { d: 1_000_000 })],
        vec![pad(10_000_000), pad(20_000_000)],
    );
    let mut found = (false, false);
    for (path, bytes) in &mut files {
        if path == "steps/pcb/layers/top/features" {
            *bytes = write_features(&top).into_bytes();
            found.0 = true;
        }
        if path == "steps/pcb/netlists/cadnet/netlist" {
            *bytes = b"UNITS=MM\nH optimize n staggered n\n$0 SIG\n#\n0 0.5 10 10 T e e staggered 0 0 0\n0 0.5 20 10 T e e staggered 0 0 0\n".to_vec();
            found.1 = true;
        }
    }
    assert_eq!(found, (true, true), "minimal-clean's files moved");
    let path = dir.join("opened.tgz");
    std::fs::write(&path, tgz(&files)).unwrap();
    path
}

#[test]
fn an_opened_job_fails_check_with_net_open() {
    let dir = tempfile::tempdir().unwrap();
    let e = ryudfm::check::path(&opened(dir.path()), None).unwrap_err();
    assert_eq!(e.code, ryudfm::error::Code::CheckFailed);
    let findings = e.details["report"]["findings"].as_array().unwrap();
    let open = findings
        .iter()
        .find(|f| f["code"] == "net_open")
        .unwrap_or_else(|| panic!("{findings:#?}"));
    assert_eq!(open["severity"], "error");
    assert_eq!(open["source"], "copper");
    assert_eq!(open["details"]["net"], "SIG");
    assert_eq!(open["details"]["islands"].as_array().unwrap().len(), 2);
}

#[test]
fn the_committed_minimal_clean_has_no_copper_errors() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let v = ryudfm::check::path(&corpus.join("minimal-clean.tgz"), None).unwrap();
    assert_eq!(v["errors"], 0, "{v:#}");
}
