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

/// A generated part's pad reference, as the board names it.
fn pad_ref(c: &drc::BoardCheck, occurrence: &str, number: &str) -> String {
    c.shapes
        .pads
        .iter()
        .find(|p| p.occurrence == occurrence && p.number.as_deref() == Some(number))
        .unwrap_or_else(|| panic!("{occurrence}/{number}"))
        .reference
        .clone()
}

/// Constraints and keepouts give exactly their findings (ryueda 7.C):
/// each part near its neighbour's pad at the gap between them holds, a
/// nanometre under fails once a pair; the leftmost parts hold to the left
/// edge at its distance and fail a nanometre under; a keepout over a
/// track or a part gives findings of that alone; drawings give none.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: seeded boards checked")]
fn constraints_and_keepouts_give_exactly_their_findings() {
    use board_gen::{MARGIN, MM, PAD, UM};
    use serde_json::json;
    for seed in 0..16u64 {
        let spec = board_gen::seeded(seed);
        let base = check_all(&spec);
        assert!(base.findings.is_empty(), "seed {seed}");
        let same_row = |n: usize| n / spec.cols == (n + 1) / spec.cols;
        // Within a row, part n's pad 2 and part n + 1's pad 1 face each
        // other: their centres a pitch less twice the pads' offset apart,
        // their copper a pad's width (0.56 mm) less.
        let gap = spec.pitch.0 - 2 * PAD - 560 * UM;
        let pairs: Vec<usize> = (0..spec.count() - 1).filter(|&n| same_row(n)).collect();
        let near = |d: i64| {
            Spec {
            constraints: pairs
                .iter()
                .map(|&n| {
                    (
                        n,
                        json!({"near": pad_ref(&base, &spec.occurrence(n + 1), "1"), "within_nm": d}),
                    )
                })
                .collect(),
            ..spec.clone()
        }
        };
        let at = check_all(&near(gap));
        assert!(
            at.findings.is_empty(),
            "seed {seed}: {:?}",
            &at.findings[..at.findings.len().min(3)]
        );
        let under = check_all(&near(gap - 1));
        assert!(
            under
                .findings
                .iter()
                .all(|f| f.code == Code::PlacementConstraintUnmet),
            "seed {seed}"
        );
        assert_eq!(under.findings.len(), pairs.len(), "seed {seed}");
        // The leftmost parts' courtyards (2 mm wide) lie MARGIN - 1 mm from
        // the left edge.
        let left: Vec<usize> = (0..spec.count()).filter(|&n| spec.leftmost(n)).collect();
        let edge = |d: i64| Spec {
            constraints: left
                .iter()
                .map(|&n| (n, json!({"edge": "left", "within_nm": d})))
                .collect(),
            ..spec.clone()
        };
        assert!(
            check_all(&edge(MARGIN - MM)).findings.is_empty(),
            "seed {seed}"
        );
        assert_eq!(
            check_all(&edge(MARGIN - MM - 1)).findings.len(),
            left.len(),
            "seed {seed}"
        );
        // A keepout keeping tracks over the middle of a connection's gap.
        let n = pairs[0];
        let base_tracks: Vec<&ryueda::board::shapes::TrackInfo> = base
            .shapes
            .tracks
            .iter()
            .filter(|t| t.id.as_str().starts_with(&format!("t{n}_")))
            .collect();
        let b = base_tracks[0].shape.bounds().unwrap();
        let (cx, cy) = ((b.min.x + b.max.x) / 2, (b.min.y + b.max.y) / 2);
        let square = |x: i64, y: i64, h: i64| {
            json!({"start": [x - h, y - h], "edges": [
                {"type": "line", "to": [x + h, y - h]}, {"type": "line", "to": [x + h, y + h]},
                {"type": "line", "to": [x - h, y + h]}, {"type": "line", "to": [x - h, y - h]}]})
        };
        let kept = check_all(&Spec {
            keepouts: vec![
                json!({"contour": square(cx, cy, 50 * UM), "layers": ["top"], "keeps": ["tracks"]}),
            ],
            ..spec.clone()
        });
        assert!(!kept.findings.is_empty(), "seed {seed}");
        assert!(
            kept.findings.iter().all(|f| f.code == Code::DrcKeepout
                && f.refs.iter().any(|r| r.contains(&format!(":track/t{n}_")))),
            "seed {seed}: {:?}",
            kept.findings
        );
        // A keepout keeping placements over one part's centre.
        let m = (spec.cols + 1).min(spec.count() - 1);
        let pm = base
            .shapes
            .pads
            .iter()
            .filter(|p| p.occurrence == spec.occurrence(m))
            .collect::<Vec<_>>();
        let (mx, my) = ((pm[0].at.x + pm[1].at.x) / 2, (pm[0].at.y + pm[1].at.y) / 2);
        let part = check_all(&Spec {
            keepouts: vec![
                json!({"contour": square(mx, my, 100 * UM), "layers": ["all"], "keeps": ["placements"]}),
            ],
            ..spec.clone()
        });
        assert_eq!(part.findings.len(), 1, "seed {seed}: {:?}", part.findings);
        assert_eq!(part.findings[0].code, Code::DrcKeepout);
        // Drawings add nothing.
        let drawn = check_all(&Spec {
            drawings: vec![
                json!({"layer": "silk.top", "type": "line", "start": [0, 0], "end": [MM, MM], "width_nm": 150_000}),
                json!({"layer": "comments", "type": "text", "text": "seed", "at": [MM, MM], "size_nm": [MM, MM], "thickness_nm": 150_000}),
            ],
            ..spec.clone()
        });
        assert!(
            drawn.findings.is_empty(),
            "seed {seed}: {:?}",
            drawn.findings
        );
    }
}

/// The legaliser on generated boards (ryueda 7.C): a part asked to sit
/// near its neighbour's pad finds a place where, moved there, it adds only
/// opens; asked to touch a pad inside another courtyard, it says no room.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: seeded boards checked")]
fn the_legaliser_finds_room_or_says_why() {
    use board_gen::{MM, PAD, UM};
    use ryueda::board::file::{Constraint, PlacementSide};
    for seed in 0..8u64 {
        let spec = board_gen::seeded(seed);
        let d = tempfile::tempdir().unwrap();
        board_gen::write(d.path(), &spec);
        let p = ryueda::project::Project::load(d.path()).unwrap();
        let id = ryueda::id::Id::parse("main", "board").unwrap();
        let c = drc::check(&p, &p.boards[&id]).unwrap();
        let design = ryueda::id::Id::parse("d", "design").unwrap();
        let (_, occs) = ryueda::board::occ::occurrences(&p, &design).unwrap();
        let m = spec.cols / 2;
        let o = occs
            .iter()
            .find(|o| o.reference == spec.occurrence(m))
            .unwrap();
        let target = pad_ref(&c, &spec.occurrence(m + 1), "1");
        let gap = spec.pitch.0 - 2 * PAD - 560 * UM;
        let ask = |within_nm: i64| {
            let cs = [Constraint::Near {
                near: target.clone(),
                within_nm,
            }];
            ryueda::board::place::legalise(
                "main",
                &c.shapes,
                &c.rules,
                &c.preset,
                &ryueda::board::place::Ask {
                    occ: o,
                    rotation_mdeg: 0,
                    side: PlacementSide::Top,
                    constraints: &cs,
                    now: None,
                },
            )
        };
        let found = ask(gap + MM).unwrap();
        // Moved there: nothing but opens.
        let path = d.path().join(format!(
            "boards/main/placements/{}.json",
            spec.occurrence(m).replace('/', ".")
        ));
        let text = format!(
            r#"{{"at":[{},{}],"format":1,"kind":"ryueda.placement","rotation_mdeg":0}}"#,
            found["at"][0], found["at"][1]
        );
        std::fs::write(&path, text).unwrap();
        let p = ryueda::project::Project::load(d.path()).unwrap();
        let moved = drc::check(&p, &p.boards[&id]).unwrap();
        assert!(
            moved.findings.iter().all(|f| f.code == Code::NetOpen),
            "seed {seed}: {:?}",
            moved.findings
        );
        assert_eq!(
            ask(1).unwrap_err().code,
            Code::PlacementNoRoom,
            "seed {seed}"
        );
    }
}

/// Placements with constraints, keepouts and drawings written by a seeded
/// board read back at the format they need and write the same bytes
/// (ryueda 7.C).
#[test]
fn placements_keepouts_and_drawings_round_trip() {
    use board_gen::{MM, UM};
    use serde_json::json;
    for seed in [0u64, 4, 9] {
        let spec = Spec {
            constraints: vec![
                (0, json!({"near": "main:R2/1", "within_nm": 2 * MM})),
                (0, json!({"side": "top"})),
                (1, json!({"edge": "any", "within_nm": 5 * MM})),
            ],
            keepouts: vec![json!({"contour": {"start": [0, 0], "edges": [
                {"type": "line", "to": [MM, 0]}, {"type": "arc", "mid": [MM + 500 * UM, 500 * UM], "to": [MM, MM]},
                {"type": "line", "to": [0, MM]}, {"type": "line", "to": [0, 0]}]},
                "layers": ["top", "bottom"], "keeps": ["holes", "vias"]})],
            drawings: vec![
                json!({"layer": "fab.bottom", "type": "circle", "center": [MM, MM], "radius_nm": 300 * UM, "width_nm": 100 * UM}),
            ],
            ..board_gen::seeded(seed)
        };
        let d = tempfile::tempdir().unwrap();
        board_gen::write(d.path(), &spec);
        let mut seen = 0;
        for sub in ["placements", "keepouts", "drawings"] {
            for e in std::fs::read_dir(d.path().join("boards/main").join(sub))
                .unwrap()
                .flatten()
            {
                let text = std::fs::read_to_string(e.path()).unwrap();
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                let kind = v["kind"].as_str().unwrap().to_owned();
                let mut bare = v.clone();
                bare.as_object_mut().unwrap().remove("kind");
                bare.as_object_mut().unwrap().remove("format");
                assert_eq!(
                    v["format"],
                    ryueda::conn::format_of(&kind, &bare),
                    "{}",
                    e.path().display()
                );
                assert_eq!(ryueda::canonical::to_string(&v), text);
                seen += 1;
            }
        }
        assert!(seen >= 3);
        let p = ryueda::project::Project::load(d.path()).unwrap();
        let id = ryueda::id::Id::parse("main", "board").unwrap();
        let b = &p.boards[&id];
        assert!(
            b.faults().iter().all(|f| !f.is_error()),
            "seed {seed}: {:?}",
            b.faults()
        );
        assert_eq!(b.keepouts.len(), 1);
        assert_eq!(b.drawings.len(), 1);
        let first = b
            .placements
            .values()
            .find_map(|l| l.item.as_ref().ok().filter(|pl| !pl.constraints.is_empty()));
        assert!(first.is_some());
    }
}
