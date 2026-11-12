//! Paths and helices through the protocol, against closed forms: a
//! rounded rim's tangent chain as one closed path that follows the box
//! when it is resized, keys and all; sketch curves across two sketches
//! chained end to end; a helix's ends and length; datums along and normal
//! to them; and the refusals.

mod common;

use common::{call, must};
use core::f64::consts::{PI, TAU};
use ryucad::doc::id::Id;
use ryucad::rebuild::Output;
use ryucad::session::Session;
use serde_json::{Value, json};

fn add(s: &mut Session, f: Value) -> Value {
    let mut f = f;
    if f.get("part").is_none() {
        f["part"] = json!("p");
    }
    must(s, "feature.add", f)
}

fn id(s: &str) -> Id {
    Id::parse(s, "feature").unwrap()
}

/// A project with part `p`: a box `b` 20 × 10 × 5 mm from the origin.
fn boxed() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
    );
    (dir, s)
}

/// The box's edge from the vertex at `a` to the one at `b`, by name.
fn edge(s: &mut Session, a: [f64; 3], b: [f64; 3]) -> String {
    edge_after(s, "b", a, b)
}

/// The box's edge from `a` to `b` as feature `f` leaves it, by name.
fn edge_after(s: &mut Session, f: &str, a: [f64; 3], b: [f64; 3]) -> String {
    let v = must(s, "inspect", json!({"part": "p", "feature": f}));
    let body = &v["bodies"][0];
    let at = |p: [f64; 3]| -> String {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            // Within resolution: a tangent touch lands a few picometres off.
            .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-8))
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .unwrap()
    };
    let (a, b) = (at(a), at(b));
    let e = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .unwrap();
    format!("p/b/{}", e["name"].as_str().unwrap())
}

/// Feature `f` of part `p` as rebuild reports it, failed or not.
fn reported(s: &mut Session, f: &str) -> Value {
    let r = match call(s, "rebuild", json!({"part": "p"})) {
        Ok(r) => r,
        Err(e) => e.details,
    };
    r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == f)
        .unwrap_or_else(|| panic!("{r}"))
        .clone()
}

fn made(s: &mut Session, f: &str) -> ryucad::path::Made {
    let r = s.rebuilt(&id("p")).unwrap();
    match &r.features.iter().find(|(x, _)| *x == id(f)).unwrap().1 {
        Ok(Output::Path(m)) => m.as_ref().clone(),
        o => panic!("{o:?}"),
    }
}

fn near(v: &Value, want: [f64; 3], tol: f64) {
    for (k, w) in want.iter().enumerate() {
        assert!(
            (v[k].as_f64().unwrap() - w).abs() < tol,
            "{v} against {want:?}"
        );
    }
}

#[test]
fn a_rounded_rim_is_one_closed_path_that_follows_its_box() {
    let (_dir, mut s) = boxed();
    let uprights: Vec<String> = [(0.0, 0.0), (0.02, 0.0), (0.02, 0.01), (0.0, 0.01)]
        .iter()
        .map(|(x, y)| edge(&mut s, [*x, *y, 0.0], [*x, *y, 0.005]))
        .collect();
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": "2 mm", "edges": uprights}),
    );
    let front = edge(&mut s, [0.0, 0.0, 0.005], [0.02, 0.0, 0.005]);
    add(
        &mut s,
        json!({"type": "path", "id": "rim", "segments": [{"curve": front, "propagate": true}]}),
    );
    let rim = reported(&mut s, "rim")["path"].clone();
    assert_eq!(rim["closed"], true, "{rim}");
    let length = |l: f64| 2.0 * (l - 0.004) + 2.0 * 0.006 + TAU * 0.002;
    assert!((rim["length_m"].as_f64().unwrap() - length(0.02)).abs() < 1e-12);
    // A plane normal to it where it starts, sketched on: its normal is the
    // rim's tangent there, along the front edge.
    add(
        &mut s,
        json!({"type": "plane", "id": "n", "by": "normal_to", "curve": "rim", "at": "start"}),
    );
    let n = reported(&mut s, "n")["datum"]["normal"].clone();
    assert!(n[0].as_f64().unwrap().abs() > 1.0 - 1e-12, "{n}");
    add(&mut s, json!({"type": "sketch", "id": "s", "plane": "n"}));
    assert_eq!(reported(&mut s, "s")["ok"], true);
    // A point halfway round, by length.
    add(
        &mut s,
        json!({"type": "point", "id": "half", "by": "along", "curve": "rim", "from": "start",
               "fraction": 0.5}),
    );
    assert_eq!(reported(&mut s, "half")["ok"], true);
    // A plane square to it where it starts and closes crosses it there
    // once and across the back once: the closing joint counts once.
    let x = rim["segments"][0]["start_m"][0].as_f64().unwrap();
    add(
        &mut s,
        json!({"type": "plane", "id": "across", "by": "offset", "from": "yz",
               "distance": format!("{} mm", (x * 1000.0).round())}),
    );
    add(
        &mut s,
        json!({"type": "point", "id": "cross", "by": "intersection", "between": ["rim", "across"]}),
    );
    let r = reported(&mut s, "cross");
    assert_eq!(r["error"]["code"], "datum_ambiguous", "{r}");
    assert_eq!(r["error"]["details"]["count"], 2, "{r}");
    // The box made longer: the path follows, its keys the same.
    let keys = |m: &ryucad::path::Made| m.pieces.iter().map(|p| p.key).collect::<Vec<_>>();
    let before = made(&mut s, "rim");
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"x": 30}}),
    );
    let after = made(&mut s, "rim");
    assert_eq!(keys(&before), keys(&after));
    assert!((after.length.value() - length(0.03)).abs() < 1e-12);
}

#[test]
fn sketch_curves_across_two_sketches_chain_end_to_end() {
    let (_dir, mut s) = boxed();
    // A line along x on xy to (10, 0, 0) ...
    add(&mut s, json!({"type": "sketch", "id": "s1", "plane": "xy"}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s1", "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p2", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"}
        ]}),
    );
    // ... and on zx (sketch x along z, y along x) a quarter arc that ends
    // there heading back along -x, so it starts there along +x when run
    // the other way.
    add(&mut s, json!({"type": "sketch", "id": "s2", "plane": "zx"}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s2", "ops": [
            {"op": "add", "id": "c", "type": "point", "at": [5, 10]},
            {"op": "add", "id": "a", "type": "point", "at": [5, 15]},
            {"op": "add", "id": "z", "type": "point", "at": [0, 10]},
            {"op": "add", "id": "a1", "type": "arc", "center": "c", "start": "a", "end": "z"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "path", "id": "w", "segments": ["p/s1/l1", "p/s2/a1"]}),
    );
    let w = reported(&mut s, "w")["path"].clone();
    assert_eq!(w["segments"][1]["reversed"], true, "{w}");
    near(&w["segments"][1]["end_m"], [0.015, 0.0, 0.005], 1e-15);
    let want = 0.01 + PI * 0.005 / 2.0;
    assert!((w["length_m"].as_f64().unwrap() - want).abs() < 1e-12);
    // The arc's key is its sketch's own.
    let m = made(&mut s, "w");
    assert_eq!(
        m.pieces[1].key.0,
        ryucad::profile::entity_key(&id("p"), &id("s2"), &id("a1"))
    );
    assert!(!m.closed);
}

#[test]
fn a_helix_meets_its_closed_forms() {
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "helix", "id": "h", "axis": "z", "radius": "8 mm", "pitch": "2 mm",
               "turns": 10}),
    );
    let h = reported(&mut s, "h")["helix"].clone();
    near(&h["segments"][0]["start_m"], [0.008, 0.0, 0.0], 1e-15);
    near(&h["segments"][0]["end_m"], [0.008, 0.0, 0.02], 1e-15);
    let (round, rise) = (TAU * 0.008, 0.002f64);
    let want = 10.0 * (round * round + rise * rise).sqrt();
    let bound = h["length_bound_m"].as_f64().unwrap_or(0.0);
    assert!((h["length_m"].as_f64().unwrap() - want).abs() < 1e-6 + bound);
    // A quarter of the way along by length is a quarter of the height up.
    add(
        &mut s,
        json!({"type": "point", "id": "q", "by": "along", "curve": "h", "from": "start",
               "fraction": 0.25}),
    );
    let q = reported(&mut s, "q")["datum"]["origin_m"].clone();
    near(
        &q,
        [0.008 * ryukei::math::cos(PI / 2.0 * 10.0), 0.0, 0.005],
        1e-6,
    );
    // Tapered 5°, its radius grows by the height's tangent.
    add(
        &mut s,
        json!({"type": "helix", "id": "t", "axis": "z", "radius": "8 mm", "pitch": "2 mm",
               "turns": 10, "taper": "5 deg"}),
    );
    let t = reported(&mut s, "t")["helix"].clone();
    let r = 0.008 + 0.02 * ryukei::math::tan(5f64.to_radians());
    near(&t["segments"][0]["end_m"], [r, 0.0, 0.02], 1e-12);
}

#[test]
fn refusals() {
    let (_dir, mut s) = boxed();
    add(&mut s, json!({"type": "sketch", "id": "s", "plane": "xy"}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p2", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "p3", "type": "point", "at": [10, 10]},
            {"op": "add", "id": "p4", "type": "point", "at": [30, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l3", "type": "line", "start": "p4", "end": "p1"},
            {"op": "add", "id": "ci", "type": "circle", "center": "p3", "r": 5}
        ]}),
    );
    for (id, fields, code) in [
        (
            "corner",
            json!({"segments": ["p/s/l1", "p/s/l2"]}),
            "path_not_tangent",
        ),
        (
            "gap",
            json!({"segments": ["p/s/l2", "p/s/ci"]}),
            "path_disconnected",
        ),
        (
            "twice",
            json!({"segments": ["p/s/l1", "p/s/l1"]}),
            "path_segment_twice",
        ),
        (
            "back",
            json!({"segments": ["p/s/l3", "p/s/l1"]}),
            "path_not_tangent",
        ),
    ] {
        let mut f = fields;
        f["type"] = json!("path");
        f["id"] = json!(id);
        add(&mut s, f);
        let r = reported(&mut s, id);
        assert_eq!(r["error"]["code"], code, "{id}: {r}");
    }
    // A whole circle after a line ending tangentially where the circle
    // starts, (15, 10): a closed curve among others, which ryukei refuses
    // before it judges any joint.
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "p5", "type": "point", "at": [15, 0]},
            {"op": "add", "id": "p6", "type": "point", "at": [15, 10]},
            {"op": "add", "id": "l4", "type": "line", "start": "p5", "end": "p6"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "path", "id": "whole", "segments": ["p/s/l4", "p/s/ci"]}),
    );
    let r = reported(&mut s, "whole");
    assert_eq!(r["error"]["code"], "path_invalid", "{r}");
    assert_eq!(r["error"]["details"]["segment"], "p/s/ci", "{r}");
    assert_eq!(r["error"]["details"]["piece"], "p/s/ci", "{r}");
    // A helix its numbers can't make.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "helix", "axis": "z", "radius": 8, "pitch": 2,
               "turns": 20000}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::HelixInvalid, "{e}");
    // What a path reads stays while it does, and nothing after it is read.
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "s"})).unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::HasDependents, "{e}");
    let e = call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "corner", "first": true}),
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::ReferenceForward, "{e}");
}

#[test]
fn an_edge_nearly_tangent_where_a_chain_goes_on_is_no_fork() {
    // A boss on the box's top whose foot circle touches its front edge at
    // (10, 0, 5): the join puts that vertex a picometre off, so the
    // circle leaves it about 2e-5 rad from +x, past the kernel's tangency
    // and the fork's 1e-6 rad. The front edge's chain runs straight on
    // through, both halves, and is no fork. (A real fork, `path_branches`,
    // is `path::tests::a_chain_that_goes_on_two_ways_forks`'s: no
    // primitive, sweep or boolean makes one exactly.)
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "operation": "join",
               "params": {"radius": 5, "height": 3}, "placement": {"origin": [10, 5, 5]}}),
    );
    let left = edge_after(&mut s, "boss", [0.0, 0.0, 0.005], [0.01, 0.0, 0.005]);
    let right = edge_after(&mut s, "boss", [0.01, 0.0, 0.005], [0.02, 0.0, 0.005]);
    add(
        &mut s,
        json!({"type": "path", "id": "front", "segments": [{"curve": left, "propagate": true}]}),
    );
    let r = reported(&mut s, "front");
    assert_eq!(r["ok"], true, "{r}");
    let from: Vec<&str> = r["path"]["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["from"].as_str().unwrap())
        .collect();
    assert_eq!(from, [left.as_str(), right.as_str()], "{r}");
    assert!(
        (r["path"]["length_m"].as_f64().unwrap() - 0.02).abs() < 1e-12,
        "{r}"
    );
}
