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

/// Seeded boards on 2, 4 and 6 copper layers, inner connections and parts
/// on the bottom among them, have no findings at all, alike on one thread
/// and eight (ryueda 7.A).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: 32 boards checked")]
fn generated_multilayer_boards_are_clean() {
    let mut layered = 0;
    let mut bottoms = 0;
    for seed in 0..32u64 {
        let spec = board_gen::seeded_multilayer(seed);
        layered += usize::from(spec.layers > 2);
        bottoms += usize::from((0..spec.count()).any(|n| spec.bottom(n)));
        let run = |threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| check(&spec))
        };
        let one = run(1);
        assert!(
            one.is_empty(),
            "seed {seed} ({} layers, bottom every {}): {:?}",
            spec.layers,
            spec.bottom_every,
            &one[..one.len().min(5)]
        );
        if seed % 4 == 0 {
            assert_eq!(
                serde_json::to_string(&one).unwrap(),
                serde_json::to_string(&run(8)).unwrap(),
                "seed {seed}"
            );
        }
    }
    assert!(
        layered >= 8 && bottoms >= 8,
        "{layered} layered, {bottoms} with bottom parts"
    );
}

/// Each seeded fault of 7.A's gives exactly its findings: a via cut short
/// of the bottom, a part turned over in place, another net's copper inside
/// a via's hole clearance.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: seeded boards checked")]
fn each_seeded_layer_fault_gives_its_finding() {
    let mut tried = 0;
    for seed in 0..32u64 {
        let spec = board_gen::seeded_multilayer(seed);
        if spec.layers == 2 {
            continue;
        }
        tried += 1;
        let set = |f: &[drc::Finding]| -> std::collections::BTreeSet<String> {
            f.iter().map(|x| format!("{:?}", x.code)).collect()
        };
        if let Some(v) = (0..spec.count() - 1).find(|&n| spec.by_vias(n)) {
            let cut = check(&Spec {
                fault: Some(Fault::ViaToIn1(v)),
                ..spec.clone()
            });
            assert_eq!(
                set(&cut),
                ["NetOpen", "DrcViaSpan"]
                    .map(String::from)
                    .into_iter()
                    .collect(),
                "seed {seed}: {cut:?}"
            );
            assert_eq!(
                cut.iter().filter(|f| f.code == Code::DrcViaSpan).count(),
                1,
                "seed {seed}"
            );
            let gap = check(&Spec {
                fault: Some(Fault::HoleGap(v)),
                ..spec.clone()
            });
            assert_eq!(
                codes(&gap),
                [Code::DrcHoleClearance],
                "seed {seed}: {gap:?}"
            );
            assert_eq!(gap[0].details["rule_nm"], 200_000, "seed {seed}");
        }
        // A top part within a row turned over where it is: its nets open,
        // no courtyard overlap.
        if let Some(n) = (1..spec.count() - 1)
            .find(|&n| !spec.bottom(n) && !spec.bottom(n - 1) && !spec.bottom(n + 1))
        {
            let flipped = check(&Spec {
                fault: Some(Fault::Flip(n)),
                ..spec.clone()
            });
            assert!(!flipped.is_empty(), "seed {seed}");
            assert!(
                flipped.iter().all(|f| f.code == Code::NetOpen),
                "seed {seed}: {flipped:?}"
            );
        }
    }
    assert!(tried >= 8, "{tried}");
}

/// Every file a seeded board writes reads back and writes the same bytes,
/// at the format it needs: files holding nothing new say format 1.
#[test]
fn layers_on_disk_round_trip() {
    for seed in [0u64, 1, 2, 3, 5, 8] {
        let spec = board_gen::seeded_multilayer(seed);
        let d = tempfile::tempdir().unwrap();
        board_gen::write(d.path(), &spec);
        let mut seen = 0;
        let mut stack = vec![d.path().join("boards")];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                let kind = v["kind"].as_str().unwrap().to_owned();
                let mut bare = v.clone();
                bare.as_object_mut().unwrap().remove("kind");
                bare.as_object_mut().unwrap().remove("format");
                assert_eq!(
                    v["format"],
                    ryueda::conn::format_of(&kind, &bare),
                    "{}",
                    path.display()
                );
                assert_eq!(ryueda::canonical::to_string(&v), text, "{}", path.display());
                seen += 1;
            }
        }
        assert!(seen > 0);
        let p = ryueda::project::Project::load(d.path()).unwrap();
        assert!(
            p.faults().iter().all(|f| !f.is_error()),
            "seed {seed}: {:?}",
            p.faults()
        );
    }
}
