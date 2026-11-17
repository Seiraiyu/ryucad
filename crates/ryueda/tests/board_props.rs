//! Generated boards (design 732): parts on a grid, tracks between their
//! pads, some by vias, have no findings; each seeded fault gives exactly
//! its finding; one thread and eight alike.

#[path = "support/board_gen.rs"]
mod board_gen;

use board_gen::{Fault, Spec};
use ryueda::board::drc;
use ryueda::error::Code;

fn check(spec: &Spec) -> Vec<drc::Finding> {
    let d = tempfile::tempdir().unwrap();
    board_gen::write(d.path(), spec);
    let p = ryueda::project::Project::load(d.path()).unwrap();
    assert!(p.faults().iter().all(|f| !f.is_error()), "{:?}", p.faults());
    let id = ryueda::id::Id::parse("main", "board").unwrap();
    drc::check(&p, &p.boards[&id]).unwrap().findings
}

fn codes(f: &[drc::Finding]) -> Vec<Code> {
    f.iter().map(|x| x.code).collect()
}

#[test]
fn seeded_boards_are_clean_and_each_fault_is_exactly_its_finding() {
    for seed in 0..12u64 {
        let spec = board_gen::seeded(seed);
        let clean = check(&spec);
        assert!(
            clean.is_empty(),
            "seed {seed}: {:?}",
            &clean[..clean.len().min(5)]
        );
        // A connection within a row, not by vias, and a part off the row's ends and below the last row.
        let n = (0..spec.count() - 1)
            .find(|&n| spec_same_row(&spec, n) && !spec.by_vias(n))
            .unwrap();
        let short = check(&Spec {
            fault: Some(Fault::Short(n)),
            ..spec.clone()
        });
        assert_eq!(codes(&short), [Code::NetShort], "seed {seed}: {short:?}");
        // Row 1, column 1: never a row's end; its neighbour above is part 3 × cols - 2.
        let m = spec.cols + 1;
        if 3 * spec.cols - 2 < spec.count() {
            let over = check(&Spec {
                fault: Some(Fault::Overlap(m)),
                ..spec.clone()
            });
            assert_eq!(
                codes(&over),
                [Code::DrcCourtyardOverlap],
                "seed {seed}: {over:?}"
            );
        }
        if let Some(v) = (0..spec.count() - 1).find(|&n| spec.by_vias(n)) {
            let shrunk = check(&Spec {
                fault: Some(Fault::ShrunkVia(v)),
                ..spec.clone()
            });
            assert_eq!(
                codes(&shrunk),
                [Code::DrcAnnularRing],
                "seed {seed}: {shrunk:?}"
            );
        }
    }
}

fn spec_same_row(s: &Spec, n: usize) -> bool {
    n / s.cols == (n + 1) / s.cols
}

#[test]
fn a_seeded_board_checks_alike_on_one_thread_and_eight() {
    let spec = Spec {
        fault: Some(Fault::Short(1)),
        ..board_gen::seeded(7)
    };
    let run = |n| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| serde_json::to_string(&check(&spec)).unwrap())
    };
    assert_eq!(run(1), run(8));
}
