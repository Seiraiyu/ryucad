//! Design 346's bar (plan 16.B): ryukei 11.D's enclosure built through
//! ryucad's features. A plate 410 mm square and 1 mm thick, castellated
//! into ten 30 mm tabs a side (notches 10 mm wide, 5 mm deep), is a base
//! flange; one flange feature turns its 40 tabs up 20 mm at a 1.5 mm
//! radius; an unfold lays it flat; two hole features drill 500 holes of
//! 1.5 mm across bend lines and 1,500 of 3 mm in the base; a refold folds
//! it back. On one thread, in release: the part rebuilt cold within 1.5
//! times the kernel's own flange, unfold, boolean and refold on the same
//! bodies, the folded enclosure's DXF within 0.5 s, and the refolded
//! drilled one's (unfolded again, #606) within 2 s. With
//! `RYUCAD_BARS=1` the bars are asserted as they are, else ten times
//! looser. Release only.

use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::{OpKey, ToolKey};
use ryukei::primitive::cylinder;
use ryukei::sheet_metal::{RefoldSpec, SheetOptions, UnfoldSpec, recognize, refold, unfold};
use serde_json::{Value, json};
use std::time::Instant;

const T: f64 = 1.0;
const TAB: f64 = 30.0;
const NOTCH: f64 = 10.0;
const DEPTH: f64 = 5.0;
const TABS: usize = 10;
const P: f64 = TABS as f64 * TAB + (TABS + 1) as f64 * NOTCH;

fn bar(secs: f64) -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        secs
    } else {
        10.0 * secs
    }
}

fn must(s: &mut Session, method: &str, mut params: Value) -> Value {
    params["part"] = json!("p");
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// A side's point at `a` along it and `d` in from it, in mm, for sides
/// bottom, right, top and left, counterclockwise.
fn on_side(side: usize, a: f64, d: f64) -> [f64; 2] {
    match side {
        0 => [a, d],
        1 => [P - d, a],
        2 => [P - a, P - d],
        _ => [d, P - a],
    }
}

/// The castellated outline, counterclockwise: each side's notched corner,
/// then each tab's four corners.
fn outline() -> Vec<[f64; 2]> {
    let mut pts = Vec::new();
    for side in 0..4 {
        pts.push(on_side(side, DEPTH, DEPTH));
        for i in 0..TABS {
            let a0 = NOTCH + i as f64 * (TAB + NOTCH);
            let a1 = a0 + TAB;
            for (a, d) in [(a0, DEPTH), (a0, 0.0), (a1, 0.0), (a1, DEPTH)] {
                pts.push(on_side(side, a, d));
            }
        }
    }
    pts
}

/// Sketch `id` on `plane` of fixed points (mm), joined by lines when
/// `closed`.
fn sketch(s: &mut Session, id: &str, plane: Value, pts: &[[f64; 2]], closed: bool) {
    must(
        s,
        "feature.add",
        json!({"type": "sketch", "id": id, "plane": plane}),
    );
    let mut ops = Vec::new();
    for (i, at) in pts.iter().enumerate() {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    if closed {
        for i in 0..pts.len() {
            ops.push(json!({"op": "add", "id": format!("l{}", i + 1), "type": "line",
                            "start": format!("p{}", i + 1), "end": format!("p{}", (i + 1) % pts.len() + 1)}));
        }
    }
    must(s, "sketch.edit", json!({"sketch": id, "ops": ops}));
}

/// The name of the edge of `body` (inspect's) between points `a` and `b`
/// (mm).
fn edge(body: &Value, a: [f64; 3], b: [f64; 3]) -> String {
    let at = |q: [f64; 3]| {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - q[k] * 1e-3).abs() < 1e-9))
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .unwrap_or_else(|| panic!("no vertex at {q:?}"))
    };
    let (a, b) = (at(a), at(b));
    let e = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .unwrap();
    format!("p/plate/{}", e["name"].as_str().unwrap())
}

fn one_thread<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(f)
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn the_bar() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), Some(ryucad::units::Unit::Mm)).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    handle(
        &mut s,
        Request::from_rpc("part.create", Some(json!({"part": "p"}))).unwrap(),
    )
    .unwrap();
    sketch(&mut s, "s", json!("xy"), &outline(), true);
    must(
        &mut s,
        "feature.add",
        json!({"type": "base_flange", "id": "plate", "sketch": "s",
        "thickness": "1 mm", "radius": "1.5 mm", "allowance": {"k_factor": "0.5"}}),
    );
    let body = must(&mut s, "inspect", json!({"feature": "plate"}))["bodies"][0].clone();
    let mut edges = Vec::new();
    for side in 0..4 {
        for i in 0..TABS {
            let a0 = NOTCH + i as f64 * (TAB + NOTCH);
            let [x0, y0] = on_side(side, a0, 0.0);
            let [x1, y1] = on_side(side, a0 + TAB, 0.0);
            edges.push(edge(&body, [x0, y0, T], [x1, y1, T]));
        }
    }
    must(
        &mut s,
        "feature.add",
        json!({"type": "flange", "id": "f", "target": "plate", "edges": edges,
        "angle": "90 deg", "length": "20 mm", "measured": "tangent", "position": "bend_outside"}),
    );
    // The folded enclosure's DXF, its flat unfolded whole in memory, timed
    // before anything is drilled; the refolded drilled one is exported at
    // the end (#606).
    one_thread(|| s.rebuilt(&Id::parse("p", "part").unwrap()).map(drop)).unwrap();
    let t = Instant::now();
    let req = Request::from_rpc("export", Some(json!({"part": "p", "format": "dxf"}))).unwrap();
    let dxf = one_thread(|| ryucad::server::answer_live(&mut s, None, None, req)).unwrap();
    let dxf_s = t.elapsed().as_secs_f64();
    let bytes = dxf.binary.unwrap().len();
    // The base: the flange most bends meet.
    let sheet = must(&mut s, "sheet.get", json!({}));
    let mut count = std::collections::BTreeMap::<String, usize>::new();
    for b in sheet["bends"].as_array().unwrap() {
        for f in b["flanges"].as_array().unwrap() {
            *count.entry(f.as_str().unwrap().to_owned()).or_default() += 1;
        }
    }
    let base = count.iter().max_by_key(|(_, n)| **n).unwrap().0.clone();
    let base = sheet["flanges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["flange"] == base)
        .unwrap();
    let fixed = base["sides"][0][0].as_str().unwrap().to_owned();
    must(
        &mut s,
        "feature.add",
        json!({"type": "unfold", "id": "u", "target": "plate", "fixed_face": fixed}),
    );
    // Holes: 12 or 13 across each bend line, the flat's bend lines from
    // flat.get, in the pattern's frame; 1,500 in the base on a grid.
    let flat = must(&mut s, "flat.get", json!({}));
    let fr = &flat["frame"];
    let world = |q: [f64; 2]| -> [f64; 3] {
        let c = |k: usize| {
            fr["origin_m"][k].as_f64().unwrap() * 1e3
                + fr["x"][k].as_f64().unwrap() * q[0]
                + fr["y"][k].as_f64().unwrap() * q[1]
        };
        [c(0), c(1), c(2)]
    };
    let mut small = Vec::new();
    for (i, b) in flat["bend_lines"].as_array().unwrap().iter().enumerate() {
        let f: [f64; 2] = serde_json::from_value(b["from_mm"].clone()).unwrap();
        let t: [f64; 2] = serde_json::from_value(b["to_mm"].clone()).unwrap();
        let (dx, dy) = (t[0] - f[0], t[1] - f[1]);
        let len = (dx * dx + dy * dy).sqrt();
        // 0.3 mm short of a bend line, so each hole (0.75 mm in radius)
        // crosses it: a hole centred on the bend-line edge itself is the
        // boolean's degenerate case (#5524's run: uncertain at p51).
        let half = 0.5 * b["developed_mm"].as_f64().unwrap() - 0.3;
        let (nx, ny) = (-dy / len * half, dx / len * half);
        let n = if i % 2 == 0 { 12 } else { 13 };
        for j in 0..n {
            let u = (j as f64 + 0.5) / n as f64;
            let w = world([f[0] + dx * u + nx, f[1] + dy * u + ny]);
            small.push([w[0], w[1]]);
        }
    }
    assert_eq!(small.len(), 500);
    let (lo, hi) = (DEPTH + 8.0, P - DEPTH - 8.0);
    let mut big = Vec::new();
    'grid: for i in 0..39 {
        for j in 0..39 {
            if big.len() == 1500 {
                break 'grid;
            }
            big.push([
                lo + (hi - lo) * i as f64 / 38.0,
                lo + (hi - lo) * j as f64 / 38.0,
            ]);
        }
    }
    // The flat lies on the base's place, z from 0 to 1 mm: holes from a
    // plane 1 mm above its top, through, so no tool's top lies flush on
    // the sheet (the kernel's own cylinders run from below it to above).
    must(
        &mut s,
        "feature.add",
        json!({"type": "plane", "id": "top", "by": "offset", "from": "xy", "distance": "2 mm"}),
    );
    sketch(&mut s, "hs", json!("top"), &small, false);
    sketch(&mut s, "hb", json!("top"), &big, false);
    for (id, sk, d) in [("small", "hs", "1.5 mm"), ("big", "hb", "3 mm")] {
        must(
            &mut s,
            "feature.add",
            json!({"type": "hole", "id": id, "sketch": sk, "diameter": d,
            "depth": "through_all", "targets": ["plate"]}),
        );
    }
    must(
        &mut s,
        "feature.add",
        json!({"type": "refold", "id": "r", "target": "plate", "unfold": "u"}),
    );
    // ryucad, cold: a session of its own with no disk cache, one thread.
    let pid = Id::parse("p", "part").unwrap();
    let mut cold = Session::open(dir.path()).unwrap();
    let t = Instant::now();
    let r = one_thread(|| cold.rebuilt(&pid).cloned()).unwrap();
    let ryucad_s = t.elapsed().as_secs_f64();
    for (id, out) in &r.features {
        assert!(out.is_ok(), "{id} failed: {:?}", out.as_ref().err());
    }
    // The refolded, drilled enclosure's flat pattern as DXF: recognized
    // and unfolded again in memory, its holes across the bend lines
    // radial (#606).
    let req = Request::from_rpc("export", Some(json!({"part": "p", "format": "dxf"}))).unwrap();
    let t = Instant::now();
    let drilled = one_thread(|| ryucad::server::answer_live(&mut cold, None, None, req))
        .unwrap_or_else(|e| panic!("the refolded enclosure's DXF: {e}"));
    let drilled_s = t.elapsed().as_secs_f64();
    let drilled_bytes = drilled.binary.expect("DXF bytes").len();
    // The kernel's own calls on the same bodies, one thread.
    let solid = |f: &str| {
        r.output(&Id::parse(f, "feature").unwrap())
            .unwrap()
            .solid(&Id::parse("plate", "feature").unwrap())
            .unwrap()
            .clone()
    };
    let (plate, flanged, unfolded, drilled) =
        (solid("plate"), solid("f"), solid("u"), solid("big"));
    let resolved = ryucad::sheet_metal::resolve_for_bar(&mut cold, &pid, "f");
    let map = match r.output(&Id::parse("u", "feature").unwrap()).unwrap() {
        ryucad::rebuild::Output::Solids(m) => m.flat.clone().expect("an unfold's flat"),
        _ => unreachable!(),
    };
    let tools = |pts: &[[f64; 2]], radius: f64, key: u128| -> (ryukei::topo::Body, Vec<Rigid>) {
        let c = cylinder(OpKey(key), radius * 1e-3, 0.003, &Rigid::IDENTITY)
            .unwrap()
            .0;
        (
            c,
            pts.iter()
                .map(|p| Rigid::translation(Vector3::new(p[0] * 1e-3, p[1] * 1e-3, -0.001)))
                .collect(),
        )
    };
    let ((sc, sp), (bc, bp)) = (tools(&small, 0.75, 30), tools(&big, 1.5, 31));
    let kernel_s = one_thread(|| {
        let t = Instant::now();
        let f = ryukei::sheet_metal::flange(
            OpKey(20),
            &plate.body,
            &resolved,
            &SheetOptions::default(),
        )
        .unwrap()
        .0;
        let m = recognize(&flanged.body, &SheetOptions::default()).unwrap();
        let fixed_name = ryucad::doc::reference::Reference::parse(&fixed, &pid)
            .unwrap()
            .name;
        let _ = unfold(
            OpKey(21),
            &flanged.body,
            &m,
            &UnfoldSpec {
                fixed: Some(fixed_name),
                ..UnfoldSpec::all(ryukei::sheet_metal::Allowance::KFactor(0.5))
            },
        )
        .unwrap();
        let mut keyed = Vec::new();
        for (i, (body, place)) in sp
            .iter()
            .map(|p| (&sc, p))
            .chain(bp.iter().map(|p| (&bc, p)))
            .enumerate()
        {
            keyed.push(Tool {
                key: ToolKey(i as u128 + 1),
                body,
                place: *place,
            });
        }
        let _ = boolean(OpKey(40), BooleanOp::Subtract, &unfolded.body, &keyed).unwrap();
        let _ = refold(OpKey(50), &drilled.body, &map.map, &RefoldSpec::default()).unwrap();
        drop(f);
        t.elapsed().as_secs_f64()
    });
    eprintln!(
        "the enclosure: ryucad cold {ryucad_s:.2} s, the kernel's own {kernel_s:.2} s ({:.2} times); DXF {dxf_s:.3} s, {bytes} bytes; refolded and drilled, DXF {drilled_s:.3} s, {drilled_bytes} bytes",
        ryucad_s / kernel_s
    );
    assert!(
        ryucad_s <= 1.5 * kernel_s * bar(1.0),
        "ryucad's cold rebuild took {ryucad_s:.2} s, the kernel's own {kernel_s:.2} s"
    );
    assert!(
        dxf_s <= bar(0.5),
        "the DXF took {dxf_s:.3} s, over {} s",
        bar(0.5)
    );
    assert!(
        drilled_s <= bar(2.0),
        "the refolded enclosure's DXF took {drilled_s:.3} s, over {} s",
        bar(2.0)
    );
}
