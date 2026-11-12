//! Standards and style files from bytes (design 348's `style` target): a
//! hand edit, a newer ryueda, a torn write. Bytes starting `S:` are a
//! style file; any other input's bytes before the first zero are a
//! standards file, the rest pin names.
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
    if let Some(rest) = data.strip_prefix(b"S:") {
        use ryueda::style::{self, Layer, Style};
        let Ok(s) = ryueda::doc::read::<Style>(rest, style::FILE, style::KIND, style::FORMAT)
        else {
            return;
        };
        let text = ryueda::doc::to_string(&s, style::KIND, style::FORMAT);
        let back: Style =
            ryueda::doc::read(text.as_bytes(), style::FILE, style::KIND, style::FORMAT)
                .expect("a written style reads back");
        assert_eq!(back, s);
        let builtin = style::builtin();
        let known = |n: &str| builtin.palette.contains_key(n) || s.palette.contains_key(n);
        for layer in [Layer::Project, Layer::Organisation] {
            let _ = s.check(layer, &known);
        }
        // As the project's layer and as the organisation's, the cascade
        // resolves an object the same twice and never panics.
        let empty = Style::default();
        for layers in [
            ryueda::cascade::Layers::new(None, &s),
            ryueda::cascade::Layers::new(Some(("org/style.json".into(), &s)), &empty),
        ] {
            let o = ryueda::cascade::Object {
                kind: style::Kind::Component,
                key: "r1".into(),
                id: "component/b1/r1".into(),
                block: ("b1".into(), "main".into()),
                regions: vec![("r1".into(), "Power".into())],
                class: None,
                rail: None,
                component_class: Some(ryueda::standards::Class::Resistor),
                libcomp: Some("r".into()),
                tags: ["hv".to_owned(), "dnp".to_owned()].into_iter().collect(),
            };
            for sheet in [None, Some(("s1", "Sheet 1"))] {
                let a = layers.resolve(&o, sheet);
                assert_eq!(a, layers.resolve(&o, sheet));
                for theme in [ryusch::style::Theme::Light, ryusch::style::Theme::Dark] {
                    let _ = layers.pen(&a.props, theme);
                }
            }
            let _ = layers.broken_locks();
            let _ = ryueda::stylelint::contrast_findings(&layers);
        }
        return;
    }
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
