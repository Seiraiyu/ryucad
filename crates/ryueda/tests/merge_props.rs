//! The merge rules on generated files: components and nets, each edited
//! at random on two sides. No change on one side gives the other, the same
//! change on both gives it, a clean merge is the same either way round,
//! and every result is canonical: clean, or carrying conflicts that make
//! loading fail with `merge_conflict`.

use proptest::prelude::*;
use ryueda::merge::{Outcome, bytes, merge};
use serde_json::{Value, json};

/// One edit to a component or a net.
#[derive(Clone, Debug)]
enum Edit {
    Pin(u8, u8),
    Unpin(u8),
    Value(u8),
    Tag(u8),
    Untag(u8),
    Refdes(u8),
    Voltage(u8),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (1u8..6, 0u8..5).prop_map(|(p, n)| Edit::Pin(p, n)),
        (1u8..6).prop_map(Edit::Unpin),
        (0u8..4).prop_map(Edit::Value),
        (0u8..4).prop_map(Edit::Tag),
        (0u8..4).prop_map(Edit::Untag),
        (1u8..9).prop_map(Edit::Refdes),
        (0u8..3).prop_map(Edit::Voltage),
    ]
}

/// A node: a net of the block, a rail, a port, or not connected.
fn node(n: u8) -> Value {
    match n {
        0 => json!("n1"),
        1 => json!("n2"),
        2 => json!({"rail": "gnd"}),
        3 => json!({"port": "p1"}),
        _ => json!("nc"),
    }
}

const TAGS: [&str; 4] = ["critical", "dnp", "hv", "x"];

/// `v` with `edits` applied, at the format its writer would choose.
fn edited(v: &Value, edits: &[Edit]) -> Value {
    let mut v = v.clone();
    let net = v["kind"] == "ryueda.net";
    for e in edits {
        match e {
            Edit::Pin(p, n) if !net => v["pins"][p.to_string()] = node(*n),
            Edit::Unpin(p) if !net => {
                if let Some(o) = v["pins"].as_object_mut() {
                    o.remove(&p.to_string());
                }
            }
            Edit::Value(x) if !net => v["value"] = json!(format!("{}k", x + 1)),
            Edit::Refdes(r) if !net => v["refdes"] = json!(format!("R{r}")),
            Edit::Voltage(x) if net => v["voltage"] = json!(format!("{}.3 V", x + 1)),
            Edit::Tag(t) => {
                let mut tags: Vec<String> =
                    serde_json::from_value(v["tags"].clone()).unwrap_or_default();
                tags.push(TAGS[*t as usize].to_owned());
                tags.sort();
                tags.dedup();
                v["tags"] = json!(tags);
            }
            Edit::Untag(t) => {
                let mut tags: Vec<String> =
                    serde_json::from_value(v["tags"].clone()).unwrap_or_default();
                tags.retain(|x| x != TAGS[*t as usize]);
                if tags.is_empty() {
                    if let Some(o) = v.as_object_mut() {
                        o.remove("tags");
                    }
                } else {
                    v["tags"] = json!(tags);
                }
            }
            _ => {}
        }
    }
    if v["pins"].as_object().is_some_and(|o| o.is_empty())
        && let Some(o) = v.as_object_mut()
    {
        o.remove("pins");
    }
    let kind = v["kind"].as_str().unwrap().to_owned();
    v["format"] = json!(ryueda::conn::format_of(&kind, &v));
    v
}

fn base(net: bool) -> Value {
    if net {
        json!({"kind": "ryueda.net", "format": 1, "name": "SDA", "voltage": "3.3 V"})
    } else {
        json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_0402"}, "refdes": "R1",
               "value": "1k", "pins": {"1": "n1", "2": {"rail": "gnd"}}})
    }
}

fn text(v: &Value) -> Vec<u8> {
    ryueda::canonical::to_string(v).into_bytes()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn merges_keep_their_promises(
        net in any::<bool>(),
        ours in proptest::collection::vec(edit(), 0..5),
        theirs in proptest::collection::vec(edit(), 0..5),
    ) {
        let b = base(net);
        let (o, t) = (edited(&b, &ours), edited(&b, &theirs));
        // No change on one side gives the other.
        let m = merge(Some(&b), &b, &t);
        prop_assert!(m.is_clean());
        prop_assert_eq!(&m.value, &t);
        // The same change on both gives it.
        let m = merge(Some(&b), &o, &o);
        prop_assert!(m.is_clean());
        prop_assert_eq!(&m.value, &o);
        // Clean either way round, the same files.
        let (x, y) = (merge(Some(&b), &o, &t), merge(Some(&b), &t, &o));
        if x.is_clean() && y.is_clean() {
            prop_assert_eq!(&x.value, &y.value);
        }
        prop_assert_eq!(x.is_clean(), y.is_clean());
        // What the driver writes is canonical, and a record never loads.
        let (out, outcome) = bytes(&text(&b), &text(&o), &text(&t));
        let parsed = ryueda::canonical::parse(std::str::from_utf8(&out).unwrap()).unwrap();
        prop_assert_eq!(ryueda::canonical::to_string(&parsed).into_bytes(), out.clone());
        let kind = if net { "ryueda.net" } else { "ryueda.component" };
        let read = ryueda::doc::read::<Value>(&out, "f.json", kind, 2);
        match outcome {
            Outcome::Clean => prop_assert!(read.is_ok()),
            Outcome::Conflicts(_) => {
                prop_assert_eq!(read.unwrap_err().code, ryueda::error::Code::MergeConflict)
            }
            Outcome::Text(_) => prop_assert!(false, "every version reads"),
        }
    }
}
