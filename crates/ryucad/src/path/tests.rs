use super::*;
use crate::session::Session;
use crate::sketch::tests::{edge_between, face_along, vertex_at, with_box};
use crate::testing::call;
use core::f64::consts::{PI, TAU};
use serde_json::json;

fn id(s: &str) -> Id {
    Id::parse(s, "feature").unwrap()
}

fn add(s: &mut Session, fields: Value) -> Result<Value> {
    let mut f = fields;
    f["part"] = json!("p");
    call(s, "feature.add", f)
}

/// Feature `f` of part `p` as rebuild reports it.
fn reported(s: &mut Session, f: &str) -> Value {
    let r = match call(s, "rebuild", json!({"part": "p"})) {
        Ok(r) => r,
        Err(e) if e.code == Code::RebuildFailed => e.details,
        Err(e) => panic!("{e}"),
    };
    r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == f)
        .unwrap_or_else(|| panic!("{r}"))
        .clone()
}

/// What path or helix `f` of part `p` built.
fn made(s: &mut Session, f: &str) -> Made {
    let r = s.rebuilt(&id("p")).unwrap();
    match &r.features.iter().find(|(x, _)| *x == id(f)).unwrap().1 {
        Ok(Output::Path(m)) => m.as_ref().clone(),
        o => panic!("{o:?}"),
    }
}

fn near(v: &Value, want: [f64; 3], tol: f64) {
    for (i, w) in want.iter().enumerate() {
        let got = v[i].as_f64().unwrap();
        assert!((got - w).abs() < tol, "{v} against {want:?}");
    }
}

/// Sketch `s` on xy: a line along x to (10, 0), a quarter arc about
/// (10, 5) to (15, 5), and a line drawn from (15, 15) down to (15, 5);
/// and a lone line from (30, 0) to (40, 0).
fn with_curves() -> (tempfile::TempDir, Session, Value) {
    let (dir, mut s, body) = with_box();
    add(&mut s, json!({"type": "sketch", "id": "s", "plane": "xy"})).unwrap();
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p2", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "p3", "type": "point", "at": [10, 5]},
            {"op": "add", "id": "p4", "type": "point", "at": [15, 5]},
            {"op": "add", "id": "p5", "type": "point", "at": [15, 15]},
            {"op": "add", "id": "p6", "type": "point", "at": [30, 0]},
            {"op": "add", "id": "p7", "type": "point", "at": [40, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "a1", "type": "arc", "center": "p3", "start": "p2", "end": "p4"},
            {"op": "add", "id": "l2", "type": "line", "start": "p5", "end": "p4"},
            {"op": "add", "id": "l3", "type": "line", "start": "p6", "end": "p7"}
        ]}),
    )
    .unwrap();
    (dir, s, body)
}

#[test]
fn sketch_curves_chain_each_the_way_that_meets() {
    let (_dir, mut s, _) = with_curves();
    add(
        &mut s,
        json!({"type": "path", "id": "w", "segments": ["p/s/l1", "p/s/a1", "p/s/l2"]}),
    )
    .unwrap();
    let r = reported(&mut s, "w");
    assert_eq!(r["ok"], true, "{r}");
    let p = &r["path"];
    assert_eq!(p["closed"], false);
    let reversed: Vec<bool> = p["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["reversed"].as_bool().unwrap())
        .collect();
    assert_eq!(reversed, [false, false, true], "{p}");
    near(&p["segments"][2]["end_m"], [0.015, 0.015, 0.0], 1e-15);
    let want = 0.01 + PI * 0.005 / 2.0 + 0.01;
    assert!(
        (p["length_m"].as_f64().unwrap() - want).abs() < 1e-12,
        "{p}"
    );
    // Turned round, it starts at (15, 15).
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "w", "reverse": true}),
    )
    .unwrap();
    let p = reported(&mut s, "w")["path"].clone();
    near(&p["segments"][0]["start_m"], [0.015, 0.015, 0.0], 1e-15);
    assert_eq!(p["segments"][0]["from"], "p/s/l2");
    // One curve alone runs as it is drawn.
    add(
        &mut s,
        json!({"type": "path", "id": "v", "segments": ["p/s/l2"]}),
    )
    .unwrap();
    let v = reported(&mut s, "v")["path"].clone();
    near(&v["segments"][0]["start_m"], [0.015, 0.015, 0.0], 1e-15);
    assert_eq!(v["segments"][0]["reversed"], false);
}

#[test]
fn datums_go_along_and_normal_to_a_path() {
    let (_dir, mut s, _) = with_curves();
    add(
        &mut s,
        json!({"type": "path", "id": "w", "segments": ["p/s/l1", "p/s/a1", "p/s/l2"]}),
    )
    .unwrap();
    // 10 mm from the start is where the first line ends; 5 mm from the
    // end is halfway up the last, which runs the other way to its line.
    add(
        &mut s,
        json!({"type": "point", "id": "q1", "by": "along", "curve": "w", "from": "start",
               "distance": "10 mm"}),
    )
    .unwrap();
    near(
        &reported(&mut s, "q1")["datum"]["origin_m"],
        [0.01, 0.0, 0.0],
        1e-12,
    );
    add(
        &mut s,
        json!({"type": "point", "id": "q2", "by": "along", "curve": "w", "from": "end",
               "distance": "5 mm"}),
    )
    .unwrap();
    near(
        &reported(&mut s, "q2")["datum"]["origin_m"],
        [0.015, 0.01, 0.0],
        1e-12,
    );
    // Normal to it at its end: along +y, the way it runs there.
    add(
        &mut s,
        json!({"type": "plane", "id": "n1", "by": "normal_to", "curve": "w", "at": "end"}),
    )
    .unwrap();
    near(
        &reported(&mut s, "n1")["datum"]["normal"],
        [0.0, 1.0, 0.0],
        1e-12,
    );
    // Where x = 12.5 mm cuts it: once, on the arc.
    add(
        &mut s,
        json!({"type": "plane", "id": "x1", "by": "offset", "from": "yz", "distance": "12.5 mm"}),
    )
    .unwrap();
    add(
        &mut s,
        json!({"type": "point", "id": "q3", "by": "intersection", "between": ["w", "x1"]}),
    )
    .unwrap();
    let q3 = reported(&mut s, "q3")["datum"]["origin_m"].clone();
    let y = 0.005 - (0.005f64 * 0.005 - 0.0025 * 0.0025).sqrt();
    near(&q3, [0.0125, y, 0.0], 1e-12);
}

#[test]
fn a_rounded_rims_tangent_chain_is_one_closed_path() {
    let (_dir, mut s, body) = with_box();
    let corner = |x: f64, y: f64| {
        let (a, b) = (
            vertex_at(&body, [x, y, 0.0]),
            vertex_at(&body, [x, y, 0.005]),
        );
        format!("p/b/{}", edge_between(&body, &a, &b))
    };
    let edges: Vec<String> = [(0.0, 0.0), (0.02, 0.0), (0.02, 0.01), (0.0, 0.01)]
        .iter()
        .map(|(x, y)| corner(*x, *y))
        .collect();
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": "2 mm", "edges": edges}),
    )
    .unwrap();
    let front = {
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.005]),
            vertex_at(&body, [0.02, 0.0, 0.005]),
        );
        format!("p/b/{}", edge_between(&body, &a, &b))
    };
    add(
        &mut s,
        json!({"type": "path", "id": "rim",
               "segments": [{"curve": front, "propagate": true}]}),
    )
    .unwrap();
    let r = reported(&mut s, "rim");
    let p = &r["path"];
    assert_eq!(p["closed"], true, "{r}");
    assert_eq!(p["segments"].as_array().unwrap().len(), 8);
    let want = 2.0 * 0.016 + 2.0 * 0.006 + TAU * 0.002;
    let got = p["length_m"].as_f64().unwrap();
    assert!((got - want).abs() < 1e-12, "{got} against {want}");
    // Its keys come from the edges' names.
    let m = made(&mut s, "rim");
    for piece in &m.pieces {
        let From::Edge { name, .. } = &piece.from else {
            panic!()
        };
        assert_eq!(piece.key, edge_key(&id("p"), *name));
    }
    assert!(m.kernel().check().unwrap());
}

#[test]
fn a_helix_is_one_segment_of_its_length() {
    let (_dir, mut s, _) = with_box();
    add(
        &mut s,
        json!({"type": "helix", "id": "h", "axis": "z", "radius": "8 mm", "pitch": "2 mm",
               "turns": 10}),
    )
    .unwrap();
    let r = reported(&mut s, "h");
    let h = &r["helix"];
    let (round, rise) = (TAU * 0.008, 0.002f64);
    let want = 10.0 * (round * round + rise * rise).sqrt();
    let got = h["length_m"].as_f64().unwrap();
    let bound = h["length_bound_m"].as_f64().unwrap_or(0.0);
    // Within the NURBS's deviation's worth of length, and its bound.
    assert!((got - want).abs() < 1e-6 + bound, "{got} against {want}");
    assert_eq!(h["helix"]["height_m"], 0.02);
    assert_eq!(h["helix"]["hand"], "right");
    assert!(h["helix"]["deviation_m"].as_f64().unwrap() <= HELIX_TOLERANCE);
    near(&h["segments"][0]["start_m"], [0.008, 0.0, 0.0], 1e-15);
    near(&h["segments"][0]["end_m"], [0.008, 0.0, 0.02], 1e-15);
    // Its axis is an axis; it meets a plane across it once.
    add(
        &mut s,
        json!({"type": "axis", "id": "ax", "by": "of", "axis": "h"}),
    )
    .unwrap();
    near(
        &reported(&mut s, "ax")["datum"]["direction"],
        [0.0, 0.0, 1.0],
        1e-15,
    );
    add(
        &mut s,
        json!({"type": "plane", "id": "mid", "by": "offset", "from": "xy", "distance": "10 mm"}),
    )
    .unwrap();
    add(
        &mut s,
        json!({"type": "point", "id": "q", "by": "intersection", "between": ["h", "mid"]}),
    )
    .unwrap();
    near(
        &reported(&mut s, "q")["datum"]["origin_m"],
        [0.008, 0.0, 0.01],
        1e-9,
    );
    // From a start point, left handed and tapered: a quarter turn on it is
    // at -y.
    add(
        &mut s,
        json!({"type": "point", "id": "st", "by": "coordinates",
               "coordinates": ["5 mm", "0", "0"]}),
    )
    .unwrap();
    add(
        &mut s,
        json!({"type": "helix", "id": "h2", "axis": "z", "start": "st", "pitch": "4 mm",
               "height": "1 mm", "hand": "left", "taper": "10 deg"}),
    )
    .unwrap();
    let h2 = reported(&mut s, "h2")["helix"].clone();
    assert_eq!(h2["helix"]["turns"], 0.25, "{h2}");
    let r = 0.005 + 0.001 * ryukei::math::tan(10f64.to_radians());
    let end = &h2["segments"][0]["end_m"];
    assert!(end[0].as_f64().unwrap().abs() < 1e-12, "{end}");
    assert!((end[1].as_f64().unwrap() + r).abs() < 1e-12, "{end}");
    // A helix in a path keeps its own key.
    add(
        &mut s,
        json!({"type": "path", "id": "w", "segments": ["h"]}),
    )
    .unwrap();
    assert_eq!(
        made(&mut s, "w").pieces[0].key,
        helix_key(&id("p"), &id("h"))
    );
}

#[test]
fn keys_follow_ids_not_shapes() {
    let (_dir, mut s, _) = with_curves();
    add(
        &mut s,
        json!({"type": "path", "id": "w", "segments": ["p/s/l1", "p/s/a1", "p/s/l2"]}),
    )
    .unwrap();
    let before = made(&mut s, "w");
    let keys = |m: &Made| m.pieces.iter().map(|p| p.key).collect::<Vec<_>>();
    assert_eq!(
        keys(&before)[0],
        SegmentKey(crate::profile::entity_key(&id("p"), &id("s"), &id("l1")))
    );
    // The last line made longer: new geometry, the same keys.
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "move", "id": "p5", "at": [15, 25]}
        ]}),
    )
    .unwrap();
    let after = made(&mut s, "w");
    assert_ne!(before.hash, after.hash);
    assert_eq!(keys(&before), keys(&after));
}

/// The code a feature's rebuild fails with.
fn fails(s: &mut Session, f: &str) -> Value {
    let r = reported(s, f);
    assert_eq!(r["ok"], false, "{r}");
    r["error"].clone()
}

#[test]
fn refusals_name_what_wont_do() {
    let (_dir, mut s, body) = with_curves();
    // Two lines that don't meet.
    add(
        &mut s,
        json!({"type": "path", "id": "gap", "segments": ["p/s/l1", "p/s/l3"]}),
    )
    .unwrap();
    let e = fails(&mut s, "gap");
    assert_eq!(e["code"], "path_disconnected", "{e}");
    assert_eq!(e["details"]["segments"], json!(["p/s/l1", "p/s/l3"]));
    assert!((e["details"]["gap_m"].as_f64().unwrap() - 0.02).abs() < 1e-15);
    // Two of the box's top edges, at a corner.
    let (a, b, c) = (
        vertex_at(&body, [0.0, 0.0, 0.005]),
        vertex_at(&body, [0.02, 0.0, 0.005]),
        vertex_at(&body, [0.02, 0.01, 0.005]),
    );
    let (e1, e2) = (
        format!("p/b/{}", edge_between(&body, &a, &b)),
        format!("p/b/{}", edge_between(&body, &b, &c)),
    );
    add(
        &mut s,
        json!({"type": "path", "id": "corner", "segments": [e1, e2]}),
    )
    .unwrap();
    let e = fails(&mut s, "corner");
    assert_eq!(e["code"], "path_not_tangent", "{e}");
    let angle = e["details"]["angle_rad"].as_f64().unwrap();
    assert!((angle - PI / 2.0).abs() < 1e-12, "{e}");
    // One line twice.
    add(
        &mut s,
        json!({"type": "path", "id": "twice", "segments": ["p/s/l1", "p/s/l1"]}),
    )
    .unwrap();
    assert_eq!(fails(&mut s, "twice")["code"], "path_segment_twice");
    // A helix whose taper narrows it to nothing.
    add(
        &mut s,
        json!({"type": "helix", "id": "cone", "axis": "z", "radius": "8 mm", "pitch": "2 mm",
               "turns": 10, "taper": "-60 deg"}),
    )
    .unwrap();
    let e = fails(&mut s, "cone");
    assert_eq!(e["code"], "helix_invalid");
    assert_eq!(e["details"]["pointer"], "/taper");
    // A helix starting on its axis.
    add(
        &mut s,
        json!({"type": "helix", "id": "on", "axis": "z", "start": "origin", "pitch": "2 mm",
               "turns": 1}),
    )
    .unwrap();
    assert_eq!(fails(&mut s, "on")["details"]["pointer"], "/start");
    // What isn't a curve is refused when written.
    let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
    for (bad, got) in [
        (top.as_str(), "face"),
        ("p/s", "sketch"),
        ("p/s/p1", "sketch point"),
    ] {
        let e = add(&mut s, json!({"type": "path", "segments": [bad]})).unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{bad}: {e}");
        assert_eq!(e.details["got"], got, "{bad}: {e}");
    }
    let e = add(&mut s, json!({"type": "path", "segments": ["xy"]})).unwrap_err();
    assert_eq!(e.code, Code::InvalidDocument, "{e}");
    // A helix takes two of pitch, turns and height.
    let e = add(
        &mut s,
        json!({"type": "helix", "axis": "z", "radius": 1, "pitch": 1, "turns": 1, "height": 1}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
}

#[test]
fn editing_a_helix_keeps_two_of_its_sizes() {
    let (_dir, mut s, _) = with_box();
    add(
        &mut s,
        json!({"type": "helix", "id": "h", "axis": "z", "radius": "8 mm", "pitch": "2 mm",
               "turns": 10}),
    )
    .unwrap();
    // A height keeps the pitch; the turns follow.
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "h", "height": "30 mm"}),
    )
    .unwrap();
    let show = call(&mut s, "feature.show", json!({"part": "p", "id": "h"})).unwrap();
    assert_eq!(show["pitch"]["value"], "2 mm", "{show}");
    assert_eq!(show["height"]["value"], "30 mm", "{show}");
    assert!(show.get("turns").is_none(), "{show}");
    assert_eq!(reported(&mut s, "h")["helix"]["helix"]["turns"], 15.0);
    // Left handed, then right again (written as nothing).
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "h", "hand": "left"}),
    )
    .unwrap();
    assert_eq!(reported(&mut s, "h")["helix"]["helix"]["hand"], "left");
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "h", "hand": "right"}),
    )
    .unwrap();
    let show = call(&mut s, "feature.show", json!({"part": "p", "id": "h"})).unwrap();
    assert!(show.get("hand").is_none(), "{show}");
}

/// A wire of three edges from the origin: one along +x, and a line and
/// an arc both leaving it along -x. The first's chain forks there.
#[test]
fn a_chain_that_goes_on_two_ways_forks() {
    use ryukei::euler::{Corner, mev, mvfs};
    use ryukei::geom::{Circle, Line as KLine, Plane as KPlane, Surface};
    let mut b = ryukei::topo::Body::new();
    let plane = b.add_surface(Surface::Plane(KPlane::new(Frame::WORLD)));
    let (m, _) = mvfs(&mut b, OpKey(1), Point3::ORIGIN, plane).unwrap();
    let x = Vector3::new(1.0, 0.0, 0.0);
    let ahead = b.add_curve(Curve::Line(KLine::new(Point3::ORIGIN, x).unwrap()));
    let (e1, _) = mev(
        &mut b,
        OpKey(2),
        Corner::Lone(m.loop_),
        Point3::new(1.0, 0.0, 0.0),
        ahead,
        [0.0, 1.0],
    )
    .unwrap();
    let back = b.add_curve(Curve::Line(KLine::new(Point3::ORIGIN, -x).unwrap()));
    let (e2, _) = mev(
        &mut b,
        OpKey(3),
        Corner::Before(e1.fins[0]),
        Point3::new(-1.0, 0.0, 0.0),
        back,
        [0.0, 1.0],
    )
    .unwrap();
    let f = Frame::new(
        Point3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, -1.0),
        Vector3::new(0.0, -1.0, 0.0),
    )
    .unwrap();
    let arc = Curve::Circle(Circle::new(f, 1.0).unwrap());
    let to = arc.point(1.0);
    let arc = b.add_curve(arc);
    let (e3, _) = mev(
        &mut b,
        OpKey(4),
        Corner::Before(e1.fins[0]),
        to,
        arc,
        [0.0, 1.0],
    )
    .unwrap();
    let mut fork = forks(&b, &[e1.edge]);
    fork.sort();
    let mut want = vec![e2.edge, e3.edge];
    want.sort();
    assert_eq!(fork, want);
    // The line back along -x goes on only into the first: no fork.
    assert!(forks(&b, &[e2.edge]).is_empty());
    // The refusal names the segment as given and each edge going on, as
    // references to the solid the segment's edge is of. No primitive,
    // sweep or boolean gives a tangent fork within `FORK` (a boss whose
    // foot touches an edge lands 2e-5 rad off: tests/paths.rs), so this
    // wire stands in for the solid.
    let name = |e: EdgeId| b.edge(e).unwrap().name;
    let segment = GeomRef::Entity(crate::doc::reference::Reference {
        feature: id("w"),
        name: name(e1.edge),
    });
    let e = branches(&id("path"), &id("p"), &segment, &id("w"), &b, &fork);
    assert_eq!(e.code, Code::PathBranches);
    assert_eq!(e.details["feature"], "path");
    assert_eq!(e.details["segment"], format!("p/w/{}", name(e1.edge)));
    let want: Vec<String> = fork.iter().map(|g| format!("p/w/{}", name(*g))).collect();
    assert_eq!(e.details["edges"], json!(want));
}

#[test]
fn path_get_gives_a_helix_as_the_app_draws_it() {
    let (_dir, mut s, _) = with_box();
    add(
        &mut s,
        json!({"type": "helix", "id": "h", "axis": "z", "radius": "8 mm", "pitch": "2 mm",
               "turns": 3}),
    )
    .unwrap();
    // At normal quality, named: the preference is shared by every test in
    // the process, and another may set it while this one runs.
    let g = call(
        &mut s,
        "path.get",
        json!({"part": "p", "path": "h", "quality": "normal"}),
    )
    .unwrap();
    assert_eq!(g["type"], "helix");
    assert_eq!(g["closed"], false);
    let revision = call(&mut s, "part.list", json!({})).unwrap()["parts"][0]["revision"].clone();
    assert_eq!(g["revision"], revision);
    let points: Vec<[f64; 3]> = serde_json::from_value(g["polyline"].clone()).unwrap();
    // From its start to its end, every point on the helix's cylinder, and
    // each step short: well under a millimetre at normal quality.
    near(&json!(points[0]), [0.008, 0.0, 0.0], 1e-15);
    near(&json!(points[points.len() - 1]), [0.008, 0.0, 0.006], 1e-12);
    for p in &points {
        let r = ryukei::math::hypot(p[0], p[1]);
        assert!((r - 0.008).abs() < 1e-6, "{r}");
    }
    for w in points.windows(2) {
        let d = [0, 1, 2].map(|k| w[1][k] - w[0][k]);
        assert!(ryukei::math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) < 0.002);
    }
    // Coarse is fewer points; what isn't a path, or isn't there, refuses.
    let coarse = call(
        &mut s,
        "path.get",
        json!({"part": "p", "path": "h", "quality": "coarse"}),
    )
    .unwrap();
    assert!(coarse["polyline"].as_array().unwrap().len() < points.len());
    let e = call(&mut s, "path.get", json!({"part": "p", "path": "b"})).unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    let e = call(&mut s, "path.get", json!({"part": "p", "path": "nope"})).unwrap_err();
    assert_eq!(e.code, Code::NotFound, "{e}");
}

#[test]
fn one_curve_is_keyed_as_a_path_of_it_would_be() {
    let (_dir, mut s, body) = with_curves();
    let front = {
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.0]),
            vertex_at(&body, [0.02, 0.0, 0.0]),
        );
        format!("p/b/{}", edge_between(&body, &a, &b))
    };
    add(
        &mut s,
        json!({"type": "path", "id": "w", "segments": [front.clone()]}),
    )
    .unwrap();
    add(
        &mut s,
        json!({"type": "path", "id": "v", "segments": ["p/s/a1"]}),
    )
    .unwrap();
    let (w, v) = (made(&mut s, "w"), made(&mut s, "v"));
    let pid = Id::parse("p", "part").unwrap();
    let part = s.project().part(&pid).unwrap().clone();
    let results = s.rebuilt(&pid).unwrap();
    let order: Vec<Id> = part.order().iter().cloned().chain([id("x")]).collect();
    let at = At {
        part: &part,
        order: &order,
        owner: &id("x"),
        results,
    };
    let g = |t: &str| GeomRef::parse(t, &pid).unwrap();
    // An edge, a sketch's arc and a path: what a path of each holds.
    for (text, want) in [(front.as_str(), &w), ("p/s/a1", &v), ("p/w", &w)] {
        let m = of_curve(&at, "path", &g(text)).unwrap();
        assert_eq!(m.pieces, want.pieces, "{text}");
        assert_eq!(m.hash, want.hash, "{text}");
    }
    // A sketch is no curve; a face neither.
    let e = of_curve(&at, "path", &g("p/s")).unwrap_err();
    assert_eq!(
        (e.code, &e.details["role"]),
        (Code::ReferenceWrongKind, &json!("path")),
        "{e}"
    );
    let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
    let e = of_curve(&at, "path", &g(&top)).unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
}
