//! Three versions of a file from bytes, merged as the merge driver merges
//! them. The first byte picks how the rest is read: raw text (three
//! versions split at `0xff` bytes, so anything at all reaches the driver),
//! or a base sketch, part list or parameters file and two sides' edits to
//! it, two bytes an edit. Merging must never panic, must give the same
//! bytes twice, must be canonical JSON whose record holds as many conflicts
//! as it says unless it fell back to lines, and readable files never fall
//! back. A side that changed nothing gives the other.

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

/// Applies edit `(op, arg)` to a part list.
fn edit_part(v: &mut Value, op: u8, arg: u8) {
    let mut list: Vec<String> = serde_json::from_value(v["features"].clone()).unwrap_or_default();
    let at = |n: usize| if n == 0 { 0 } else { arg as usize % n };
    match op % 3 {
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

fn text(v: &Value) -> Vec<u8> {
    canonical::to_string(v).into_bytes()
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let (base, ours, theirs, readable) = if mode % 4 == 3 {
        let mut parts = rest.splitn(3, |b| *b == 0xff);
        let (b, o, t) = (
            parts.next().unwrap_or(&[]).to_vec(),
            parts.next().unwrap_or(&[]).to_vec(),
            parts.next().unwrap_or(&[]).to_vec(),
        );
        (b, o, t, false)
    } else {
        let base = match mode % 4 {
            0 => sketch(),
            1 => json!({"features": ["a", "b", "c", "d"], "format": 1, "kind": "ryucad.part"}),
            _ => json!({"format": 1, "kind": "ryucad.parameters",
                        "parameters": {"p0": "1 mm", "p1": "p0 * 2"}}),
        };
        let edit = |v: &mut Value, op: u8, arg: u8| match mode % 4 {
            0 => edit_sketch(v, op, arg),
            1 => edit_part(v, op, arg),
            _ => edit_parameters(v, op, arg),
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
