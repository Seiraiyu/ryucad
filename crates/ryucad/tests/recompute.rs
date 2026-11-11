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
/// bottom with a point, a hole `drill` there, and a pattern `row` of it;
/// a datum plane `d1` above `b`'s top, a sketch `v` on it extruded as
/// `g`, an axis `a1` where `d1` meets zx and a point `q1` on it; a helix
/// `hx` about `a1` and a path `pw` along it; a half disc `wp` on xy swept
/// up a line of `wl` on zx as `sw`; a circle `la` on xy lofted to a
/// smaller one `lb` on a datum plane `dl` 10 mm above it as `lf` (circles:
/// a loft of squares is four spline faces whose self-intersection check
/// takes a debug build tens of seconds on every rebuild from nothing); a
/// box `tb` split by a surface wall `tw` across it as `tsp`; and a
/// reference import of OCCT's box, which edits make a solid and back.
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
    add(
        &mut s,
        json!({"type": "plane", "id": "d1", "by": "offset", "from": top, "distance": "d"}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "v", "plane": "p/d1"}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "v", "ops": rectangle("4 mm", "3 mm", [60.0, 0.0])}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "g", "sketch": "v", "extent": {"length": "1 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "axis", "id": "a1", "by": "planes", "between": ["p/d1", "zx"]}),
    );
    add(
        &mut s,
        json!({"type": "point", "id": "q1", "by": "projection", "point": "origin", "onto": "p/a1"}),
    );
    add(
        &mut s,
        json!({"type": "helix", "id": "hx", "axis": "p/a1", "radius": "d", "pitch": "1 mm",
               "turns": 2}),
    );
    add(
        &mut s,
        json!({"type": "path", "id": "pw", "segments": ["p/hx"]}),
    );
    add(&mut s, json!({"type": "sketch", "id": "wp", "plane": "xy"}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "wp", "ops": [
            {"op": "add", "id": "c", "type": "point", "at": [100, 0]},
            {"op": "add", "id": "p1", "type": "point", "at": [102, 0]},
            {"op": "add", "id": "p2", "type": "point", "at": [100, 2]},
            {"op": "add", "id": "p3", "type": "point", "at": [98, 0]},
            {"op": "add", "id": "a1", "type": "arc", "center": "c", "start": "p1", "end": "p2"},
            {"op": "add", "id": "a2", "type": "arc", "center": "c", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l1", "type": "line", "start": "p3", "end": "p1"}
        ]}),
    );
    add(&mut s, json!({"type": "sketch", "id": "wl", "plane": "zx"}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "wl", "ops": [
            {"op": "add", "id": "a", "type": "point", "at": [0, 100]},
            {"op": "add", "id": "b", "type": "point", "at": [20, 100]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "sweep", "id": "sw", "sketch": "wp", "path": "p/wl/l1"}),
    );
    add(
        &mut s,
        json!({"type": "plane", "id": "dl", "by": "offset", "from": "xy", "distance": "10 mm"}),
    );
    for (id, plane, r) in [("la", "xy", 3.0), ("lb", "p/dl", 2.0)] {
        add(&mut s, json!({"type": "sketch", "id": id, "plane": plane}));
        must(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": id, "ops": [
                {"op": "add", "id": "c", "type": "point", "at": [150.0, 0.0]},
                {"op": "add", "id": "ci", "type": "circle", "center": "c", "r": r}
            ]}),
        );
    }
    add(
        &mut s,
        json!({"type": "loft", "id": "lf", "sections": ["la", "lb"]}),
    );
    // A surface wall from an open chain, and a planar face of a circle.
    add(
        &mut s,
        json!({"type": "sketch", "id": "swl", "plane": "xy"}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "swl", "ops": [
            {"op": "add", "id": "a", "type": "point", "at": [60, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [70, 0]},
            {"op": "add", "id": "c", "type": "point", "at": [70, 8]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"},
            {"op": "add", "id": "l2", "type": "line", "start": "b", "end": "c"}]}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "wall", "sketch": "swl", "surface": true,
               "extent": {"type": "blind", "length": "6 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "planar", "id": "pad", "sketch": "la"}),
    );
    // A fill of `v`'s rectangle, and the planar face thickened as a solid
    // of its own (consuming it).
    let sides: Vec<Value> = ["l1", "l2", "l3", "l4"]
        .iter()
        .map(|l| json!({"curve": format!("p/v/{l}")}))
        .collect();
    add(&mut s, json!({"type": "fill", "id": "lid", "sides": sides}));
    add(
        &mut s,
        json!({"type": "thicken", "id": "slab", "sheet": "pad", "front": "1 mm"}),
    );
    // A box split by a wall square to its side (a trim's tool), which
    // edits move along x and flip.
    add(
        &mut s,
        json!({"type": "box", "id": "tb", "params": {"x": 10, "y": 10, "z": 10},
               "placement": {"origin": [90, 0, 0]}}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "sts", "plane": "xy"}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "sts", "ops": [
            {"op": "add", "id": "a", "type": "point", "at": [95, -5]},
            {"op": "add", "id": "b", "type": "point", "at": [95, 15]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}]}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "tw", "sketch": "sts", "surface": true,
               "extent": {"type": "symmetric", "length": "30 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "split", "id": "tsp", "target": "tb", "tool": "tw"}),
    );
    // A reference import of OCCT's box, which edits convert and back.
    let file = format!(
        "{}/../ryustep/tests/data/occt/box_ap214.stp",
        env!("CARGO_MANIFEST_DIR")
    );
    must(&mut s, "import", json!({"path": file, "into": "p"}));
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
    let (method, params) = match g.next_u64() % 28 {
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
        // The wall's height, or its chain's far end moved.
        14 if g.next_u64().is_multiple_of(2) => (
            "feature.edit",
            json!({"id": "wall", "extent": {"length": mm(g, 3, 10)}}),
        ),
        14 => (
            "sketch.edit",
            json!({"sketch": "swl", "ops": [{"op": "move", "id": "c",
                   "at": [70, 4 + g.next_u64() % 8]}]}),
        ),
        15 => ("feature.edit", json!({"id": "d1", "distance": mm(g, 1, 9)})),
        // Built another way, and back.
        16 if g.next_u64().is_multiple_of(2) => (
            "feature.edit",
            json!({"id": "d1", "by": "parallel", "from": "xy",
                   "through": vertex(s, "b", [0.0, 0.0, 0.0])}),
        ),
        16 => (
            "feature.edit",
            json!({"id": "d1", "by": "offset", "from": face(s, "b", [0.0, 0.0, 1.0]),
                   "distance": "d"}),
        ),
        17 => (
            "feature.edit",
            json!({"id": "hx", "turns": (1 + g.next_u64() % 4).to_string()}),
        ),
        18 => (
            "feature.edit",
            json!({"id": "pw", "reverse": g.next_u64().is_multiple_of(2)}),
        ),
        // The sweep's path lengthened or shortened, and its law switched.
        // A twist or a scale carries the profile in the path's frame,
        // about 25 s a rebuild in a debug build on cox-worker-1, which a
        // test that rebuilds from nothing after every edit can't afford:
        // tests/sweeps_along.rs and the golden section cover them.
        19 => (
            "sketch.edit",
            json!({"sketch": "wl", "ops": [{"op": "move", "id": "b",
                   "at": [10 + 2 * (g.next_u64() % 11), 100]}]}),
        ),
        20 => (
            "feature.edit",
            if g.next_u64().is_multiple_of(2) {
                json!({"id": "sw", "law": "binormal", "direction": "x"})
            } else {
                json!({"id": "sw", "law": "minimal"})
            },
        ),
        // The loft's top section moved across, and its start tangent set
        // or dropped.
        21 => (
            "sketch.edit",
            json!({"sketch": "lb", "ops": [{"op": "move", "id": "c",
                   "at": [149 + g.next_u64() % 3, g.next_u64() % 3]}]}),
        ),
        22 => (
            "feature.edit",
            if g.next_u64().is_multiple_of(2) {
                json!({"id": "lf", "start_tangent": "normal", "start_weight": "0.5"})
            } else {
                json!({"id": "lf", "start_tangent": ""})
            },
        ),
        24 => ("feature.edit", json!({"id": "slab", "front": mm(g, 1, 4)})),
        25 => (
            "feature.edit",
            json!({"id": "lid",
                   "tolerance": format!("{} mm", [0.001, 0.002, 0.005][(g.next_u64() % 3) as usize])}),
        ),
        // The split's wall moved along x, under the box's size, or the
        // split flipped.
        26 => {
            let x = 93 + g.next_u64() % 5;
            (
                "sketch.edit",
                json!({"sketch": "sts", "ops": [
                    {"op": "move", "id": "a", "at": [x, -5]},
                    {"op": "move", "id": "b", "at": [x, 15]}]}),
            )
        }
        27 => (
            "feature.edit",
            json!({"id": "tsp", "flip": g.next_u64().is_multiple_of(2)}),
        ),
        // The import made a healed solid, or a reference again.
        23 => (
            "import.convert",
            json!({"reference": g.next_u64().is_multiple_of(2)}),
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

/// Part `p`'s import placed `mm` millimetres along x (none for 0),
/// written by hand, since no request sets a place, and the session
/// refreshed.
fn place_import(s: &mut Session, mm: u64) {
    let root = s.root().to_path_buf();
    let dir = root.join("parts/p/features");
    for e in std::fs::read_dir(&dir).unwrap() {
        let path = e.unwrap().path();
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        if v["type"] != "import" {
            continue;
        }
        if mm == 0 {
            v.as_object_mut().unwrap().remove("place");
        } else {
            v["place"] = json!({
                "pose": {"origin_m": [mm as f64 / 1000.0, 0, 0], "x": [1, 0, 0], "z": [0, 0, 1]},
                "via": [{"product": {"id": "part 2", "name": "part 2"}, "usage": "1"}],
            });
        }
        v["format"] = json!(ryucad::doc::files::feature_format(&v));
        ryucad::doc::project::write_json(&path, &v).unwrap();
    }
    s.refresh().unwrap();
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

// One test per seed, so nextest runs them side by side: all three in one
// test took about 160 s on a CI runner and passed its 300 s limit on a
// loaded one (#462). Each still makes 30 random edits and compares the
// incremental rebuild with full ones at 1 and 8 threads.
#[test]
fn incremental_equals_full() {
    incremental_equals_full_from(1);
}

#[test]
fn incremental_equals_full_seed_2() {
    incremental_equals_full_from(2);
}

#[test]
fn incremental_equals_full_seed_3() {
    incremental_equals_full_from(3);
}

fn incremental_equals_full_from(seed: u64) {
    let (_dir, mut s) = project();
    let part = Id::parse("p", "part").unwrap();
    let first = s.rebuilt(&part).unwrap();
    assert_eq!(first.failed(), 0, "{:?}", summary(first));
    let mut g = Lcg::new(seed);
    let mut reused = 0;
    for step in 0..30 {
        edit(&mut g, &mut s);
        // Every tenth step the import is also placed a millimetre or two
        // along x, as a flattened import places a body, or at its origin
        // again, by the seed and step rather than the generator: drawn
        // from it, a 27th arm reshuffled seed 1 into a countersink edit
        // after a two-distance bevel that took over 300 s to rebuild.
        if step % 10 == 9 {
            place_import(&mut s, (seed + step / 10) % 3);
        }
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

#[test]
fn a_placed_import_recomputes_as_from_nothing() {
    let (_dir, mut s) = project();
    let part = Id::parse("p", "part").unwrap();
    let start = s.rebuilt(&part).unwrap().hash.clone();
    for mm in [1, 2, 0] {
        place_import(&mut s, mm);
        let (one, eight) = (full(&s, 1), full(&s, 8));
        let r = s.rebuilt(&part).unwrap();
        let incremental = (r.hash.clone(), summary(r));
        assert_eq!(incremental, one, "{mm} mm");
        assert_eq!(incremental, eight, "{mm} mm");
        assert_eq!(r.hash == start, mm == 0, "{mm} mm");
    }
}

/// Part `p`: a box `b` (`w` × 12 × 8 mm) shelled open at its top as `sh`
/// (walls `t`, its floor `2 * t`), and a cylinder `c` grown as `o`.
fn shelled() -> (tempfile::TempDir, Session, String, String) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"w": "20 mm", "t": "1.5 mm"}}),
    );
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": "w", "y": "12 mm", "z": "8 mm"}}),
    );
    let top = face(&mut s, "b", [0.0, 0.0, 1.0]);
    let bottom = face(&mut s, "b", [0.0, 0.0, -1.0]);
    add(
        &mut s,
        json!({"type": "shell", "id": "sh", "thickness": "t", "open": [top],
               "faces": [{"thickness": "2 * t", "faces": [bottom]}]}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "c", "params": {"radius": "4 mm", "height": "6 mm"},
               "placement": {"origin": [40, 0, 0]}}),
    );
    add(
        &mut s,
        json!({"type": "offset", "id": "o", "target": "c", "distance": "0.5 mm"}),
    );
    (dir, s, top, bottom)
}

/// One random edit of [`shelled`]'s part.
fn shell_edit(g: &mut Lcg, s: &mut Session, top: &str, bottom: &str) {
    let (method, params) = match g.next_u64() % 6 {
        0 => ("param.set", json!({"parameters": {"t": mm(g, 1, 3)}})),
        1 => ("param.set", json!({"parameters": {"w": mm(g, 15, 30)}})),
        2 => ("feature.edit", json!({"id": "o", "distance": mm(g, 1, 3)})),
        3 => (
            "feature.edit",
            json!({"id": "sh",
                   "direction": if g.next_u64().is_multiple_of(2) { "outward" } else { "inward" }}),
        ),
        4 => ("feature.edit", json!({"id": "sh", "open": [], "faces": []})),
        _ => (
            "feature.edit",
            json!({"id": "sh", "open": [top],
                   "faces": [{"thickness": "2 * t", "faces": [bottom]}]}),
        ),
    };
    let mut params = params;
    params["part"] = json!("p");
    let _ = call(s, method, params);
}

#[test]
fn shells_and_offsets_recompute_as_from_nothing() {
    for seed in [4, 5] {
        let (_dir, mut s, top, bottom) = shelled();
        let part = Id::parse("p", "part").unwrap();
        let first = s.rebuilt(&part).unwrap();
        assert_eq!(first.failed(), 0, "{:?}", summary(first));
        let mut g = Lcg::new(seed);
        for step in 0..10 {
            shell_edit(&mut g, &mut s, &top, &bottom);
            let (one, eight) = (full(&s, 1), full(&s, 8));
            let r = s.rebuilt(&part).unwrap();
            let incremental = (r.hash.clone(), summary(r));
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
        }
    }
}

/// Part `p`: a box `b` (`w` × 12 × 8 mm) whose top `m` raises by `d`, a
/// datum plane `lvl` up from xy, and a box `c` beside it whose top `r`
/// makes flush with `lvl`. Returns the top's and `c`'s top's references.
fn tweaked() -> (tempfile::TempDir, Session, String, String) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"w": "20 mm", "d": "2 mm", "lvl": "5 mm"}}),
    );
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": "w", "y": "12 mm", "z": "8 mm"}}),
    );
    let top = face(&mut s, "b", [0.0, 0.0, 1.0]);
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "z",
                       "distance": "d", "faces": [top.clone()]}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "c", "params": {"x": "6 mm", "y": "6 mm", "z": "8 mm"},
                       "placement": {"origin": [40, 0, 0]}}),
    );
    let lid = face(&mut s, "c", [0.0, 0.0, 1.0]);
    add(
        &mut s,
        json!({"type": "plane", "id": "lvl", "by": "offset", "from": "xy", "distance": "lvl"}),
    );
    add(
        &mut s,
        json!({"type": "replace_face", "id": "r",
                       "groups": [{"with": "p/lvl", "faces": [lid.clone()]}]}),
    );
    (dir, s, top, lid)
}

/// One random edit of [`tweaked`]'s part.
fn tweak_edit(g: &mut Lcg, s: &mut Session, top: &str) {
    let (method, params) = match g.next_u64() % 6 {
        0 => ("param.set", json!({"parameters": {"w": mm(g, 15, 30)}})),
        1 => ("param.set", json!({"parameters": {"d": mm(g, 1, 4)}})),
        2 => ("param.set", json!({"parameters": {"lvl": mm(g, 3, 7)}})),
        3 => (
            "feature.edit",
            json!({"id": "m", "by": "rotate", "axis": "x", "angle": "2 deg"}),
        ),
        4 => (
            "feature.edit",
            json!({"id": "m", "by": "translate", "direction": "z",
                                     "distance": "d", "faces": [top]}),
        ),
        _ => (
            "feature.edit",
            json!({"id": "m", "by": "offset", "distance": "d"}),
        ),
    };
    let mut params = params;
    params["part"] = json!("p");
    let _ = call(s, method, params);
}

#[test]
fn tweaks_recompute_as_from_nothing() {
    for seed in [8, 9] {
        let (_dir, mut s, top, _lid) = tweaked();
        let part = Id::parse("p", "part").unwrap();
        let first = s.rebuilt(&part).unwrap();
        assert_eq!(first.failed(), 0, "{:?}", summary(first));
        let mut g = Lcg::new(seed);
        for step in 0..10 {
            tweak_edit(&mut g, &mut s, &top);
            let (one, eight) = (full(&s, 1), full(&s, 8));
            let r = s.rebuilt(&part).unwrap();
            let incremental = (r.hash.clone(), summary(r));
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
        }
    }
}

/// Part `p`: half a bracket `half` (`w` × 10 × 5 mm, an upright `up`
/// joined on it), mirrored and joined as `m`; a datum `cut` `lvl` up from
/// xy (`lvl`) splitting the whole as `s`; and a copy `c` of the side in
/// front, as a solid of its own.
fn mirrored() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"w": "20 mm", "lvl": "3 mm"}}),
    );
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "box", "id": "half", "params": {"x": "w", "y": "10 mm", "z": "5 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "up", "operation": "join", "targets": ["half"],
                       "params": {"x": "5 mm", "y": "10 mm", "z": "15 mm"},
                       "placement": {"origin": [0, 0, 5]}}),
    );
    add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "half", "plane": "yz"}),
    );
    add(
        &mut s,
        json!({"type": "plane", "id": "cut", "by": "offset", "from": "xy", "distance": "lvl"}),
    );
    add(
        &mut s,
        json!({"type": "split", "id": "s", "target": "half", "plane": "p/cut"}),
    );
    add(
        &mut s,
        json!({"type": "mirror", "id": "c", "solid": "s", "plane": "zx", "operation": "new"}),
    );
    (dir, s)
}

/// One random edit of [`mirrored`]'s part.
fn mirror_edit(g: &mut Lcg, s: &mut Session) {
    let (method, params) = match g.next_u64() % 6 {
        0 => ("param.set", json!({"parameters": {"w": mm(g, 15, 30)}})),
        1 => ("param.set", json!({"parameters": {"lvl": mm(g, 1, 4)}})),
        2 => (
            "feature.edit",
            json!({"id": "s", "flip": g.next_u64().is_multiple_of(2)}),
        ),
        3 => (
            "feature.edit",
            json!({"id": "m",
              "plane": if g.next_u64().is_multiple_of(2) { "yz" } else { "zx" }}),
        ),
        4 => (
            "feature.edit",
            json!({"id": "c",
              "operation": if g.next_u64().is_multiple_of(2) { "new" } else { "cut" },
              "targets": ["half"]}),
        ),
        _ => ("feature.edit", json!({"id": "c", "operation": "new"})),
    };
    let mut params = params;
    params["part"] = json!("p");
    let _ = call(s, method, params);
}

#[test]
fn mirrors_and_splits_recompute_as_from_nothing() {
    for seed in [6, 7] {
        let (_dir, mut s) = mirrored();
        let part = Id::parse("p", "part").unwrap();
        let first = s.rebuilt(&part).unwrap();
        assert_eq!(first.failed(), 0, "{:?}", summary(first));
        let mut g = Lcg::new(seed);
        for step in 0..10 {
            mirror_edit(&mut g, &mut s);
            let (one, eight) = (full(&s, 1), full(&s, 8));
            let r = s.rebuilt(&part).unwrap();
            let incremental = (r.hash.clone(), summary(r));
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
        }
    }
}

/// Part `p`: a plate `plate` 60 × 40 mm, `t` thick with an inner radius
/// `r` (parameters), and a flange `f` on its right top edge, an end of it
/// set back with a relief.
fn sheeted() -> (tempfile::TempDir, Session, String) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"t": "1.5 mm", "r": "1 mm"}}),
    );
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f)
    };
    add(&mut s, json!({"type": "sketch", "id": "s", "plane": "xy"}));
    let mut ops = Vec::new();
    for (i, at) in [[0, 0], [60, 0], [60, 40], [0, 40]].into_iter().enumerate() {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    for i in 1..=4 {
        ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                        "start": format!("p{i}"), "end": format!("p{}", i % 4 + 1)}));
    }
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": ops}),
    );
    add(
        &mut s,
        json!({"type": "base_flange", "id": "plate", "sketch": "s", "thickness": "t",
                       "radius": "r", "allowance": {"k_factor": "0.44"},
                       "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}),
    );
    let b = must(&mut s, "inspect", json!({"part": "p", "feature": "plate"}))["bodies"][0].clone();
    // The rim at x = 60 on top (z = t), and its far corner, by name.
    let top = |e: &Value| {
        let pt = |n: &Value| {
            b["vertices"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == *n)
                .unwrap()["point"]
                .clone()
        };
        let (a, c) = (pt(&e["start"]), pt(&e["end"]));
        (a[0].as_f64().unwrap() - 0.06).abs() < 1e-12
            && (c[0].as_f64().unwrap() - 0.06).abs() < 1e-12
            && a[2].as_f64().unwrap() > 1e-4
            && c[2].as_f64().unwrap() > 1e-4
    };
    let rim = b["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| top(e))
        .unwrap();
    let corner = format!("p/plate/{}", rim["end"].as_str().unwrap());
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": [format!("p/plate/{}", rim["name"].as_str().unwrap())],
                       "angle": "90 deg", "length": "20 mm",
                       "ends": [{"vertex": corner, "setback": "4 mm"}]}),
    );
    (dir, s, corner)
}

/// One random edit of [`sheeted`]'s part.
fn sheet_edit(g: &mut Lcg, s: &mut Session, corner: &str) {
    let (method, params) = match g.next_u64() % 7 {
        0 => ("param.set", json!({"parameters": {"t": mm(g, 1, 2)}})),
        1 => ("param.set", json!({"parameters": {"r": mm(g, 1, 3)}})),
        2 => (
            "feature.edit",
            json!({"id": "plate", "allowance": {"k_factor": "0.4"}}),
        ),
        3 => (
            "feature.edit",
            json!({"id": "f", "angle": if g.next_u64().is_multiple_of(2) { "90 deg" } else { "75 deg" }}),
        ),
        4 => ("feature.edit", json!({"id": "f", "length": mm(g, 10, 25)})),
        5 => (
            "feature.edit",
            json!({"id": "plate", "relief": {"kind": "obround", "width": "1 mm", "depth": "1 mm"}}),
        ),
        _ => (
            "feature.edit",
            json!({"id": "f", "ends": [{"vertex": corner, "setback": mm(g, 0, 5)}]}),
        ),
    };
    let mut params = params;
    params["part"] = json!("p");
    let _ = call(s, method, params);
}

#[test]
fn sheet_metal_recomputes_as_from_nothing() {
    for seed in [8, 9] {
        let (_dir, mut s, corner) = sheeted();
        let part = Id::parse("p", "part").unwrap();
        let first = s.rebuilt(&part).unwrap();
        assert_eq!(first.failed(), 0, "{:?}", summary(first));
        let mut g = Lcg::new(seed);
        for step in 0..10 {
            sheet_edit(&mut g, &mut s, &corner);
            let (one, eight) = (full(&s, 1), full(&s, 8));
            let r = s.rebuilt(&part).unwrap();
            let incremental = (r.hash.clone(), summary(r));
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
        }
    }
}

/// [`sheeted`]'s part unfolded (`u`), a slot cut across the flange's bend
/// line in the flat (`c`: the bend line lies `r + t` in from the plate's
/// right edge, between 55 and 58 mm for every thickness and radius the
/// edits give, and the strip runs on past 59 mm), and folded back (`rf`).
fn flattened() -> (tempfile::TempDir, Session, String) {
    let (dir, mut s, corner) = sheeted();
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f)
    };
    add(
        &mut s,
        json!({"type": "unfold", "id": "u", "target": "plate"}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "c", "operation": "cut", "targets": ["plate"],
               "params": {"x": 5, "y": 4, "z": 6}, "placement": {"origin": [54, 12, -2]}}),
    );
    add(
        &mut s,
        json!({"type": "refold", "id": "rf", "target": "plate", "unfold": "u"}),
    );
    (dir, s, corner)
}

/// One random edit of [`flattened`]'s part: one of [`sheet_edit`]'s, or
/// the unfold's allowance, the cut moved along the bend line, or the
/// refold's strips all again.
fn flat_edit(g: &mut Lcg, s: &mut Session, corner: &str) {
    let (method, params) = match g.next_u64() % 4 {
        0 => return sheet_edit(g, s, corner),
        1 => (
            "feature.edit",
            json!({"id": "u", "allowance": {"k_factor": format!("0.{}", 3 + g.next_u64() % 4)}}),
        ),
        2 => (
            "feature.edit",
            json!({"id": "c", "placement": {"origin": [54, 8 + g.next_u64() % 16, -2]}}),
        ),
        _ => ("feature.edit", json!({"id": "u", "allowance": ""})),
    };
    let mut params = params;
    params["part"] = json!("p");
    let _ = call(s, method, params);
}

#[test]
fn unfold_cut_and_refold_recompute_as_from_nothing() {
    for seed in [10, 11] {
        let (_dir, mut s, corner) = flattened();
        let part = Id::parse("p", "part").unwrap();
        let first = s.rebuilt(&part).unwrap();
        assert_eq!(first.failed(), 0, "{:?}", summary(first));
        let mut g = Lcg::new(seed);
        for step in 0..10 {
            flat_edit(&mut g, &mut s, &corner);
            let (one, eight) = (full(&s, 1), full(&s, 8));
            let r = s.rebuilt(&part).unwrap();
            let incremental = (r.hash.clone(), summary(r));
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
        }
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

/// A frame placing a pair of bolts twice and a plate.
fn assembled() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "bolt"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "bolt", "type": "cylinder", "id": "shank",
               "params": {"radius": 3, "height": 17}}),
    );
    must(&mut s, "part.create", json!({"part": "plate"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "plate", "type": "box", "id": "slab",
               "params": {"x": 60, "y": 30, "z": 6}}),
    );
    for a in ["pair", "frame"] {
        must(
            &mut s,
            "part.create",
            json!({"part": a, "kind": "assembly"}),
        );
    }
    for (id, x) in [("left", -8), ("right", 8)] {
        must(
            &mut s,
            "feature.add",
            json!({"part": "pair", "type": "instance", "id": id, "of": "bolt",
                   "placement": {"origin": [x, 0, 0]}}),
        );
    }
    must(
        &mut s,
        "feature.add",
        json!({"part": "frame", "type": "instance", "id": "base", "of": "plate"}),
    );
    for (id, x) in [("p1", 15), ("p2", 45)] {
        must(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": id, "of": "pair",
                   "placement": {"origin": [x, 15, 6]}}),
        );
    }
    (dir, s)
}

/// One random edit of the assembled project's parts or instances.
fn assembly_edit(g: &mut Lcg, s: &mut Session) {
    let (method, params) = match g.next_u64() % 6 {
        0 => (
            "feature.edit",
            json!({"part": "bolt", "id": "shank", "params": {"radius": mm(g, 2, 4)}}),
        ),
        1 => (
            "feature.edit",
            json!({"part": "plate", "id": "slab", "params": {"z": mm(g, 4, 8)}}),
        ),
        2 => (
            "assembly.move",
            json!({"part": "frame", "instance": "p2", "translate": [mm(g, 0, 2), 0, 0]}),
        ),
        3 => (
            "assembly.move",
            json!({"part": "pair", "instance": "left", "axis": [0, 0, 1], "angle": "15 deg"}),
        ),
        4 => (
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "extra", "of": "bolt",
                   "placement": {"origin": [mm(g, 1, 50), 5, 6]}}),
        ),
        _ => ("feature.delete", json!({"part": "frame", "id": "extra"})),
    };
    let _ = call(s, method, params);
}

/// The frame rebuilt from nothing on `threads` threads: its model's hash
/// and each part's rebuild hash.
fn assembly_full(s: &Session, threads: usize) -> Vec<String> {
    use ryucad::assembly::{Pins, Rebuild, Store};
    use ryucad::doc::assembly::Rev;
    let project = s.project().clone();
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            let pins = Pins::new();
            let mut r = Rebuild::new(&project, &pins, Store::default());
            let frame = Id::parse("frame", "part").unwrap();
            let mut out = vec![r.model(&frame, &Rev::Working).unwrap().hash.clone()];
            for p in ["bolt", "plate", "pair", "frame"] {
                out.push(
                    r.result(&Id::parse(p, "part").unwrap())
                        .unwrap()
                        .hash
                        .clone(),
                );
            }
            out
        })
}

#[test]
fn assemblies_recompute_as_from_nothing() {
    use ryucad::doc::assembly::Rev;
    let hashes = |s: &mut Session| {
        let frame = Id::parse("frame", "part").unwrap();
        let mut out = vec![s.model(&frame, &Rev::Working).unwrap().hash.clone()];
        for p in ["bolt", "plate", "pair", "frame"] {
            let id = Id::parse(p, "part").unwrap();
            out.push(s.rebuilt(&id).unwrap().hash.clone());
        }
        out
    };
    for seed in [1, 2] {
        let (dir, mut s) = assembled();
        // Beside it, a session whose budget holds nothing: every part's
        // rebuild goes after each request and comes back when asked for.
        let mut tight = Session::open(dir.path()).unwrap().with_budget(1);
        let mut g = Lcg::new(seed);
        for step in 0..20 {
            assembly_edit(&mut g, &mut s);
            let incremental = hashes(&mut s);
            tight.refresh().unwrap();
            assert_eq!(hashes(&mut tight), incremental, "seed {seed}, step {step}");
            tight.trim();
            assert_eq!(
                incremental,
                assembly_full(&s, 1),
                "seed {seed}, step {step}"
            );
            assert_eq!(
                incremental,
                assembly_full(&s, 8),
                "seed {seed}, step {step}"
            );
        }
    }
}

/// The planar face of `part`'s solid `solid` facing `n`.
fn face_along(s: &mut Session, part: &str, solid: &str, n: [f64; 3]) -> String {
    let v = must(s, "inspect", json!({"part": part, "feature": solid}));
    v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let z = &f["surface"]["frame"]["z"];
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && (0..3).all(|k| (z[k].as_f64().unwrap() * sign - n[k]).abs() < 1e-12)
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .expect("a face along n")
}

/// A frame with mates: a plate fixed, a bolt on a revolute through its
/// top, copies of it round the plate's z, and a cap fastened up a copy.
fn mated() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "bolt"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "bolt", "type": "cylinder", "id": "shank",
               "params": {"radius": 3, "height": 17}}),
    );
    must(&mut s, "part.create", json!({"part": "plate"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "plate", "type": "box", "id": "slab",
               "params": {"x": 60, "y": 30, "z": 6}}),
    );
    must(
        &mut s,
        "part.create",
        json!({"part": "frame", "kind": "assembly"}),
    );
    for (id, of, fixed) in [
        ("base", "plate", true),
        ("pin", "bolt", false),
        ("cap", "bolt", false),
    ] {
        must(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": id, "of": of, "fixed": fixed}),
        );
    }
    let top = face_along(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
    let foot = face_along(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
    must(
        &mut s,
        "feature.add",
        json!({"part": "frame", "type": "mate", "id": "m1", "form": "revolute",
               "a": format!("frame/base/slab/{top}"),
               "a_offset": {"origin": ["20 mm", "10 mm", "0 mm"]},
               "b": format!("frame/pin/shank/{foot}"), "b_offset": {"flip": true}}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "frame", "type": "instance_pattern", "id": "ring",
               "instances": ["pin"], "axis": "z", "count": 3, "angle": "360 deg"}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "frame", "type": "mate", "id": "m2", "form": "fastened",
               "a": "frame/ring:1/pin", "a_offset": {"origin": ["0 mm", "0 mm", "20 mm"]},
               "b": "frame/cap"}),
    );
    (dir, s)
}

/// One random edit of the mated frame or the parts it places.
fn mated_edit(g: &mut Lcg, s: &mut Session) {
    let (method, params) = match g.next_u64() % 6 {
        0 => (
            "feature.edit",
            json!({"part": "plate", "id": "slab", "params": {"z": mm(g, 4, 8)}}),
        ),
        1 => (
            "feature.edit",
            json!({"part": "frame", "id": "m1",
                   "a_offset": {"origin": [mm(g, 10, 25), "10 mm", "0 mm"]}}),
        ),
        2 => (
            "feature.edit",
            json!({"part": "frame", "id": "m1", "angle": format!("{} deg", g.next_u64() % 90)}),
        ),
        3 => (
            "feature.edit",
            json!({"part": "frame", "id": "ring", "count": 2 + g.next_u64() % 4}),
        ),
        4 => (
            "assembly.move",
            json!({"part": "frame", "instance": "pin", "axis": [0, 0, 1], "angle": "15 deg"}),
        ),
        _ => (
            "feature.edit",
            json!({"part": "frame", "id": "m1", "angle": ""}),
        ),
    };
    let _ = call(s, method, params);
}

/// The mated frame rebuilt from nothing on `threads` threads: its model's
/// hash and each part's rebuild hash.
fn mated_full(s: &Session, threads: usize) -> Vec<String> {
    use ryucad::assembly::{Pins, Rebuild, Store};
    use ryucad::doc::assembly::Rev;
    let project = s.project().clone();
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            let pins = Pins::new();
            let mut r = Rebuild::new(&project, &pins, Store::default());
            let frame = Id::parse("frame", "part").unwrap();
            let mut out = vec![r.model(&frame, &Rev::Working).unwrap().hash.clone()];
            for p in ["bolt", "plate", "frame"] {
                out.push(
                    r.result(&Id::parse(p, "part").unwrap())
                        .unwrap()
                        .hash
                        .clone(),
                );
            }
            out
        })
}

#[test]
fn mated_assemblies_recompute_as_from_nothing() {
    use ryucad::doc::assembly::Rev;
    for seed in [1, 2] {
        let (_dir, mut s) = mated();
        let mut g = Lcg::new(seed);
        for step in 0..20 {
            mated_edit(&mut g, &mut s);
            let frame = Id::parse("frame", "part").unwrap();
            let mut incremental = vec![s.model(&frame, &Rev::Working).unwrap().hash.clone()];
            for p in ["bolt", "plate", "frame"] {
                let id = Id::parse(p, "part").unwrap();
                incremental.push(s.rebuilt(&id).unwrap().hash.clone());
            }
            // Writes keep it current.
            let r = s.rebuilt(&frame).unwrap();
            assert!(
                r.solve.as_ref().unwrap().report.converged,
                "seed {seed}, step {step}"
            );
            assert!(
                !r.solve.as_ref().unwrap().stale(),
                "seed {seed}, step {step}"
            );
            assert_eq!(incremental, mated_full(&s, 1), "seed {seed}, step {step}");
            assert_eq!(incremental, mated_full(&s, 8), "seed {seed}, step {step}");
        }
    }
}

/// Each part's hash in a `rebuild` of the whole project, answered with
/// `job`, from a session opened afresh on `root` (so nothing is in memory
/// yet) or the one `slot` holds.
fn rebuilt_hashes(
    slot: &mut Option<Session>,
    root: &std::path::Path,
    job: &ryu_session::job::Job,
) -> ryucad::error::Result<Vec<String>> {
    let req = Request::from_rpc("rebuild", Some(json!({}))).unwrap();
    let a = ryucad::server::handle_job(slot, None, None, root, req, job)?;
    Ok(a.result["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| format!("{} {}", p["part"], p["hash"]))
        .collect())
}

/// A rebuild cancelled at any of its boundaries answers `cancelled` with
/// its counts, and leaves nothing that changes a later answer: the next
/// rebuild in the same session is the one never cancelled, on one thread
/// and on eight, for a part whose features read each other and for an
/// assembly with what it places.
#[test]
fn a_cancelled_rebuild_changes_no_later_answer() {
    use ryu_session::job::Job;
    for make in [project as fn() -> (tempfile::TempDir, Session), assembled] {
        let (dir, _) = make();
        let expected = rebuilt_hashes(&mut None, dir.path(), &Job::default()).unwrap();
        for threads in [1, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let mut stopped = 0;
            for n in [1, 2, 3, 5, 8, 13, 21, 34, 55] {
                let mut slot = None;
                let first =
                    pool.install(|| rebuilt_hashes(&mut slot, dir.path(), &Job::cancelled_at(n)));
                match first {
                    Err(e) => {
                        assert_eq!(e.code, ryucad::error::Code::Cancelled, "{e:?}");
                        assert!(e.details["total"].as_u64().is_some(), "{e:?}");
                        stopped += 1;
                    }
                    Ok(h) => assert_eq!(h, expected, "{threads} threads, boundary {n}"),
                }
                let again = pool.install(|| rebuilt_hashes(&mut slot, dir.path(), &Job::default()));
                assert_eq!(again.unwrap(), expected, "{threads} threads, boundary {n}");
            }
            assert!(
                stopped >= 3,
                "{threads} threads: only {stopped} cancels landed"
            );
        }
    }
}

/// Parts `p` and `q` over the project's parameters `w` and `r`: `p`'s box
/// reads `w`, its cylinder `r`; `q`'s ball reads `r` through its own `k`.
fn shared() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(
        &mut s,
        "param.set",
        json!({"scope": "project", "parameters": {"w": "20 mm", "r": "3 mm"}}),
    );
    for part in ["p", "q"] {
        must(&mut s, "part.create", json!({"part": part}));
    }
    must(
        &mut s,
        "param.set",
        json!({"part": "q", "parameters": {"k": "r + 1 mm"}}),
    );
    for f in [
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": "w", "y": 10, "z": 5}}),
        json!({"part": "p", "type": "cylinder", "id": "c", "params": {"radius": "r", "height": 4},
               "placement": {"origin": [40, 0, 0]}}),
        json!({"part": "q", "type": "sphere", "id": "ball", "params": {"radius": "k"}}),
        json!({"part": "q", "type": "box", "id": "lug", "params": {"x": 2, "y": 2, "z": 2},
               "placement": {"origin": [10, 0, 0]}}),
    ] {
        must(&mut s, "feature.add", f);
    }
    (dir, s)
}

/// Every part's rebuild from nothing on `threads` threads.
fn shared_full(s: &Session, threads: usize) -> Vec<(String, Vec<String>)> {
    s.project()
        .parts
        .values()
        .map(|p| {
            let r = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| rebuild_part(p));
            (r.hash.clone(), summary(&r))
        })
        .collect()
}

#[test]
fn project_parameters_recompute_as_from_nothing() {
    let parts = [
        Id::parse("p", "part").unwrap(),
        Id::parse("q", "part").unwrap(),
    ];
    for seed in [1, 2, 3] {
        let (_dir, mut s) = shared();
        for p in &parts {
            assert_eq!(s.rebuilt(p).unwrap().failed(), 0);
        }
        let mut g = Lcg::new(seed);
        for step in 0..20 {
            let (method, params) = match g.next_u64() % 4 {
                0 => (
                    "param.set",
                    json!({"scope": "project", "parameters": {"w": mm(&mut g, 15, 30)}}),
                ),
                1 => (
                    "param.set",
                    json!({"scope": "project", "parameters": {"r": mm(&mut g, 0, 4)}}),
                ),
                2 => (
                    "param.set",
                    json!({"part": "q", "parameters": {"k": format!("r + {}", mm(&mut g, 0, 2))}}),
                ),
                _ => (
                    "feature.edit",
                    json!({"part": "q", "id": "lug", "params": {"x": mm(&mut g, 1, 4)}}),
                ),
            };
            let q_before = s.project().part(&parts[1]).unwrap().revision.clone();
            // Writes that would break a feature are refused: fine.
            let wrote = call(&mut s, method, params.clone()).is_ok();
            // A part reading nothing that changed keeps its revision, so
            // its rebuild is the one the session has.
            if wrote && params["parameters"].get("w").is_some() {
                let q_after = &s.project().part(&parts[1]).unwrap().revision;
                assert_eq!(q_after, &q_before, "seed {seed}, step {step}");
            }
            let (one, eight) = (shared_full(&s, 1), shared_full(&s, 8));
            let mut incremental = Vec::new();
            for p in &parts {
                let r = s.rebuilt(p).unwrap();
                incremental.push((r.hash.clone(), summary(r)));
            }
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
        }
    }
}

/// A plate with an `nx` × `ny` pattern of holes through it: one slow cut,
/// as `tests/bar.rs` builds it, smaller.
fn plate_with_holes(nx: u32, ny: u32) -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), Some(ryucad::units::Unit::M)).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "plate"}));
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("plate");
        must(s, "feature.add", f)
    };
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": nx, "y": ny, "z": 0.5}}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "t", "placement": {"origin": [0, 0, 0.5]}}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "plate", "sketch": "t", "ops": [
            {"op": "add", "type": "point", "at": [0.5, 0.5]},
            {"op": "add", "type": "fix", "a": "p1"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "t", "diameter": 0.6, "depth": "through_all"}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "grid", "features": ["h"], "direction": "x",
               "spacing": 1, "count": nx, "direction2": "y", "spacing2": 1, "count2": ny}),
    );
    (dir, s)
}

/// A cancel during a slow cut stops it inside the kernel (well before the
/// cut would have finished, judged in release builds), keeps nothing of
/// it, and the next rebuild is the same as one from nothing.
#[test]
fn a_cancel_inside_a_kernel_call_keeps_nothing() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    let (_dir, s) = plate_with_holes(20, 20);
    let part = s
        .project()
        .part(&Id::parse("plate", "part").unwrap())
        .unwrap()
        .clone();
    let whole = {
        let t = Instant::now();
        let fresh = rebuild_part(&part);
        assert!(fresh.features.iter().all(|(_, r)| r.is_ok()));
        (t.elapsed(), fresh.hash)
    };
    let flag = Arc::new(AtomicBool::new(false));
    let job = ryu_session::job::Job::new("t", "p", "rebuild", flag.clone());
    let cache = ryucad::rebuild::Cache::new();
    let setter = {
        let flag = flag.clone();
        let wait = whole.0 / 4;
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            flag.store(true, Ordering::SeqCst);
            Instant::now()
        })
    };
    let r = ryucad::rebuild::rebuild_part_in(
        &part,
        &cache,
        &ryucad::assembly::Children::new(),
        None,
        &ryucad::rebuild::Run::with_job(&job),
    );
    let ended = Instant::now();
    let set = setter.join().unwrap();
    let Err(stopped) = r else {
        panic!("finished despite the cancel")
    };
    assert_eq!(stopped.error.code, ryucad::error::Code::Cancelled);
    let after = ended.saturating_duration_since(set);
    // In a debug build the cut's last step, building the body after every
    // certificate (unmetered: it is linear in what it builds), checks the
    // topology after each Euler operator and dominates: the time is judged
    // in release builds only.
    assert!(
        cfg!(debug_assertions) || after < whole.0 / 4 + Duration::from_millis(200),
        "stopped {after:?} after the cancel; the cut takes {:?}",
        whole.0
    );
    let again = ryucad::rebuild::rebuild_part_with(&part, &stopped.cache).0;
    assert_eq!(again.hash, whole.1);
}

/// Part `p`: a datum plane `lvl` up from xy (`lvl`), a box `b`
/// (`w` × 12 × 8 mm) whose walls `d` drafts by `a` about xy. Returns the
/// walls' references, +x first and +y third.
fn drafted() -> (tempfile::TempDir, Session, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"w": "20 mm", "a": "2 deg", "lvl": "4 mm"}}),
    );
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "plane", "id": "lvl", "by": "offset", "from": "xy", "distance": "lvl"}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": "w", "y": "12 mm", "z": "8 mm"}}),
    );
    let walls: Vec<String> = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
    ]
    .iter()
    .map(|n| face(&mut s, "b", *n))
    .collect();
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "a", "neutral": "xy", "faces": walls.clone()}),
    );
    (dir, s, walls)
}

/// One random edit of [`drafted`]'s part.
fn draft_edit(g: &mut Lcg, s: &mut Session, walls: &[String]) {
    let (method, params) = match g.next_u64() % 7 {
        0 => ("param.set", json!({"parameters": {"w": mm(g, 15, 30)}})),
        1 => (
            "param.set",
            json!({"parameters": {"a": format!("{} deg", 1 + g.next_u64() % 4)}}),
        ),
        2 => ("param.set", json!({"parameters": {"lvl": mm(g, 2, 6)}})),
        3 => ("feature.edit", json!({"id": "d", "neutral": "p/lvl"})),
        4 => ("feature.edit", json!({"id": "d", "neutral": "xy"})),
        5 => (
            "feature.edit",
            json!({"id": "d", "flip": if g.next_u64().is_multiple_of(2) { json!([walls[2]]) } else { json!([]) }}),
        ),
        _ => (
            "feature.edit",
            json!({"id": "d", "tangent": g.next_u64().is_multiple_of(2)}),
        ),
    };
    let mut params = params;
    params["part"] = json!("p");
    let _ = call(s, method, params);
}

#[test]
fn drafts_recompute_as_from_nothing() {
    for seed in [10, 11] {
        let (_dir, mut s, walls) = drafted();
        let part = Id::parse("p", "part").unwrap();
        let first = s.rebuilt(&part).unwrap();
        assert_eq!(first.failed(), 0, "{:?}", summary(first));
        let mut g = Lcg::new(seed);
        for step in 0..10 {
            draft_edit(&mut g, &mut s, &walls);
            let (one, eight) = (full(&s, 1), full(&s, 8));
            let r = s.rebuilt(&part).unwrap();
            let incremental = (r.hash.clone(), summary(r));
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
        }
    }
}

/// Part `p`: a plate `b` (`w` × 30 × 10 mm) with a hole `h` (`dia`) at
/// (15, 15), deleted by its seed (`d`), and a box `c` beside it whose −x
/// wall a draft `t` (`ang`) leans past its +x wall with topology: change.
fn edited() -> (tempfile::TempDir, Session, String) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"w": "40 mm", "dia": "6 mm", "ang": "1.3 rad"}}),
    );
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": "w", "y": "30 mm", "z": "10 mm"}}),
    );
    let top = face(&mut s, "b", [0.0, 0.0, 1.0]);
    add(&mut s, json!({"type": "sketch", "id": "s", "face": top}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
        {"op": "add", "type": "point", "at": [15, 15]}, {"op": "add", "type": "fix", "a": "p1"}]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "s", "points": ["p1"],
        "diameter": "dia", "depth": "through_all", "targets": ["b"]}),
    );
    let v = must(&mut s, "inspect", json!({"part": "p", "feature": "h"}));
    let bore = v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["surface"]["type"] == "cylinder")
        .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
        .unwrap();
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "seeds": [bore.clone()]}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "c", "params": {"x": "40 mm", "y": "30 mm", "z": "20 mm"},
                       "placement": {"origin": [80, 0, 0]}}),
    );
    let wall = face(&mut s, "c", [-1.0, 0.0, 0.0]);
    add(
        &mut s,
        json!({"type": "draft", "id": "t", "target": "c", "angle": "ang", "neutral": "xy",
                       "faces": [wall], "topology": "change"}),
    );
    (dir, s, bore)
}

/// One random edit of [`edited`]'s part.
fn edit_edit(g: &mut Lcg, s: &mut Session) {
    let (method, params) = match g.next_u64() % 6 {
        0 => ("param.set", json!({"parameters": {"w": mm(g, 30, 50)}})),
        1 => ("param.set", json!({"parameters": {"dia": mm(g, 3, 8)}})),
        2 => (
            "param.set",
            json!({"parameters": {"ang": format!("{} rad", 1.1 + (g.next_u64() % 4) as f64 * 0.1)}}),
        ),
        3 => (
            "feature.edit",
            json!({"id": "d", "heal": (["auto", "cap", "grow"][(g.next_u64() % 3) as usize])}),
        ),
        4 => (
            "feature.edit",
            json!({"id": "t", "topology": (["keep", "change"][(g.next_u64() % 2) as usize])}),
        ),
        _ => (
            "feature.edit",
            json!({"id": "h", "form": "counterbore",
                                     "counterbore": {"diameter": "10 mm", "depth": "3 mm"}}),
        ),
    };
    let mut params = params;
    params["part"] = json!("p");
    let _ = call(s, method, params);
}

#[test]
fn direct_edits_recompute_as_from_nothing() {
    for seed in [22, 23] {
        let (_dir, mut s, _bore) = edited();
        let part = Id::parse("p", "part").unwrap();
        let first = s.rebuilt(&part).unwrap();
        assert_eq!(first.failed(), 0, "{:?}", summary(first));
        let mut g = Lcg::new(seed);
        for step in 0..10 {
            edit_edit(&mut g, &mut s);
            let (one, eight) = (full(&s, 1), full(&s, 8));
            let r = s.rebuilt(&part).unwrap();
            let incremental = (r.hash.clone(), summary(r));
            assert_eq!(incremental, one, "seed {seed}, step {step}");
            assert_eq!(incremental, eight, "seed {seed}, step {step}");
        }
    }
}
