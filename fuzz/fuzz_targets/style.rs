//! Standards files from bytes (design 348's `style` target; plan 2.E adds
//! style files to it): a hand edit, a newer ryueda, a torn write. The
//! bytes before the first zero are a standards file, the rest pin names.
//! Any file gives a typed refusal, or standards that write canonically and
//! read back the same; standards that pass their checks match any name
//! (linear in it), and every template they draw is a typed refusal or a
//! symbol that passes its own checks and lints without panicking. Any
//! panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryueda::standards::{self, Class, Standards};
use ryueda::symbol::PinType;
use ryueda::template::{self, PinSpec};

fuzz_target!(|data: &[u8]| {
    let cut = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    let (file, rest) = (&data[..cut], data.get(cut + 1..).unwrap_or(&[]));
    let path = "library/standards.json";
    let Ok(s) = ryueda::doc::read::<Standards>(file, path, standards::KIND, standards::FORMAT)
    else {
        return;
    };
    let text = ryueda::doc::to_string(&s, standards::KIND, standards::FORMAT);
    let back: Standards =
        ryueda::doc::read(text.as_bytes(), path, standards::KIND, standards::FORMAT)
            .expect("written standards read back");
    assert_eq!(back, s);
    if s.check().iter().any(|f| f.is_error()) {
        return;
    }
    let names: Vec<String> = String::from_utf8_lossy(rest)
        .split('\n')
        .take(64)
        .map(str::to_owned)
        .collect();
    let types = [
        PinType::PowerIn,
        PinType::Input,
        PinType::Output,
        PinType::Bidirectional,
    ];
    let pins: Vec<PinSpec> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let _ = s.group_of(n);
            let _ = s.role(n, types[i % 4]);
            PinSpec {
                number: (i + 1).to_string(),
                name: n.clone(),
                ty: types[i % 4],
            }
        })
        .collect();
    let prefs = ryusch::lint::Prefs::default();
    let cx = ryueda::symlint::Context {
        standards: &s,
        prefs: &prefs,
        from: "self",
    };
    for class in template::TEMPLATES {
        let given: &[PinSpec] = if matches!(class, Class::Ic | Class::Regulator | Class::Connector)
        {
            &pins
        } else {
            &[]
        };
        if let Ok(sym) = template::template("F", class, given, &s) {
            assert!(sym.check().iter().all(|f| !f.is_error()), "{class:?}");
            let _ = ryueda::symlint::lint(&sym, Some(class), &cx);
        }
    }
});
