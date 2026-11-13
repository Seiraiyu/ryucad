//! ryueda's merge driver from bytes (design 518): three versions of a
//! file merged as `ryueda git merge-driver` merges them. The first byte
//! picks the reading: below 0x80, raw (three versions split at 0xff
//! bytes, so anything at all reaches the driver); else a seeded file of
//! one of five kinds (a component, a net, a block, a style, a layout) and
//! two sides' edits to it, two bytes an edit, the sides split at a 0xfe
//! byte. Merging never panics and gives the same bytes twice; a merge by
//! value is canonical JSON whose record holds as many conflicts as it
//! says, and one with a record refuses to load as `merge_conflict`;
//! seeded files never fall back to lines; a side that changed nothing
//! gives the other, as written; and both sides adding the file (an empty
//! base) merges too.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryueda::merge::{Outcome, bytes};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn base(kind: usize) -> Value {
    match kind {
        0 => json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r"}, "refdes": "R1",
                    "pins": {"1": "n1", "2": {"rail": "gnd"}}}),
        1 => json!({"kind": "ryueda.net", "format": 1, "name": "N1"}),
        2 => json!({"kind": "ryueda.block", "format": 1, "name": "B",
                    "ports": {"p1": {"name": "P1", "direction": "in"}}}),
        3 => json!({"kind": "ryueda.style", "format": 1,
                    "palette": {"wire": {"light": "#000000", "dark": "#ffffff"}}}),
        _ => json!({"kind": "ryueda.layout", "format": 5, "placed": {}}),
    }
}

/// A set member toggled, the set written sorted (as writers write
/// `tags` and `locks`), the member dropped when empty.
fn toggle(v: &mut Value, field: &str, x: u8) {
    let item = ["a", "b", "c", "d"][usize::from(x) % 4];
    let mut set: BTreeSet<String> = v[field]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect();
    if !set.remove(item) {
        set.insert(item.to_owned());
    }
    match (set.is_empty(), v.as_object_mut()) {
        (true, Some(o)) => {
            o.remove(field);
        }
        _ => v[field] = json!(set),
    }
}

fn remove(v: &mut Value, key: &str) {
    if let Some(o) = v.as_object_mut() {
        o.remove(key);
    }
}

fn edited(kind: usize, b: &Value, edits: &[u8]) -> Value {
    let mut v = b.clone();
    for e in edits.chunks(2) {
        let (op, x) = (e[0], e.get(1).copied().unwrap_or(0));
        match (kind, op % 6) {
            (0, 0) => {
                v["pins"][format!("{}", x % 4 + 1)] = match x % 3 {
                    0 => json!(format!("n{}", x % 3)),
                    1 => json!({"rail": "gnd"}),
                    _ => json!("nc"),
                }
            }
            (0, 1) => remove(&mut v["pins"], &format!("{}", x % 4 + 1)),
            (0, 2) => v["value"] = json!(format!("{}k", x % 5)),
            (0, 3) => toggle(&mut v, "tags", x),
            (0, 4) => v["refdes"] = json!(format!("R{}", x % 9 + 1)),
            (0, _) => v["footprint"] = json!({"id": format!("f{}", x % 3)}),
            (1, 0) => v["name"] = json!(format!("N{}", x % 4)),
            (1, 1) => v["voltage"] = json!(format!("{} V", x % 5)),
            (1, 2) => toggle(&mut v, "tags", x),
            (1, 3) => {
                v["pair"] = json!({"with": format!("n{}", x % 3),
                    "polarity": if x % 2 == 0 { "p" } else { "n" }})
            }
            (1, _) => remove(&mut v, "voltage"),
            (2, 0) => {
                v["ports"][format!("p{}", x % 3)] =
                    json!({"name": format!("P{}", x % 5), "direction": "in"})
            }
            (2, 1) => remove(&mut v["ports"], &format!("p{}", x % 3)),
            (2, 2) => {
                v["buses"][format!("b{}", x % 2)] =
                    json!({"name": "B", "nets": [format!("n{}", x % 3)]})
            }
            (2, _) => v["name"] = json!(format!("B{}", x % 4)),
            (3, 0) => {
                v["palette"][format!("c{}", x % 4)] =
                    json!({"light": format!("#{:06x}", u32::from(x) * 4099), "dark": "#000000"})
            }
            (3, 1) => remove(&mut v["palette"], &format!("c{}", x % 4)),
            (3, 2) => toggle(&mut v, "locks", x),
            (3, _) => v["rules"] = json!([{"select": format!("s{}", x % 3)}]),
            (_, 0) => {
                v["placed"][format!("r{}", x % 4)] =
                    json!({"sheet": "s1", "at": [i64::from(x) * 1_270_000, 0]})
            }
            (_, 1) => remove(&mut v["placed"], &format!("r{}", x % 4)),
            (_, _) => v["instructions"] = json!([format!("beside R{} R1 left", x % 3)]),
        }
    }
    // The format its writer would choose.
    if kind < 3 {
        let k = v["kind"].as_str().unwrap_or_default().to_owned();
        v["format"] = json!(ryueda::conn::format_of(&k, &v));
    }
    v
}

/// Merges, checks what every merge must hold, and answers the bytes.
fn check(b: &[u8], o: &[u8], t: &[u8], readable: bool) -> Vec<u8> {
    let (out, outcome) = bytes(b, o, t);
    assert_eq!(bytes(b, o, t), (out.clone(), outcome), "the same bytes twice");
    let n = match outcome {
        Outcome::Text(_) => {
            assert!(!readable, "a readable file fell back to lines");
            return out;
        }
        Outcome::Clean => 0,
        Outcome::Conflicts(n) => n,
    };
    let text = std::str::from_utf8(&out).expect("UTF-8");
    let v = ryueda::canonical::parse(text).expect("JSON");
    assert_eq!(ryueda::canonical::to_string(&v), text, "canonical");
    let record = v
        .get("conflicts")
        .and_then(Value::as_object)
        .map_or(0, |o| o.len());
    assert_eq!(record, n, "the record holds what the outcome says");
    if n > 0 {
        let kind = v["kind"].as_str().unwrap_or_default().to_owned();
        let e = ryueda::doc::from_value::<Value>(v, "f.json", &kind, u64::MAX).unwrap_err();
        assert_eq!(e.code, ryueda::error::Code::MergeConflict);
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    if mode < 0x80 {
        let mut parts = rest.splitn(3, |b| *b == 0xff);
        let (b, o, t) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
        );
        check(b, o, t, false);
        return;
    }
    let kind = usize::from(mode) % 5;
    let mut sides = rest.splitn(2, |b| *b == 0xfe);
    let (eo, et) = (
        sides.next().unwrap_or_default(),
        sides.next().unwrap_or_default(),
    );
    let b = edited(kind, &base(kind), &[]);
    let (o, t) = (edited(kind, &b, eo), edited(kind, &b, et));
    let text = |v: &Value| ryueda::canonical::to_string(v).into_bytes();
    let (bb, ob, tb) = (text(&b), text(&o), text(&t));
    let out = check(&bb, &ob, &tb, true);
    if o == b {
        assert_eq!(out, tb, "ours unchanged gives theirs");
    }
    if t == b {
        assert_eq!(out, ob, "theirs unchanged gives ours");
    }
    check(&[], &ob, &tb, true);
});
