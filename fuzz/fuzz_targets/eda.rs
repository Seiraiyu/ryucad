//! ryueda's connectivity from bytes: a project of blocks with ports,
//! nets, components and instances (cycles, dangling references and
//! unreadable files included), rails and a design, over a small library,
//! loaded, flattened and checked. Loading never fails on what files hold;
//! a design either flattens or is refused with a typed error; ERC's
//! findings and the netlist are the same on one thread and eight. Any
//! panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryueda::id::Id;
use ryueda::project::Project;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The bytes, read one at a time: zeros once they run out.
struct Bytes<'a>(&'a [u8], usize);

impl Bytes<'_> {
    fn next(&mut self) -> usize {
        let b = self.0.get(self.1).copied().unwrap_or(0);
        self.1 += 1;
        usize::from(b)
    }

    fn rest(&self) -> &[u8] {
        self.0.get(self.1..).unwrap_or_default()
    }
}

const TYPES: [&str; 12] = [
    "input", "output", "bidirectional", "tri_state", "passive", "free", "unspecified",
    "power_in", "power_out", "open_collector", "open_emitter", "no_connect",
];
const DIRECTIONS: [&str; 5] = ["in", "out", "bidirectional", "power", "passive"];
const VOLTAGES: [Option<&str>; 4] = [None, Some("3.3 V"), Some("3300 mV"), Some("5 V")];

fn put(files: &mut BTreeMap<String, Vec<u8>>, path: &str, kind: &str, mut v: Value) {
    v["kind"] = json!(kind);
    v["format"] = json!(1);
    files.insert(path.to_owned(), ryueda::canonical::to_string(&v).into_bytes());
}

/// What a pin or an instance's port connects to in a block of `nets`
/// nets and `ports` ports, by a byte: nothing, a net, a rail, a port,
/// `nc`, or something that isn't there.
fn node(b: &mut Bytes, nets: usize, ports: usize) -> Option<Value> {
    let x = b.next();
    match b.next() % 6 {
        0 => None,
        1 if nets > 0 => Some(json!(format!("n{}", x % nets))),
        2 => Some(json!({"rail": format!("r{}", x % 3)})),
        3 if ports > 0 => Some(json!({"port": format!("p{}", x % ports)})),
        4 => Some(json!("nc")),
        _ => Some(json!("gone")),
    }
}

fn project(b: &mut Bytes) -> BTreeMap<String, Vec<u8>> {
    let mut f = BTreeMap::new();
    put(
        &mut f,
        "ryueda.json",
        "ryueda.project",
        json!({"project": "0123456789abcdef0123456789abcdef", "default_unit": "mm"}),
    );
    let pins: serde_json::Map<String, Value> = (1..=8)
        .map(|i| {
            let t = TYPES[b.next() % TYPES.len()];
            (
                i.to_string(),
                json!({"name": format!("P{i}"), "type": t, "at": [0, i * 2_540_000], "side": "left", "length_nm": 2_540_000}),
            )
        })
        .collect();
    put(&mut f, "library/symbols/s.json", "ryueda.symbol", json!({"name": "S", "units": 1, "pins": pins}));
    let pads: serde_json::Map<String, Value> = (1..=8 - b.next() % 3)
        .map(|i| {
            (
                format!("p{i}"),
                json!({"number": i.to_string(), "kind": "smd", "shape": {"type": "rect"}, "at": [i * 1_000_000, 0],
                       "size": [500_000, 500_000], "layers": ["copper.top"]}),
            )
        })
        .collect();
    put(&mut f, "library/footprints/f.json", "ryueda.footprint", json!({"name": "F", "mount": "smd", "pads": pads}));
    let footprints = if b.next() % 4 == 0 { json!([]) } else { json!([{"footprint": {"id": "f"}}]) };
    put(&mut f, "library/components/c.json", "ryueda.libcomp", json!({"symbol": {"id": "s"}, "prefix": "U", "footprints": footprints}));
    for r in 0..2 {
        let mut v = json!({"name": format!("R{r}")});
        if let Some(volts) = VOLTAGES[b.next() % VOLTAGES.len()] {
            v["voltage"] = json!(volts);
        }
        put(&mut f, &format!("rails/r{r}.json"), "ryueda.rail", v);
    }
    let blocks = 1 + b.next() % 4;
    let shape: Vec<(usize, usize, usize, usize)> = (0..blocks)
        .map(|_| (b.next() % 4, b.next() % 5, b.next() % 5, b.next() % 3))
        .collect();
    for (k, &(ports, nets, comps, insts)) in shape.iter().enumerate() {
        let dir = format!("blocks/b{k}");
        let ports_v: serde_json::Map<String, Value> = (0..ports)
            .map(|p| {
                let mut v = json!({"name": format!("P{p}"), "direction": DIRECTIONS[b.next() % DIRECTIONS.len()]});
                if let Some(volts) = VOLTAGES[b.next() % VOLTAGES.len()] {
                    v["voltage"] = json!(volts);
                }
                (format!("p{p}"), v)
            })
            .collect();
        put(&mut f, &format!("{dir}/block.json"), "ryueda.block", json!({"name": format!("B{k}"), "ports": ports_v}));
        for n in 0..nets {
            let mut v = json!({"name": format!("N{}", b.next() % 6)});
            if b.next() % 4 == 0 {
                v["pair"] = json!({"with": format!("n{}", b.next() % nets), "polarity": if b.next() % 2 == 0 { "p" } else { "n" }});
            }
            if let Some(volts) = VOLTAGES[b.next() % VOLTAGES.len()] {
                v["voltage"] = json!(volts);
            }
            put(&mut f, &format!("{dir}/nets/n{n}.json"), "ryueda.net", v);
        }
        for c in 0..comps {
            let mut pins = serde_json::Map::new();
            for pin in 1..=8 + b.next() % 2 {
                if let Some(n) = node(b, nets, ports) {
                    pins.insert(pin.to_string(), n);
                }
            }
            let mut v = json!({"of": {"id": "c"}, "pins": pins});
            if b.next() % 5 != 0 {
                v["refdes"] = json!(format!("U{}", b.next() % 4));
            }
            put(&mut f, &format!("{dir}/components/u{c}.json"), "ryueda.component", v);
        }
        for i in 0..insts {
            let child = b.next() % blocks;
            let mut map = serde_json::Map::new();
            for p in 0..shape[child].0 {
                if let Some(n) = node(b, nets, ports) {
                    map.insert(format!("p{p}"), n);
                }
            }
            let mut v = json!({"of": format!("b{child}"), "ports": map});
            match b.next() % 3 {
                0 => v["refdes"] = json!({"rule": "suffix", "value": format!("_{i}")}),
                1 => v["refdes"] = json!({"rule": "offset", "value": 100 * (i + 1)}),
                _ => {}
            }
            put(&mut f, &format!("{dir}/instances/i{i}.json"), "ryueda.instance", v);
        }
    }
    put(&mut f, "designs/d.json", "ryueda.design", json!({"name": "D", "root": "b0"}));
    // Now and then a file holds what remains instead.
    if b.next() % 8 == 0 && !b.rest().is_empty() {
        let at = b.next() % f.len();
        let path = f.keys().nth(at).cloned().expect("in range");
        if path != "ryueda.json" {
            f.insert(path, b.rest().to_vec());
        }
    }
    f
}

fn pools() -> &'static (rayon::ThreadPool, rayon::ThreadPool) {
    static POOLS: OnceLock<(rayon::ThreadPool, rayon::ThreadPool)> = OnceLock::new();
    POOLS.get_or_init(|| {
        let pool = |n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
        (pool(1), pool(8))
    })
}

fuzz_target!(|data: &[u8]| {
    let files = project(&mut Bytes(data, 0));
    let p = Project::load_from(std::path::Path::new("/fuzz"), &files, &BTreeMap::new())
        .expect("a project whose manifest reads loads");
    let _ = p.faults();
    let d = Id::parse("d", "design").unwrap();
    let (one, eight) = pools();
    let run = |pool: &rayon::ThreadPool| {
        pool.install(|| {
            ryueda::netlist::flatten(&p, &d).map(|n| {
                let found = ryueda::erc::check_netlist(&p, &n);
                (n.to_json(), found)
            })
        })
    };
    match (run(one), run(eight)) {
        (Ok(a), Ok(b)) => assert_eq!(a, b, "one thread and eight differ"),
        (Err(a), Err(b)) => assert_eq!(a.code, b.code),
        (a, b) => panic!("one thread and eight disagree: {:?} vs {:?}", a.is_ok(), b.is_ok()),
    }
    let _ = ryueda::erc::erc(&p, None);
});
