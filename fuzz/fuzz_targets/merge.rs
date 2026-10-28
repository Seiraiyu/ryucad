//! Three versions of a file from bytes, merged as the merge driver merges
//! them. The first byte picks how the rest is read: raw text (three
//! versions split at `0xff` bytes, so anything at all reaches the driver),
//! or a base sketch, part list, parameters file, part's properties,
//! release record, pattern, instance or mate and two sides' edits to it,
//! two bytes an edit; `0xfe` an import feature, `0xff` an instance an
//! import made and `0xfd` the material library. Merging must never panic, must give the same bytes twice,
//! must be canonical JSON whose record holds as many conflicts as it says
//! unless it fell back to lines, and readable files never fall back. A
//! side that changed nothing gives the other, an instance's pose never
//! conflicts (it is solver state), an assembly's item numbers conflict
//! only part by part, and two sides re-importing different bytes conflict
//! on the file's hash.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::doc::canonical;
use ryucad::merge::{Outcome, bytes, names_in, record};
use serde_json::{Value, json};

/// A square sketch with a length.
fn sketch() -> Value {
    json!({"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "xy",
           "entities": {
               "p1": {"type": "point", "m": [0, 0]},
               "p2": {"type": "point", "m": [0.01, 0]},
               "p3": {"type": "point", "m": [0.01, 0.01]},
               "p4": {"type": "point", "m": [0, 0.01]},
               "l1": {"type": "line", "start": "p1", "end": "p2"},
               "l2": {"type": "line", "start": "p2", "end": "p3"},
               "l3": {"type": "line", "start": "p3", "end": "p4"},
               "l4": {"type": "line", "start": "p4", "end": "p1"}},
           "constraints": {"d1": {"type": "length", "a": "l1", "value": "10 mm"}}})
}

/// The keys of `v[group]`, sorted.
fn keys(v: &Value, group: &str) -> Vec<String> {
    v[group]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// One past the largest ID with `prefix`.
fn next(v: &Value, prefix: &str) -> String {
    let n = ["entities", "constraints"]
        .iter()
        .flat_map(|g| keys(v, g))
        .filter_map(|k| k.strip_prefix(prefix)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{}", n + 1)
}

/// Applies edit `(op, arg)` to a sketch, keeping it valid.
fn edit_sketch(v: &mut Value, op: u8, arg: u8) {
    let entities = keys(v, "entities");
    let pick =
        |list: &[String]| (!list.is_empty()).then(|| list[arg as usize % list.len()].clone());
    let kind = |id: &str, v: &Value| v["entities"][id]["type"].as_str().unwrap_or("").to_owned();
    let points: Vec<String> = entities
        .iter()
        .filter(|e| kind(e, v) == "point")
        .cloned()
        .collect();
    let lines: Vec<String> = entities
        .iter()
        .filter(|e| kind(e, v) == "line")
        .cloned()
        .collect();
    let x = f64::from(arg) / 1000.0;
    match op % 7 {
        0 => {
            if let Some(p) = pick(&points) {
                v["entities"][p]["m"] = json!([x, -x]);
            }
        }
        1 => {
            let id = next(v, "p");
            v["entities"][id] = json!({"type": "point", "m": [x, x]});
        }
        2 => {
            if let (Some(a), Some(b)) = (pick(&points), points.first().cloned())
                && a != b
            {
                let id = next(v, "l");
                v["entities"][id] = json!({"type": "line", "start": a, "end": b});
            }
        }
        3 => {
            if let Some(id) = pick(&entities) {
                let mut gone = vec![id];
                while let Some(g) = gone.pop() {
                    for group in ["entities", "constraints"] {
                        if let Some(o) = v[group].as_object_mut() {
                            o.remove(&g);
                            gone.extend(
                                o.iter()
                                    .filter(|(_, m)| names_in(m).contains(&g.as_str()))
                                    .map(|(k, _)| k.clone()),
                            );
                        }
                    }
                }
            }
        }
        4 => {
            if let Some(l) = pick(&lines) {
                let id = next(v, "d");
                v["constraints"][id] =
                    json!({"type": "length", "a": l, "value": format!("{} mm", arg % 50 + 1)});
            }
        }
        5 => {
            if let Some(l) = pick(&lines)
                && let Some(e) = v["entities"][l].as_object_mut()
                && e.remove("construction").is_none()
            {
                e.insert("construction".to_owned(), json!(true));
            }
        }
        _ => v["plane"] = json!(["xy", "yz", "zx"][arg as usize % 3]),
    }
}

/// Applies edit `(op, arg)` to an assembly's part list: its features, or
/// its item numbers (a part numbered, or its number taken away).
fn edit_part(v: &mut Value, op: u8, arg: u8) {
    let mut list: Vec<String> = serde_json::from_value(v["features"].clone()).unwrap_or_default();
    let at = |n: usize| if n == 0 { 0 } else { arg as usize % n };
    let part = format!("q{}", arg % 6);
    match op % 5 {
        3 => {
            v["items"][part.as_str()] = json!(1 + arg % 4);
            return;
        }
        4 => {
            if let Some(items) = v["items"].as_object_mut() {
                items.remove(&part);
            }
            return;
        }
        _ => {}
    }
    match op % 5 {
        0 if !list.is_empty() => {
            let i = at(list.len());
            list.remove(i);
        }
        1 if !list.is_empty() => {
            let f = list.remove(at(list.len()));
            let i = at(list.len() + 1);
            list.insert(i, f);
        }
        _ => {
            let id = format!("f{arg}");
            if !list.contains(&id) {
                let i = at(list.len() + 1);
                list.insert(i, id);
            }
        }
    }
    v["features"] = json!(list);
}

/// Applies edit `(op, arg)` to a parameters file.
fn edit_parameters(v: &mut Value, op: u8, arg: u8) {
    let name = format!("p{}", arg % 8);
    let ps = v["parameters"].as_object_mut().expect("an object");
    match op % 2 {
        0 => {
            ps.remove(&name);
        }
        _ => {
            ps.insert(name, json!(format!("{} mm", op / 2)));
        }
    }
}

/// Applies edit `(op, arg)` to a part's properties: a property removed,
/// set to text, or set to a number.
fn edit_properties(v: &mut Value, op: u8, arg: u8) {
    let names = [
        "number",
        "name",
        "description",
        "material",
        "finish",
        "mass",
    ];
    let name = names[arg as usize % names.len()];
    let o = v.as_object_mut().expect("an object");
    match op % 3 {
        0 => {
            o.remove(name);
        }
        1 => {
            o.insert(name.to_owned(), json!(format!("v{}", op / 3)));
        }
        _ => {
            o.insert(name.to_owned(), json!(op / 3));
        }
    }
}

/// Applies edit `(op, arg)` to a release record: its note, content or
/// properties changed. A record merges as a whole file.
fn edit_record(v: &mut Value, op: u8, arg: u8) {
    match op % 3 {
        0 => v["note"] = json!(format!("note {arg}")),
        1 => v["content"] = json!(format!("{arg:064x}")),
        _ => v["properties"]["number"] = json!(format!("{}", 100 + u32::from(arg))),
    }
}

/// Applies edit `(op, arg)` to a pattern: a seed added or removed, an
/// instance skipped or not, its count or spacing set. Its seeds and skips
/// are arrays, one value each.
fn edit_pattern(v: &mut Value, op: u8, arg: u8) {
    let toggle = |list: &mut Vec<Value>, x: Value| match list.iter().position(|y| *y == x) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(x),
    };
    match op % 4 {
        0 => {
            let mut l = v["features"].as_array().cloned().unwrap_or_default();
            toggle(&mut l, json!(format!("h{}", arg % 4)));
            v["features"] = json!(l);
        }
        1 => {
            let mut l = v["skip"].as_array().cloned().unwrap_or_default();
            toggle(&mut l, json!(1 + arg % 3));
            let o = v.as_object_mut().expect("an object");
            if l.is_empty() {
                o.remove("skip");
            } else {
                o.insert("skip".to_owned(), json!(l));
            }
        }
        2 => v["count"] = json!(format!("{}", 2 + arg % 5)),
        _ => v["spacing"] = json!(format!("{} mm", 1 + arg % 9)),
    }
}

/// Applies edit `(op, arg)` to an instance: its pose moved or turned (as
/// a solver would write it), its revision or its fixing changed.
fn edit_instance(v: &mut Value, op: u8, arg: u8) {
    let x = f64::from(arg) * 1e-3;
    match op % 4 {
        0 => v["pose"]["origin_m"][usize::from(arg % 3)] = json!(x),
        1 => {
            let (c, s) = (1.0 - x * x / 2.0, x);
            v["pose"]["x"] = json!([c, s, 0]);
        }
        2 => {
            v["rev"] = match arg % 3 {
                0 => json!("working"),
                n => json!({"label": if n == 1 { "A" } else { "B" }}),
            }
        }
        _ => v["fixed"] = json!(arg % 2 == 1),
    }
}

/// Applies edit `(op, arg)` to a mate: a connector aimed elsewhere or
/// offset, its form or its angle changed. A connector is one value.
fn edit_mate(v: &mut Value, op: u8, arg: u8) {
    let side = if arg & 1 == 0 { "a" } else { "b" };
    match op % 4 {
        0 => {
            v[side]["at"] = match arg % 3 {
                0 => json!("origin"),
                n => json!({"feature": "slab", "name": format!("{n}").repeat(32)}),
            }
        }
        1 => v[side]["offset"] = json!({"origin": ["0 mm", "0 mm", format!("{} mm", arg % 7)]}),
        2 => v["form"] = json!(["fastened", "revolute", "slider", "ball"][usize::from(arg % 4)]),
        _ => v["angle"] = json!(format!("{} deg", arg % 90)),
    }
}

/// Applies edit `(op, arg)` to an import feature: the file re-imported
/// (a new hash), another body, another product or none, or its ordinal.
fn edit_import(v: &mut Value, op: u8, arg: u8) {
    match op % 4 {
        0 => v["file"]["blake3"] = json!(format!("{arg:064x}")),
        1 => v["body"] = json!(arg % 4),
        2 => match arg % 3 {
            0 => {
                v.as_object_mut().unwrap().remove("product");
            }
            n => v["product"] = json!({"id": format!("P-{n}"), "name": format!("Part {n}")}),
        },
        _ => {
            if v["product"].is_object() {
                match arg % 3 {
                    0 => {
                        v["product"].as_object_mut().unwrap().remove("ordinal");
                    }
                    n => v["product"]["ordinal"] = json!(n),
                }
            }
        }
    }
}

/// Applies edit `(op, arg)` to an instance an import made: its pose (as
/// an update or a solver writes it), its usage, its ordinal, its fixing.
fn edit_imported(v: &mut Value, op: u8, arg: u8) {
    match op % 4 {
        0 => edit_instance(v, 0, arg),
        1 => v["import"]["usage"] = json!(format!("NAUO{}", arg % 3)),
        2 => match arg % 3 {
            0 => {
                v["import"].as_object_mut().unwrap().remove("ordinal");
            }
            n => v["import"]["ordinal"] = json!(n),
        },
        _ => v["fixed"] = json!(arg % 2 == 1),
    }
}

/// Applies edit `(op, arg)` to a material library: a material's
/// density set (adding it), a material removed (the default with it), or
/// the default moved, kept valid.
fn edit_materials(v: &mut Value, op: u8, arg: u8) {
    let name = format!("m{}", arg % 3);
    match op % 3 {
        0 => v["materials"][&name] = json!({"density": format!("{} kg/m^3", u32::from(arg) + 1)}),
        1 => {
            v["materials"].as_object_mut().unwrap().remove(&name);
            if v["default"] == json!(name) {
                v.as_object_mut().unwrap().remove("default");
            }
        }
        _ => {
            if v["materials"].get(&name).is_some() {
                v["default"] = json!(name);
            } else {
                v.as_object_mut().unwrap().remove("default");
            }
        }
    }
}

fn text(v: &Value) -> Vec<u8> {
    canonical::to_string(v).into_bytes()
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    // `0xfe` and `0xff` are the import modes (9 and 10), `0xfd` the
    // material library (11); every other byte picks as it did before
    // them.
    let kind = match mode {
        0xfd => 11,
        0xfe => 9,
        0xff => 10,
        m => m % 9,
    };
    let (base, ours, theirs, readable) = if kind == 3 {
        let mut parts = rest.splitn(3, |b| *b == 0xff);
        let (b, o, t) = (
            parts.next().unwrap_or(&[]).to_vec(),
            parts.next().unwrap_or(&[]).to_vec(),
            parts.next().unwrap_or(&[]).to_vec(),
        );
        (b, o, t, false)
    } else {
        let base = match kind {
            0 => sketch(),
            1 => json!({"features": ["a", "b", "c", "d"], "format": 3, "kind": "ryucad.part",
                        "part_kind": "assembly", "items": {"q0": 1, "q1": 2}}),
            2 => json!({"format": 1, "kind": "ryucad.parameters",
                        "parameters": {"p0": "1 mm", "p1": "p0 * 2"}}),
            4 => json!({"format": 1, "kind": "ryupdm.properties", "name": "P", "number": "100"}),
            6 => json!({"format": 3, "kind": "ryucad.feature", "type": "pattern", "form": "linear",
                        "features": ["h0"], "direction": "x", "spacing": "3 mm", "count": "3",
                        "targets": ["b"]}),
            7 => json!({"format": 4, "kind": "ryucad.feature", "type": "instance", "part": "bolt",
                        "rev": "working",
                        "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            8 => json!({"format": 4, "kind": "ryucad.feature", "type": "mate", "form": "revolute",
                        "a": {"instance": ["base"], "at": "origin"},
                        "b": {"instance": ["pin"], "at": "origin"}}),
            9 => json!({"format": 5, "kind": "ryucad.feature", "type": "import", "body": 0,
                        "file": {"path": "imports/f.step", "blake3": "0".repeat(64)},
                        "product": {"id": "P-0", "name": "Part 0"}}),
            11 => json!({"default": "m0", "format": 1, "kind": "ryucad.materials",
                         "materials": {"m0": {"density": "7850 kg/m^3"},
                                       "m1": {"colour": "#888888", "density": "2.7 g/cm^3"}}}),
            10 => json!({"format": 5, "kind": "ryucad.feature", "type": "instance", "part": "bolt",
                         "rev": "working", "fixed": true,
                         "import": {"file": "imports/f.step",
                                    "product": {"id": "ASM", "name": "Assembly"},
                                    "usage": "NAUO0"},
                         "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            _ => json!({"built_with": {"commit": "c", "ryucad": "0", "ryukei": "0"},
                        "content": "0".repeat(64), "format": 1, "kind": "ryupdm.release",
                        "label": "A", "part": "p", "project": "0123456789abcdef0123456789abcdef",
                        "properties": {"number": "100"}, "released_by": {"participant": "agent"},
                        "results": {}, "supersedes": null}),
        };
        let edit = |v: &mut Value, op: u8, arg: u8| match kind {
            0 => edit_sketch(v, op, arg),
            1 => edit_part(v, op, arg),
            2 => edit_parameters(v, op, arg),
            4 => edit_properties(v, op, arg),
            6 => edit_pattern(v, op, arg),
            7 => edit_instance(v, op, arg),
            8 => edit_mate(v, op, arg),
            9 => edit_import(v, op, arg),
            10 => edit_imported(v, op, arg),
            11 => edit_materials(v, op, arg),
            _ => edit_record(v, op, arg),
        };
        // Edits alternate between the sides: the high bit of `op` says which.
        let (mut ours, mut theirs) = (base.clone(), base.clone());
        for pair in rest.chunks_exact(2) {
            let side = if pair[0] & 0x80 == 0 {
                &mut ours
            } else {
                &mut theirs
            };
            edit(side, pair[0] & 0x7f, pair[1]);
        }
        // A side that changed nothing gives the other.
        let (out, outcome) = bytes(&text(&base), &text(&base), &text(&theirs));
        assert_eq!((out, outcome), (text(&theirs), Outcome::Clean));
        // Item numbers merge per part: a conflict is only ever at one.
        if kind == 1 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys().filter(|k| k.starts_with("/items")) {
                assert!(k.len() > "/items/".len(), "items conflicted whole: {v}");
            }
        }
        // Poses are solver state: moved on both sides, no conflict.
        if kind == 7 || kind == 10 {
            let (_, outcome) = bytes(&text(&base), &text(&ours), &text(&theirs));
            if let Outcome::Conflicts(_) = outcome {
                let v: Value =
                    serde_json::from_slice(&bytes(&text(&base), &text(&ours), &text(&theirs)).0)
                        .expect("JSON");
                assert!(
                    record(&v).keys().all(|k| !k.starts_with("/pose")),
                    "a pose conflicted: {v}"
                );
            }
        }
        // Two re-imports of different bytes conflict on the hash.
        if kind == 9 {
            let hash = |v: &Value| v["file"]["blake3"].clone();
            if hash(&ours) != hash(&base)
                && hash(&theirs) != hash(&base)
                && hash(&ours) != hash(&theirs)
            {
                let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
                let v: Value = serde_json::from_slice(&merged).expect("JSON");
                assert!(
                    record(&v).keys().any(|k| k.starts_with("/file")),
                    "two re-imports merged: {v}"
                );
            }
        }
        (text(&base), text(&ours), text(&theirs), true)
    };
    let (out, outcome) = bytes(&base, &ours, &theirs);
    assert_eq!(bytes(&base, &ours, &theirs), (out.clone(), outcome));
    match outcome {
        Outcome::Text(_) => assert!(!readable, "readable files fell back to lines"),
        Outcome::Clean | Outcome::Conflicts(_) => {
            let text = std::str::from_utf8(&out).expect("UTF-8");
            assert!(canonical::is_canonical(text), "not canonical: {text}");
            let v: Value = serde_json::from_str(text).expect("JSON");
            let n = record(&v).len();
            match outcome {
                Outcome::Clean => assert!(v.get("conflicts").is_none()),
                _ => assert_eq!(Outcome::Conflicts(n), outcome),
            }
        }
    }
});
