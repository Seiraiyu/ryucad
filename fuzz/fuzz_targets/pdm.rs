//! ryupdm's files from bytes: settings and properties as a hand edit, a
//! merge or a newer build might leave them. Settings that read must write
//! back to themselves; properties must read back as written and be judged
//! against the settings without panicking. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::doc::canonical;
use ryucad::doc::id::Id;
use ryucad::units::Unit;
use ryupdm::config::Config;
use ryupdm::props::{Properties, check, coerce, fits};

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
    let Ok(v) = canonical::parse(text) else {
        return;
    };
    if let Ok(c) = Config::from_json(&v) {
        let again = Config::from_json(&c.to_json()).expect("written settings read");
        assert_eq!(again, c);
        let out = canonical::to_string(&c.to_json());
        assert_eq!(canonical::to_string(&again.to_json()), out);
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
