//! A seeded library of resistors and capacitors written as files, for the
//! parts search's golden answers and its bar: `n` library components with
//! values, packages, tolerances, ratings and dielectrics drawn from a
//! fixed sequence; every third has no maker part number, and every fifth
//! of the rest no offer. With `design`, a block places the first twelve,
//! the fourth marked `dnp`.

#![allow(dead_code)]

use serde_json::{Map, Value, json};
use std::path::Path;

fn put(dir: &Path, rel: &str, kind: &str, mut v: Value) {
    v["kind"] = json!(kind);
    v["format"] = json!(1);
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, ryueda::canonical::to_string(&v)).unwrap();
}

/// A 64-bit linear congruential sequence: the same parts everywhere.
struct Lcg(u64);

impl Lcg {
    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        from[((self.0 >> 33) % from.len() as u64) as usize]
    }
}

const RESISTANCES: [&str; 6] = [
    "100 Ohm", "1 kOhm", "4.7 kOhm", "10 kOhm", "47 kOhm", "100 kOhm",
];
const CAPACITANCES: [&str; 5] = ["10 pF", "100 nF", "1 uF", "4.7 uF", "10 uF"];
const PACKAGES: [&str; 3] = ["0402", "0603", "0805"];
const VOLTAGES: [&str; 4] = ["6.3 V", "16 V", "25 V", "50 V"];
const DIELECTRICS: [&str; 3] = ["C0G", "X5R", "X7R"];
const TOLERANCES: [&str; 3] = ["1%", "5%", "±10%"];

pub fn write(dir: &Path, n: usize, design: bool) {
    put(
        dir,
        "ryueda.json",
        "ryueda.project",
        json!({"project": "0123456789abcdef0123456789abcdef", "default_unit": "mm"}),
    );
    for (id, name) in [("r", "R"), ("c", "C")] {
        put(
            dir,
            &format!("library/symbols/{id}.json"),
            "ryueda.symbol",
            json!({"name": name, "units": 1, "pins": {
                "1": {"name": "~", "type": "passive", "at": [0, 3_810_000], "side": "down", "length_nm": 1_270_000},
                "2": {"name": "~", "type": "passive", "at": [0, -3_810_000], "side": "up", "length_nm": 1_270_000}
            }}),
        );
    }
    let pad = |n: &str, x: i64| {
        json!({"number": n, "kind": "smd", "shape": {"type": "rect"}, "at": [x, 0], "size": [560_000, 620_000],
               "layers": ["copper.top", "mask.top", "paste.top"]})
    };
    put(
        dir,
        "library/footprints/chip.json",
        "ryueda.footprint",
        json!({"name": "Chip", "mount": "smd",
        "pads": {"p1": pad("1", -480_000), "p2": pad("2", 480_000)},
        "graphics": {"g1": {"layer": "courtyard.top", "type": "rect", "start": [-1_000_000, 500_000],
                            "end": [1_000_000, -500_000], "width_nm": 50_000}}}),
    );
    let mut lcg = Lcg(0x6a);
    for i in 0..n {
        let cap = i % 2 == 1;
        let mut params = Map::new();
        params.insert("package".into(), json!(lcg.pick(&PACKAGES)));
        let (prefix, value) = if cap {
            let v = lcg.pick(&CAPACITANCES);
            params.insert("capacitance".into(), json!(v));
            params.insert("voltage_rating".into(), json!(lcg.pick(&VOLTAGES)));
            params.insert("dielectric".into(), json!(lcg.pick(&DIELECTRICS)));
            ("C", v)
        } else {
            let v = lcg.pick(&RESISTANCES);
            params.insert("resistance".into(), json!(v));
            ("R", v)
        };
        params.insert("tolerance".into(), json!(lcg.pick(&TOLERANCES)));
        let mut v = json!({"symbol": {"id": if cap { "c" } else { "r" }}, "prefix": prefix, "value": value,
                           "parameters": params, "footprints": [{"footprint": {"id": "chip"}}]});
        if i % 3 != 0 {
            v["manufacturer"] = json!("Maker");
            v["mpn"] = json!(format!("PN-{i:05}"));
            if i % 5 != 0 {
                v["offers"] = json!([{"supplier": "LCSC", "sku": format!("C{}", 10_000 + i)}]);
            }
        }
        put(
            dir,
            &format!("library/components/p{i:05}.json"),
            "ryueda.libcomp",
            v,
        );
    }
    if design {
        put(dir, "rails/gnd.json", "ryueda.rail", json!({"name": "GND"}));
        put(
            dir,
            "blocks/top/block.json",
            "ryueda.block",
            json!({"name": "Top"}),
        );
        for k in 0..12.min(n) {
            let mut c = json!({"of": {"id": format!("p{k:05}")}, "refdes": format!("X{}", k + 1),
                               "pins": {"1": {"rail": "gnd"}, "2": {"rail": "gnd"}}});
            if k == 3 {
                c["parameters"] = json!({"dnp": "yes"});
            }
            put(
                dir,
                &format!("blocks/top/components/x{k}.json"),
                "ryueda.component",
                c,
            );
        }
        put(
            dir,
            "designs/d.json",
            "ryueda.design",
            json!({"name": "Parts", "root": "top"}),
        );
    }
}
