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
fn fresh(dir: &Path, name: &str) -> Vec<Value> {
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
    out
}

/// The findings of one code.
fn coded<'a>(found: &'a [Value], code: &str) -> Vec<&'a Value> {
    found.iter().filter(|f| f["code"] == code).collect()
}

/// Laid out from nothing, the corpus is drawn as the lint teaches:
/// recognised circuits in their standard shapes (requirement 54), banks in
/// rows (55), local circuits wired (56).
#[test]
fn the_corpus_from_nothing_draws_what_the_lint_teaches() {
    let dir = tempfile::tempdir().unwrap();
    let all: Vec<(&str, Vec<Value>)> = ["breakout", "rp2040", "buck", "frontend", "ethernet"]
        .into_iter()
        .map(|name| (name, fresh(dir.path(), name)))
        .collect();
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
    // Local circuits wired: at most 35 floating parts over the corpus (41
    // before two-pin parts on local nets joined their pins). The plan's 20
    // needs placement across clusters: what still floats is clamps and
    // terminations labelled to the ports or connectors they guard (a TVS
    // and its terminations, BAV99 clamps on output nets) and parts on nets
    // with several active pins. Round 1 and 1.D's metric bars judge them.
    for (name, found) in &all {
        eprintln!("{name}: {} floating", coded(found, "sch_floating").len());
    }
    let floating: usize = all
        .iter()
        .map(|(_, f)| coded(f, "sch_floating").len())
        .sum();
    assert!(floating <= 35, "{floating} parts float");
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
