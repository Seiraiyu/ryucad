//! The parts fuzz target's checks over chosen bytes: a cache entry reads
//! as a typed miss or a record that encodes and decodes to itself, and no
//! text panics the vocabulary or a condition.

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
