//! Drawings of seeded parts (design 417, plan 9.D): a box of seeded
//! sizes, drilled through at a seeded place, on a seeded sheet, with the
//! standard views and a dimension of each of its three sizes and of its
//! hole, each with a seeded tolerance. Every build is the same twice
//! (`drawing.get` with every item, and the PDF's bytes, from two fresh
//! sessions); every dimension's value is what `measure` gives for the
//! same entities; every item lies on its sheet.
//!
//! 256 seeds in 9 s in a debug build (cox-worker-3: Intel Core Ultra 5
//! 250K Plus, 18 threads, load 5), so it runs with every test.

use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::path::Path;

/// How many seeds are judged.
const SEEDS: u64 = 256;

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
}

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    call(s, method, params).unwrap_or_else(|e| panic!("{method} {e}"))
}

/// A small fixed generator (SplitMix64): the same draws on every machine.
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A whole number from `lo` to `hi`.
    fn int(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }

    fn pick<'a, T>(&mut self, of: &'a [T]) -> &'a T {
        &of[(self.next() % of.len() as u64) as usize]
    }
}

/// What a seed drew: the box's sizes, the hole's radius and centre, all
/// in mm, the sheet, and each dimension's tolerance.
struct Case {
    size: [u64; 3],
    radius: u64,
    at: [u64; 2],
    sheet: &'static str,
    tolerances: [Value; 4],
}

fn case(seed: u64) -> Case {
    let mut d = Draw(seed);
    let size = [d.int(20, 90), d.int(20, 70), d.int(5, 40)];
    let radius = d.int(2, size[0].min(size[1]) / 5);
    let at = [
        d.int(radius + 2, size[0] - radius - 2),
        d.int(radius + 2, size[1] - radius - 2),
    ];
    let sheet = *d.pick(&["ansi_b", "ansi_c", "iso_a3", "iso_a2"]);
    let forms = [
        json!(null),
        json!({"symmetric": "0.1 mm"}),
        json!({"deviation": ["0.05 mm", "-0.02 mm"]}),
        json!({"limits": ["0.1 mm", "0 mm"]}),
        json!({"fit": "H7"}),
        json!({"basic": true}),
        json!({"reference": true}),
    ];
    let tolerances = [0, 1, 2, 3].map(|_| d.pick(&forms).clone());
    Case {
        size,
        radius,
        at,
        sheet,
        tolerances,
    }
}

/// The project folder of `c`: part `p`, a box `b` drilled by `h`, drawing
/// `d` with its standard views.
fn project(c: &Case, root: &Path) {
    ryucad::server::init(root, None).unwrap();
    let mut s = Session::open(root).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b",
               "params": {"x": c.size[0], "y": c.size[1], "z": c.size[2]}}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "cylinder", "id": "h", "operation": "cut", "targets": ["b"],
               "params": {"radius": c.radius, "height": c.size[2]},
               "placement": {"origin": [format!("{} mm", c.at[0]), format!("{} mm", c.at[1]), "0 mm"],
                             "z": [0, 0, 1], "x": [1, 0, 0]}}),
    );
    must(
        &mut s,
        "drawing.create",
        json!({"part": "p", "drawing": "d", "name": "Seeded", "size": c.sheet, "views": "standard"}),
    );
}

/// The faces of `b`: the pair of planes square to axis `k`, and the
/// hole's wall.
fn faces(s: &mut Session) -> ([[String; 2]; 3], String) {
    let b = must(s, "inspect", json!({"part": "p"}))["bodies"][0].clone();
    let fs = b["faces"].as_array().unwrap();
    let planes = |k: usize| -> [String; 2] {
        let mut out: Vec<String> = fs
            .iter()
            .filter(|f| {
                f["surface"]["type"] == "plane"
                    && f["surface"]["frame"]["z"][k].as_f64().unwrap().abs() == 1.0
            })
            .map(|f| format!("b/{}", f["name"].as_str().unwrap()))
            .collect();
        out.sort();
        [out[0].clone(), out[1].clone()]
    };
    let wall = fs
        .iter()
        .find(|f| f["surface"]["type"] == "cylinder")
        .map(|f| format!("b/{}", f["name"].as_str().unwrap()))
        .unwrap();
    ([planes(0), planes(1), planes(2)], wall)
}

/// Every item's box, from `drawing.get --items`' exact geometry: its
/// points, an arc's centre grown by its radius.
fn item_box(it: &Value) -> [[f64; 2]; 2] {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut take = |p: &Value, r: f64| {
        for k in 0..2 {
            let x = p[k].as_f64().unwrap();
            lo[k] = lo[k].min(x - r);
            hi[k] = hi[k].max(x + r);
        }
    };
    if let Some(p) = it.get("path") {
        take(&p["start"], 0.0);
        for sg in p["segments"].as_array().unwrap() {
            if let Some(q) = sg.get("line") {
                take(q, 0.0);
            } else if let Some(a) = sg.get("arc") {
                take(&a["centre"], a["radius"].as_f64().unwrap());
            } else if let Some(e) = sg.get("ellipse") {
                let r = [&e["a"], &e["b"]]
                    .iter()
                    .map(|v| ryu_math::hypot(v[0].as_f64().unwrap(), v[1].as_f64().unwrap()))
                    .fold(0.0, f64::max);
                take(&e["centre"], r);
            } else if let Some(b) = sg.get("bezier") {
                for q in b["points"].as_array().unwrap() {
                    take(q, 0.0);
                }
            }
        }
    } else {
        take(&it["at"], 0.0);
    }
    [lo, hi]
}

fn judge(seed: u64) -> Result<(), String> {
    let c = case(seed);
    let dir = tempfile::tempdir().unwrap();
    project(&c, dir.path());
    let mut s = Session::open(dir.path()).unwrap();
    let ([xs, ys, zs], wall) = faces(&mut s);
    // x in the top view, z in the front, y in the right, the hole's
    // diameter in the top: each true length there.
    let dims = [
        (
            "v2",
            "linear",
            json!({"a": xs[0], "b": xs[1], "along": "horizontal"}),
        ),
        (
            "v1",
            "linear",
            json!({"a": zs[0], "b": zs[1], "along": "vertical"}),
        ),
        (
            "v3",
            "linear",
            json!({"a": ys[0], "b": ys[1], "along": "horizontal"}),
        ),
        ("v2", "diameter", json!({"of": wall})),
    ];
    let mut ops = Vec::new();
    for (i, (view, ty, ends)) in dims.iter().enumerate() {
        let mut v = ends.clone();
        v["type"] = json!(ty);
        v["view"] = json!(view);
        if !c.tolerances[i].is_null() {
            v["tolerance"] = c.tolerances[i].clone();
        }
        ops.push(json!({"op": "add", "kind": "annotation", "value": v}));
    }
    call(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": ops}),
    )
    .map_err(|e| format!("seed {seed}: the dimensions were refused: {e}"))?;
    // Each value against measure.
    let got = must(
        &mut s,
        "drawing.get",
        json!({"part": "p", "drawing": "d", "items": true}),
    );
    let sheet = &got["sheets"][0];
    let anns = sheet["annotations"].as_array().unwrap();
    for (i, (_, ty, ends)) in dims.iter().enumerate() {
        let a = &anns[i];
        let value = a["value"]["value"]
            .as_f64()
            .ok_or_else(|| format!("seed {seed}: d{} has no value: {a}", i + 1))?;
        let want = if *ty == "diameter" {
            let m = must(
                &mut s,
                "measure",
                json!({"entities": [format!("p/{}", ends["of"].as_str().unwrap())]}),
            );
            2.0 * m["shape"]["radius"]["m"].as_f64().unwrap()
        } else {
            let refs: Vec<String> = ["a", "b"]
                .iter()
                .map(|k| format!("p/{}", ends[*k].as_str().unwrap()))
                .collect();
            must(&mut s, "measure", json!({"entities": refs}))["distance"]["m"]
                .as_f64()
                .unwrap()
        };
        if (value - want).abs() > 1e-12 {
            return Err(format!(
                "seed {seed}: d{} reads {value}, measure {want}",
                i + 1
            ));
        }
    }
    // Every item on its sheet.
    let paper = [
        sheet["paper_mm"][0].as_f64().unwrap(),
        sheet["paper_mm"][1].as_f64().unwrap(),
    ];
    for (i, it) in sheet["items"].as_array().unwrap().iter().enumerate() {
        let [lo, hi] = item_box(it);
        if lo[0] < -1e-9 || lo[1] < -1e-9 || hi[0] > paper[0] + 1e-9 || hi[1] > paper[1] + 1e-9 {
            return Err(format!(
                "seed {seed}: item {i} leaves the {} sheet: {lo:?} {hi:?}",
                c.sheet
            ));
        }
    }
    // The same twice, from a fresh session each time: the built drawing
    // and the PDF.
    let pdf = |root: &Path| -> (Value, Vec<u8>) {
        let mut s = Session::open(root).unwrap();
        let g = must(
            &mut s,
            "drawing.get",
            json!({"part": "p", "drawing": "d", "items": true}),
        );
        let out = tempfile::tempdir().unwrap();
        let f = out.path().join("d.pdf");
        must(
            &mut s,
            "drawing.export",
            json!({"part": "p", "drawing": "d", "out": f.to_str().unwrap()}),
        );
        (g, std::fs::read(&f).unwrap())
    };
    let (g1, p1) = pdf(dir.path());
    let (g2, p2) = pdf(dir.path());
    if g1 != g2 || p1 != p2 {
        return Err(format!("seed {seed}: two builds differ"));
    }
    if g1["sheets"] != got["sheets"] {
        return Err(format!(
            "seed {seed}: a fresh session builds it differently"
        ));
    }
    Ok(())
}

#[test]
fn seeded_drawings_measure_the_model_lie_on_their_sheets_and_build_alike() {
    use rayon::prelude::*;
    let faults: Vec<String> = (0..SEEDS)
        .into_par_iter()
        .filter_map(|seed| judge(seed).err())
        .collect();
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}
