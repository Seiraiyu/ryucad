//! Bit-exact results on every platform and CPU level:
//! `tests/golden/layout.txt` holds hashes of seeded blocks' layouts (from
//! nothing, after instructions and over a previous layout after an edit),
//! their SVG, lint and descriptions. After a deliberate change, regenerate
//! with `UPDATE_GOLDEN=1 cargo test -p ryusch --test determinism` and
//! review the diff like any other change.

use ryusch::engine::lay_out;
use ryusch::geom::Side;
use ryusch::layout::{Instruction, NetStyle};
use ryusch::testing::seeded;
use std::collections::BTreeMap;

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..32].to_owned()
}

fn record(got: &mut BTreeMap<String, String>, name: &str, block: &ryusch::input::Block) {
    let out = lay_out(block).unwrap();
    let (findings, scores) = ryusch::lint::lint(block, &out);
    got.insert(
        format!("{name}.layout"),
        hash(serde_json::to_string(&out.layout).unwrap().as_bytes()),
    );
    got.insert(
        format!("{name}.svg"),
        hash(ryusch::svg::draw(block, &out.layout, &[]).as_bytes()),
    );
    got.insert(
        format!("{name}.lint"),
        hash(
            serde_json::to_string(&(findings.clone(), scores.clone()))
                .unwrap()
                .as_bytes(),
        ),
    );
    got.insert(
        format!("{name}.describe"),
        hash(
            serde_json::to_string(&ryusch::describe::describe(block, &out, &findings, &scores))
                .unwrap()
                .as_bytes(),
        ),
    );
}

#[test]
fn layouts_are_bit_exact() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/layout.txt");
    let mut got = BTreeMap::new();
    for (seed, ics) in [(1, 2), (2, 4), (3, 6), (4, 9)] {
        record(&mut got, &format!("seeded{seed}"), &seeded(seed, ics));
    }
    // Instructions, then an edit over the layout they gave.
    let mut b = seeded(5, 5);
    b.instructions.insert(
        "i1".into(),
        Instruction::Beside {
            parts: vec!["u1".into()],
            of: "u0".into(),
            side: Side::Down,
        },
    );
    b.instructions.insert(
        "i2".into(),
        Instruction::Net {
            net: "rail:gnd".into(),
            style: NetStyle::Label,
        },
    );
    record(&mut got, "instructed", &b);
    b.previous = Some(lay_out(&b).unwrap().layout);
    let p = b.part("u2").unwrap();
    b.parts[p].value = "LOGIC-HC".into();
    record(&mut got, "edited", &b);
    let text: String = std::iter::once(
        "# ryusch's golden hashes: UPDATE_GOLDEN=1 cargo test -p ryusch --test determinism\n"
            .to_owned(),
    )
    .chain(got.iter().map(|(k, v)| format!("{k} {v}\n")))
    .collect();
    if std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(path, text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(path).unwrap_or_default();
    let differ: Vec<&str> = text
        .lines()
        .zip(want.lines())
        .filter(|(a, b)| a != b)
        .map(|(a, _)| a)
        .collect();
    assert!(
        differ.is_empty() && text.lines().count() == want.lines().count(),
        "golden hashes differ: {differ:?}; regenerate with UPDATE_GOLDEN=1 if that was meant"
    );
}
