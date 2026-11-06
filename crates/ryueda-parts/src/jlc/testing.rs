//! A small catalogue in jlcparts' format 4, written by us (no jlcparts
//! data is committed): two capacitors, two resistors and a regulator, the
//! lookup table and the manifest, gzipped as jlcparts serves them.

use serde_json::{Value, json};
use std::io::Write;

pub const BASE: &str = "https://jlc.test/data";

pub fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).expect("in memory");
    e.finish().expect("in memory")
}

fn attr(primary: &str, value: Value, unit: &str) -> Value {
    json!({"format": format!("${{{primary}}}"), "primary": primary,
           "values": {primary: [value, unit]}})
}

/// The lookup table: `[name, value]` by index.
pub fn lut() -> Vec<Value> {
    vec![
        json!([
            "Capacitance",
            attr("capacitance", json!(1e-7), "capacitance")
        ]), // 0
        json!(["Voltage Rated", attr("voltage", json!(16.0), "voltage")]), // 1
        json!(["Tolerance", attr("tolerance", json!(10.0), "percentage")]), // 2
        json!([
            "Temperature Coefficient",
            attr("default", json!("X7R"), "string")
        ]), // 3
        json!(["Package", attr("default", json!("0402"), "string")]),      // 4
        json!(["Basic/Extended", attr("default", json!("Basic"), "string")]), // 5
        json!([
            "Manufacturer",
            attr("default", json!("Samsung Electro-Mechanics"), "string")
        ]), // 6
        json!([
            "Capacitance",
            attr("capacitance", json!(1e-6), "capacitance")
        ]), // 7
        json!([
            "Basic/Extended",
            attr("default", json!("Extended"), "string")
        ]), // 8
        json!([
            "Resistance",
            attr("resistance", json!(100000.0), "resistance")
        ]), // 9
        json!(["Power(Watts)", attr("power", json!(0.0625), "power")]),    // 10
        json!(["Tolerance", attr("tolerance", json!(1.0), "percentage")]), // 11
        json!([
            "Manufacturer",
            attr("default", json!("UNI-ROYAL(Uniroyal Elec)"), "string")
        ]), // 12
        json!([
            "Resistance",
            attr("resistance", json!(10000.0), "resistance")
        ]), // 13
        json!([
            "Operating Temperature",
            json!({"format": "${temperature min} ~ ${temperature max}",
            "primary": "temperature min",
            "values": {"temperature min": [-55.0, "temperature"], "temperature max": [125.0, "temperature"]}})
        ]), // 14
        json!([
            "Basic/Extended",
            attr("default", json!("Preferred"), "string")
        ]), // 15
        json!(["Package", attr("default", json!("SOT-23-5"), "string")]),  // 16
        json!([
            "Manufacturer",
            attr("default", json!("Texas Instruments"), "string")
        ]), // 17
        json!(["Output Voltage", attr("voltage", json!(3.3), "voltage")]), // 18
    ]
}

/// A shard's first line.
pub const SCHEMA: &str = r#"{"lcsc":0,"mfr":1,"joints":2,"description":3,"datasheet":4,"price":5,"img":6,"url":7,"attributes":8,"stock":9,"subcategory":10}"#;

fn row(lcsc: &str, mfr: &str, desc: &str, attrs: &[usize], stock: u64, sub: u64) -> String {
    json!([lcsc, mfr, 2, desc, format!("https://datasheets.test/{lcsc}.pdf"),
           [{"qFrom": 1, "qTo": 99, "price": 0.0012}, {"qFrom": 100, "qTo": null, "price": 0.0009}],
           null, "slug", attrs, stock, sub])
    .to_string()
}

/// A shard of `rows` (each a JSON array's text) after the schema line.
pub fn shard(rows: &[String]) -> Vec<u8> {
    gz(format!("{SCHEMA}\n{}\n", rows.join("\n")).as_bytes())
}

/// Every URL of the catalogue and its body, the manifest `created` at
/// `created`.
pub fn site(created: &str) -> Vec<(String, Vec<u8>)> {
    let caps = shard(&[
        row(
            "C1525",
            "CL05B104KO5NNNC",
            "100nF 16V X7R 0402",
            &[0, 1, 2, 3, 4, 5, 6],
            1_200_000,
            1,
        ),
        row(
            "C52923",
            "CL05A105KA5NNNC",
            "1uF 25V X5R 0402",
            &[7, 4, 8, 6],
            40,
            1,
        ),
    ]);
    let res = shard(&[
        row(
            "C25741",
            "0402WGF1003TCE",
            "100k 1% 0402",
            &[9, 10, 11, 4, 5, 12, 14],
            3_000_000,
            2,
        ),
        row(
            "C25744",
            "0402WGF1002TCE",
            "10k 1% 0402",
            &[13, 10, 11, 4, 15, 12],
            0,
            2,
        ),
    ]);
    let regs = shard(&[row(
        "C5446",
        "TPS7A2033PDBVR",
        "LDO 3.3V",
        &[16, 8, 17, 18],
        900,
        3,
    )]);
    let manifest = json!({
        "version": 4, "created": created, "totalComponents": 5,
        "attributesLut": "attributes-lut.json.gz",
        "categories": [
            {"id": 1, "category": "Capacitors", "subcategory": "Multilayer Ceramic Capacitors MLCC - SMD/SMT",
             "componentCount": 2, "shards": ["components-caps-001.jsonl.gz"], "browseShards": [], "rawCategories": []},
            {"id": 2, "category": "Resistors", "subcategory": "Chip Resistor - Surface Mount",
             "componentCount": 2, "shards": ["components-res-001.jsonl.gz"], "browseShards": [], "rawCategories": []},
            {"id": 3, "category": "Power Management", "subcategory": "Linear Voltage Regulators (LDO)",
             "componentCount": 1, "shards": ["components-regs-001.jsonl.gz"], "browseShards": [], "rawCategories": []}
        ],
        "files": {}
    });
    vec![
        (
            format!("{BASE}/manifest.json"),
            manifest.to_string().into_bytes(),
        ),
        (
            format!("{BASE}/attributes-lut.json.gz"),
            gz(Value::from(lut()).to_string().as_bytes()),
        ),
        (format!("{BASE}/components-caps-001.jsonl.gz"), caps),
        (format!("{BASE}/components-res-001.jsonl.gz"), res),
        (format!("{BASE}/components-regs-001.jsonl.gz"), regs),
    ]
}

/// [`site`] as a fake fetcher.
pub fn fake(created: &str) -> crate::fetch::Fake {
    site(created)
        .into_iter()
        .fold(crate::fetch::Fake::default(), |f, (u, b)| f.with(&u, b))
}
