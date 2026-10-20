//! Sketches swept into bodies: names that survive edits, sound bodies, and
//! results that don't depend on thread count.

use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn call(s: &mut Session, method: &str, params: Value) -> Value {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// The ops for a rectangle `w` × `h` with its corner at the origin, its
/// lines drawn counterclockwise or clockwise.
fn rectangle(w: &str, h: &str, clockwise: bool) -> Value {
    let lines = if clockwise {
        [
            ("l1", "p2", "p1"),
            ("l2", "p3", "p2"),
            ("l3", "p4", "p3"),
            ("l4", "p1", "p4"),
        ]
    } else {
        [
            ("l1", "p1", "p2"),
            ("l2", "p2", "p3"),
            ("l3", "p3", "p4"),
            ("l4", "p4", "p1"),
        ]
    };
    let mut ops = vec![
        json!({"op": "add", "id": "p1", "type": "point", "at": [0, 0]}),
        json!({"op": "add", "id": "p2", "type": "point", "at": [19, 1]}),
        json!({"op": "add", "id": "p3", "type": "point", "at": [21, 9]}),
        json!({"op": "add", "id": "p4", "type": "point", "at": [1, 11]}),
    ];
    for (id, a, b) in lines {
        ops.push(json!({"op": "add", "id": id, "type": "line", "start": a, "end": b}));
    }
    for (ty, a) in [
        ("horizontal", "l1"),
        ("horizontal", "l3"),
        ("vertical", "l2"),
        ("vertical", "l4"),
    ] {
        ops.push(json!({"op": "add", "type": ty, "a": a}));
    }
    ops.push(json!({"op": "add", "type": "fix", "a": "p1"}));
    ops.push(
        json!({"op": "add", "id": "w", "type": "distance_x", "a": "p1", "b": "p3", "value": w}),
    );
    ops.push(
        json!({"op": "add", "id": "h", "type": "distance_y", "a": "p1", "b": "p3", "value": h}),
    );
    json!(ops)
}

/// A project whose part `p` has parameter `width`, the rectangle sketch
/// `s` and `rest` added after it.
fn project(clockwise: bool, rest: &[Value]) -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    call(&mut s, "part.create", json!({"part": "p"}));
    call(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"width": "20 mm"}}),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "s"}),
    );
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": rectangle("width", "10 mm", clockwise)}),
    );
    for f in rest {
        let mut f = f.clone();
        f["part"] = json!("p");
        call(&mut s, "feature.add", f);
    }
    (dir, s)
}

fn extrude() -> Value {
    json!({"type": "extrude", "id": "e", "sketch": "s", "extent": {"length": "5 mm"}})
}

fn revolve(angle: Value) -> Value {
    json!({"type": "revolve", "id": "r", "sketch": "s", "axis": "l4", "angle": angle})
}

/// Every face, edge and vertex name of feature `f`, with its origin.
fn names(s: &mut Session, f: &str) -> BTreeMap<String, Value> {
    let v = call(s, "inspect", json!({"part": "p", "feature": f}));
    let b = &v["bodies"][0];
    let mut out = BTreeMap::new();
    for list in ["faces", "edges", "vertices"] {
        for e in b[list].as_array().unwrap() {
            out.insert(e["name"].as_str().unwrap().to_owned(), e["origin"].clone());
        }
    }
    out
}

#[test]
fn dimensions_and_extents_rename_nothing() {
    for sweep in [extrude(), revolve(json!(90)), revolve(json!("full"))] {
        let (_dir, mut s) = project(false, std::slice::from_ref(&sweep));
        let f = sweep["id"].as_str().unwrap();
        let before = names(&mut s, f);
        assert!(!before.is_empty());
        let r = call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"width": "35 mm"}}),
        );
        assert_eq!(r["sketches"][0]["ok"], true);
        let edit = match f {
            "e" => json!({"extent": {"type": "symmetric", "length": "12 mm"}, "reverse": true}),
            _ => json!({"symmetric": true}),
        };
        let mut edit = edit;
        edit["part"] = json!("p");
        edit["id"] = json!(f);
        call(&mut s, "feature.edit", edit);
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "set", "id": "h", "value": "4 mm"}]}),
        );
        assert_eq!(names(&mut s, f), before, "{f}");
    }
}

#[test]
fn which_way_lines_run_renames_nothing() {
    let (_a, mut ccw) = project(false, &[extrude(), revolve(json!(120))]);
    let (_b, mut cw) = project(true, &[extrude(), revolve(json!(120))]);
    for f in ["e", "r"] {
        assert_eq!(names(&mut ccw, f), names(&mut cw, f), "{f}");
    }
}

/// Whether an origin mentions entity `e`.
fn mentions(origin: &Value, e: &str) -> bool {
    origin["entity"] == e
        || origin["entities"]
            .as_array()
            .is_some_and(|a| a.contains(&json!(e)))
}

#[test]
fn a_new_segment_renames_only_what_sat_where_it_joins() {
    let (_dir, mut s) = project(false, &[extrude()]);
    let before = names(&mut s, "e");
    // Split the top: l3 now ends at a new point p5, and l6 runs on to p4.
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "remove", "id": "l3"},
            {"op": "add", "id": "p5", "type": "point", "at": [8, 10]},
            {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p5"},
            {"op": "add", "id": "l6", "type": "line", "start": "p5", "end": "p4"},
            {"op": "add", "type": "horizontal", "a": "l3"},
            {"op": "add", "type": "horizontal", "a": "l6"}
        ]}),
    );
    let after = names(&mut s, "e");
    let gone: BTreeSet<&String> = before.keys().filter(|n| !after.contains_key(*n)).collect();
    let new: BTreeSet<&String> = after.keys().filter(|n| !before.contains_key(*n)).collect();
    // What sat at the corner l3 shared with l4 is gone; nothing else.
    for n in &gone {
        assert!(
            mentions(&before[*n], "l3") && mentions(&before[*n], "l4"),
            "{}",
            before[*n]
        );
    }
    assert_eq!(gone.len(), 3, "a lateral edge and its two vertices");
    // Everything new touches l6.
    for n in &new {
        assert!(mentions(&after[*n], "l6"), "{}", after[*n]);
    }
    // l3's own side and cap edges keep their names.
    let side = |m: &BTreeMap<String, Value>| {
        m.iter()
            .find(|(_, o)| **o == json!({"sketch": "s", "entity": "l3"}))
            .map(|(n, _)| n.clone())
    };
    assert_eq!(side(&before), side(&after));
    // Deleting it again retires exactly what it made.
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "remove", "id": "l3"}, {"op": "remove", "id": "l6"}, {"op": "remove", "id": "p5"},
            {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "type": "horizontal", "a": "l3"}
        ]}),
    );
    assert_eq!(names(&mut s, "e"), before);
}

#[test]
fn a_curve_that_sorts_first_keeps_the_caps() {
    let (_dir, mut s) = project(false, &[extrude()]);
    let caps = |m: &BTreeMap<String, Value>| -> Vec<String> {
        m.iter()
            .filter(|(_, o)| o.get("cap").is_some() && o.get("loop").is_some())
            .map(|(n, _)| n.clone())
            .collect()
    };
    let before = caps(&names(&mut s, "e"));
    assert_eq!(before.len(), 2);
    // The top becomes an arc a1, whose ID sorts before every line's, about
    // a new centre point.
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "remove", "id": "l3"},
            {"op": "add", "type": "point", "at": [10, 10]},
            {"op": "add", "id": "a1", "type": "arc", "center": "p5", "start": "p3", "end": "p4"}
        ]}),
    );
    let after = names(&mut s, "e");
    assert_eq!(caps(&after), before);
    assert!(after.values().any(|o| o["loop"] == "p1"));
}

#[test]
fn swept_bodies_are_sound() {
    let (_dir, mut s) = project(
        false,
        &[
            extrude(),
            revolve(json!(90)),
            json!({"type": "revolve", "id": "full", "sketch": "s", "axis": "l4", "angle": "full"}),
            json!({"type": "extrude", "id": "both", "sketch": "s",
                   "extent": {"type": "symmetric", "length": "3 mm"}, "reverse": true}),
        ],
    );
    let part = ryucad::doc::id::Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    assert_eq!(r.failed(), 0);
    for (id, o) in &r.features {
        let Some(b) = o.as_ref().unwrap().body() else {
            continue;
        };
        assert!(ryukei::check::check(&b.body).is_empty(), "{id}");
        assert!(
            ryukei::check::check_intersections(&b.body).is_empty(),
            "{id}"
        );
    }
}

#[test]
fn thread_count_doesnt_change_sweeps() {
    let (_dir, s) = project(false, &[extrude(), revolve(json!(90))]);
    let part = s
        .project()
        .part(&ryucad::doc::id::Id::parse("p", "part").unwrap())
        .unwrap()
        .clone();
    let run = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| ryucad::rebuild::rebuild_part(&part))
    };
    assert_eq!(run(1).hash, run(8).hash);
}
