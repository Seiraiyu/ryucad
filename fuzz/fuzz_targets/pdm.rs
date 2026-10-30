//! ryupdm's files from bytes: settings, properties and release records as
//! a hand edit, a merge or a newer build might leave them. Settings and
//! records that read must write back to themselves; properties must read
//! back as written and be judged against the settings without panicking;
//! a record's labels must read the same in every scheme they belong to.
//! The text is also an `amount` against every unit of measure: one that
//! reads is a positive number and judges a part counted in that unit
//! sound, written at format 2 and read back as written. The bytes are
//! also read as `approvals` and as a release tag's message: an attestation
//! that parses writes back to its own canonical text, and a verdict is
//! always one of four, never a panic. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::doc::canonical;
use ryucad::doc::id::Id;
use ryucad::units::Unit;
use ryupdm::config::{Config, Scheme, UOMS};
use ryupdm::label;
use ryupdm::props::{Properties, amount, check, coerce, fits, missing};
use ryupdm::record::{Kind, Record};

/// Settings with one property of every type, to judge properties against.
const SETTINGS: &str = r#"{"format": 1, "kind": "ryupdm.config",
  "part_number": {"pattern": "[A-Z]{2}-\\d+"},
  "properties": {
    "c": {"type": "integer"}, "e": {"type": "enum", "values": ["x", "y"]},
    "f": {"type": "number"}, "g": {"type": "quantity", "dim": "length^2"},
    "h": {"type": "boolean"}, "t": {"type": "text"}
  }}"#;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    amounts(text);
    attestation(data);
    let Ok(v) = canonical::parse(text) else {
        return;
    };
    if let Ok(c) = Config::from_json(&v) {
        let again = Config::from_json(&c.to_json()).expect("written settings read");
        assert_eq!(again, c);
        let out = canonical::to_string(&c.to_json());
        assert_eq!(canonical::to_string(&again.to_json()), out);
    }
    if let Ok(a) = ryupdm::approvals::Approvals::from_json(&v, "/approvals") {
        let again = ryupdm::approvals::Approvals::from_json(&a.to_json(), "/approvals")
            .expect("written approvals read");
        assert_eq!(again, a);
        for r in &a.rules {
            assert!(r.count >= 1 && r.count as usize <= r.from.len());
        }
    }
    for kind in [Kind::Release, Kind::Obsolete] {
        let Ok(r) = Record::from_json(&v, kind) else {
            continue;
        };
        let again = Record::from_json(&r.to_json(), kind).expect("written records read");
        assert_eq!(again, r);
        assert_eq!(
            canonical::to_string(&again.to_json()),
            canonical::to_string(&r.to_json())
        );
        for scheme in Scheme::ALL {
            if let Some(n) = label::index(scheme, r.label()) {
                assert_eq!(label::label(scheme, n), r.label());
                let next = label::next(scheme, r.label()).unwrap();
                assert_eq!(label::index(scheme, &next), Some(n + 1));
            }
        }
    }
    if let Ok(p) = Properties::from_json(&v) {
        assert_eq!(Properties::from_json(&p.to_json()).unwrap(), p);
        let settings = Config::from_json(&canonical::parse(SETTINGS).unwrap()).unwrap();
        let part = Id::parse("p", "part").unwrap();
        let faults = check(&settings, Unit::Mm, &part, &p);
        assert!(faults.iter().all(|e| e.details["part"] == "p"));
        for (name, value) in &p.values {
            if let Some(d) = settings.decl(name) {
                let typed = coerce(&d, value.clone());
                // Coercing never turns a fitting value into one that
                // doesn't fit.
                if fits(&d, value, Unit::Mm).is_none() {
                    assert!(fits(&d, &typed, Unit::Mm).is_none(), "{name}: {value}");
                }
            }
        }
    }
});

/// `data` as a release tag's message: parsed, written back, and judged.
fn attestation(data: &[u8]) {
    use ryupdm::attest::{Expect, Verdict, message, parse_message, verify_message};
    if let Ok((a, sig, text)) = parse_message(data) {
        assert_eq!(a.canonical(), text);
        let again = message(&a, &sig);
        assert_eq!(parse_message(again.as_bytes()).unwrap().0, a);
    }
    let project = ryucad::doc::id::ProjectId::parse("0123456789abcdef0123456789abcdef").unwrap();
    let part = Id::parse("p", "part").unwrap();
    let keys = std::collections::BTreeMap::new();
    let v = verify_message(
        data,
        &Expect {
            project: &project,
            part: &part,
            label: "A",
            kind: Kind::Release,
            commit: &"0".repeat(40),
            record: &"0".repeat(64),
            rules: &[],
            rules_at: &"0".repeat(40),
            keys: &keys,
        },
    );
    // No key is trusted, so nothing verifies.
    assert!(matches!(v, Verdict::Invalid(_)));
}

/// `text` as the `amount` of a part counted in each unit of measure.
fn amounts(text: &str) {
    let settings = Config::from_json(&canonical::parse(SETTINGS).unwrap()).unwrap();
    let part = Id::parse("p", "part").unwrap();
    for uom in UOMS {
        let read = amount(text, uom);
        if let Ok(x) = read {
            assert!(x.is_finite() && x > 0.0, "{text:?} in {uom}: {x}");
        }
        let mut values = std::collections::BTreeMap::new();
        values.insert("uom".to_owned(), serde_json::json!(uom));
        values.insert("amount".to_owned(), serde_json::json!(text));
        let p = Properties { values };
        let v = p.to_json();
        assert_eq!(v["format"], 2);
        assert_eq!(Properties::from_json(&v).unwrap(), p);
        let faults = check(&settings, Unit::Mm, &part, &p);
        let judged = faults.iter().any(|e| e.details["property"] == "amount");
        // Judged unsound when it isn't one line of text, or not an amount.
        let decl = settings.decl("amount").unwrap();
        let bad = fits(&decl, &serde_json::json!(text), Unit::Mm).is_some() || read.is_err();
        assert_eq!(judged, bad, "{text:?} in {uom}: {faults:?}");
        // With an amount, only the number is missing to release.
        assert_eq!(missing(&settings, &p), ["number"]);
    }
}
