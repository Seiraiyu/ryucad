//! minimal-clean broken on purpose: each breakage fails the job with its
//! code, or loads with its fault and the rest of the board intact.

use std::path::Path;

use ryudfm::odb::{Code, Job, Limits, ReadOptions, read};
use ryudfm::testing::{Breakage, broken, folder, summary};

fn job(b: Breakage) -> Result<ryu_board::model::Board, ryudfm::odb::Error> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/minimal-clean");
    let bytes = broken(&folder(&dir), "pcb", "top", b);
    read(
        &Job::from_archive("minimal-clean", &bytes, Limits::default())?,
        &ReadOptions::default(),
    )
}

fn faults(b: &ryu_board::model::Board) -> Vec<(String, Option<String>)> {
    b.faults
        .iter()
        .map(|f| (f.code.clone(), f.about.clone()))
        .collect()
}

#[test]
fn each_breakage_fails_what_it_should() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/minimal-clean");
    let whole = read(
        &Job::open(&dir, Limits::default()).unwrap(),
        &ReadOptions::default(),
    )
    .unwrap();
    let top = whole.layer("top").unwrap();
    for b in Breakage::ALL {
        let r = job(b);
        match b {
            Breakage::NoMatrix => assert_eq!(r.unwrap_err().code, Code::NotABoard),
            Breakage::DotDot => assert_eq!(r.unwrap_err().code, Code::ArchiveUnsafe),
            Breakage::EmptyLayer => {
                // The layer reads, empty; eda/data still names its features.
                let board = r.unwrap();
                assert_eq!(
                    faults(&board),
                    [("odb_eda_invalid".into(), Some("pcb".into()))]
                );
                assert!(board.steps[0].layers[top].features().is_empty());
            }
            Breakage::CorruptFeatures | Breakage::MissingLayer => {
                let board = r.unwrap();
                assert_eq!(
                    faults(&board),
                    [("odb_layer_unreadable".into(), Some("pcb/top".into()))],
                    "{b:?}"
                );
                let f = &board.faults[0];
                let at = f.location.as_ref().unwrap();
                assert_eq!(at.path, "steps/pcb/layers/top/features");
                if b == Breakage::CorruptFeatures {
                    assert!(at.line.is_some());
                }
                assert!(board.steps[0].layers[top].features().is_empty());
                // Everything else loads.
                let bottom = board.layer("bottom").unwrap();
                assert_eq!(
                    board.steps[0].layers[bottom].features().len(),
                    whole.steps[0].layers[bottom].features().len()
                );
            }
            Breakage::UnknownSymbol => {
                let board = r.unwrap();
                assert_eq!(
                    faults(&board),
                    [("odb_symbol_unknown".into(), Some("pcb/top".into()))]
                );
                assert!(board.faults[0].message.contains("no_such_symbol"));
                let last = board.steps[0].layers[top].features().len() - 1;
                assert!(board.feature_shape(0, top, last).is_err());
            }
            Breakage::SymbolCycle => {
                let board = r.unwrap();
                assert_eq!(
                    faults(&board),
                    [("odb_symbol_cycle".into(), Some("loop_a".into()))]
                );
                assert!(board.faults[0].message.contains("loop_a, loop_b"));
                let last = board.steps[0].layers[top].features().len() - 1;
                assert!(board.feature_shape(0, top, last).is_err());
            }
        }
    }
}

#[test]
fn a_broken_layer_leaves_the_rest_as_it_was() {
    // Reading minimal-clean with the top layer gone changes only that
    // layer's count and the faults.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/minimal-clean");
    let good = read(
        &Job::open(&dir, Limits::default()).unwrap(),
        &ReadOptions::default(),
    )
    .unwrap();
    let bad = job(Breakage::MissingLayer).unwrap();
    let (g, b) = (summary(&good), summary(&bad));
    for (x, y) in g.lines().zip(b.lines()) {
        if x != y {
            println!("{x}\n{y}");
        }
    }
    let differing: Vec<&str> = g
        .lines()
        .zip(b.lines())
        .filter(|(x, y)| x != y)
        .map(|(x, _)| x.split_whitespace().next().unwrap())
        .collect();
    // The top layer's attributes went with it: its copper weight, and so
    // the stackup's completeness.
    assert_eq!(differing, ["features", "capabilities", "faults"]);
}

#[test]
fn steps_versions_and_info() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/minimal-clean");
    let files = folder(&dir);
    let with = |edit: &dyn Fn(&mut ryudfm::testing::Files)| {
        let mut f = files.clone();
        edit(&mut f);
        Job::from_files(ryudfm::odb::Container::Folder, "m", f, Limits::default()).unwrap()
    };
    // A second step nothing repeats: two candidates.
    let two = with(&|f| {
        for (p, d) in f.iter_mut() {
            if p == "matrix/matrix" {
                d.extend_from_slice(b"STEP {\nCOL=2\nNAME=COUPON\n}\n");
            }
        }
        f.push(("steps/coupon/stephdr".into(), b"UNITS=INCH\n".to_vec()));
    });
    let e = read(&two, &ReadOptions::default()).unwrap_err();
    assert_eq!(
        (e.code, e.steps.clone()),
        (
            Code::StepAmbiguous,
            vec!["pcb".to_string(), "coupon".into()]
        )
    );
    let b = read(
        &two,
        &ReadOptions {
            step: Some("PCB".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((b.steps.len(), b.board_step), (2, 0));
    // The coupon's layers have no files: a fault each, and the board loads.
    assert!(
        b.faults
            .iter()
            .all(|f| f.about.as_deref().is_some_and(|a| a.starts_with("coupon")))
    );
    let e = read(
        &two,
        &ReadOptions {
            step: Some("panel".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.code, Code::StepNotFound);
    // Version 6 is refused; no misc/info is read as 7.0 with a warning.
    let six = with(&|f| {
        for (p, d) in f.iter_mut() {
            if p == "misc/info" {
                *d = String::from_utf8_lossy(d)
                    .replace("ODB_VERSION_MAJOR=8", "ODB_VERSION_MAJOR=6")
                    .into_bytes();
            }
        }
    });
    assert_eq!(
        read(&six, &ReadOptions::default()).unwrap_err().code,
        Code::VersionUnsupported
    );
    let none = with(&|f| f.retain(|(p, _)| p != "misc/info"));
    let b = read(&none, &ReadOptions::default()).unwrap();
    assert_eq!(b.source.version, "7.0");
    assert!(b.warnings.iter().any(|w| w.code == "odb_info_missing"));
    // No folder for the board step fails the job.
    let gone = with(&|f| f.retain(|(p, _)| !p.starts_with("steps/")));
    assert_eq!(
        read(&gone, &ReadOptions::default()).unwrap_err().code,
        Code::Unreadable
    );
}
