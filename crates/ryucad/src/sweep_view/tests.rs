use crate::error::Code;
use crate::session::Session;
use crate::testing::call;
use serde_json::{Value, json};

/// A project with an empty part `p`.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    crate::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    (dir, s)
}

fn add(s: &mut Session, f: Value) {
    let mut f = f;
    f["part"] = json!("p");
    call(s, "feature.add", f).unwrap();
}

/// Sketch `id` on `plane` with `ops`.
fn sketch(s: &mut Session, id: &str, plane: &str, ops: Value) {
    add(s, json!({"type": "sketch", "id": id, "plane": plane}));
    call(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": id, "ops": ops}),
    )
    .unwrap();
}

/// A square of side `a` mm centred on the origin, its corners `p1` to
/// `p4` counterclockwise from the bottom left.
fn square(a: f64) -> Value {
    let h = a / 2.0;
    json!([
        {"op": "add", "id": "p1", "type": "point", "at": [-h, -h]},
        {"op": "add", "id": "p2", "type": "point", "at": [h, -h]},
        {"op": "add", "id": "p3", "type": "point", "at": [h, h]},
        {"op": "add", "id": "p4", "type": "point", "at": [-h, h]},
        {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
        {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"},
        {"op": "add", "id": "l4", "type": "line", "start": "p4", "end": "p1"}
    ])
}

fn points(v: &Value) -> Vec<[f64; 3]> {
    serde_json::from_value(v.clone()).unwrap()
}

fn near(got: [f64; 3], want: [f64; 3], tol: f64) {
    for k in 0..3 {
        assert!((got[k] - want[k]).abs() < tol, "{got:?} against {want:?}");
    }
}

#[test]
fn a_sweep_gives_its_profile_and_its_path() {
    let (_dir, mut s) = project();
    sketch(
        &mut s,
        "s1",
        "xy",
        json!([
            {"op": "add", "id": "c", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "ci", "type": "circle", "center": "c", "r": 2}
        ]),
    );
    // On zx, x runs along z: a line from the origin 20 mm up z.
    sketch(
        &mut s,
        "s2",
        "zx",
        json!([
            {"op": "add", "id": "q1", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "q2", "type": "point", "at": [20, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "q1", "end": "q2"}
        ]),
    );
    add(
        &mut s,
        json!({"type": "sweep", "id": "w", "sketch": "s1", "path": "p/s2/l1"}),
    );
    let g = call(&mut s, "sweep.get", json!({"part": "p", "feature": "w"})).unwrap();
    assert_eq!(
        (g["type"].clone(), g["feature"].clone()),
        (json!("sweep"), json!("w"))
    );
    let revision = call(&mut s, "part.list", json!({})).unwrap()["parts"][0]["revision"].clone();
    assert_eq!(g["revision"], revision);
    let sections = g["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0]["source"], json!({"sketch": "s1"}));
    assert_eq!(sections[0]["turned"], false);
    let loops = sections[0]["loops"].as_array().unwrap();
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0]["anchor"], "ci");
    // The circle closes on itself, every point on it.
    let ring = points(&loops[0]["polyline"]);
    assert!(ring.len() > 8);
    near(ring[0], ring[ring.len() - 1], 1e-12);
    for p in &ring {
        let r = ryukei::math::hypot(p[0], p[1]);
        assert!((r - 0.002).abs() < 1e-9 && p[2].abs() < 1e-12, "{p:?}");
    }
    // The path: a line, its two ends.
    assert_eq!(g["path"]["closed"], false);
    let path = points(&g["path"]["polyline"]);
    assert_eq!(path.len(), 2);
    near(path[0], [0.0, 0.0, 0.0], 1e-12);
    near(path[1], [0.0, 0.0, 0.02], 1e-12);
    // Coarse cuts the circle into fewer pieces.
    let coarse = call(
        &mut s,
        "sweep.get",
        json!({"part": "p", "feature": "w", "quality": "coarse"}),
    )
    .unwrap();
    assert!(
        coarse["sections"][0]["loops"][0]["polyline"]
            .as_array()
            .unwrap()
            .len()
            < ring.len()
    );
}

#[test]
fn a_loft_gives_its_sections_each_from_its_start() {
    let (_dir, mut s) = project();
    sketch(&mut s, "s1", "xy", square(10.0));
    // A plane 10 mm up with its normal down: its section is turned over.
    add(
        &mut s,
        json!({"type": "plane", "id": "d", "by": "offset", "from": "xy",
               "distance": "10 mm", "flip": true}),
    );
    sketch(&mut s, "s2", "p/d", square(6.0));
    add(
        &mut s,
        json!({"type": "loft", "id": "l", "sections": ["s1", "s2"]}),
    );
    let g = call(&mut s, "sweep.get", json!({"part": "p", "feature": "l"})).unwrap();
    assert_eq!(
        (g["type"].clone(), g["closed"].clone()),
        (json!("loft"), json!(false))
    );
    let sections = g["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 2);
    assert_eq!(
        (sections[0]["turned"].clone(), sections[1]["turned"].clone()),
        (json!(false), json!(true))
    );
    near(
        points(&json!([sections[0]["centroid_m"]]))[0],
        [0.0, 0.0, 0.0],
        1e-12,
    );
    near(
        points(&json!([sections[1]["centroid_m"]]))[0],
        [0.0, 0.0, 0.01],
        1e-12,
    );
    // The first section starts at its anchor; its loop runs from there.
    let first = &sections[0]["loops"][0];
    assert_eq!(first["start"], "p1");
    near(
        points(&json!([first["start_m"]]))[0],
        [-0.005, -0.005, 0.0],
        1e-12,
    );
    let ring = points(&first["polyline"]);
    assert_eq!(ring.len(), 5);
    near(ring[0], [-0.005, -0.005, 0.0], 1e-12);
    near(ring[4], ring[0], 1e-12);
    // The second starts nearest the first's start, its centroid brought
    // over: the plane's own p1 lies at y's other end once turned, so the
    // start is still the corner at (-3, -3) in the model.
    let second = &sections[1]["loops"][0];
    near(
        points(&json!([second["start_m"]]))[0],
        [-0.003, -0.003, 0.01],
        1e-12,
    );
    let ring = points(&second["polyline"]);
    near(ring[0], [-0.003, -0.003, 0.01], 1e-12);
    // What isn't a sweep or a loft, or isn't there, refuses.
    let e = call(&mut s, "sweep.get", json!({"part": "p", "feature": "s1"})).unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    let e = call(&mut s, "sweep.get", json!({"part": "p", "feature": "nope"})).unwrap_err();
    assert_eq!(e.code, Code::NotFound, "{e}");
}
