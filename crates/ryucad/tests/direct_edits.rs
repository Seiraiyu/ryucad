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

/// A plate 40 × 40 × 10 mm with a boss (r 4, 6 high) at its middle, slid
/// 18 mm along −x: 2 mm of it overhangs. Kept topology refuses; changed,
/// the boss stays whole.
#[test]
fn a_boss_slid_over_its_plates_edge() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 40, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "j", "operation": "join",
        "params": {"radius": 4, "height": 6}, "placement": {"origin": [20, 20, 10]}}),
    );
    let j = body(&mut s, "j");
    let mut boss = cylinders(&j, "b");
    boss.push(plane(&j, "b", 2, 1.0, 16.0));
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "x",
        "distance": -18, "faces": boss, "tangent": false}),
    );
    let e = failure(&mut s, "m");
    assert_eq!(e.code, Code::TweakFailed, "{e}");
    assert!(
        e.details["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("topology"),
        "{e}"
    );
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "m", "topology": "change"}),
    );
    near(
        volume(&mut s, "m", "b"),
        40.0 * 40.0 * 10.0 + PI * 16.0 * 6.0,
        1e-4,
    );
    // The plate's −x side is still there, and the boss's faces keep
    // their names.
    plane(&body(&mut s, "m"), "b", 0, -1.0, 0.0);
    let after = names(&mut s, "m");
    for f in &boss {
        assert!(after.contains(&f[4..].to_owned()), "{f}");
    }
    report(&mut s, "m");
}

/// A plate 40 × 40 × 8 mm with a groove 4 wide and 4 deep along y from 8
/// to 32 mm; both its end walls offset 12 mm into the plate run it
/// through, splitting the top in two. Both pieces keep the top's origin.
#[test]
fn a_groove_run_through_splits_the_top_keeping_its_origin() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 40, "z": 8}}),
    );
    let top = plane(&body(&mut s, "b"), "b", 2, 1.0, 8.0);
    let was = built(&mut s, "b", "b")
        .origins
        .get(&top[4..])
        .cloned()
        .unwrap();
    add(
        &mut s,
        json!({"type": "box", "id": "g", "operation": "cut", "targets": ["b"],
        "params": {"x": 4, "y": 24, "z": 10}, "placement": {"origin": [18, 8, 4]}}),
    );
    let g = body(&mut s, "g");
    let ends = vec![plane(&g, "b", 1, 1.0, 8.0), plane(&g, "b", 1, -1.0, 32.0)];
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "offset", "distance": -12,
        "faces": ends, "topology": "change"}),
    );
    near(
        volume(&mut s, "m", "b"),
        40.0 * 40.0 * 8.0 - 4.0 * 40.0 * 4.0,
        1e-6,
    );
    let tops = planes(&body(&mut s, "m"), "b", 2, 1.0, 8.0);
    assert_eq!(tops.len(), 2, "{tops:?}");
    let after = built(&mut s, "m", "b");
    for t in &tops {
        assert_eq!(after.origins.get(&t[4..]), Some(&was), "{t}");
    }
    let v = must(&mut s, "inspect", json!({"part": "p", "feature": "m"}));
    // The top's two pieces are new. ryukei's re-trim also splits the
    // faces it only re-trims (the four sides) and merges the pieces again,
    // so those come back under new names too (in metres as here, the
    // kernel's naming gap); each new face still has an origin from before
    // the edit, none the move's own.
    let created: Vec<String> = v["move_face"]["created"]["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_owned())
        .collect();
    for t in &tops {
        assert!(created.contains(t), "{t} in {v}");
    }
    let faces: Vec<String> = body(&mut s, "m")["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .collect();
    for r in created
        .iter()
        .filter(|r| faces.contains(&r[4..].to_owned()))
    {
        let o = after.origins.get(&r[4..]).unwrap();
        assert_ne!(o["feature"], "m", "{r}: {o}");
    }
    report(&mut s, "m");
}

/// A box's top raised 2 mm past its fillet (r 4 along the top's +x edge),
/// the fillet not moving with it: refused, unless blends: rebuild makes
/// it again; every name kept.
#[test]
fn a_top_raised_past_its_fillet_makes_it_again() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let rim = edge(&b, "b", [40.0, 0.0, 20.0], [40.0, 30.0, 20.0]);
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 4, "edges": [rim]}),
    );
    let top = plane(&body(&mut s, "f"), "b", 2, 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "z",
        "distance": 2, "faces": [top], "tangent": false}),
    );
    let e = failure(&mut s, "m");
    assert_eq!(e.details["reason"], "tangency_broken", "{e}");
    assert!(
        e.details["hint"]
            .as_str()
            .unwrap()
            .contains("blends: rebuild"),
        "{e}"
    );
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "m", "blends": "rebuild"}),
    );
    near(
        volume(&mut s, "m", "b"),
        40.0 * 30.0 * 22.0 - (1.0 - PI / 4.0) * 16.0 * 30.0,
        1e-5,
    );
    assert_eq!(names(&mut s, "m"), names(&mut s, "f"));
    report(&mut s, "m");
}

/// Every ryucad 17 tweak that builds builds the same bits with topology:
/// change: the fixed engine runs first and its result stands.
#[test]
fn a_tweak_that_builds_kept_builds_the_same_bits_changed() {
    let (_dir, mut s) = boxed();
    let top = plane(&body(&mut s, "b"), "b", 2, 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "translate", "direction": "z",
        "distance": 5, "faces": [top]}),
    );
    let kept = built(&mut s, "m", "b").body.dump();
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "m", "topology": "change"}),
    );
    assert_eq!(built(&mut s, "m", "b").body.dump(), kept);
}

/// A budget of one unit refuses the edit with its own code; the default
/// builds it again, the same bits as before (the budget keys the result,
/// so the refusal is never taken for the default's).
#[test]
fn the_edit_budget_refuses_and_keys_its_result() {
    let (_dir, mut s) = boxed();
    let wall = plane(&body(&mut s, "b"), "b", 0, -1.0, 0.0);
    add(
        &mut s,
        json!({"type": "draft", "id": "d", "angle": "1.3 rad", "neutral": "xy",
        "faces": [wall], "topology": "change"}),
    );
    let built_first = built(&mut s, "d", "b").body.dump();
    s.set_edit_budget(1);
    let e = failure(&mut s, "d");
    assert_eq!(e.code, Code::EditBudgetExceeded, "{e}");
    assert_eq!(e.details["budget"], 1);
    assert!(e.details["spent"].as_u64().unwrap() > 1, "{e}");
    s.set_edit_budget(ryucad::tweak::DEFAULT_EDIT_BUDGET);
    assert_eq!(built(&mut s, "d", "b").body.dump(), built_first);
    // A kept tweak isn't budgeted: a box of its own, its top moved.
    s.set_edit_budget(1);
    add(
        &mut s,
        json!({"type": "box", "id": "c", "params": {"x": 10, "y": 10, "z": 10},
        "placement": {"origin": [60, 0, 0]}}),
    );
    let top = plane(&body(&mut s, "c"), "c", 2, 1.0, 10.0);
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "target": "c", "by": "translate",
        "direction": "z", "distance": 1, "faces": [top]}),
    );
    built(&mut s, "m", "c");
}

/// Part `p`: a plate `b` 40 × 30 × 10 mm and a hole `h` of 6 mm through it
/// at (20, 15), sketched on its top.
fn drilled() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
    );
    let top = plane(&body(&mut s, "b"), "b", 2, 1.0, 10.0);
    add(&mut s, json!({"type": "sketch", "id": "s", "face": top}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
        {"op": "add", "type": "point", "at": [20, 15]},
        {"op": "add", "type": "fix", "a": "p1"}]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "s", "points": ["p1"],
        "diameter": 6, "depth": "through_all", "targets": ["b"]}),
    );
    (dir, s)
}

#[test]
fn a_through_hole_deleted_by_its_seed_is_capped() {
    let (_dir, mut s) = drilled();
    let bore = cylinders(&body(&mut s, "h"), "b");
    assert!(!bore.is_empty());
    write_feature(
        &mut s,
        "d",
        json!({"type": "delete_face", "target": "b",
        "seeds": [file_ref(&bore[0])]}),
    );
    near(volume(&mut s, "d", "b"), 40.0 * 30.0 * 10.0, 1e-9);
    assert!(cylinders(&body(&mut s, "d"), "b").is_empty());
    assert_eq!(body(&mut s, "d")["faces"].as_array().unwrap().len(), 6);
    // The report names what the seed found: the bore, all of it.
    let rep = report(&mut s, "d");
    let found = rep["seeds"][0]["faces"].as_array().unwrap().len();
    assert_eq!(found, bore.len(), "{rep}");
    // inspect --feature and feature.show give it as a tweak's: fields as
    // text, what each seed found, what went.
    let v = must(&mut s, "inspect", json!({"part": "p", "feature": "d"}));
    let d = &v["delete_face"];
    assert_eq!(d["seeds"][0]["seed"], json!(bore[0]), "{v}");
    assert_eq!(d["heal"], "auto", "{v}");
    assert_eq!(
        d["deleted"]["faces"].as_u64(),
        Some(bore.len() as u64),
        "{v}"
    );
    let shown = must(&mut s, "feature.show", json!({"part": "p", "id": "d"}));
    assert_eq!(shown["seeds"], json!([bore[0]]), "{shown}");
}

#[test]
fn feature_faces_shows_what_a_seed_takes() {
    let (_dir, mut s) = drilled();
    let bore = cylinders(&body(&mut s, "h"), "b");
    let v = must(&mut s, "feature_faces.get", json!({"face": bore[0]}));
    let mut got: Vec<String> = v["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect();
    got.sort();
    let mut want = bore.clone();
    want.sort();
    assert_eq!(got, want, "{v}");
    // A box's side is part of no feature.
    let side = plane(&body(&mut s, "b"), "b", 0, 1.0, 40.0);
    let e = call(&mut s, "feature_faces.get", json!({"face": side})).unwrap_err();
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::TweakFailed, Some("no_feature")),
        "{e}"
    );
    // An edge isn't a face.
    let b = body(&mut s, "b");
    let rim = edge(&b, "b", [0.0, 0.0, 10.0], [40.0, 0.0, 10.0]);
    let e = call(&mut s, "feature_faces.get", json!({"face": rim})).unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
}
