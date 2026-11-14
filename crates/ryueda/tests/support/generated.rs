//! A design of any size, written as files: `channels` instances of a
//! block holding a chain of `parts` resistors, each channel's output
//! joined to the next one's input, the first fed from rail VIN and the
//! last to GND. Each channel holds `2 × parts - 1` nets (the chain's,
//! and a spare per part), so 100 channels of 100 parts are 10,000
//! component occurrences and about 20,000 nets.

#![allow(dead_code)]

use serde_json::{Value, json};
use std::path::Path;

fn put(dir: &Path, rel: &str, kind: &str, mut v: Value) {
    v["kind"] = json!(kind);
    v["format"] = json!(1);
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, ryueda::canonical::to_string(&v)).unwrap();
}

/// Writes the project in `dir`.
pub fn write(dir: &Path, channels: usize, parts: usize) {
    put(
        dir,
        "ryueda.json",
        "ryueda.project",
        json!({"project": "0123456789abcdef0123456789abcdef", "default_unit": "mm"}),
    );
    put(
        dir,
        "library/symbols/r.json",
        "ryueda.symbol",
        json!({"name": "R", "units": 1, "pins": {
            "1": {"name": "~", "type": "passive", "at": [0, 3_810_000], "side": "down", "length_nm": 1_270_000},
            "2": {"name": "~", "type": "passive", "at": [0, -3_810_000], "side": "up", "length_nm": 1_270_000}
        }}),
    );
    let pad = |n: &str, x: i64| {
        json!({"number": n, "kind": "smd", "shape": {"type": "rect"}, "at": [x, 0], "size": [560_000, 620_000],
               "layers": ["copper.top", "mask.top", "paste.top"]})
    };
    put(
        dir,
        "library/footprints/r_0402.json",
        "ryueda.footprint",
        json!({"name": "R_0402", "mount": "smd",
               "pads": {"p1": pad("1", -480_000), "p2": pad("2", 480_000)},
               "graphics": {"g1": {"layer": "courtyard.top", "type": "rect", "start": [-1_000_000, 500_000],
                                   "end": [1_000_000, -500_000], "width_nm": 50_000}}}),
    );
    put(
        dir,
        "library/components/r.json",
        "ryueda.libcomp",
        json!({"symbol": {"id": "r"}, "prefix": "R", "value": "1k",
               "manufacturer": "Yageo", "mpn": "RC0402FR-071KL",
               "offers": [{"supplier": "LCSC", "sku": "C11702"}],
               "footprints": [{"footprint": {"id": "r_0402"}}]}),
    );
    put(
        dir,
        "rails/vin.json",
        "ryueda.rail",
        json!({"name": "VIN", "voltage": "5 V"}),
    );
    put(dir, "rails/gnd.json", "ryueda.rail", json!({"name": "GND"}));
    put(
        dir,
        "blocks/ch/block.json",
        "ryueda.block",
        json!({"name": "Channel", "ports": {
            "in": {"name": "IN", "direction": "passive"},
            "out": {"name": "OUT", "direction": "passive"}
        }}),
    );
    for i in 0..parts {
        let one = if i == 0 {
            json!({"port": "in"})
        } else {
            json!(format!("n{i}"))
        };
        let two = if i + 1 == parts {
            json!({"port": "out"})
        } else {
            json!(format!("n{}", i + 1))
        };
        put(
            dir,
            &format!("blocks/ch/components/r{i}.json"),
            "ryueda.component",
            json!({"of": {"id": "r"}, "refdes": format!("R{}", i + 1), "pins": {"1": one, "2": two}}),
        );
        if i > 0 {
            put(
                dir,
                &format!("blocks/ch/nets/n{i}.json"),
                "ryueda.net",
                json!({"name": format!("N{i}")}),
            );
        }
        put(
            dir,
            &format!("blocks/ch/nets/s{i}.json"),
            "ryueda.net",
            json!({"name": format!("SPARE{i}")}),
        );
    }
    put(
        dir,
        "blocks/top/block.json",
        "ryueda.block",
        json!({"name": "Top"}),
    );
    for k in 0..channels {
        let input = if k == 0 {
            json!({"rail": "vin"})
        } else {
            json!(format!("l{k}"))
        };
        let output = if k + 1 == channels {
            json!({"rail": "gnd"})
        } else {
            json!(format!("l{}", k + 1))
        };
        put(
            dir,
            &format!("blocks/top/instances/c{k}.json"),
            "ryueda.instance",
            json!({"of": "ch", "ports": {"in": input, "out": output},
                   "refdes": {"rule": "offset", "value": 1000 * k}}),
        );
        if k > 0 {
            put(
                dir,
                &format!("blocks/top/nets/l{k}.json"),
                "ryueda.net",
                json!({"name": format!("LINK{k}")}),
            );
        }
    }
    put(
        dir,
        "designs/d.json",
        "ryueda.design",
        json!({"name": "Generated", "root": "top"}),
    );
}

/// splitmix64: the seeded designs' only randomness.
fn next(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A design `d` of 1 to 40 resistors of 1 to 6 library components,
/// some with a value of their own, some marked `dnp`, some components
/// without a maker part number: seeded by `seed`.
pub fn seeded(dir: &Path, seed: u64) {
    let mut s = seed;
    write(dir, 1, 1); // manifest, symbol, footprint, rails, design `d`, block `ch`
    let comps = 1 + next(&mut s) % 6;
    for c in 0..comps {
        let mpn = if next(&mut s) % 4 == 0 {
            "".to_owned()
        } else {
            format!("MPN-{c}")
        };
        put(
            dir,
            &format!("library/components/c{c}.json"),
            "ryueda.libcomp",
            json!({"symbol": {"id": "r"}, "prefix": "R", "value": format!("{}k", c + 1),
                   "manufacturer": "Yageo", "mpn": mpn,
                   "footprints": [{"footprint": {"id": "r_0402"}}]}),
        );
    }
    let _ = std::fs::remove_dir_all(dir.join("blocks/ch/components"));
    let _ = std::fs::remove_dir_all(dir.join("blocks/ch/nets"));
    let parts = 1 + next(&mut s) % 40;
    for i in 0..parts {
        let mut v = json!({"of": {"id": format!("c{}", next(&mut s) % comps)}, "refdes": format!("R{}", i + 1)});
        if next(&mut s) % 5 == 0 {
            v["value"] = json!(format!("{}R", next(&mut s) % 3));
        }
        if next(&mut s) % 6 == 0 {
            v["parameters"] = json!({"dnp": "1"});
        }
        put(
            dir,
            &format!("blocks/ch/components/r{i}.json"),
            "ryueda.component",
            v,
        );
    }
}

/// A library project of `n` components (`c00000`…) sharing one symbol
/// and footprint, each with a maker part number: the bars' library.
pub fn library(dir: &Path, n: usize) {
    write(dir, 1, 1);
    put(
        dir,
        "ryueda.json",
        "ryueda.project",
        json!({"project": "fedcba9876543210fedcba9876543210", "default_unit": "mm", "project_kind": "library"}),
    );
    let _ = std::fs::remove_dir_all(dir.join("blocks"));
    let _ = std::fs::remove_dir_all(dir.join("designs"));
    let _ = std::fs::remove_dir_all(dir.join("rails"));
    let _ = std::fs::remove_file(dir.join("library/components/r.json"));
    for i in 0..n {
        put(
            dir,
            &format!("library/components/c{i:05}.json"),
            "ryueda.libcomp",
            json!({"symbol": {"id": "r"}, "prefix": "R", "value": format!("{i}R"),
                   "manufacturer": "Yageo", "mpn": format!("RC-{i:05}"),
                   "footprints": [{"footprint": {"id": "r_0402"}}]}),
        );
    }
}

/// Makes every component of `dir`'s design place `component` of the
/// library at `lib` (alias `alias`, by path, naming its project) instead
/// of its own `r`.
pub fn place_from(dir: &Path, alias: &str, lib: &Path, project: &str, component: &str) {
    let path = dir.join("ryueda.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["format"] = json!(3);
    m["libraries"] = json!({alias: {"path": lib.display().to_string(), "project": project}});
    std::fs::write(&path, ryueda::canonical::to_string(&m)).unwrap();
    for f in std::fs::read_dir(dir.join("blocks/ch/components")).unwrap() {
        let f = f.unwrap().path();
        let mut v: Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        v["of"] = json!({"library": alias, "id": component});
        std::fs::write(&f, ryueda::canonical::to_string(&v)).unwrap();
    }
}
