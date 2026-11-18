#![no_main]

use libfuzzer_sys::fuzz_target;
use ryueda::project::Project;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::OnceLock;

struct Bytes<'a>(&'a [u8], usize);

impl Bytes<'_> {
    fn next(&mut self) -> usize {
        let b = self.0.get(self.1).copied().unwrap_or(0);
        self.1 += 1;
        usize::from(b)
    }

    /// A coordinate: mostly within 40 mm on a 0.1 mm grid, sometimes far.
    fn coord(&mut self) -> i64 {
        match self.next() % 16 {
            0 => 1 << 41,
            1 => -(self.next() as i64) * 1_000_000_000,
            _ => (self.next() as i64 % 400) * 100_000,
        }
    }

    /// A length: mostly 0.1-2.5 mm, sometimes zero or negative.
    fn length(&mut self) -> i64 {
        match self.next() % 12 {
            0 => 0,
            1 => -100_000,
            _ => (1 + self.next() as i64 % 25) * 100_000,
        }
    }
}

fn put(files: &mut BTreeMap<String, Vec<u8>>, path: &str, kind: &str, mut v: Value) {
    v["kind"] = json!(kind);
    v["format"] = json!(1);
    files.insert(path.to_owned(), ryueda::canonical::to_string(&v).into_bytes());
}

/// The design: four 0402 resistors in a chain from rail VIN to GND.
fn design() -> BTreeMap<String, Vec<u8>> {
    let mut f = BTreeMap::new();
    put(&mut f, "ryueda.json", "ryueda.project", json!({"project": "0123456789abcdef0123456789abcdef", "default_unit": "mm"}));
    put(&mut f, "library/symbols/r.json", "ryueda.symbol", json!({"name": "R", "units": 1, "pins": {
        "1": {"name": "~", "type": "passive", "at": [0, 3_810_000], "side": "down", "length_nm": 1_270_000},
        "2": {"name": "~", "type": "passive", "at": [0, -3_810_000], "side": "up", "length_nm": 1_270_000}}}));
    let pad = |n: &str, x: i64| json!({"number": n, "kind": "smd", "shape": {"type": "rect"}, "at": [x, 0],
        "size": [560_000, 620_000], "layers": ["copper.top", "mask.top", "paste.top"]});
    put(&mut f, "library/footprints/r_0402.json", "ryueda.footprint", json!({"name": "R_0402", "mount": "smd",
        "pads": {"p1": pad("1", -480_000), "p2": pad("2", 480_000)},
        "graphics": {"g1": {"layer": "courtyard.top", "type": "rect", "start": [-1_000_000, 500_000], "end": [1_000_000, -500_000], "width_nm": 50_000}}}));
    put(&mut f, "library/components/r.json", "ryueda.libcomp", json!({"symbol": {"id": "r"}, "prefix": "R", "value": "1k",
        "footprints": [{"footprint": {"id": "r_0402"}}]}));
    put(&mut f, "rails/vin.json", "ryueda.rail", json!({"name": "VIN"}));
    put(&mut f, "rails/gnd.json", "ryueda.rail", json!({"name": "GND"}));
    put(&mut f, "blocks/top/block.json", "ryueda.block", json!({"name": "Top"}));
    for i in 0..4 {
        let one = if i == 0 { json!({"rail": "vin"}) } else { json!(format!("n{i}")) };
        let two = if i == 3 { json!({"rail": "gnd"}) } else { json!(format!("n{}", i + 1)) };
        put(&mut f, &format!("blocks/top/components/r{i}.json"), "ryueda.component",
            json!({"of": {"id": "r"}, "refdes": format!("R{}", i + 1), "pins": {"1": one, "2": two}}));
        if i > 0 {
            put(&mut f, &format!("blocks/top/nets/n{i}.json"), "ryueda.net", json!({"name": format!("N{i}")}));
        }
    }
    put(&mut f, "designs/d.json", "ryueda.design", json!({"name": "D", "root": "top"}));
    f
}

fn load(files: &BTreeMap<String, Vec<u8>>) -> Project {
    Project::load_from(std::path::Path::new("/fuzz"), files, &BTreeMap::new(), &ryueda::pin::NoPins)
        .expect("a project whose manifest reads loads")
}

fn pools() -> &'static (rayon::ThreadPool, rayon::ThreadPool) {
    static POOLS: OnceLock<(rayon::ThreadPool, rayon::ThreadPool)> = OnceLock::new();
    POOLS.get_or_init(|| {
        let pool = |n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
        (pool(1), pool(8))
    })
}

fn nets() -> &'static Vec<String> {
    static NETS: OnceLock<Vec<String>> = OnceLock::new();
    NETS.get_or_init(|| {
        let p = load(&design());
        let d = ryueda::id::Id::parse("d", "design").unwrap();
        let mut v: Vec<String> = ryueda::board::occ::occurrences(&p, &d).unwrap().1
            .iter().flat_map(|o| o.nets.values().cloned()).collect();
        v.sort();
        v.dedup();
        v.push("gone".into());
        v
    })
}

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes(data, 0);
    let mut files = design();
    let (w, h) = (b.length().abs().max(1) * 20, b.length().abs().max(1) * 20);
    let outline = if b.next() % 8 == 0 {
        json!({"start": [0, 0], "edges": [{"type": "line", "to": [w, h]}, {"type": "line", "to": [w, 0]},
            {"type": "line", "to": [0, h]}, {"type": "line", "to": [0, 0]}]})
    } else {
        json!({"start": [0, 0], "edges": [{"type": "line", "to": [w, 0]}, {"type": "line", "to": [w, h]},
            {"type": "line", "to": [0, h]}, {"type": "line", "to": [0, 0]}]})
    };
    put(&mut files, "boards/f/board.json", "ryueda.board", json!({"name": "F", "design": "d", "preset": "jlc-2l", "outline": outline}));
    for i in 0..5 {
        if b.next() % 4 != 0 {
            let rot = if b.next() % 2 == 0 { (b.next() as i64 % 4) * 90_000 } else { b.next() as i64 * 1_409 };
            put(&mut files, &format!("boards/f/placements/r{i}.json"), "ryueda.placement",
                json!({"at": [b.coord(), b.coord()], "rotation_mdeg": rot}));
        }
    }
    let nets = nets();
    for i in 0..b.next() % 8 {
        let n = b.next() % (b.next() % 6 + 2);
        let points: Vec<Value> = (0..n).map(|_| json!([b.coord(), b.coord()])).collect();
        put(&mut files, &format!("boards/f/tracks/t{i}.json"), "ryueda.track", json!({
            "net": nets[b.next() % nets.len()], "layer": if b.next() % 2 == 0 { "top" } else { "bottom" },
            "width_nm": b.length(), "points": points}));
    }
    for i in 0..b.next() % 4 {
        put(&mut files, &format!("boards/f/vias/v{i}.json"), "ryueda.via", json!({
            "net": nets[b.next() % nets.len()], "at": [b.coord(), b.coord()], "drill_nm": b.length(), "diameter_nm": b.length()}));
    }
    for i in 0..b.next() % 3 {
        put(&mut files, &format!("boards/f/holes/h{i}.json"), "ryueda.hole", json!({"at": [b.coord(), b.coord()], "drill_nm": b.length()}));
    }
    let p = load(&files);
    let _ = p.faults();
    let (one, eight) = pools();
    for board in p.boards.values() {
        let run = |pool: &rayon::ThreadPool| {
            pool.install(|| match ryueda::board::drc::check(&p, board) {
                Ok(c) => {
                    // The package's fabrication files: a result or a
                    // typed refusal, never ryueda's own bug.
                    let fab = match ryueda::board::export::fabrication(board.id.as_str(), &c.shapes, "fuzz") {
                        Ok(files) => files.into_iter().map(|f| f.name + &f.text).collect::<String>(),
                        Err(e) => {
                            assert_ne!(e.code, ryueda::error::Code::Internal, "{e:?}");
                            format!("{:?}", e.code)
                        }
                    };
                    let rows = board.board.item.as_ref().ok().and_then(|b| {
                        ryueda::board::occ::occurrences(&p, &b.design)
                            .ok()
                            .map(|(_, occs)| format!("{:?}", ryueda::board::assembly::rows(&occs, board)))
                    });
                    Ok((serde_json::to_string(&c.findings).unwrap(), fab, rows))
                }
                Err(e) => Err(e.code),
            })
        };
        assert_eq!(run(one), run(eight), "one thread and eight");
    }
});
