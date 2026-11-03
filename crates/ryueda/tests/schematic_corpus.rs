//! The schematic corpus: each circuit entered by its script (symbols,
//! library components and connectivity, by command), laid out from
//! nothing, every hard rule held on every sheet, the same layout on one
//! thread and eight, and each circuit's sheets rendered into
//! `target/ryueda-look` for people to look at; scripted edits changing
//! only what they name, and scripted branches merging their layouts.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/buck.rs"]
mod buck;
#[path = "support/draw.rs"]
mod draw;
#[path = "support/ethernet.rs"]
mod ethernet;
#[path = "support/frontend.rs"]
mod frontend;
#[path = "support/rp2040.rs"]
mod rp2040;

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

/// `ryueda` in `dir`, in process, its IDs a fixed sequence.
fn ryueda(dir: &Path, threads: usize, args: &[&str]) -> Value {
    let out = Command::cargo_bin("ryueda")
        .unwrap()
        .current_dir(dir)
        .env("RYUEDA_NO_DAEMON", "1")
        .env("RYUEDA_ID_SEED", "corpus")
        .env("RAYON_NUM_THREADS", threads.to_string())
        .env("RYUEDA_CONFIG_DIR", dir.join("..").join("config"))
        .args(args)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true, "{args:?}: {v}");
    v["result"].clone()
}

/// A project holding a circuit, entered by its script.
fn circuit(dir: &Path, name: &str) -> std::path::PathBuf {
    let board = dir.join(name);
    std::fs::create_dir_all(&board).unwrap();
    ryueda(&board, 1, &["init"]);
    let script = match name {
        "breakout" => {
            for (path, text) in breakout::footprints() {
                let at = board.join(path);
                std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                std::fs::write(at, text).unwrap();
            }
            breakout::script()
        }
        "buck" => buck::script(),
        "frontend" => frontend::script(),
        "ethernet" => ethernet::script(),
        _ => rp2040::script(),
    };
    for args in script {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ryueda(&board, 1, &args);
    }
    board
}

/// Every finding of a circuit's blocks, the circuit entered by its script
/// and then every block laid out again from nothing with no instructions,
/// as round projects are (each finding with its `block`).
fn fresh(dir: &Path, name: &str) -> (std::path::PathBuf, Vec<Value>) {
    let board = circuit(dir, name);
    let p = ryueda::project::Project::load(&board).unwrap();
    for b in p.conn.blocks.values() {
        ryueda(&board, 1, &["layout", "--relayout", b.id.as_str()]);
    }
    let d = ryueda(&board, 1, &["describe"]);
    let mut out = Vec::new();
    for b in d["blocks"].as_array().unwrap() {
        for s in b["sheets"].as_array().unwrap() {
            for f in s["findings"].as_array().unwrap() {
                let mut f = f.clone();
                f["block"] = b["block_id"].clone();
                out.push(f);
            }
        }
    }
    (board, out)
}

/// The findings of one code.
fn coded<'a>(found: &'a [Value], code: &str) -> Vec<&'a Value> {
    found.iter().filter(|f| f["code"] == code).collect()
}

/// Laid out from nothing, the corpus is drawn as the lint teaches:
/// recognised circuits in their standard shapes (requirement 54), banks in
/// rows (55), local circuits wired (56). Release only (one to two minutes
/// in a debug build on a fast machine): its own step in full.yml.
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn the_corpus_from_nothing_draws_what_the_lint_teaches() {
    let dir = tempfile::tempdir().unwrap();
    let built: Vec<(&str, std::path::PathBuf, Vec<Value>)> =
        ["breakout", "rp2040", "buck", "frontend", "ethernet"]
            .into_iter()
            .map(|name| {
                let (board, found) = fresh(dir.path(), name);
                (name, board, found)
            })
            .collect();
    // Every hard rule on every sheet, each block laid out again over its
    // own layout unchanged; each block's sheets and scores printed (the
    // numbers round 1's report starts from).
    for (name, board, _) in &built {
        let p = ryueda::project::Project::load(board).unwrap();
        for b in p.conn.blocks.values() {
            let laid = ryueda::schem::lay(&p, b, None, &[], ryusch::input::Relayout::None).unwrap();
            assert!(
                !laid.stale && laid.invalid.is_none(),
                "{name}/{}: stale {} invalid {:?}",
                b.id,
                laid.stale,
                laid.invalid
            );
            let broken = ryusch::rules::check(&laid.built.input, &laid.outcome.layout);
            assert!(broken.is_empty(), "{name}/{}: {broken:#?}", b.id);
            let (_, scores) = ryusch::lint::lint(&laid.built.input, &laid.outcome);
            // Findings an agent can act on (round 1's F20): the bypass
            // capacitor where its idiom put it isn't out of line, and the
            // jumper joined to EN's pull-down isn't far from its idiom.
            let refused = [
                ("breakout", "sch_out_of_line", "c1"),
                ("breakout", "sch_out_of_line", "u1"),
                ("buck", "sch_idiom_missed", "j5"),
            ];
            for f in ryusch::lint::lint(&laid.built.input, &laid.outcome).0 {
                assert!(
                    !refused
                        .iter()
                        .any(|(n, c, r)| n == name && *c == f.code && f.refs[0] == *r),
                    "{name}/{}: {}",
                    b.id,
                    f.message
                );
            }
            eprintln!(
                "from nothing {name}/{}: {:?}",
                b.id,
                scores
                    .iter()
                    .map(|s| (&s.sheet, s.score))
                    .collect::<Vec<_>>()
            );
            // Each block's sheets at least its threshold.
            let least = THRESHOLDS
                .iter()
                .find(|(c, k, _)| *c == *name && *k == b.id.as_str())
                .map(|(_, _, t)| *t)
                .unwrap_or_else(|| panic!("{name}/{}: no threshold", b.id));
            for s in &scores {
                assert!(
                    i64::from(s.score) >= least,
                    "{name}/{}: sheet {} scores {}, under its threshold {least}",
                    b.id,
                    s.sheet,
                    s.score
                );
            }
            // No sheet at 0, but the Ethernet PHY's (issue #429: its pull-ups
            // and bypass capacitors stand far from the 32-pin LAN8710A,
            // satellite placement round dense ICs that plan 1.D doesn't
            // hold; Task 13's thresholds record its score).
            if (*name, b.id.as_str()) != ("ethernet", "eth") {
                for s in &scores {
                    assert!(s.score > 0, "{name}/{}: sheet {} scores 0", b.id, s.sheet);
                }
            }
        }
    }
    let all: Vec<(&str, Vec<Value>)> = built.into_iter().map(|(n, _, f)| (n, f)).collect();
    let of = |name: &str| &all.iter().find(|(n, _)| *n == name).unwrap().1;
    // Recognised circuits in their standard shapes: the buck's power stage
    // and input (EN taken off before the bypass capacitors), the front
    // end's op-amp stages, wired.
    for (name, found) in &all {
        assert!(
            coded(found, "sch_topology").is_empty(),
            "{name}: {:#?}",
            coded(found, "sch_topology")
        );
    }
    assert!(
        !coded(of("buck"), "sch_jog").iter().any(|f| f["refs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "buck_en")),
        "BUCK-EN jogs"
    );
    // Banks in rows, on every circuit.
    for (name, found) in &all {
        assert!(
            coded(found, "sch_bank_not_in_row").is_empty(),
            "{name}: {:#?}",
            coded(found, "sch_bank_not_in_row")
        );
    }
    // Placement across clusters (work item 4530): at most 20 floating
    // parts over the corpus from nothing (35 before plan 1.D, 41 before
    // 1.C wired local circuits).
    for (name, found) in &all {
        eprintln!("{name}: {} floating", coded(found, "sch_floating").len());
        for f in coded(found, "sch_floating") {
            eprintln!("  {}", f["message"]);
        }
    }
    let floating: usize = all
        .iter()
        .map(|(_, f)| coded(f, "sch_floating").len())
        .sum();
    assert!(floating <= 20, "{floating} parts float");
    // Design 236's quality bar: each circuit's crossings at most 1.5 times
    // its reference's within the cut (the manifest's `crossings`).
    let mut over = Vec::new();
    for (name, found) in &all {
        let ours: u64 = coded(found, "sch_crossings")
            .iter()
            .map(|f| f["details"]["crossings"].as_u64().unwrap_or(1))
            .sum();
        let theirs = reference_crossings(name);
        // 1.5 times the reference's, and at least one crossing allowed
        // where the reference has none (a reference of 0 would make any
        // crossing a failure; design 236's bar is a ratio).
        let bar = (theirs * 3).div_ceil(2).max(1);
        eprintln!("{name}: {ours} crossings, the reference {theirs}, bar {bar}");
        if ours > bar {
            over.push(format!(
                "{name}: {ours} crossings against {theirs} in the reference"
            ));
        }
    }
    assert!(over.is_empty(), "{over:#?}");
}

/// Design 236's score bars: each block's least sheet score laid out from
/// nothing, (circuit, block, threshold). Each is the score measured less
/// 5, measured on cox-worker-2 (AMD Ryzen 7 9700X, 8 cores, shared) on
/// 2026-11-03 after plan 1.D's Tasks 1 to 11. Ethernet's PHY sheet scores
/// 0 (issue #429).
const THRESHOLDS: &[(&str, &str, i64)] = &[
    ("breakout", "breakout", 85),
    ("breakout", "sensor", 95),
    ("rp2040", "mitayi", 31),
    ("buck", "buck", 32),
    ("frontend", "amp", 47),
    ("frontend", "baldiff", 46),
    ("frontend", "ivconv", 83),
    ("ethernet", "eth", 0),
    ("ethernet", "status_led", 92),
];

/// A circuit's reference's crossings within its cut, from the manifest.
fn reference_crossings(name: &str) -> u64 {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/schematic_corpus/manifest.txt"),
    )
    .unwrap();
    let mut at = None;
    for line in text.lines() {
        if let Some(c) = line.strip_prefix("circuit ") {
            at = Some(c.trim());
        } else if at == Some(name)
            && let Some(n) = line.trim().strip_prefix("crossings ")
        {
            return n.parse().unwrap();
        }
    }
    panic!("{name}: no crossings in the manifest")
}

/// A net asked for as wires is wired once its parts are together (round
/// 1: the buck's N_R7_1 stayed two labels with R7 standing on R8, and
/// `--move "net N_R7_1 wire"` was accepted and dropped). Since 1.D R7
/// stands beside R8 from nothing and the net is wired, so a move labels
/// it first.
#[test]
fn a_net_asked_for_as_wires_is_wired() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "buck");
    ryueda(&board, 1, &["place", "buck", "--move", "net N_R7_1 label"]);
    let before = layout_of(&board, "buck");
    assert!(
        before["labels"].get("n_r7_1").is_some(),
        "labelled to start"
    );
    let out = ryueda(&board, 1, &["place", "buck", "--move", "net N_R7_1 wire"]);
    let after = layout_of(&board, "buck");
    let wired = after["labels"].get("n_r7_1").is_none() && after["routes"].get("n_r7_1").is_some();
    let noted = out["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["code"] == "net_style_not_held");
    assert!(wired || noted, "neither wired nor said why: {out}");
    assert!(wired, "{out}");
}

/// `ryueda` in `dir` as `ryueda` does, its whole envelope, whether or not
/// it succeeds.
fn ryueda_envelope(dir: &Path, args: &[&str]) -> Value {
    let out = Command::cargo_bin("ryueda")
        .unwrap()
        .current_dir(dir)
        .env("RYUEDA_NO_DAEMON", "1")
        .env("RYUEDA_ID_SEED", "corpus")
        .env("RAYON_NUM_THREADS", "1")
        .env("RYUEDA_CONFIG_DIR", dir.join("..").join("config"))
        .args(args)
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

/// `place` never writes a layout that breaks a hard rule (round 1, F11:
/// an RP2040 session's move overlapped r6 and a label, the file was then
/// laid out from nothing on the next read, and its earlier moves were
/// lost).
#[test]
fn place_refuses_a_layout_that_breaks_a_hard_rule() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "rp2040");
    ryueda(
        &board,
        1,
        &[
            "place",
            "mitayi",
            "--move",
            "sheet U2 C13 C16 C10 C5 C7 C9 JP4 D1 U3:2 C11 C14 C4 C2 C3 J7 J10 D3 R8 \"Sheet 1\"",
            "--move",
            "sheet U3:1 U3:3 Y1 C17 C18 R10 R9 SW1 C19 U1 C1 C12 C15 C6 C8 R1 R5 R6 JP6 R2 R11 SW2 \"Sheet 2\"",
            "--move",
            "sheet J1 J2 J3 J4 J5 J6 J8 J9 R3 R4 JP5 D2 R7 JP1 Connectors",
        ],
    );
    ryueda(
        &board,
        1,
        &[
            "place",
            "mitayi",
            "--move",
            "align R9 C19 SW1 column",
            "--move",
            "beside U1 U3:3 below",
        ],
    );
    let file = board.join("blocks/mitayi/layout.json");
    let before = std::fs::read(&file).unwrap();
    for dry in [true, false] {
        // A move the engine still can't fit: it overlaps a label. (Plan
        // 1.D's engine fits the moves this test first used; when it fits
        // this one too, choose another that `place --dry-run` refuses.)
        let mut args = vec!["place", "mitayi", "--move", "beside U3:1 U3:3 above"];
        if dry {
            args.push("--dry-run");
        }
        let v = ryueda_envelope(&board, &args);
        assert_eq!(v["error"]["code"], "layout_rule", "dry {dry}: {v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("overlap"),
            "{v}"
        );
    }
    assert_eq!(std::fs::read(&file).unwrap(), before, "nothing written");
}

/// Releasing an instruction lays out again what it placed (round 1, F10:
/// the parts stayed where the released instruction had put them, and
/// getting back took four undos).
#[test]
fn a_release_lays_out_again_what_it_placed() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "breakout");
    let before = layout_of(&board, "breakout")["placed"].clone();
    let start = before["r1"].clone();
    ryueda(&board, 1, &["place", "breakout", "beside R1 J1 below"]);
    let held = layout_of(&board, "breakout")["placed"]["r1"].clone();
    assert_ne!(held, start, "the instruction moves R1");
    let out = ryueda(&board, 1, &["place", "breakout", "release R1"]);
    assert_eq!(out["released"], serde_json::json!(["i1"]));
    let placed = layout_of(&board, "breakout")["placed"].clone();
    let after = placed["r1"].clone();
    assert_ne!(after, held, "R1 laid out again, not left where it was put");
    // Plan 13.4: back to the layout before the instruction, up to free
    // places. Every other part is where it was before the instruction,
    // and R1, laid out again into free space near its neighbours, is on
    // its old sheet within a few grid steps of where it stood.
    let mut others = placed.as_object().unwrap().clone();
    let mut was = before.as_object().unwrap().clone();
    others.remove("r1");
    was.remove("r1");
    assert_eq!(others, was, "every other part as before the instruction");
    assert_eq!(after["sheet"], start["sheet"], "R1 on its old sheet");
    let grid = 1_270_000.0; // 50 mil
    let d = |i: usize| (after["at"][i].as_f64().unwrap() - start["at"][i].as_f64().unwrap()).abs();
    assert!(
        d(0) <= 4.0 * grid && d(1) <= 4.0 * grid,
        "R1 near where it stood: {start} then {after}"
    );
}

/// `layout --dry-run --render` draws what laying a block out would write
/// (1.C's 13.8 left it): the same picture as rendering after the real
/// layout.
#[test]
fn a_layout_dry_run_renders() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "buck");
    let before = std::fs::read(board.join("blocks/buck/layout.json")).unwrap();
    let out = ryueda(
        &board,
        1,
        &[
            "layout",
            "--relayout",
            "buck",
            "--dry-run",
            "--render",
            "dry.svg",
            "--fit",
        ],
    );
    assert_eq!(out["format"], "svg", "{out}");
    assert_eq!(
        std::fs::read(board.join("blocks/buck/layout.json")).unwrap(),
        before,
        "a dry run writes nothing"
    );
    ryueda(&board, 1, &["layout", "--relayout", "buck"]);
    ryueda(
        &board,
        1,
        &["render", "--block", "buck", "--fit", "--out", "real.svg"],
    );
    assert_eq!(
        std::fs::read(board.join("dry.svg")).unwrap(),
        std::fs::read(board.join("real.svg")).unwrap()
    );
    // A picture is of one block: a design's layout with --render is refused.
    let v = ryueda_envelope(&board, &["layout", "--dry-run", "--render", "x.svg"]);
    assert_eq!(v["error"]["code"], "invalid_param", "{v}");
}

/// A move brings only the idioms anchored on the part it moves: moving one
/// op-amp unit beside the other moves no part of the other unit's stage
/// (round 1, F5: any move of U11:2 scattered baldiff).
#[test]
fn a_move_brings_only_its_own_idioms() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "frontend");
    let before = layout_of(&board, "baldiff")["placed"].clone();
    ryueda(
        &board,
        1,
        &["place", "baldiff", "--move", "beside U11:2 U11:1 right"],
    );
    let after = layout_of(&board, "baldiff")["placed"].clone();
    // U11:2 and what its idioms anchor on it (its stage, its divider and
    // the capacitor on its input).
    let d = ryueda(&board, 1, &["describe", "--block", "baldiff"]);
    let mut mine: Vec<String> = vec!["u11:2".into()];
    for i in d["blocks"][0]["idioms"].as_array().unwrap() {
        if i["serves"].as_str().unwrap().starts_with("u11:2/") {
            mine.extend(
                i["parts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap().to_owned()),
            );
        }
    }
    for (k, x) in before.as_object().unwrap() {
        if mine.contains(k) {
            continue;
        }
        assert_eq!(&after[k], x, "{k} moved");
    }
}

/// Pins line up and parts are nudged (round 1, F3): `align J1.3
/// Sensor.SCL row` reads, holds and writes back by the pin's name, its
/// layout file at format 2 (a 1.C build refuses it, rather than reading a
/// pin as a part that isn't there); `shift` is a move, held where it puts
/// a part, refused as a lasting instruction or where it overlaps.
#[test]
fn pins_line_up_and_parts_shift() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "breakout");
    let file = board.join("blocks/breakout/layout.json");
    let format = |f: &Path| -> u64 {
        serde_json::from_slice::<Value>(&std::fs::read(f).unwrap()).unwrap()["format"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(format(&file), 1);
    let out = ryueda(
        &board,
        1,
        &["place", "breakout", "align J1.3 Sensor.SCL row"],
    );
    let ins = &out["instructions"][0];
    assert_eq!(ins["text"], "align J1.3 instance:s1.SCL row", "{ins}");
    assert_eq!(ins["holds"], true, "{ins}");
    assert_eq!(format(&file), 2, "a pin-level align is layout format 2");
    // The text it writes back reads as the same instruction.
    let again = ryueda(
        &board,
        1,
        &["place", "breakout", "align J1.3 instance:s1.SCL row"],
    );
    assert_eq!(again["added"], serde_json::json!([]), "{again}");
    // Released, the file goes back to format 1.
    ryueda(&board, 1, &["place", "breakout", "release i1"]);
    assert_eq!(format(&file), 1);
    // A shift is a move, held where it puts the part.
    let before = layout_of(&board, "breakout")["placed"]["r1"].clone();
    ryueda(&board, 1, &["place", "breakout", "--move", "shift R1 up 2"]);
    let after = layout_of(&board, "breakout")["placed"]["r1"].clone();
    let grid = 1_270_000;
    assert_eq!(after["at"][0], before["at"][0]);
    assert_eq!(
        after["at"][1].as_i64().unwrap(),
        before["at"][1].as_i64().unwrap() + 2 * grid
    );
    assert_eq!(after["held"], true);
    let v = ryueda_envelope(&board, &["place", "breakout", "shift R1 up 2"]);
    assert_eq!(v["error"]["code"], "invalid_param", "{v}");
    let v = ryueda_envelope(&board, &["place", "breakout", "--move", "shift C2 left 10"]);
    assert_eq!(v["error"]["code"], "layout_rule", "{v}");
}

/// Findings carry fixes in the agent's names: each circuit laid out from
/// nothing, each finding's first fix sent alone as a dry run, and the
/// share of fixes that remove their finding printed per rule (the lint's
/// evidence, plan 1.D's Task 18). Every flipped part's fix works. Release
/// only (a dry run per finding).
#[test]
#[cfg_attr(debug_assertions, ignore = "a dry run per finding: release only")]
fn the_corpus_from_nothing_findings_carry_fixes() {
    let dir = tempfile::tempdir().unwrap();
    let mut tally: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for name in ["breakout", "rp2040", "buck", "frontend", "ethernet"] {
        let (board, found) = fresh(dir.path(), name);
        for f in &found {
            let Some(first) = f["fixes"].as_array().and_then(|a| a.first()) else {
                continue;
            };
            let code = f["code"].as_str().unwrap().to_owned();
            let block = f["block"].as_str().unwrap();
            let mut args = vec!["place", block, "--dry-run"];
            let texts: Vec<&str> = first
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap())
                .collect();
            args.extend(texts.iter().copied());
            let v = ryueda_envelope(&board, &args);
            let gone = v["ok"] == true
                && !v["result"]["description"]["blocks"][0]["sheets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|s| s["findings"].as_array().unwrap().iter())
                    .any(|g| g["code"] == f["code"] && g["refs"] == f["refs"]);
            let e = tally.entry(code.clone()).or_default();
            e.0 += 1;
            e.1 += usize::from(gone);
            if code == "sch_part_flipped" {
                assert!(gone, "{name}: {texts:?} left {f}: {v}");
            }
        }
    }
    for (code, (sent, worked)) in &tally {
        eprintln!("fixes {code}: {worked} of {sent} remove their finding");
    }
    assert!(!tally.is_empty(), "no finding carried a fix");
}

/// A preference tried without writing: each laid-out block's sheets'
/// scores before and after, and the findings that come and go, each
/// saying which preference drove it.
#[test]
fn a_preference_is_tried_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "buck");
    let out = ryueda(
        &board,
        1,
        &[
            "lint",
            "configure",
            "--prefer",
            "resistor=iec",
            "--weight",
            "sch_floating=0",
            "--dry-run",
        ],
    );
    assert!(
        !board.join("lint.json").exists(),
        "a dry run writes nothing"
    );
    assert_eq!(out["dry_run"], true, "{out}");
    let sheet = &out["blocks"][0]["sheets"][0];
    assert!(
        sheet["after"]["score"].as_i64() != sheet["before"]["score"].as_i64(),
        "{out}"
    );
    let came: Vec<&Value> = sheet["came"].as_array().unwrap().iter().collect();
    assert!(
        came.iter().any(|f| f["code"] == "sch_resistor_style"
            && f["message"]
                .as_str()
                .unwrap()
                .contains("prefer.resistor = iec")),
        "{out}"
    );
    assert!(sheet["went"].as_array().is_some(), "{out}");
}

/// `holds` is what the drawing shows, with the reason when it isn't, and
/// what changed nothing says so (round 1, F1 and F2: `beside R7 R8 above`
/// held with the parts 175 mm apart, and moves that did nothing said
/// nothing).
#[test]
fn holds_is_what_the_drawing_shows() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "buck");
    // R22 stands far from R9 in the buck laid out from nothing, and the
    // engine doesn't bring it closer: beside doesn't hold, and says why.
    // (Since 1.D R7 stands beside R8 from nothing, which round 1 named.)
    let out = ryueda(
        &board,
        1,
        &["place", "buck", "--dry-run", "beside R22 R9 below"],
    );
    let ins = &out["instructions"][0];
    assert_eq!(ins["holds"], false, "{ins}");
    let why = ins["why"].as_str().unwrap();
    assert!(
        why.starts_with("R22 is ") && why.ends_with(" mm from R9; beside means within 20.3 mm"),
        "{why}"
    );
    assert_eq!(ins["unchanged"], true, "{ins}");
    // A move that changes nothing says so: BOOT is wired already.
    let out = ryueda(
        &board,
        1,
        &["place", "buck", "--dry-run", "--move", "net BOOT wire"],
    );
    assert!(
        out["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["code"] == "move_unchanged"),
        "{}",
        out["notes"]
    );
}

/// Every name `describe` and renders print is accepted, and messages name
/// ports by name (round 1, F13 and F14).
#[test]
fn names_as_drawn_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let (breakout, found) = fresh(dir.path(), "breakout");
    // A child block by its drawn name.
    ryueda(
        &breakout,
        1,
        &["place", "breakout", "--dry-run", "beside Sensor J1 right"],
    );
    // No message names a port by its key.
    let ports: Vec<String> = ["breakout", "sensor"]
        .iter()
        .flat_map(|b| {
            layout_of(&breakout, b)["placed"]
                .as_object()
                .unwrap()
                .keys()
                .filter(|k| k.starts_with("port:"))
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(!ports.is_empty());
    for f in &found {
        let m = f["message"].as_str().unwrap();
        assert!(!ports.iter().any(|k| m.contains(k.as_str())), "{m}");
    }
    let (rp, _) = fresh(dir.path(), "rp2040");
    // A unit by its drawn designator.
    ryueda(
        &rp,
        1,
        &["place", "mitayi", "--dry-run", "align U3B U3C row"],
    );
    // A sheet by its ID.
    let out = ryueda(&rp, 1, &["place", "mitayi", "--dry-run", "sheet R3 s2"]);
    assert!(
        out["instructions"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with("\"Sheet 2\"")
            || out["instructions"][0]["text"]
                .as_str()
                .unwrap()
                .ends_with("Sheet 2"),
        "{}",
        out["instructions"]
    );
    let (eth, _) = fresh(dir.path(), "ethernet");
    // A pin by one part of a name like RXD0/MODE0, and a drawn unit.
    ryueda(&eth, 1, &["place", "eth", "--dry-run", "near RM1D U4.RXD0"]);
}

/// `place` answers in `describe`'s shape, written or not, and the scores it
/// gives are `describe`'s after it (round 1, F15: three shapes, and a
/// place's 64 against describe's 61).
#[test]
fn place_answers_as_describe_does() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "buck");
    for args in [
        vec!["place", "buck", "--dry-run", "--move", "near C7 U1.1"],
        vec!["place", "buck", "--move", "near C7 U1.1"],
    ] {
        let out = ryueda(&board, 1, &args);
        let d = &out["description"]["blocks"][0];
        assert_eq!(d["block_id"], "buck", "{args:?}");
        let scores: Vec<&Value> = d["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| &s["measures"])
            .collect();
        assert_eq!(out["scores"], serde_json::json!(scores), "{args:?}");
    }
    // After the write, describe says what place said.
    let placed = ryueda(&board, 1, &["place", "buck", "--move", "near C4 U1.4"]);
    let described = ryueda(&board, 1, &["describe", "--block", "buck"]);
    let a = &placed["description"]["blocks"][0];
    let b = &described["blocks"][0];
    assert_eq!(a["sheets"], b["sheets"]);
    assert_eq!(a["revision"], b["revision"]);
}

/// A dry run renders what it would write (round 1, F16: every look cost a
/// write, a render and an undo).
#[test]
fn a_dry_run_renders() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "buck");
    let before = std::fs::read(board.join("blocks/buck/layout.json")).unwrap();
    let out = ryueda(
        &board,
        1,
        &[
            "place",
            "buck",
            "--dry-run",
            "--move",
            "near C7 U1.1",
            "--render",
            "dry.png",
            "--fit",
            "--scale",
            "3",
        ],
    );
    assert_eq!(out["format"], "png", "{out}");
    assert_eq!(
        std::fs::read(board.join("blocks/buck/layout.json")).unwrap(),
        before,
        "a dry run writes nothing"
    );
    ryueda(&board, 1, &["place", "buck", "--move", "near C7 U1.1"]);
    ryueda(
        &board,
        1,
        &[
            "render", "--block", "buck", "--fit", "--scale", "3", "--out", "real.png",
        ],
    );
    assert_eq!(
        std::fs::read(board.join("dry.png")).unwrap(),
        std::fs::read(board.join("real.png")).unwrap()
    );
    // Without --dry-run, --render is refused.
    let v = ryueda_envelope(
        &board,
        &[
            "place",
            "buck",
            "--move",
            "near C4 U1.4",
            "--render",
            "x.png",
        ],
    );
    assert_eq!(v["error"]["code"], "usage", "{v}");
}

/// `describe` gives each part's box and its connected pins' ends and sides
/// (round 1, F17: sessions read symbols to learn which side a pin was on).
#[test]
fn describe_gives_boxes_and_pins() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "buck");
    let d = ryueda(&board, 1, &["describe", "--block", "buck"]);
    let u1 = d["blocks"][0]["sheets"][0]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["part"] == "u1")
        .unwrap()
        .clone();
    assert_eq!(u1["box_mm"].as_array().unwrap().len(), 4, "{u1}");
    let ph = &u1["pins"]["8"];
    assert_eq!(ph["name"], "PH", "{u1}");
    assert_eq!(ph["side"], "right", "{u1}");
    assert_eq!(ph["at_mm"].as_array().unwrap().len(), 2);
}

/// A sheet is renamed by its ID or name, and sheet instructions follow it
/// (round 1, F18: sheets couldn't be renamed, so sessions kept "Sheet 1").
#[test]
fn a_sheet_is_renamed() {
    let dir = tempfile::tempdir().unwrap();
    let (board, _) = fresh(dir.path(), "rp2040");
    let before = layout_of(&board, "mitayi");
    ryueda(&board, 1, &["place", "mitayi", "sheet rename s1 Power"]);
    let after = layout_of(&board, "mitayi");
    assert_eq!(
        after["sheets"]["s1"]["name"], "Power",
        "{}",
        after["sheets"]
    );
    assert_eq!(before["placed"], after["placed"], "renaming moves nothing");
    // A sheet instruction by the new name puts a part there.
    ryueda(&board, 1, &["place", "mitayi", "sheet R3 Power"]);
    assert_eq!(layout_of(&board, "mitayi")["placed"]["r3"]["sheet"], "s1");
    // Renaming it again carries the instruction with it.
    ryueda(
        &board,
        1,
        &["place", "mitayi", "sheet rename Power Supplies"],
    );
    let list = ryueda(&board, 1, &["place", "mitayi", "--list"]);
    assert!(
        list["instructions"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with("Supplies"),
        "{list}"
    );
    let v = ryueda_envelope(&board, &["place", "mitayi", "sheet rename s7 X"]);
    assert_eq!(v["error"]["code"], "not_found", "{v}");
}

#[test]
fn the_corpus_lays_out_keeping_every_rule() {
    let dir = tempfile::tempdir().unwrap();
    let look = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ryueda-look");
    std::fs::create_dir_all(&look).unwrap();
    for name in ["breakout", "rp2040", "buck", "frontend", "ethernet"] {
        let board = circuit(dir.path(), name);
        let one = ryueda(&board, 1, &["layout", "--dry-run"]);
        assert_eq!(ryueda(&board, 8, &["layout", "--dry-run"]), one, "{name}");
        ryueda(&board, 1, &["layout"]);
        let p = ryueda::project::Project::load(&board).unwrap();
        for b in p.conn.blocks.values() {
            let laid = ryueda::schem::lay(&p, b, None, &[], ryusch::input::Relayout::None).unwrap();
            assert!(
                !laid.stale && laid.invalid.is_none(),
                "{name}/{}: stale {} invalid {:?}",
                b.id,
                laid.stale,
                laid.invalid
            );
            let broken = ryusch::rules::check(&laid.built.input, &laid.outcome.layout);
            assert!(broken.is_empty(), "{name}/{}: {broken:#?}", b.id);
            let (findings, scores) = ryusch::lint::lint(&laid.built.input, &laid.outcome);
            eprintln!(
                "{name}/{}: sheets {:?}, findings {}, notes {:?}",
                b.id,
                scores
                    .iter()
                    .map(|s| (s.sheet.clone(), s.score))
                    .collect::<Vec<_>>(),
                findings.len(),
                laid.outcome.notes
            );
            // Its lint, for people to read beside its render.
            std::fs::write(
                look.join(format!("{name}-{}.lint.json", b.id)),
                serde_json::to_string_pretty(&serde_json::json!({
                    "scores": scores,
                    "findings": findings,
                }))
                .unwrap(),
            )
            .unwrap();
            let out = format!("{name}-{}.png", b.id);
            let drawn = ryueda(
                &board,
                1,
                &[
                    "render",
                    "--block",
                    b.id.as_str(),
                    "--fit",
                    "--scale",
                    "8",
                    "--out",
                    &out,
                ],
            );
            std::fs::copy(board.join(&out), look.join(&out)).unwrap();
            // A stack's second layer, its own designators.
            if let Some(layer) = drawn["layers"].as_array().and_then(|l| l.get(1)) {
                let path = layer["path"].as_str().or(layer.as_str()).unwrap_or("B");
                let out = format!("{name}-{}-B.png", b.id);
                ryueda(
                    &board,
                    1,
                    &[
                        "render",
                        "--block",
                        b.id.as_str(),
                        "--layer",
                        path,
                        "--fit",
                        "--scale",
                        "8",
                        "--out",
                        &out,
                    ],
                );
                std::fs::copy(board.join(&out), look.join(&out)).unwrap();
            }
        }
        // The breakout described under a project's preferences: the box
        // resistor preferred and weighted, jogs weighted more.
        if name == "breakout" {
            ryueda(
                &board,
                1,
                &[
                    "lint",
                    "configure",
                    "--prefer",
                    "resistor=iec",
                    "--weight",
                    "sch_resistor_style=4",
                    "--weight",
                    "sch_jog=5",
                ],
            );
            let d = ryueda(&board, 1, &["describe", "--block", "breakout"]);
            let lint = ryueda(&board, 1, &["lint"]);
            std::fs::write(
                look.join("breakout-breakout.prefs.json"),
                serde_json::to_string_pretty(&serde_json::json!({
                    "prefs": lint["prefs"],
                    "sheets": d["blocks"][0]["sheets"],
                }))
                .unwrap(),
            )
            .unwrap();
            ryueda(
                &board,
                1,
                &[
                    "lint",
                    "configure",
                    "--prefer",
                    "resistor=default",
                    "--weight",
                    "sch_resistor_style=default",
                    "--weight",
                    "sch_jog=default",
                ],
            );
        }
        // The design paged into a PDF.
        let pdf = format!("{name}.pdf");
        ryueda(&board, 1, &["output", "--out", &pdf]);
        std::fs::copy(board.join(&pdf), look.join(&pdf)).unwrap();
    }
}

/// A block's layout file, as JSON.
fn layout_of(board: &Path, block: &str) -> Value {
    let text = std::fs::read(board.join(format!("blocks/{block}/layout.json"))).unwrap();
    serde_json::from_slice(&text).unwrap()
}

/// The layout keys of the nets a component's pins are on.
fn nets_of(board: &Path, block: &str, component: &str) -> Vec<String> {
    let c = ryueda(
        board,
        1,
        &["component", "show", &format!("{block}/{component}")],
    );
    c["connections"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|v| {
            let v = v.as_str()?;
            Some(match v.strip_prefix("rail/") {
                Some(r) => format!("rail:{r}"),
                None => v.split_once('/')?.1.to_owned(),
            })
        })
        .collect()
}

/// Holds an edit to the bar: nothing outside `parts` and `nets` moved;
/// a rail's ports compared one by one, those of `parts` aside.
fn unchanged(before: &Value, after: &Value, parts: &[String], nets: &[String], what: &str) {
    let mine = |k: &str| {
        parts
            .iter()
            .any(|p| k == p || k.starts_with(&format!("{p}:")))
    };
    for field in ["placed", "text"] {
        for (k, v) in before[field].as_object().unwrap() {
            if !mine(k) && after[field].get(k).is_some() {
                assert_eq!(&after[field][k], v, "{what}: {field} {k}");
            }
        }
    }
    let keys = |f: &str| -> std::collections::BTreeSet<String> {
        before[f]
            .as_object()
            .unwrap()
            .keys()
            .chain(after[f].as_object().unwrap().keys())
            .cloned()
            .collect()
    };
    for k in keys("routes") {
        if !nets.contains(&k) && !k.starts_with("rail:") {
            assert_eq!(
                before["routes"].get(&k),
                after["routes"].get(&k),
                "{what}: wires of {k}"
            );
        }
    }
    for k in keys("labels") {
        if k.starts_with("rail:") {
            let keep = |v: &Value| -> Vec<Value> {
                v.as_array()
                    .map(|a| {
                        a.iter()
                            .filter(|l| {
                                let on = l["on"].as_str().unwrap();
                                !mine(on.rsplit_once('/').unwrap().0)
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            };
            assert_eq!(
                keep(&before["labels"][&k]),
                keep(&after["labels"][&k]),
                "{what}: ports of {k}"
            );
        } else if !nets.contains(&k) {
            assert_eq!(
                before["labels"].get(&k),
                after["labels"].get(&k),
                "{what}: labels of {k}"
            );
        }
    }
}

#[test]
fn edits_to_the_corpus_change_only_what_they_name() {
    let dir = tempfile::tempdir().unwrap();
    // (circuit, block, a component to revalue, a net to rename, a net to
    // hang a capacitor on, a pin to move and the net it goes to, a
    // component to delete)
    let cases = [
        (
            "breakout", "breakout", "r1", "SDA", "SCL", "r2/1", "SDA_X", "c3",
        ),
        (
            "rp2040", "mitayi", "r9", "XOUT", "RUN", "r10/1", "XIN", "c15",
        ),
        ("buck", "buck", "r9", "VSNS", "SS", "r1/2", "COMP", "c6"),
        (
            "frontend", "baldiff", "r45", "N_C36_1", "N_C57_1", "r49/2", "N_C55_2", "c53",
        ),
        (
            "ethernet", "eth", "r26", "NRST", "VDDCR", "r41/2", "PHYAD1", "c18",
        ),
    ];
    for (name, block, revalue, rename, hang, pin, to, delete) in cases {
        let board = circuit(dir.path(), name);
        ryueda(&board, 1, &["layout"]);
        let s = |x: &str| x.to_owned();
        let net_key = |n: &str| {
            ryueda(&board, 1, &["net", "show", &format!("{block}/{n}")])["reference"]
                .as_str()
                .unwrap()
                .split_once('/')
                .unwrap()
                .1
                .to_owned()
        };
        let mut edits: Vec<(Vec<String>, Vec<String>, Vec<String>)> = Vec::new();
        edits.push((
            vec![
                s("component"),
                s("edit"),
                format!("{block}/{revalue}"),
                s("--value"),
                s("47k"),
            ],
            vec![s(revalue)],
            vec![],
        ));
        edits.push((
            vec![
                s("net"),
                s("rename"),
                format!("{block}/{rename}"),
                format!("{rename}_X"),
            ],
            vec![],
            vec![net_key(rename)],
        ));
        edits.push((
            vec![
                s("component"),
                s("add"),
                s(block),
                s("--of"),
                s(match name {
                    "breakout" => "c_100n",
                    "frontend" | "ethernet" => "c_100nf",
                    _ => "c_0_1uf",
                }),
                s("--id"),
                s("c99"),
            ],
            vec![s("c99")],
            vec![],
        ));
        edits.push((
            vec![s("connect"), format!("{block}/c99/1"), s(hang)],
            vec![s("c99")],
            vec![net_key(hang)],
        ));
        let (comp, _) = pin.split_once('/').unwrap();
        let mut moved = nets_of(&board, block, comp);
        // The net's ID is the same after the rename.
        moved.push(net_key(to.trim_end_matches("_X")));
        edits.push((
            vec![s("connect"), format!("{block}/{pin}"), s(to)],
            vec![],
            moved,
        ));
        let mut gone = nets_of(&board, block, delete);
        gone.sort();
        // A pin's bypass capacitors are one drawing (the first carries
        // its port, the rest join its wire): deleting one may draw the
        // others again.
        let mut with = vec![s(delete)];
        {
            let p = ryueda::project::Project::load(&board).unwrap();
            let b = p
                .conn
                .block(&ryueda::id::Id::parse(block, "b").unwrap())
                .unwrap();
            let laid = ryueda::schem::lay(&p, b, None, &[], ryusch::input::Relayout::None).unwrap();
            let parts = &laid.built.input.parts;
            let bypass: Vec<_> = laid
                .outcome
                .idioms
                .iter()
                .filter(|i| i.kind == ryusch::layout::IdiomKind::Decoupling)
                .collect();
            if let Some(mine) = bypass.iter().find(|i| parts[i.parts[0]].key == delete) {
                for i in &bypass {
                    if i.anchor == mine.anchor && parts[i.parts[0]].key != delete {
                        with.push(parts[i.parts[0]].key.clone());
                    }
                }
            }
        }
        edits.push((
            vec![s("component"), s("delete"), format!("{block}/{delete}")],
            with,
            gone,
        ));
        for (args, parts, nets) in edits {
            let before = layout_of(&board, block);
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            ryueda(&board, 1, &a);
            let after = layout_of(&board, block);
            unchanged(
                &before,
                &after,
                &parts,
                &nets,
                &format!("{name}: {}", args.join(" ")),
            );
            let p = ryueda::project::Project::load(&board).unwrap();
            let b = p
                .conn
                .block(&ryueda::id::Id::parse(block, "b").unwrap())
                .unwrap();
            let laid = ryueda::schem::lay(&p, b, None, &[], ryusch::input::Relayout::None).unwrap();
            assert!(
                !laid.stale && laid.invalid.is_none(),
                "{name}: {args:?}: {:?}",
                laid.invalid
            );
        }
    }
}

/// Copies a project folder.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type().unwrap().is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// Every file under `dir`, by its path from there.
fn files(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                let rel = e
                    .path()
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read(e.path()).unwrap());
            }
        }
    }
    out
}

#[test]
fn scripted_branches_merge_their_layouts_by_area() {
    let dir = tempfile::tempdir().unwrap();
    let base = circuit(dir.path(), "breakout");
    ryueda(&base, 1, &["layout"]);
    // Ours hangs a capacitor on SCL; theirs another on SCL too, and
    // revalues R1: both new parts look for room in the same place.
    let ours = dir.path().join("ours");
    let theirs = dir.path().join("theirs");
    copy_dir(&base, &ours);
    copy_dir(&base, &theirs);
    for (side, id, refdes) in [(&ours, "c98", "C98"), (&theirs, "c97", "C97")] {
        ryueda(
            side,
            1,
            &[
                "component",
                "add",
                "breakout",
                "--of",
                "c_100n",
                "--id",
                id,
                "--refdes",
                refdes,
            ],
        );
        ryueda(side, 1, &["connect", &format!("breakout/{id}/1"), "SCL"]);
    }
    ryueda(
        &theirs,
        1,
        &["component", "edit", "breakout/r1", "--value", "47k"],
    );
    // The connectivity merges by file (the sides touched different
    // files); the layout by ryusch's merge.
    let merged = dir.path().join("merged");
    copy_dir(&base, &merged);
    let b0 = files(&base);
    for side in [&ours, &theirs] {
        for (path, bytes) in files(side) {
            if !path.ends_with("layout.json") && b0.get(&path) != Some(&bytes) {
                let at = merged.join(&path);
                std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                std::fs::write(at, bytes).unwrap();
            }
        }
    }
    let read = |d: &Path| {
        let path = d.join("blocks/breakout/layout.json");
        ryueda::schem::read_layout(&std::fs::read(path).unwrap(), "layout.json").unwrap()
    };
    let (lb, lo, lt) = (read(&base), read(&ours), read(&theirs));
    let p = ryueda::project::Project::load(&merged).unwrap();
    let b = p
        .conn
        .block(&ryueda::id::Id::parse("breakout", "b").unwrap())
        .unwrap();
    let built = ryueda::schem::build(&p, b).unwrap();
    let m = ryusch::merge::merge(&built.input, &lb, &lo, &lt);
    // What neither side touched keeps its bits.
    for (k, x) in &lb.placed {
        if lo.placed.get(k) == Some(x) && lt.placed.get(k) == Some(x) {
            assert_eq!(m.layout.placed.get(k), Some(x), "{k}");
        }
    }
    // The two capacitors took the same free space: one conflict there;
    // R1's new value, apart from it, merged cleanly.
    assert_eq!(m.conflicts.len(), 1, "{:#?}", m.conflicts);
    assert!(m.conflicts[0].ours.contains(&"placed/c98".to_owned()));
    assert!(m.conflicts[0].theirs.contains(&"placed/c97".to_owned()));
    let repairs = vec![ryusch::merge::Repair::Relayout; m.conflicts.len()];
    let out = ryusch::merge::resolve(&built.input, &lb, &lo, &lt, &m, &repairs).unwrap();
    assert_eq!(ryusch::rules::check(&built.input, &out.layout), []);
    let touched: std::collections::BTreeSet<String> = m
        .conflicts
        .iter()
        .flat_map(|c| c.ours.iter().chain(&c.theirs))
        .filter_map(|k| k.strip_prefix("placed/").map(str::to_owned))
        .collect();
    for (k, x) in &lb.placed {
        if lo.placed.get(k) == Some(x) && lt.placed.get(k) == Some(x) && !touched.contains(k) {
            assert_eq!(out.layout.placed.get(k), Some(x), "{k} moved");
        }
    }
    assert!(out.layout.placed.contains_key("c98") && out.layout.placed.contains_key("c97"));
    // Written, the merged project lays out as it stands.
    std::fs::write(
        merged.join("blocks/breakout/layout.json"),
        ryueda::schem::layout_text(&out.layout),
    )
    .unwrap();
    let p = ryueda::project::Project::load(&merged).unwrap();
    let b = p
        .conn
        .block(&ryueda::id::Id::parse("breakout", "b").unwrap())
        .unwrap();
    let laid = ryueda::schem::lay(&p, b, None, &[], ryusch::input::Relayout::None).unwrap();
    assert!(!laid.stale && laid.invalid.is_none(), "{:?}", laid.invalid);
}

#[test]
fn a_unit_to_another_sheet_and_a_new_layer_change_only_what_they_name() {
    let dir = tempfile::tempdir().unwrap();
    // The RP2040's power unit onto a sheet of its own: it, the idioms
    // serving it and their nets may move.
    let pico = circuit(dir.path(), "rp2040");
    ryueda(&pico, 1, &["layout"]);
    let d = ryueda(&pico, 1, &["describe", "--block", "mitayi"]);
    let mut parts = vec!["u3".to_owned()];
    for i in d["blocks"][0]["idioms"].as_array().unwrap() {
        if i["serves"].as_str().is_some_and(|s| s.starts_with("u3:2/")) {
            for p in i["parts"].as_array().unwrap() {
                parts.push(p.as_str().unwrap().to_owned());
            }
        }
    }
    let mut nets: Vec<String> = Vec::new();
    for p in &parts {
        nets.extend(nets_of(&pico, "mitayi", p.split(':').next().unwrap()));
    }
    let before = layout_of(&pico, "mitayi");
    let placed = ryueda(&pico, 1, &["place", "mitayi", "sheet U3:2 Power"]);
    assert_eq!(placed["instructions"][0]["holds"], true, "{placed}");
    let after = layout_of(&pico, "mitayi");
    unchanged(&before, &after, &parts, &nets, "rp2040: U3B to sheet Power");
    // A third I/V converter: a new layer of its stack; only the stack and
    // its nets may move, and the new layer draws its own designators.
    let fe = circuit(dir.path(), "frontend");
    ryueda(&fe, 1, &["layout"]);
    // An instance's ports' nets, as its file names them, by layout key.
    let stack_nets = |b: &Path| {
        let mut v = Vec::new();
        for i in ["iv_l", "iv_r"] {
            let text = std::fs::read(b.join(format!("blocks/amp/instances/{i}.json"))).unwrap();
            let f: Value = serde_json::from_slice(&text).unwrap();
            for node in f["ports"].as_object().unwrap().values() {
                v.push(match node {
                    Value::String(n) => n.clone(),
                    Value::Object(o) if o.contains_key("rail") => {
                        format!("rail:{}", o["rail"].as_str().unwrap())
                    }
                    Value::Object(o) => format!("port:{}", o["port"].as_str().unwrap()),
                    _ => continue,
                });
            }
        }
        v
    };
    let before = layout_of(&fe, "amp");
    let mut nets = stack_nets(&fe);
    for (id, name) in [
        ("iv_x_p", "IV_X_P"),
        ("iv_x_n", "IV_X_N"),
        ("in_x_p", "IN_X_P"),
        ("in_x_n", "IN_X_N"),
    ] {
        ryueda(&fe, 1, &["net", "add", "amp", name, "--id", id]);
        nets.push(id.to_owned());
    }
    ryueda(
        &fe,
        1,
        &[
            "instance",
            "add",
            "amp",
            "--of",
            "ivconv",
            "--id",
            "iv_x",
            "--refdes",
            "suffix_letter",
            "--port",
            "IVCONV_IN_P=IN_X_P",
            "--port",
            "IVCONV_IN_N=IN_X_N",
            "--port",
            "IVCONV_OUT_P=IV_X_P",
            "--port",
            "IVCONV_OUT_N=IV_X_N",
        ],
    );
    let after = layout_of(&fe, "amp");
    unchanged(
        &before,
        &after,
        &["instance:iv_l".to_owned()],
        &nets,
        "frontend: a third layer",
    );
    let r = ryueda(
        &fe,
        1,
        &[
            "render", "--block", "ivconv", "--layer", "C", "--out", "c.svg",
        ],
    );
    assert_eq!(r["layers"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(r["layer"]["path"], "iv_x");
    let svg = std::fs::read_to_string(fe.join("c.svg")).unwrap();
    assert!(
        svg.contains("_C<") && !svg.contains("_A<"),
        "the third layer's designators"
    );
    let top = ryueda(&fe, 1, &["render", "--block", "amp", "--out", "amp.svg"]);
    assert_eq!(top["block"], "amp");
    assert!(
        std::fs::read_to_string(fe.join("amp.svg"))
            .unwrap()
            .contains(">×3<")
    );
}

/// Agent round projects: each circuit entered by its script and every
/// block laid out again from nothing, with no instructions, written into
/// `RYUEDA_ROUND_OUT/<circuit>` for a round's sessions to start from (with
/// each block's lint as `<circuit>/baseline.json`). Run by hand:
/// `RYUEDA_ROUND_OUT=<dir> cargo test -p ryueda --test schematic_corpus -- --ignored round_projects`.
#[test]
#[ignore]
fn round_projects() {
    let out = std::path::PathBuf::from(
        std::env::var("RYUEDA_ROUND_OUT").expect("RYUEDA_ROUND_OUT names the folder to write"),
    );
    std::fs::create_dir_all(&out).unwrap();
    for name in ["breakout", "rp2040", "buck", "frontend", "ethernet"] {
        assert!(
            !out.join(name).exists(),
            "{} exists: start from an empty folder",
            out.join(name).display()
        );
        let board = circuit(&out, name);
        let p = ryueda::project::Project::load(&board).unwrap();
        let mut baseline = serde_json::Map::new();
        for b in p.conn.blocks.values() {
            ryueda(&board, 1, &["layout", "--relayout", b.id.as_str()]);
        }
        let p = ryueda::project::Project::load(&board).unwrap();
        for b in p.conn.blocks.values() {
            let laid = ryueda::schem::lay(&p, b, None, &[], ryusch::input::Relayout::None).unwrap();
            assert!(!laid.stale, "{name}/{}: laid out again, it moves", b.id);
            let broken = ryusch::rules::check(&laid.built.input, &laid.outcome.layout);
            assert!(broken.is_empty(), "{name}/{}: {broken:#?}", b.id);
            let (findings, scores) = ryusch::lint::lint(&laid.built.input, &laid.outcome);
            baseline.insert(
                b.id.to_string(),
                serde_json::json!({"scores": scores, "findings": findings}),
            );
        }
        std::fs::write(
            board.join("baseline.json"),
            serde_json::to_string_pretty(&Value::Object(baseline)).unwrap() + "\n",
        )
        .unwrap();
    }
}
