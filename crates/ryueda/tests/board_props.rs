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
    let mut inner_moves = 0;
    for seed in 0..32u64 {
        let spec = board_gen::seeded_multilayer(seed);
        if spec.layers == 2 {
            continue;
        }
        tried += 1;
        let set = |f: &[drc::Finding]| -> std::collections::BTreeSet<String> {
            f.iter().map(|x| format!("{:?}", x.code)).collect()
        };
        // An inner connection's vias made blind, top to in1: their spans
        // alone. Its in1 leg moved to in2, below them: it meets neither via,
        // so exactly its net opens as well.
        if let Some(n) = (0..spec.count() - 1).find(|&n| spec.inner(n)) {
            let blind = check(&Spec {
                fault: Some(Fault::InnerBlind(n)),
                ..spec.clone()
            });
            assert_eq!(
                codes(&blind),
                [Code::DrcViaSpan, Code::DrcViaSpan],
                "seed {seed}: {blind:?}"
            );
            let moved = check(&Spec {
                fault: Some(Fault::InnerMove(n)),
                ..spec.clone()
            });
            let opens: Vec<&drc::Finding> =
                moved.iter().filter(|f| f.code == Code::NetOpen).collect();
            assert_eq!(opens.len(), 1, "seed {seed}: {moved:?}");
            assert_eq!(
                moved.len(),
                3,
                "seed {seed}: the two spans and the open: {moved:?}"
            );
            inner_moves += 1;
        }
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
    assert!(inner_moves >= 4, "{inner_moves}");
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

fn check_all(spec: &Spec) -> drc::BoardCheck {
    let d = tempfile::tempdir().unwrap();
    board_gen::write(d.path(), spec);
    let p = ryueda::project::Project::load(d.path()).unwrap();
    assert!(p.faults().iter().all(|f| !f.is_error()), "{:?}", p.faults());
    let id = ryueda::id::Id::parse("main", "board").unwrap();
    drc::check(&p, &p.boards[&id]).unwrap()
}

fn tracks_of(spec: &Spec) -> usize {
    let d = tempfile::tempdir().unwrap();
    board_gen::write(d.path(), spec);
    std::fs::read_dir(d.path().join("boards/main/tracks"))
        .unwrap()
        .count()
}

/// Board rules raise exactly the findings they should (ryueda 7.B): a
/// clearance just over the gap between neighbouring rows gives gaps and
/// nothing else, the preset's own gives none, a width a nanometre over
/// the tracks' gives one finding a track; waiving every gap clears the
/// board, and a track nudged a nanometre closer exceeds its waivers.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: seeded boards checked")]
fn rules_raise_exactly_the_findings_they_should() {
    let floor = ryueda::board::preset::JLC_2L.clearance_nm;
    for seed in 0..16u64 {
        let spec = board_gen::seeded(seed);
        // Neighbouring rows' tracks run along the rows' centres, the row
        // pitch apart: their copper is the pitch less a track's width
        // apart. A nanometre over that, every row's track is under it
        // (pads, wider than tracks, closer still).
        let gap = spec.pitch.1 - spec.width;
        let ruled = |rules: serde_json::Value| Spec {
            rules: Some(rules),
            ..spec.clone()
        };
        let over = ruled(serde_json::json!({"defaults": {"clearance_nm": gap + 1}}));
        let c = check_all(&over);
        assert!(!c.findings.is_empty(), "seed {seed}");
        assert!(
            c.findings.iter().all(|f| f.code == Code::DrcClearance),
            "seed {seed}: {:?}",
            &c.findings[..c.findings.len().min(5)]
        );
        let at_floor = check_all(&ruled(
            serde_json::json!({"defaults": {"clearance_nm": floor}}),
        ));
        assert!(
            at_floor.findings.is_empty(),
            "seed {seed}: {:?}",
            at_floor.findings
        );
        let wide = check_all(&ruled(
            serde_json::json!({"defaults": {"track_width_nm": spec.width + 1}}),
        ));
        assert!(
            wide.findings.iter().all(|f| f.code == Code::DrcTrackWidth),
            "seed {seed}: {:?}",
            &wide.findings[..wide.findings.len().min(5)]
        );
        assert_eq!(wide.findings.len(), tracks_of(&spec), "seed {seed}");

        // Every gap waived at its own distance: no errors, none unused.
        let waivers: serde_json::Map<String, serde_json::Value> = c
            .findings
            .iter()
            .map(|f| {
                (
                    f.id.clone(),
                    serde_json::json!({"code": f.code, "refs": f.refs, "reason": "seeded",
                        "by": "agent", "limit": f.details["distance_nm"]}),
                )
            })
            .collect();
        let rules = serde_json::json!({"defaults": {"clearance_nm": gap + 1}, "waivers": waivers});
        let waived = check_all(&ruled(rules.clone()));
        assert_eq!(waived.errors(), 0, "seed {seed}");
        assert_eq!(waived.waived(), c.findings.len(), "seed {seed}");
        assert!(
            waived.findings.iter().all(|f| f.code != Code::WaiverUnused),
            "seed {seed}"
        );

        // A connection within a row below the first, on top, moved a
        // nanometre towards the row above: the gaps it shares with that
        // row get worse than their waivers, and only those.
        let n = (spec.cols..spec.count() - 1)
            .find(|&n| spec_same_row(&spec, n) && !spec.by_vias(n))
            .unwrap();
        let nudged = check_all(&Spec {
            fault: Some(Fault::Nudge(n)),
            rules: Some(rules),
            ..spec.clone()
        });
        let exceeded: Vec<&drc::Finding> = nudged
            .findings
            .iter()
            .filter(|f| f.details.get("waiver_exceeded").is_some())
            .collect();
        assert!(!exceeded.is_empty(), "seed {seed}");
        let track = format!("main:track/t{n}_");
        assert!(
            exceeded
                .iter()
                .all(|f| f.refs.iter().any(|r| r.starts_with(&track))),
            "seed {seed}: {exceeded:?}"
        );
        assert_eq!(nudged.errors(), exceeded.len(), "seed {seed}");
        assert!(exceeded.iter().all(|f| f.is_error()), "seed {seed}");
    }
}

/// A seeded `rules.json` (defaults, two classes, waivers) reads back and
/// writes the same bytes (ryueda 7.B).
#[test]
fn rules_on_disk_round_trip() {
    for seed in [0u64, 3, 9] {
        let mut s = seed;
        let mut draw = |lo: i64, span: i64| lo + (board_gen::next(&mut s) as i64).rem_euclid(span);
        let rules = serde_json::json!({
            "defaults": {"clearance_nm": draw(100_000, 200_000), "track_width_nm": draw(100_000, 300_000),
                "via": {"drill_nm": 300_000, "diameter_nm": draw(600_000, 200_000)}},
            "classes": {
                "power": {"track_width_nm": draw(300_000, 500_000), "clearance_nm": draw(150_000, 100_000)},
                "signal": {"clearance_nm": draw(100_000, 100_000),
                    "pour": {"clearance_nm": draw(200_000, 100_000)}}},
            "waivers": {
                format!("{:016x}", draw(0, i64::MAX)): {"code": "drc_clearance",
                    "refs": ["main:R1/1", "main:R2/1"], "reason": "measured", "by": "agent",
                    "limit": draw(50_000, 50_000)},
                format!("{:016x}", draw(0, i64::MAX)): {"code": "drc_track_width",
                    "refs": ["main:track/t0_0"], "reason": "neck", "by": "agent"}},
        });
        let d = tempfile::tempdir().unwrap();
        board_gen::write(
            d.path(),
            &Spec {
                rules: Some(rules),
                ..board_gen::seeded(seed)
            },
        );
        let path = d.path().join("boards/main/rules.json");
        let text = std::fs::read_to_string(&path).unwrap();
        let p = ryueda::project::Project::load(d.path()).unwrap();
        let id = ryueda::id::Id::parse("main", "board").unwrap();
        let loaded = p.boards[&id].rules.as_ref().unwrap();
        let file = loaded.item.as_ref().unwrap();
        let mut v = serde_json::to_value(file).unwrap();
        v["format"] = serde_json::json!(ryueda::conn::format_of("ryueda.board-rules", &v));
        v["kind"] = serde_json::json!("ryueda.board-rules");
        assert_eq!(ryueda::canonical::to_string(&v), text, "seed {seed}");
    }
}
