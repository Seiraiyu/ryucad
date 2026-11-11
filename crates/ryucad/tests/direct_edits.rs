//! Direct edits through the session (design 445, plan 22.A): face tweaks
//! that change the solid's topology or remake its blends, and deleted
//! faces healed by a cap or by growth, against exact volumes; names,
//! origins, references to what went, reference imports, the edit budget,
//! refusals and their hints.

use core::f64::consts::PI;
use ryucad::doc::id::Id;
use ryucad::doc::project::{feature_path, part_path, write_json};
use ryucad::error::Code;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
}

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    call(s, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn add(s: &mut Session, f: Value) -> Value {
    let mut f = f;
    if f.get("part").is_none() {
        f["part"] = json!("p");
    }
    must(s, "feature.add", f)
}

/// Writes feature `feature` of part `p` as `v` (a feature file's fields,
/// header added at the edit format), listed last.
fn write_feature(s: &mut Session, feature: &str, v: Value) {
    let mut v = v;
    v["format"] = json!(ryucad::doc::edit::FORMAT);
    v["kind"] = json!("ryucad.feature");
    let root = s.root().to_owned();
    let p = Id::parse("p", "part").unwrap();
    let fid = Id::parse(feature, "feature").unwrap();
    write_json(&feature_path(&root, &p, &fid), &v).unwrap();
    let text = std::fs::read_to_string(part_path(&root, &p)).unwrap();
    let mut list: Value = serde_json::from_str(&text).unwrap();
    let features = list["features"].as_array_mut().unwrap();
    if !features.iter().any(|f| f == feature) {
        features.push(json!(feature));
    }
    write_json(&part_path(&root, &p), &list).unwrap();
    s.reload().unwrap();
}

/// A reference `p/<solid>/<name>` as a feature file holds it.
fn file_ref(r: &str) -> Value {
    let mut it = r.split('/').skip(1);
    json!({"feature": it.next().unwrap(), "name": it.next().unwrap()})
}

/// A project with part `p` and a box `b` 40 × 30 × 20 mm from the origin.
fn boxed() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 20}}),
    );
    (dir, s)
}

/// What feature `f` leaves, in `inspect`.
fn body(s: &mut Session, f: &str) -> Value {
    must(s, "inspect", json!({"part": "p", "feature": f}))["bodies"][0].clone()
}

/// The planar face of `body` whose outward normal along `axis` is `n`, at
/// `at` mm along it, as a reference into `solid`.
fn plane(body: &Value, solid: &str, axis: usize, n: f64, at: f64) -> String {
    planes(body, solid, axis, n, at)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no face facing {n} at {at} mm along {axis}"))
}

/// Every such face (a split face is several).
fn planes(body: &Value, solid: &str, axis: usize, n: f64, at: f64) -> Vec<String> {
    body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][axis].as_f64().unwrap() * sign == n
                && (f["surface"]["frame"]["origin"][axis].as_f64().unwrap() - at * 1e-3).abs()
                    < 1e-9
        })
        .map(|f| format!("p/{solid}/{}", f["name"].as_str().unwrap()))
        .collect()
}

/// The faces of `body` on cylinders, as references into `solid`.
fn cylinders(body: &Value, solid: &str) -> Vec<String> {
    body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["surface"]["type"] == "cylinder")
        .map(|f| format!("p/{solid}/{}", f["name"].as_str().unwrap()))
        .collect()
}

/// The name of the vertex of `body` at `p` mm.
fn vertex(body: &Value, p: [f64; 3]) -> String {
    body["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k] * 1e-3).abs() < 1e-9))
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| panic!("no vertex at {p:?}"))
}

/// The reference to the edge of `solid` from `a` to `b` (mm).
fn edge(body: &Value, solid: &str, a: [f64; 3], b: [f64; 3]) -> String {
    let (a, b) = (vertex(body, a), vertex(body, b));
    let e = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .expect("an edge from a to b");
    format!("p/{solid}/{}", e["name"].as_str().unwrap())
}

/// Every name of feature `f`'s body, sorted.
fn names(s: &mut Session, f: &str) -> Vec<String> {
    let b = body(s, f);
    let mut n: Vec<String> = ["faces", "edges", "vertices"]
        .iter()
        .flat_map(|k| b[*k].as_array().unwrap().clone())
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect();
    n.sort();
    n
}

/// Solid `solid` as feature `f` left it, built.
fn built(s: &mut Session, f: &str, solid: &str) -> ryucad::rebuild::Built {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed");
    let out = out.as_ref().unwrap_or_else(|e| panic!("{f}: {e}"));
    out.solid(&Id::parse(solid, "feature").unwrap())
        .expect("the solid")
        .clone()
}

/// The volume in mm³ of `solid` as feature `f` left it, after the
/// kernel's checks, meshed finely.
fn volume(s: &mut Session, f: &str, solid: &str) -> f64 {
    let b = built(s, f, solid);
    assert!(ryukei::check::check(&b.body).is_empty(), "{f}");
    assert_eq!(ryukei::check::check_intersections(&b.body), [], "{f}");
    let m = ryukei::mesh::tessellate(&b.body, &ryukei::mesh::MeshParams::new(2e-7, 0.05)).unwrap();
    assert!(m.faults.is_empty(), "{f}: {:?}", m.faults);
    ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9
}

/// Feature `f`'s refusal at rebuild.
fn failure(s: &mut Session, f: &str) -> ryucad::error::Error {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed");
    out.as_ref()
        .err()
        .unwrap_or_else(|| panic!("{f} built"))
        .clone()
}

/// Feature `f`'s kept report (`Made::report`), and that it spent no more
/// than a quarter of the default edit budget, printed for Task 20.
fn report(s: &mut Session, f: &str) -> Value {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed");
    let Ok(ryucad::rebuild::Output::Solids(m)) = out else {
        panic!("{f} made no solids")
    };
    let rep = m.report.as_ref().expect("a report").as_ref().clone();
    let spent = rep["spent"].as_u64().expect("spent");
    println!("edit spent {f}: {spent}");
    assert!(
        4 * spent <= ryucad::tweak::DEFAULT_EDIT_BUDGET,
        "{f} spent {spent}"
    );
    rep
}

fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want,
        "{got} mm³ against {want} mm³"
    );
}

/// The box's −x wall drafted 1.3 rad about its bottom meets its +x wall
/// at 40 / tan 1.3 mm high, so the top goes and the box is a wedge.
#[test]
fn a_wall_drafted_past_its_neighbour_is_a_wedge() {
    let (_dir, mut s) = boxed();
    let wall = plane(&body(&mut s, "b"), "b", 0, -1.0, 0.0);
    let draft = json!({"type": "draft", "target": "b", "angle": "1.3 rad", "neutral": "xy",
                       "faces": [file_ref(&wall)]});
    // Kept topology: refused, the hint naming topology: change. ryukei
    // gives `uncertain` here (at the top corner the wall reaches past), in
    // metres as in millimetres, so that hint covers an uncertain refusal
    // of a tweak that keeps its topology too.
    let mut kept = draft.clone();
    kept["format"] = json!(ryucad::doc::tweak::DRAFT_FORMAT);
    write_feature(&mut s, "d", kept);
    let e = failure(&mut s, "d");
    assert_eq!(e.code, Code::TweakFailed, "{e}");
    assert!(
        e.details["hint"]
            .as_str()
            .unwrap()
            .contains("topology: change"),
        "{e}"
    );
    // Changed: a wedge, its top gone.
    let mut changed = draft;
    changed["topology"] = json!("change");
    write_feature(&mut s, "d", changed);
    let h = 40.0 / ryukei::math::tan(1.3);
    near(volume(&mut s, "d", "b"), 30.0 * 0.5 * 40.0 * h, 1e-6);
    assert!(planes(&body(&mut s, "d"), "b", 2, 1.0, 20.0).is_empty());
    report(&mut s, "d");
    // inspect --feature counts what went: the top and its edges and
    // vertices; the report gives the units spent.
    let v = must(&mut s, "inspect", json!({"part": "p", "feature": "d"}));
    let d = &v["draft"];
    assert_eq!(d["deleted"]["faces"], 1, "{v}");
    assert!(d["deleted"]["edges"].as_u64().unwrap() >= 4, "{v}");
    assert!(
        d["deleted"]["references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r
                .as_str()
                .unwrap()
                .ends_with(&plane(&body(&mut s, "b"), "b", 2, 1.0, 20.0)[4..])),
        "{v}"
    );
    assert!(d["spent"].as_u64().unwrap() > 0, "{v}");
    // The rebuild report carries no stray import report for it.
    let r = must(&mut s, "rebuild", json!({"part": "p"}));
    let f = r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "d")
        .unwrap()
        .clone();
    assert!(f.get("import").is_none(), "{f}");
}
