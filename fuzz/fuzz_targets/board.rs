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
    v["format"] = json!(ryueda::conn::format_of(kind, &v));
    v["kind"] = json!(kind);
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
            // N1 is a power net (ryueda 7.B): its class's rules apply.
            let mut net = json!({"name": format!("N{i}")});
            if i == 1 {
                net["class"] = json!("power");
            }
            put(&mut f, &format!("blocks/top/nets/n{i}.json"), "ryueda.net", net);
        }
    }
    put(&mut f, "designs/d.json", "ryueda.design", json!({"name": "D", "root": "top"}));
    put(&mut f, "classes/power.json", "ryueda.class", json!({"name": "Power"}));
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
    let mut board = json!({"name": "F", "design": "d", "preset": "jlc-2l", "outline": outline});
    // 2, 4 or 6 copper layers (ryueda 7.A), now and then a 32-layer one.
    let layers = [2usize, 4, 6, 32][b.next() % 4];
    if layers > 2 {
        let st = ryueda::board::stackup::even(&ryueda::board::preset::JLC_2L, layers.min(6)).unwrap();
        board["stackup"] = if layers == 32 {
            let cu = json!({"kind": "copper", "thickness_nm": 10_000});
            let mut l = vec![cu.clone()];
            for _ in 1..32 {
                l.push(json!({"kind": "dielectric", "thickness_nm": 40_000}));
                l.push(cu.clone());
            }
            json!({"layers": l})
        } else {
            serde_json::to_value(st).unwrap()
        };
    }
    put(&mut files, "boards/f/board.json", "ryueda.board", board);
    // Any name `CopperLayer::parse` takes, the board's or not.
    const LAYERS: [&str; 8] = ["top", "bottom", "in1", "in2", "in3", "in4", "in7", "in30"];
    for i in 0..5 {
        if b.next() % 4 != 0 {
            let rot = if b.next() % 2 == 0 { (b.next() as i64 % 4) * 90_000 } else { b.next() as i64 * 1_409 };
            let mut pl = json!({"at": [b.coord(), b.coord()], "rotation_mdeg": rot});
            if b.next() % 3 == 0 {
                pl["side"] = json!("bottom");
            }
            // Placement intent (ryueda 7.C): constraints, a group, field places.
            let mut cs = Vec::new();
            for _ in 0..b.next() % 3 {
                cs.push(match b.next() % 4 {
                    0 => json!({"near": format!("f:R{}/{}", b.next() % 6, 1 + b.next() % 2), "within_nm": b.length()}),
                    1 => json!({"near": format!("R{}", b.next() % 6), "within_nm": b.length()}),
                    2 => {
                        let e = ["left", "right", "top", "bottom", "any"][b.next() % 5];
                        json!({"edge": e, "within_nm": b.length()})
                    },
                    _ => {
                        let side = ["top", "bottom"][b.next() % 2];
                        json!({"side": side})
                    },
                });
            }
            if !cs.is_empty() {
                pl["constraints"] = json!(cs);
            }
            if b.next() % 4 == 0 {
                let g = ["g", "", "a/b", "power"][b.next() % 4];
                pl["group"] = json!(g);
            }
            if b.next() % 4 == 0 {
                pl["fields"] = json!({"reference": {"at": [b.coord(), b.coord()], "rotation_mdeg": b.next() as i64 * 1_000,
                    "hidden": b.next() % 2 == 0, "size_nm": [b.length(), b.length()]}});
            }
            put(&mut files, &format!("boards/f/placements/r{i}.json"), "ryueda.placement", pl);
        }
    }
    let nets = nets();
    for i in 0..b.next() % 8 {
        let n = b.next() % (b.next() % 6 + 2);
        let points: Vec<Value> = (0..n).map(|_| json!([b.coord(), b.coord()])).collect();
        put(&mut files, &format!("boards/f/tracks/t{i}.json"), "ryueda.track", json!({
            "net": nets[b.next() % nets.len()], "layer": LAYERS[b.next() % LAYERS.len()],
            "width_nm": b.length(), "points": points}));
    }
    for i in 0..b.next() % 4 {
        let mut via = json!({
            "net": nets[b.next() % nets.len()], "at": [b.coord(), b.coord()], "drill_nm": b.length(), "diameter_nm": b.length()});
        // A span: through, or from and to any layer, either way round.
        for end in ["from", "to"] {
            let k = b.next() % (LAYERS.len() + 2);
            if k < LAYERS.len() {
                via[end] = json!(LAYERS[k]);
            }
        }
        put(&mut files, &format!("boards/f/vias/v{i}.json"), "ryueda.via", via);
    }
    for i in 0..b.next() % 3 {
        put(&mut files, &format!("boards/f/holes/h{i}.json"), "ryueda.hole", json!({"at": [b.coord(), b.coord()], "drill_nm": b.length()}));
    }
    // The board's rules (ryueda 7.B): none, or defaults drawn around the
    // preset's floor (under it too), a class that exists and one that
    // doesn't, and up to two waivers with drawn IDs and limits.
    if b.next() % 3 != 0 {
        let around = |b: &mut Bytes| 100_000 + (b.next() as i64 - 128) * 1_000;
        let mut rules = json!({"defaults": {}});
        for key in ["clearance_nm", "track_width_nm", "edge_clearance_nm", "hole_clearance_nm"] {
            if b.next() % 2 == 0 {
                rules["defaults"][key] = json!(around(&mut b));
            }
        }
        if b.next() % 2 == 0 {
            rules["classes"]["power"] = json!({"track_width_nm": around(&mut b), "clearance_nm": around(&mut b)});
        }
        if b.next() % 4 == 0 {
            rules["classes"]["ghost"] = json!({"clearance_nm": around(&mut b)});
        }
        const CODES: [&str; 4] = ["drc_clearance", "drc_track_width", "net_short", "net_open"];
        for _ in 0..b.next() % 3 {
            let id = format!("{:016x}", (b.next() as u64) << 8 | b.next() as u64);
            let mut w = json!({"code": CODES[b.next() % CODES.len()], "refs": ["f:R1/1"], "reason": "fuzz", "by": "fuzz"});
            if b.next() % 2 == 0 {
                w["limit"] = json!(around(&mut b));
            }
            rules["waivers"][id] = w;
        }
        put(&mut files, "boards/f/rules.json", "ryueda.board-rules", rules);
    }
    // Keepouts and drawings (ryueda 7.C).
    const KEEPS: [&str; 7] = ["copper", "tracks", "vias", "pads", "pours", "placements", "holes"];
    for i in 0..b.next() % 3 {
        let (x, y, w, h) = (b.coord(), b.coord(), b.length(), b.length());
        let contour = if b.next() % 5 == 0 {
            json!({"start": [x, y], "edges": [{"type": "line", "to": [x + w, y]}, {"type": "line", "to": [x + w, y + h]}]})
        } else {
            json!({"start": [x, y], "edges": [{"type": "line", "to": [x + w, y]}, {"type": "line", "to": [x + w, y + h]},
                {"type": "line", "to": [x, y + h]}, {"type": "line", "to": [x, y]}]})
        };
        let layers: Vec<&str> = (0..1 + b.next() % 2)
            .map(|_| if b.next() % 4 == 0 { "all" } else { LAYERS[b.next() % LAYERS.len()] })
            .collect();
        let keeps: Vec<&str> = (0..b.next() % 3).map(|_| KEEPS[b.next() % KEEPS.len()]).collect();
        put(&mut files, &format!("boards/f/keepouts/k{i}.json"), "ryueda.keepout",
            json!({"contour": contour, "layers": layers, "keeps": keeps}));
    }
    const DRAW_LAYERS: [&str; 8] = ["silk.top", "silk.bottom", "fab.top", "mask.bottom", "comments", "user.3", "copper.top", "edge"];
    for i in 0..b.next() % 3 {
        let layer = DRAW_LAYERS[b.next() % DRAW_LAYERS.len()];
        let d = match b.next() % 4 {
            0 => json!({"layer": layer, "type": "line", "start": [b.coord(), b.coord()], "end": [b.coord(), b.coord()], "width_nm": b.length()}),
            1 => json!({"layer": layer, "type": "circle", "center": [b.coord(), b.coord()], "radius_nm": b.length(), "width_nm": b.length()}),
            2 => json!({"layer": layer, "type": "rect", "start": [b.coord(), b.coord()], "end": [b.coord(), b.coord()], "width_nm": b.length(), "fill": b.next() % 2 == 0}),
            _ => {
                let text: String = (0..b.next() % 6).map(|_| char::from_u32(32 + (b.next() as u32 * 7) % 400).unwrap_or('?')).collect();
                json!({"layer": layer, "type": "text", "text": text, "at": [b.coord(), b.coord()], "rotation_mdeg": b.next() as i64 * 1_409,
                    "size_nm": [b.length(), b.length()], "thickness_nm": b.length()})
            }
        };
        put(&mut files, &format!("boards/f/drawings/d{i}.json"), "ryueda.board-drawing", d);
    }
    // Pours (ryueda 7.D): a net of the design or not, any layer, the
    // whole board, a rectangle or a contour that doesn't close, and drawn
    // settings.
    for i in 0..b.next() % 3 {
        let net = if b.next() % 5 == 0 { "nope".to_owned() } else { nets[b.next() % nets.len()].clone() };
        let mut pour = json!({"net": net, "layer": LAYERS[b.next() % LAYERS.len()]});
        let (x, y, w, h) = (b.coord(), b.coord(), b.length(), b.length());
        match b.next() % 4 {
            0 | 1 => pour["whole"] = json!(true),
            2 => {
                pour["contour"] = json!({"start": [x, y], "edges": [{"type": "line", "to": [x + w, y]}, {"type": "line", "to": [x + w, y + h]},
                    {"type": "line", "to": [x, y + h]}, {"type": "line", "to": [x, y]}]})
            }
            _ => pour["contour"] = json!({"start": [x, y], "edges": [{"type": "line", "to": [x + w, y]}, {"type": "line", "to": [x + w, y + h]}]}),
        }
        if b.next() % 2 == 0 {
            pour["priority"] = json!(b.next() as i64 % 4 - 1);
        }
        if b.next() % 2 == 0 {
            pour["clearance_nm"] = json!(b.length());
        }
        if b.next() % 2 == 0 {
            pour["min_width_nm"] = json!(b.length());
        }
        if b.next() % 3 == 0 {
            pour["connect"] = json!("solid");
        }
        if b.next() % 3 == 0 {
            pour["thermal"] = json!({"gap_nm": b.length(), "spoke_nm": b.length()});
        }
        if b.next() % 3 == 0 {
            pour["islands"] = json!("keep");
        }
        put(&mut files, &format!("boards/f/pours/p{i}.json"), "ryueda.pour", pour);
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
                            ryueda::error::details::check(&e);
                            assert_ne!(e.code, ryueda::error::Code::Internal, "{e:?}");
                            format!("{:?}", e.code)
                        }
                    };
                    let rows = board.board.item.as_ref().ok().and_then(|b| {
                        ryueda::board::occ::occurrences(&p, &b.design)
                            .ok()
                            .map(|(_, occs)| format!("{:?}", ryueda::board::assembly::rows(&occs, board)))
                    });
                    // The legaliser for the first part with a near or edge
                    // constraint: a place or a typed refusal.
                    let legal = board.board.item.as_ref().ok().and_then(|bf| {
                        let (_, occs) = ryueda::board::occ::occurrences(&p, &bf.design).ok()?;
                        let (o, pl) = occs.iter().find_map(|o| {
                            let pl = board.placements.get(&o.reference)?.item.as_ref().ok()?;
                            pl.constraints
                                .iter()
                                .any(|c| !matches!(c, ryueda::board::file::Constraint::Side { .. }))
                                .then_some((o, pl))
                        })?;
                        let ask = ryueda::board::place::Ask {
                            occ: o,
                            rotation_mdeg: pl.rotation_mdeg,
                            side: pl.side,
                            constraints: &pl.constraints,
                            now: Some(pl.at),
                        };
                        Some(match ryueda::board::place::legalise(board.id.as_str(), &c.shapes, &c.rules, &c.preset, &ask) {
                            Ok(v) => v.to_string(),
                            Err(e) => {
                                assert_ne!(e.code, ryueda::error::Code::Internal, "{e:?}");
                                format!("{:?}", e.code)
                            }
                        })
                    });
                    Ok((serde_json::to_string(&c.findings).unwrap(), fab, rows, legal))
                }
                Err(e) => Err(e.code),
            })
        };
        assert_eq!(run(one), run(eight), "one thread and eight");
    }
    // Edit sequences (ryueda 7.E): up to four edits drawn from the bytes
    // left, one `Live` carried through them, saying after each what a
    // check built from the files says.
    let Some(f) = p.boards.values().find(|x| x.id.as_str() == "f") else {
        return;
    };
    let Ok(mut live) = ryueda::board::live::Live::build(&p, f) else {
        return;
    };
    for step in 0..b.next() % 5 {
        edit(&mut files, &mut b, nets);
        let p = load(&files);
        let Some(f) = p.boards.values().find(|x| x.id.as_str() == "f") else {
            return;
        };
        let fresh = ryueda::board::drc::check(&p, f);
        match (live.update(&p, f), fresh) {
            (Ok(_), Ok(c)) => assert_eq!(
                ryueda::board::live::said(live.check()),
                ryueda::board::live::said(&c),
                "step {step}"
            ),
            (Err(a), Err(c)) => {
                assert_eq!(a.code, c.code, "step {step}");
                return;
            }
            (a, c) => panic!("step {step}: live {:?}, a check {:?}", a.map(|_| ()), c.map(|_| ())),
        }
    }
});

/// A file's JSON, if it is there and reads.
fn read(files: &BTreeMap<String, Vec<u8>>, path: &str) -> Option<Value> {
    serde_json::from_slice(files.get(path)?).ok()
}

/// One edit to the board's files: a track added, moved or removed; a
/// placement moved or turned; a via added or removed; a pour's
/// `min_width_nm` changed; or the design alone edited, a component on N1
/// added unplaced or removed.
fn edit(files: &mut BTreeMap<String, Vec<u8>>, b: &mut Bytes, nets: &[String]) {
    const LAYERS: [&str; 3] = ["top", "bottom", "in1"];
    match b.next() % 5 {
        0 => {
            let path = format!("boards/f/tracks/t{}.json", b.next() % 9);
            match (read(files, &path), b.next() % 3) {
                (Some(_), 0) => {
                    files.remove(&path);
                }
                (Some(mut t), _) => {
                    let (dx, dy) = (b.coord() / 4, b.coord() / 4);
                    if let Some(ps) = t["points"].as_array_mut() {
                        for q in ps {
                            let x = q[0].as_i64().unwrap_or(0).saturating_add(dx);
                            let y = q[1].as_i64().unwrap_or(0).saturating_add(dy);
                            *q = json!([x, y]);
                        }
                    }
                    put(files, &path, "ryueda.track", t);
                }
                (None, _) => {
                    let n = 2 + b.next() % 3;
                    let points: Vec<Value> = (0..n).map(|_| json!([b.coord(), b.coord()])).collect();
                    put(files, &path, "ryueda.track", json!({
                        "net": nets[b.next() % nets.len()], "layer": LAYERS[b.next() % LAYERS.len()],
                        "width_nm": b.length(), "points": points}));
                }
            }
        }
        1 => {
            let path = format!("boards/f/placements/r{}.json", b.next() % 5);
            if let Some(mut pl) = read(files, &path) {
                if b.next() % 2 == 0 {
                    pl["at"] = json!([b.coord(), b.coord()]);
                } else {
                    let r = pl["rotation_mdeg"].as_i64().unwrap_or(0);
                    pl["rotation_mdeg"] = json!((r + 90_000) % 360_000);
                }
                put(files, &path, "ryueda.placement", pl);
            }
        }
        2 => {
            let path = format!("boards/f/vias/v{}.json", b.next() % 5);
            if files.remove(&path).is_none() {
                put(files, &path, "ryueda.via", json!({
                    "net": nets[b.next() % nets.len()], "at": [b.coord(), b.coord()],
                    "drill_nm": b.length(), "diameter_nm": b.length()}));
            }
        }
        3 => {
            let path = format!("boards/f/pours/p{}.json", b.next() % 3);
            if let Some(mut pour) = read(files, &path) {
                pour["min_width_nm"] = json!(b.length());
                put(files, &path, "ryueda.pour", pour);
            }
        }
        _ => {
            let path = "blocks/top/components/x0.json";
            if files.remove(path).is_none() {
                put(files, path, "ryueda.component",
                    json!({"of": {"id": "r"}, "refdes": "R9", "pins": {"1": "n1", "2": "n1"}}));
            }
        }
    }
}
