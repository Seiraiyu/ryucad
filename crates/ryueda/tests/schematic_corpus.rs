//! The schematic corpus: each circuit entered by its script (symbols,
//! library components and connectivity, by command), laid out from
//! nothing, every hard rule held on every sheet, the same layout on one
//! thread and eight, and each circuit's sheets rendered into
//! `target/ryueda-look` for people to look at.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;
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
        _ => rp2040::script(),
    };
    for args in script {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ryueda(&board, 1, &args);
    }
    board
}

#[test]
fn the_corpus_lays_out_keeping_every_rule() {
    let dir = tempfile::tempdir().unwrap();
    let look = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ryueda-look");
    std::fs::create_dir_all(&look).unwrap();
    for name in ["breakout", "rp2040"] {
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
            let out = format!("{name}-{}.png", b.id);
            ryueda(
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
        }
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
                s(if name == "breakout" {
                    "c_100n"
                } else {
                    "c_0_1uf"
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
        edits.push((
            vec![s("component"), s("delete"), format!("{block}/{delete}")],
            vec![s(delete)],
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
