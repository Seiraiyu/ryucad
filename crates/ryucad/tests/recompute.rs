//! Incremental rebuilds equal full ones, bit for bit: random edits to a
//! part whose features refer to each other in every way, each rebuild
//! reusing what it can, compared with a rebuild from nothing on one thread
//! and on eight.

use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::rebuild::rebuild_part;
use ryucad::server::handle;
use ryucad::session::Session;
use ryukei_testkit::Lcg;
use serde_json::{Value, json};

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
}

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    call(s, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// An `inspect` body's entities of `kind` (`faces`, `edges`, `vertices`).
fn entities(s: &mut Session, f: &str, kind: &str) -> Vec<Value> {
    let v = must(s, "inspect", json!({"part": "p", "feature": f}));
    v["bodies"][0][kind].as_array().unwrap().clone()
}

/// The reference to the face of `f` whose outward normal is `n`, at
/// height `z` when given.
fn face(s: &mut Session, f: &str, n: [f64; 3]) -> String {
    let face = entities(s, f, "faces")
        .into_iter()
        .find(|x| {
            let z = &x["surface"]["frame"]["z"];
            let sign = if x["reversed"] == true { -1.0 } else { 1.0 };
            x["surface"]["type"] == "plane"
                && (0..3).all(|k| (z[k].as_f64().unwrap() * sign - n[k]).abs() < 1e-12)
        })
        .unwrap();
    format!("p/{f}/{}", face["name"].as_str().unwrap())
}

/// The reference to the vertex of `f` at `p`.
fn vertex(s: &mut Session, f: &str, p: [f64; 3]) -> String {
    let v = entities(s, f, "vertices")
        .into_iter()
        .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-12))
        .unwrap();
    format!("p/{f}/{}", v["name"].as_str().unwrap())
}

/// The reference to the edge of `f` between the vertices at `a` and `b`.
fn edge(s: &mut Session, f: &str, a: [f64; 3], b: [f64; 3]) -> String {
    let name = |r: String| r.rsplit('/').next().unwrap().to_owned();
    let (a, b) = (name(vertex(s, f, a)), name(vertex(s, f, b)));
    let e = entities(s, f, "edges")
        .into_iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .unwrap();
    format!("p/{f}/{}", e["name"].as_str().unwrap())
}

/// A rectangle `w` × `h` with its corner at `at`, held square.
fn rectangle(w: &str, h: &str, at: [f64; 2]) -> Value {
    let p = |x: f64, y: f64| json!([at[0] + x, at[1] + y]);
    json!([
        {"op": "add", "type": "point", "at": p(0.0, 0.0)},
        {"op": "add", "type": "point", "at": p(9.0, 1.0)},
        {"op": "add", "type": "point", "at": p(11.0, 7.0)},
        {"op": "add", "type": "point", "at": p(1.0, 5.0)},
        {"op": "add", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "type": "line", "start": "p2", "end": "p3"},
        {"op": "add", "type": "line", "start": "p3", "end": "p4"},
        {"op": "add", "type": "line", "start": "p4", "end": "p1"},
        {"op": "add", "type": "horizontal", "a": "l1"},
        {"op": "add", "type": "horizontal", "a": "l3"},
        {"op": "add", "type": "vertical", "a": "l2"},
        {"op": "add", "type": "vertical", "a": "l4"},
        {"op": "add", "id": "width", "type": "length", "a": "l1", "value": w},
        {"op": "add", "id": "height", "type": "length", "a": "l2", "value": h}
    ])
}

/// Part `p`: a box `b`; a sketch `s` on its top and an extrude `e` of it;
/// a sketch `t` on xy with a circle about a projected corner of the box,
/// extruded up to `e`'s top as `f`; a cylinder `c`; a sketch `u` on zx
/// revolved about the box's bottom edge as `r`; a hole cut through `b`;
/// a lug united with `c` by a boolean; a fillet of one of `e`'s vertical
/// edges, and a chamfer of `b`'s top front edge; a sketch `q` on `b`'s
/// bottom with a point, a hole `drill` there, and a pattern `row` of it.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"w": "20 mm", "h": "10 mm", "d": "5 mm", "r": "3 mm"}}),
    );
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": "w", "y": "h", "z": "d"}}),
    );
    let top = face(&mut s, "b", [0.0, 0.0, 1.0]);
    add(&mut s, json!({"type": "sketch", "id": "s", "face": top}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": rectangle("w / 2", "h / 2", [2.0, 2.0])}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "e", "sketch": "s", "extent": {"length": "d"}}),
    );
    add(&mut s, json!({"type": "sketch", "id": "t"}));
    let corner = vertex(&mut s, "b", [0.02, 0.01, 0.005]);
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": [
            {"op": "add", "id": "k", "type": "point", "projected": corner},
            {"op": "add", "type": "circle", "center": "k", "r": 2},
            {"op": "add", "id": "rad", "type": "radius", "a": "ci1", "value": "r"}
        ]}),
    );
    let lid = face(&mut s, "e", [0.0, 0.0, 1.0]);
    add(
        &mut s,
        json!({"type": "extrude", "id": "f", "sketch": "t",
               "extent": {"type": "up_to", "target": lid, "offset": "1 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "c", "params": {"radius": "r", "height": 4},
               "placement": {"origin": [40, 0, 0]}}),
    );
    add(&mut s, json!({"type": "sketch", "id": "u", "plane": "zx"}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "u", "ops": rectangle("8 mm", "6 mm", [-20.0, 0.0])}),
    );
    let along_x = edge(&mut s, "b", [0.0, 0.0, 0.0], [0.02, 0.0, 0.0]);
    add(
        &mut s,
        json!({"type": "revolve", "id": "r", "sketch": "u", "axis": along_x, "angle": 90}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "hole", "operation": "cut", "targets": ["b"],
               "params": {"radius": "r / 2", "height": 30}, "placement": {"origin": [5, 5, -10]}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "lug", "params": {"x": 6, "y": 2, "z": 3},
               "placement": {"origin": [40, -1, 1]}}),
    );
    add(
        &mut s,
        json!({"type": "boolean", "id": "both", "op": "unite", "target": "c", "tools": ["lug"]}),
    );
    // The first of e's vertical edges (the sketch's corners float).
    let up = entities(&mut s, "e", "edges")
        .into_iter()
        .find(|x| {
            x["curve"]["type"] == "line" && x["curve"]["dir"][2].as_f64().unwrap().abs() == 1.0
        })
        .map(|x| format!("p/e/{}", x["name"].as_str().unwrap()))
        .unwrap();
    add(
        &mut s,
        json!({"type": "fillet", "id": "round", "radius": "1 mm", "edges": [up]}),
    );
    let front = edge(&mut s, "b", [0.0, 0.0, 0.005], [0.02, 0.0, 0.005]);
    add(
        &mut s,
        json!({"type": "chamfer", "id": "bevel", "d": "0.5 mm", "edges": [front]}),
    );
    // Drilled from below, clear of everything else on b.
    let under = face(&mut s, "b", [0.0, 0.0, -1.0]);
    add(&mut s, json!({"type": "sketch", "id": "q", "face": under}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "q", "ops": [
            {"op": "add", "type": "point", "at": [12, -8]},
            {"op": "add", "type": "fix", "a": "p1"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "drill", "sketch": "q", "diameter": "1 mm",
               "depth": {"blind": "1.5 mm"}, "targets": ["b"]}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "row", "features": ["drill"], "direction": "x",
               "spacing": "2 mm", "count": 2}),
    );
    (dir, s)
}

/// A quantity in millimeters from `lo` to `hi`, in steps of 0.5.
fn mm(g: &mut Lcg, lo: u64, hi: u64) -> String {
    let half = 2 * lo + g.next_u64() % (2 * (hi - lo) + 1);
    format!("{} mm", half as f64 / 2.0)
}

/// One random edit. Some are refused, or leave a feature failing; both
/// must rebuild the same way however they are reached.
fn edit(g: &mut Lcg, s: &mut Session) {
    let (method, params) = match g.next_u64() % 15 {
        0 => ("param.set", json!({"parameters": {"w": mm(g, 15, 30)}})),
        1 => (
            "param.set",
            json!({"parameters": {"h": mm(g, 6, 14), "d": mm(g, 2, 8)}}),
        ),
        2 => ("param.set", json!({"parameters": {"r": mm(g, 1, 4)}})),
        3 => (
            "sketch.edit",
            json!({"sketch": "s", "ops": [{"op": "set", "id": "height", "value": mm(g, 2, 6)}]}),
        ),
        4 => (
            "feature.edit",
            json!({"id": "f", "extent": {"offset": mm(g, 0, 3)}}),
        ),
        5 => (
            "feature.edit",
            json!({"id": "r", "angle": format!("{} deg", 30 + g.next_u64() % 300)}),
        ),
        6 => (
            "feature.edit",
            json!({"id": "hole", "placement": {"origin": [mm(g, 2, 18), "5 mm", "-10 mm"]}}),
        ),
        7 => (
            "feature.edit",
            json!({"id": "lug", "params": {"x": mm(g, 2, 9)}}),
        ),
        8 => (
            "feature.edit",
            json!({"id": "round", "radius": mm(g, 1, 3)}),
        ),
        9 => (
            "feature.edit",
            json!({"id": "bevel", "d": mm(g, 1, 2), "propagate": g.next_u64().is_multiple_of(2)}),
        ),
        10 => (
            "feature.edit",
            json!({"id": "bevel", "form": "distances", "d1": mm(g, 1, 2), "d2": "0.5 mm",
                   "face": face(s, "b", [0.0, -1.0, 0.0])}),
        ),
        11 => (
            "feature.edit",
            json!({"id": "drill", "diameter": mm(g, 1, 2),
                   "form": if g.next_u64().is_multiple_of(2) { "simple" } else { "countersink" },
                   "countersink": {"diameter": "2.5 mm", "angle": "90 deg"}}),
        ),
        12 => (
            "feature.edit",
            json!({"id": "row", "spacing": mm(g, 2, 5), "count": 1 + g.next_u64() % 3}),
        ),
        13 => (
            "feature.edit",
            json!({"id": "row", "skip": if g.next_u64().is_multiple_of(2) { json!([]) } else { json!([1]) }}),
        ),
        _ => (
            "sketch.edit",
            json!({"sketch": "u", "ops": [{"op": "set", "id": "width", "value": mm(g, 4, 12)}]}),
        ),
    };
    let mut params = params;
    params["part"] = json!("p");
    let _ = call(s, method, params);
}

/// The rebuild from nothing on `threads` threads: its hash and each
/// feature's hash or error.
fn full(s: &Session, threads: usize) -> (String, Vec<String>) {
    let part = s
        .project()
        .part(&Id::parse("p", "part").unwrap())
        .unwrap()
        .clone();
    let r = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| rebuild_part(&part));
    (r.hash.clone(), summary(&r))
}

fn summary(r: &ryucad::rebuild::PartResult) -> Vec<String> {
    r.features
        .iter()
        .map(|(id, o)| match o {
            Ok(o) => format!("{id} {}", o.hash()),
            Err(e) => format!("{id} {:?}", e.code),
        })
        .collect()
}

#[test]
fn incremental_equals_full() {
    for seed in [1, 2, 3] {
        let (_dir, mut s) = project();
        let part = Id::parse("p", "part").unwrap();
        let first = s.rebuilt(&part).unwrap();
        assert_eq!(first.failed(), 0, "{:?}", summary(first));
        let mut g = Lcg::new(seed);
        let mut reused = 0;
        for step in 0..30 {
            edit(&mut g, &mut s);
            let (one, eight) = (full(&s, 1), full(&s, 8));
            let r = s.rebuilt(&part).unwrap();
            let incremental = (r.hash.clone(), summary(r));
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
            reused += r.features.len() - r.built.len();
        }
        // Edits touch a few features each; most results are reused.
        assert!(reused > 30 * 14 / 2, "seed {seed}: reused {reused}");
    }
}

#[test]
fn an_edit_at_a_long_solids_end_rebuilds_one_feature_and_at_its_founder_the_chain_once() {
    // A plate and 49 pockets cut along it: one solid of 50 features.
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 100, "y": 10, "z": 5}}),
    );
    for i in 1..50 {
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": format!("c{i}"), "operation": "cut",
                   "targets": ["b"], "params": {"x": 1, "y": 4, "z": 2},
                   "placement": {"origin": [2 * i - 1, 3, 4]}}),
        );
    }
    let part = Id::parse("p", "part").unwrap();
    assert_eq!(s.rebuilt(&part).unwrap().failed(), 0);
    let ids = |v: &[Id]| v.iter().map(Id::to_string).collect::<Vec<_>>();
    // The last pocket deeper: it alone is built again, its own body too.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "c49", "params": {"x": 1, "y": 4, "z": 3}}),
    );
    let r = s.rebuilt(&part).unwrap();
    assert_eq!(r.failed(), 0);
    assert_eq!(ids(&r.built), ["c49"]);
    assert_eq!(ids(&r.tools), ["c49"]);
    // The plate longer: every feature of the chain is built once, and no
    // pocket's own body is built again.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"x": 101, "y": 10, "z": 5}}),
    );
    let r = s.rebuilt(&part).unwrap();
    assert_eq!(r.failed(), 0);
    let mut all = vec!["b".to_owned()];
    all.extend((1..50).map(|i| format!("c{i}")));
    assert_eq!(ids(&r.built), all);
    assert!(r.tools.is_empty(), "{:?}", r.tools);
    // Incremental is full.
    let hash = r.hash.clone();
    assert_eq!(hash, full(&s, 1).0);
}
