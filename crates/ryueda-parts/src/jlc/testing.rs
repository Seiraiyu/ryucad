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

/// Records of the catalogue for `parts` (`(key, mpn, stock, basic)`),
/// each a resistor with an LCSC offer of its key, as a synced index under
/// `root` (the parts cache's `jlc/`), fetched at `fetched_at`.
pub fn catalogue_of(root: &std::path::Path, parts: &[(&str, &str, u64, bool)], fetched_at: u64) {
    let recs: Vec<crate::record::Record> = parts
        .iter()
        .map(|(k, mpn, stock, basic)| {
            let mut r = crate::record::Record::new(super::NAME, k);
            r.mpn = (*mpn).to_owned();
            r.stock = Some(*stock);
            r.basic = Some(*basic);
            r.category = Some(crate::category::Category::Resistor);
            r.offers = vec![crate::record::Offer {
                supplier: "LCSC".into(),
                sku: (*k).to_owned(),
            }];
            r
        })
        .collect();
    super::index::write_next(root, &recs, fetched_at, "fixture").expect("an index");
    super::index::swap(root).expect("swapped in");
}

/// `n` records over every category, a third of them capacitors, keys
/// `C<i>`, their parameters drawn by a fixed linear congruential sequence
/// from small tables (capacitances 1 pF to 100 uF in E6 steps, voltages,
/// packages, dielectrics, tolerances; resistances likewise), stock and
/// Basic as the sequence gives: a catalogue's size without its data.
pub fn seeded(n: usize) -> Vec<crate::record::Record> {
    use crate::category::{ALL, Category};
    use ryu_doc::units::Dim;
    const E6: [&str; 6] = ["1", "1.5", "2.2", "3.3", "4.7", "6.8"];
    // An E6 value times a power of ten, read from its text: exact, the
    // same bits everywhere.
    let e6 = |i: usize, exp: i32| -> f64 { format!("{}e{exp}", E6[i]).parse().expect("a number") };
    const VOLTS: [&str; 6] = ["6.3 V", "10 V", "16 V", "25 V", "50 V", "100 V"];
    const PACKAGES: [&str; 5] = ["0201", "0402", "0603", "0805", "1206"];
    const DIELECTRICS: [&str; 4] = ["C0G", "X5R", "X7R", "Y5V"];
    const TOLERANCES: [&str; 4] = ["1%", "5%", "10%", "20%"];
    let mut x: u64 = 0x5eed;
    let mut next = |m: usize| {
        x = x
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((x >> 33) % m as u64) as usize
    };
    let others: Vec<Category> = ALL
        .into_iter()
        .filter(|c| *c != Category::Capacitor)
        .collect();
    (0..n)
        .map(|i| {
            let cat = if i % 3 == 0 {
                Category::Capacitor
            } else {
                others[next(others.len())]
            };
            let mut r = crate::record::Record::new(super::NAME, &format!("C{i}"));
            r.category = Some(cat);
            r.mpn = format!("MPN-{i:07}");
            r.manufacturer = "Maker".into();
            r.offers = vec![crate::record::Offer {
                supplier: "LCSC".into(),
                sku: r.key.clone(),
            }];
            r.stock = Some([0, 10, 500, 5_000, 50_000, 1_000_000][next(6)]);
            r.basic = Some(next(4) == 0);
            let mut put = |k: &str, v: String| {
                r.parameters.insert(k.to_owned(), v);
            };
            put("package", PACKAGES[next(PACKAGES.len())].to_owned());
            put("tolerance", TOLERANCES[next(TOLERANCES.len())].to_owned());
            match cat {
                Category::Capacitor => {
                    // 1 pF to 100 uF: eight decades of E6, and 100 uF.
                    let k = next(8 * 6 + 1);
                    let si = if k == 48 {
                        1e-4
                    } else {
                        e6(k % 6, k as i32 / 6 - 12)
                    };
                    if let Some(t) = crate::param::si_text(si, Dim::CAPACITANCE) {
                        put("capacitance", t);
                    }
                    put("voltage_rating", VOLTS[next(VOLTS.len())].to_owned());
                    put(
                        "dielectric",
                        DIELECTRICS[next(DIELECTRICS.len())].to_owned(),
                    );
                }
                Category::Resistor => {
                    let si = e6(next(6), next(7) as i32);
                    if let Some(t) = crate::param::si_text(si, Dim::RESISTANCE) {
                        put("resistance", t);
                    }
                }
                _ => {}
            }
            r
        })
        .collect()
}
