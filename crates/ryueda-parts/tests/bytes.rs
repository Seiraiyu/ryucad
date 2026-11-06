//! The parts fuzz target's checks over chosen bytes: a cache entry reads
//! as a typed miss or a record that encodes and decodes to itself, no
//! text panics the vocabulary or a condition, and a catalogue's files
//! (jlcparts' manifest, lookup table, shards and attributes) read as a
//! typed refusal or records whose every parameter reads as its kind.

use ryueda_parts::cache::{Decoded, decode, encode};
use ryueda_parts::param::{VOCABULARY, read};
use ryueda_parts::query::Condition;
use ryueda_parts::record::{Record, Store, Terms};

pub fn check(data: &[u8]) {
    if let Decoded::Entry(record, terms) = decode(data) {
        let again = encode(&record, &terms);
        match decode(again.as_bytes()) {
            Decoded::Entry(r, t) => assert_eq!((r, t), (record, terms)),
            other => panic!("an entry that decoded doesn't once written: {other:?} {again}"),
        }
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    for (name, _, _) in VOCABULARY {
        let _ = read(name, text);
    }
    let _ = Condition::parse(text);
    if let Ok(c) = Condition::parse("capacitance>=1nF") {
        let _ = c.judge(Some(text));
    }
    jlc(data);
}

/// As jlcparts' files: a typed refusal or what reads, never a panic.
fn jlc(data: &[u8]) {
    use ryueda_parts::jlc::{attr, rows, testing, wire};
    let _ = wire::Manifest::read(data);
    let _ = wire::Lut::read(data);
    let packed = testing::gz(data);
    let lut = wire::Lut::read(&testing::gz(
        serde_json::Value::from(testing::lut())
            .to_string()
            .as_bytes(),
    ))
    .expect("the fixture's table reads");
    let cat = wire::Category {
        id: 1,
        category: "Capacitors".into(),
        subcategory: "MLCC".into(),
        component_count: 0,
        shards: vec![],
    };
    for bytes in [data, packed.as_slice()] {
        if let Ok(shard) = rows::read_shard(bytes, &lut, &cat) {
            for r in shard.records {
                for (k, v) in &r.parameters {
                    assert!(matches!(read(k, v), Some(Ok(_))), "{k}={v}");
                }
            }
        }
    }
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
        for name in [
            "Capacitance",
            "Tolerance",
            "Operating Temperature",
            "Package",
            "Power(Watts)",
            "Temperature Coefficient",
        ] {
            for c in [
                ryueda_parts::category::Category::Resistor,
                ryueda_parts::category::Category::Capacitor,
            ] {
                if let Some((k, text)) = attr::vocabulary(name, &v, c) {
                    assert!(matches!(read(k, &text), Some(Ok(_))), "{k}={text}");
                }
            }
        }
    }
}

#[test]
fn chosen_bytes_hold() {
    let mut r = Record::new("jlc", "C1525");
    r.parameters.insert("capacitance".into(), "100 nF".into());
    let terms = Terms {
        store: Store::Ttl(86_400),
        write_parameters: false,
        attribution: "x".into(),
    };
    let entry = encode(&r, &terms);
    let zero = entry.replace("\"format\": 1", "\"format\": 0");
    let mut fixtures: Vec<Vec<u8>> = ryueda_parts::jlc::testing::site("x")
        .into_iter()
        .map(|(_, b)| b)
        .collect();
    for rows in [
        "[]",
        "[null]",
        r#"["C1", 5]"#,
        r#"["C1", "M", 2, "d", "", [], null, "", [999999, -1, "x", 1e300], 5, 1]"#,
        r#"["C2", "M", 2, "d", "", [{"qFrom": -1, "price": "x"}], null, "", [0, 1, 2], 18446744073709551615, 1]"#,
    ] {
        fixtures.push(ryueda_parts::jlc::testing::gz(
            format!("{}\n{rows}\n", ryueda_parts::jlc::testing::SCHEMA).as_bytes(),
        ));
    }
    for v in [
        r#"{"primary": "capacitance", "values": {"capacitance": [1e308, "capacitance"]}}"#,
        r#"{"primary": "x", "values": {"x": [-1e-320, "percentage"]}}"#,
        r#"{"values": {"temperature min": [125, "temperature"], "temperature max": [-55, "temperature"]}}"#,
        r#"{"primary": "default", "values": {"default": ["", "string"]}}"#,
    ] {
        fixtures.push(v.as_bytes().to_vec());
    }
    for bytes in &fixtures {
        check(bytes);
    }
    let cases: [&[u8]; 10] = [
        entry.as_bytes(),
        zero.as_bytes(),
        b"",
        b"\xff\xfe",
        b"{}",
        b"capacitance>=",
        "±±5%".as_bytes(),
        b"1e2147483647 nF",
        b"-55 \xc2\xb0C to",
        b"9999999999999999999999999999999 V",
    ];
    for bytes in cases {
        check(bytes);
    }
    assert!(matches!(decode(entry.as_bytes()), Decoded::Entry(..)));
}
