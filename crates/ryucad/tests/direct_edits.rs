//! Direct edits through the session (design 445, plans 22.A and 22.B):
//! face tweaks that change the solid's topology or remake its blends,
//! deleted faces healed by a cap or by growth, and blends listed, deleted
//! and resized, against exact volumes; names,
//! origins, references to what went, reference imports, the edit budget,
//! refusals and their hints.

mod common;

use common::{call, must};
use core::f64::consts::PI;
use ryucad::doc::id::Id;
use ryucad::doc::project::{feature_path, part_path, write_json};
use ryucad::error::Code;
use ryucad::session::Session;
use serde_json::{Value, json};

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

/// Solid `solid` in the part's model, in `inspect` (a reference import
/// shows only there).
fn model_body(s: &mut Session, solid: &str) -> Value {
    let v = must(s, "inspect", json!({"part": "p"}));
    v["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["solid"] == solid)
        .unwrap_or_else(|| panic!("no {solid} in {v}"))
        .clone()
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

/// The hole made a counterbore after the delete was written: the seed
/// still names the bore and the delete follows it, found again on the
/// rebuild. A seed's feature is the smallest a cap closes (ryukei's
/// `feature_faces`), and the counterbore's floor caps the bore, so the
/// bore goes and the counterbore (10 mm across, 3 deep) stays; seeded from
/// the counterbore's wall, the delete takes it all.
#[test]
fn a_counterbore_added_before_the_delete_is_followed_by_its_seed() {
    let (_dir, mut s) = drilled();
    let bore = cylinders(&body(&mut s, "h"), "b");
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "features": [bore[0]]}),
    );
    near(volume(&mut s, "d", "b"), 12000.0, 1e-9);
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "h", "form": "counterbore",
        "counterbore": {"diameter": 10, "depth": 3}}),
    );
    near(volume(&mut s, "d", "b"), 12000.0 - PI * 25.0 * 3.0, 1e-6);
    let left = cylinders(&body(&mut s, "d"), "b");
    assert!(
        !left.is_empty() && bore.iter().all(|f| !left.contains(f)),
        "{left:?}"
    );
    let rep = report(&mut s, "d");
    assert_eq!(
        rep["seeds"][0]["faces"].as_array().unwrap().len(),
        bore.len(),
        "{rep}"
    );
    // Seeded from the counterbore's wall: its floor and the bore go too.
    let wall = cylinders(&body(&mut s, "h"), "b")
        .into_iter()
        .find(|f| !bore.contains(f))
        .expect("the counterbore's wall");
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "d", "features": [wall]}),
    );
    near(volume(&mut s, "d", "b"), 12000.0, 1e-9);
    assert!(cylinders(&body(&mut s, "d"), "b").is_empty());
    report(&mut s, "d");
}

/// A boss (r 5, 5 high) with a 1 mm fillet round its foot, and a pocket
/// 20 × 10 × 5 mm: each deleted by one seed and capped.
#[test]
fn a_boss_with_its_foot_fillet_and_a_pocket_are_deleted_whole() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 60, "y": 30, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "j", "operation": "join",
        "params": {"radius": 5, "height": 5}, "placement": {"origin": [15, 15, 10]}}),
    );
    let joined = body(&mut s, "j");
    let foot = joined["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["curve"]["type"] == "circle"
                && (e["curve"]["frame"]["origin"][2].as_f64().unwrap() - 0.010).abs() < 1e-12
        })
        .map(|e| format!("p/b/{}", e["name"].as_str().unwrap()))
        .expect("the boss's foot");
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 1, "edges": [foot]}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "k", "operation": "cut", "targets": ["b"],
        "params": {"x": 20, "y": 10, "z": 10}, "placement": {"origin": [35, 10, 5]}}),
    );
    let k = body(&mut s, "k");
    let boss = cylinders(&k, "b")[0].clone();
    let floor = plane(&k, "b", 2, 1.0, 5.0);
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "features": [boss, floor]}),
    );
    near(volume(&mut s, "d", "b"), 60.0 * 30.0 * 10.0, 1e-6);
    let rep = report(&mut s, "d");
    // The boss's wall, top and fillet; the pocket's floor and four walls.
    assert_eq!(
        rep["seeds"][0]["faces"].as_array().unwrap().len(),
        3,
        "{rep}"
    );
    assert_eq!(
        rep["seeds"][1]["faces"].as_array().unwrap().len(),
        5,
        "{rep}"
    );
}

/// An edge's fillet deleted and grown back to the box; the edge growth
/// makes in its place says what it replaces.
#[test]
fn an_edges_fillet_grows_back_to_the_box() {
    let (_dir, mut s) = boxed();
    let rim = edge(
        &body(&mut s, "b"),
        "b",
        [40.0, 0.0, 20.0],
        [40.0, 30.0, 20.0],
    );
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 4, "edges": [rim]}),
    );
    let round = cylinders(&body(&mut s, "f"), "b");
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "faces": round, "heal": "grow"}),
    );
    near(volume(&mut s, "d", "b"), 24000.0, 1e-9);
    let b = built(&mut s, "d", "b");
    assert!(
        b.origins
            .values()
            .any(|o| o["feature"] == "d" && o.get("replaces").is_some()),
        "{:?}",
        b.origins.iter().collect::<Vec<_>>()
    );
    report(&mut s, "d");
}

/// A move of the pocket's floor reads it; a delete of the pocket moved
/// in before the move takes the floor away: the move fails, naming the
/// delete.
#[test]
fn a_reference_to_a_deleted_face_fails_naming_the_delete() {
    let (_dir, mut s) = boxed();
    add(
        &mut s,
        json!({"type": "box", "id": "k", "operation": "cut", "targets": ["b"],
        "params": {"x": 20, "y": 10, "z": 30}, "placement": {"origin": [10, 10, 15]}}),
    );
    let floor = plane(&body(&mut s, "k"), "b", 2, 1.0, 15.0);
    add(
        &mut s,
        json!({"type": "move_face", "id": "m", "by": "offset", "distance": 1,
        "faces": [floor.clone()]}),
    );
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "features": [floor]}),
    );
    must(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "d", "after": "k"}),
    );
    let e = failure(&mut s, "m");
    assert_eq!(e.code, Code::ReferenceUnresolved, "{e}");
    assert_eq!(e.details["removed_by"], "d", "{e}");
}

/// A reference import is refused, with the convert hint; converted to a
/// solid, its hole is deleted.
#[test]
fn a_reference_import_is_converted_then_its_hole_deleted() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    let bytes = std::fs::read(format!(
        "{}/../ryustep/tests/data/occt/plate_hole.stp",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let root = s.root().to_owned();
    ryucad::doc::project::write_atomic(&root.join("imports/imp.step"), &bytes).unwrap();
    let file = ryucad::doc::project::ImportFile::new(bytes.to_vec());
    write_feature(
        &mut s,
        "imp",
        json!({"type": "import", "body": 0, "mode": "reference",
        "product": {"id": "part 10", "name": "part 10"},
        "file": {"blake3": file.blake3, "path": "imports/imp.step"}}),
    );
    let bore = cylinders(&model_body(&mut s, "imp"), "imp");
    assert!(!bore.is_empty());
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "delete_face", "id": "d",
        "features": [bore[0]]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceBody, "{e}");
    assert!(
        e.details["hint"]
            .as_str()
            .unwrap()
            .contains("import convert"),
        "{e}"
    );
    must(
        &mut s,
        "import.convert",
        json!({"part": "p", "feature": "imp"}),
    );
    let bore = cylinders(&model_body(&mut s, "imp"), "imp");
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "features": [bore[0]]}),
    );
    // OCCT's plate: 10 × 20 × 30 mm, its hole gone.
    near(volume(&mut s, "d", "imp"), 10.0 * 20.0 * 30.0, 1e-6);
}

#[test]
fn deletes_refuse_with_their_hints() {
    // A box's side: nothing closes the gap.
    {
        let (_dir, mut s) = boxed();
        let side = plane(&body(&mut s, "b"), "b", 0, 1.0, 40.0);
        add(
            &mut s,
            json!({"type": "delete_face", "id": "d", "faces": [side]}),
        );
        let e = failure(&mut s, "d");
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            (Code::TweakFailed, Some("gap_not_healed")),
            "{e}"
        );
        assert!(e.details["hint"].as_str().unwrap().contains("heal"), "{e}");
    }
    let (_dir, mut s) = drilled();
    let side = plane(&body(&mut s, "b"), "b", 0, 1.0, 40.0);
    // A seed on no feature.
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "seeds": [side.clone()]}),
    );
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "d", "faces": [], "seeds": [side]}),
    );
    let e = failure(&mut s, "d");
    assert_eq!(e.details["reason"], "no_feature", "{e}");
    // A hole's bore grown: a cap closes it, not growth.
    let bore = cylinders(&body(&mut s, "h"), "b");
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "d", "seeds": [],
        "faces": bore, "heal": "grow"}),
    );
    let e = failure(&mut s, "d");
    assert_eq!(e.details["reason"], "grow_would_cap", "{e}");
    assert!(
        e.details["hint"].as_str().unwrap().contains("heal: cap"),
        "{e}"
    );
}

#[test]
fn deletes_build_alike_on_one_thread_and_eight() {
    let (_dir, mut s) = drilled();
    let bore = cylinders(&body(&mut s, "h"), "b");
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "features": [bore[0]]}),
    );
    let part = Id::parse("p", "part").unwrap();
    let p = s.project().part(&part).unwrap().clone();
    let on = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| ryucad::rebuild::rebuild_part(&p))
    };
    assert_eq!(on(1).hash, on(8).hash);
}

/// A drilled plate's side deleted: like a box's, nothing closes the gap,
/// so a typed refusal, `gap_not_healed` or `heal_ambiguous` (ryukei's
/// whole-body delete seeds every face it keeps since ryukei 16.C, #692;
/// it had built the hole's plug).
#[test]
fn a_drilled_plates_side_is_not_healed() {
    let (_dir, mut s) = drilled();
    let side = plane(&body(&mut s, "b"), "b", 0, 1.0, 40.0);
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "faces": [side]}),
    );
    let e = failure(&mut s, "d");
    assert!(
        matches!(
            e.details["reason"].as_str(),
            Some("gap_not_healed" | "heal_ambiguous")
        ),
        "{e}"
    );
}

/// The same with a 6 mm cylinder cut through a plate 40 × 30 × 10 mm,
/// whose plug ryukei had built inside out: ryucad's guard refused it
/// `inside_out`. Since ryukei 16.C (#692) the kernel refuses the delete
/// itself, typed, before the guard sees a body; the guard stays for any
/// other inside-out result.
#[test]
fn a_cut_plates_side_is_refused_by_the_kernel() {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "h", "operation": "cut", "targets": ["b"],
        "params": {"radius": 3, "height": 20}, "placement": {"origin": [20, 15, -5]}}),
    );
    let side = plane(&body(&mut s, "b"), "b", 0, 1.0, 40.0);
    add(
        &mut s,
        json!({"type": "delete_face", "id": "d", "faces": [side]}),
    );
    let e = failure(&mut s, "d");
    assert_eq!(e.code, Code::TweakFailed, "{e}");
    assert!(
        matches!(
            e.details["reason"].as_str(),
            Some("gap_not_healed" | "heal_ambiguous")
        ),
        "{e}"
    );
}

/// Every edge of the box `b` filleted at `r` mm, in one fillet `f`: a
/// rounded box, its twelve fillets meeting in eight ball corners.
fn rounded(s: &mut Session, r: f64) {
    let edges: Vec<String> = body(s, "b")["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| format!("p/b/{}", e["name"].as_str().unwrap()))
        .collect();
    assert_eq!(edges.len(), 12);
    add(
        s,
        json!({"type": "fillet", "id": "f", "radius": r, "edges": edges}),
    );
}

/// A rounded box defeatured by size: every fillet at most 3.5 mm goes,
/// the corners with them, and the box is a box again, its faces' names
/// the box's. Below 2 mm nothing goes: the same bits.
#[test]
fn a_rounded_box_is_defeatured_by_size() {
    let (_dir, mut s) = boxed();
    rounded(&mut s, 3.0);
    add(
        &mut s,
        json!({"type": "delete_blend", "id": "d", "below": "3.5 mm"}),
    );
    near(volume(&mut s, "d", "b"), 40.0 * 30.0 * 20.0, 1e-9);
    let rep = report(&mut s, "d");
    assert_eq!(rep["blends"].as_array().unwrap().len(), 12, "{rep}");
    assert_eq!(rep["corners"].as_array().unwrap().len(), 8, "{rep}");
    assert_eq!(rep["blends"][0]["kind"], "fillet", "{rep}");
    // inspect --feature and feature.show give it as a tweak's: fields as
    // text, each blend it took with its references and sizes.
    let v = must(&mut s, "inspect", json!({"part": "p", "feature": "d"}));
    let d = &v["delete_blend"];
    assert_eq!(d["below"]["value"], "3.5 mm", "{v}");
    assert_eq!(d["blends"].as_array().unwrap().len(), 12, "{v}");
    assert_eq!(d["blends"][0]["radius"]["value"], "3 mm", "{v}");
    assert!(
        d["blends"][0]["faces"][0]
            .as_str()
            .unwrap()
            .starts_with("p/b/"),
        "{v}"
    );
    assert_eq!(d["corners"].as_array().unwrap().len(), 8, "{v}");
    assert_eq!(d["deleted"]["faces"].as_u64(), Some(20), "{v}");
    let shown = must(&mut s, "feature.show", json!({"part": "p", "id": "d"}));
    assert_eq!(shown["below"]["m"], 0.0035, "{shown}");
    // The box's six faces are back under their own names.
    let faces: Vec<String> = body(&mut s, "d")["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .collect();
    let mut was: Vec<String> = body(&mut s, "b")["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .collect();
    let mut faces = faces;
    faces.sort();
    was.sort();
    assert_eq!(faces, was);
    // Nothing at most 2 mm: nothing goes.
    let kept = built(&mut s, "f", "b").body.dump();
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "d", "below": "2 mm"}),
    );
    assert_eq!(built(&mut s, "d", "b").body.dump(), kept);
    assert_eq!(report(&mut s, "d")["blends"], json!([]));
}

/// A fillet named by its face goes, its chamfer neighbour stays: the
/// box less the chamfer's 2 × 2 / 2 × 30 mm³.
#[test]
fn a_named_fillet_is_deleted_leaving_the_chamfer() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let rim = edge(&b, "b", [40.0, 0.0, 20.0], [40.0, 30.0, 20.0]);
    let lip = edge(&b, "b", [0.0, 0.0, 20.0], [0.0, 30.0, 20.0]);
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 4, "edges": [rim]}),
    );
    add(
        &mut s,
        json!({"type": "chamfer", "id": "c", "d": 2, "edges": [lip]}),
    );
    let round = cylinders(&body(&mut s, "c"), "b");
    add(
        &mut s,
        json!({"type": "delete_blend", "id": "d", "faces": [round[0]]}),
    );
    near(volume(&mut s, "d", "b"), 24000.0 - 60.0, 1e-9);
    let rep = report(&mut s, "d");
    assert_eq!(rep["blends"].as_array().unwrap().len(), 1, "{rep}");
    // A grown edge in the fillet's place is the delete's, replacing it.
    let b = built(&mut s, "d", "b");
    assert!(
        b.origins
            .values()
            .any(|o| o["feature"] == "d" && o.get("replaces").is_some()),
        "{:?}",
        b.origins.iter().collect::<Vec<_>>()
    );
}

/// A fillet resized from 4 to 3 mm: the box less (1 − π/4) × 9 × 30 mm³,
/// every name kept and the fillet's faces still the fillet's. A hole on
/// the top, the fillet's neighbour, sketched after the resize, survives
/// the resize's every size.
#[test]
fn a_fillet_resized_keeps_its_names() {
    let (_dir, mut s) = boxed();
    let rim = edge(
        &body(&mut s, "b"),
        "b",
        [40.0, 0.0, 20.0],
        [40.0, 30.0, 20.0],
    );
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 4, "edges": [rim]}),
    );
    let round = cylinders(&body(&mut s, "f"), "b");
    add(
        &mut s,
        json!({"type": "resize_blend", "id": "r",
               "groups": [{"radius": "3 mm", "faces": [round[0]]}]}),
    );
    near(
        volume(&mut s, "r", "b"),
        24000.0 - (1.0 - PI / 4.0) * 9.0 * 30.0,
        1e-5,
    );
    assert_eq!(names(&mut s, "r"), names(&mut s, "f"));
    let b = built(&mut s, "r", "b");
    let name = round[0].rsplit('/').next().unwrap();
    assert_eq!(b.origins.get(name).unwrap()["feature"], "f");
    let rep = report(&mut s, "r");
    assert_eq!(rep["resized"][0]["was"]["radius"], 4e-3, "{rep}");
    assert_eq!(rep["resized"][0]["now"]["radius"], 3e-3, "{rep}");
    let v = must(&mut s, "inspect", json!({"part": "p", "feature": "r"}));
    let r = &v["resize_blend"];
    assert_eq!(r["groups"][0]["faces"][0], json!(round[0]), "{v}");
    assert_eq!(r["resized"][0]["was"]["radius"]["value"], "4 mm", "{v}");
    assert_eq!(r["resized"][0]["now"]["radius"]["value"], "3 mm", "{v}");
    assert_eq!(r["created"]["faces"].as_u64(), Some(0), "{v}");
    // A hole in the top, after the resize.
    let top = plane(&body(&mut s, "r"), "b", 2, 1.0, 20.0);
    add(&mut s, json!({"type": "sketch", "id": "s", "face": top}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
        {"op": "add", "type": "point", "at": [10, 15]}, {"op": "add", "type": "fix", "a": "p1"}]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "s", "points": ["p1"],
        "diameter": "4 mm", "depth": "through_all", "targets": ["b"]}),
    );
    for r in ["2 mm", "5 mm", "3 mm"] {
        must(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "r", "groups": [{"radius": r, "faces": [round[0]]}]}),
        );
        let v = must(&mut s, "rebuild", json!({"part": "p"}));
        assert_eq!(v["parts"][0]["failed"], 0, "{r}: {v}");
    }
}

/// A chamfer resized from 2 to 3 mm (the box less 4.5 × 30 mm³), then to
/// 1 mm on the top and 3 mm on the side (1.5 × 30 mm³); a fillet given a
/// chamfer's size (8 × 30 mm³).
#[test]
fn chamfers_resize_and_a_fillet_becomes_one() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let lip = edge(&b, "b", [0.0, 0.0, 20.0], [0.0, 30.0, 20.0]);
    add(
        &mut s,
        json!({"type": "chamfer", "id": "c", "d": 2, "edges": [lip]}),
    );
    let bevel = body(&mut s, "c")["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let z = &f["surface"]["frame"]["z"];
            f["surface"]["type"] == "plane"
                && z[0].as_f64().unwrap() != 0.0
                && z[2].as_f64().unwrap() != 0.0
        })
        .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
        .unwrap();
    add(
        &mut s,
        json!({"type": "resize_blend", "id": "r",
               "groups": [{"form": "equal", "d": "3 mm", "faces": [bevel.clone()]}]}),
    );
    near(volume(&mut s, "r", "b"), 24000.0 - 4.5 * 30.0, 1e-9);
    let top = plane(&body(&mut s, "c"), "b", 2, 1.0, 20.0);
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "r", "groups": [{"form": "distances", "d1": "1 mm",
               "d2": "3 mm", "face": top, "faces": [bevel]}]}),
    );
    near(volume(&mut s, "r", "b"), 24000.0 - 1.5 * 30.0, 1e-9);
    // A fillet made a 4 mm chamfer.
    let (_dir2, mut t) = boxed();
    let rim = edge(
        &body(&mut t, "b"),
        "b",
        [40.0, 0.0, 20.0],
        [40.0, 30.0, 20.0],
    );
    add(
        &mut t,
        json!({"type": "fillet", "id": "f", "radius": 4, "edges": [rim]}),
    );
    let round = cylinders(&body(&mut t, "f"), "b");
    add(
        &mut t,
        json!({"type": "resize_blend", "id": "r",
               "groups": [{"form": "equal", "d": "4 mm", "faces": [round[0]]}]}),
    );
    near(volume(&mut t, "r", "b"), 24000.0 - 8.0 * 30.0, 1e-9);
    let rep = report(&mut t, "r");
    assert_eq!(rep["resized"][0]["now"]["kind"], "chamfer", "{rep}");
}

/// `blends.get` lists the rounded box's twelve fillets and eight corners,
/// with sizes and references, and with `below` exactly what a
/// `delete_blend --below` takes (its report's blends); a second ask comes
/// from the cache; a bad tolerance and a reference import are refused.
#[test]
fn the_blends_query_lists_what_delete_blend_takes() {
    let (_dir, mut s) = boxed();
    rounded(&mut s, 3.0);
    // A 5 mm fillet on a second box: kept by below 3.5 mm.
    add(
        &mut s,
        json!({"type": "box", "id": "c", "params": {"x": 40, "y": 30, "z": 20},
               "placement": {"origin": [60, 0, 0]}}),
    );
    let rim = edge(
        &body(&mut s, "c"),
        "c",
        [100.0, 0.0, 20.0],
        [100.0, 30.0, 20.0],
    );
    add(
        &mut s,
        json!({"type": "fillet", "id": "g", "radius": 5, "edges": [rim]}),
    );
    let all = must(&mut s, "blends.get", json!({"part": "p", "solid": "b"}));
    assert_eq!(all["blends"].as_array().unwrap().len(), 12, "{all}");
    assert_eq!(all["corners"].as_array().unwrap().len(), 8, "{all}");
    let b0 = &all["blends"][0];
    assert_eq!(b0["kind"], "fillet");
    assert_eq!(b0["size"]["value"], "3 mm");
    assert!(b0["faces"][0].as_str().unwrap().starts_with("p/b/"));
    assert_eq!(b0["supports"].as_array().unwrap().len(), 2);
    assert_eq!(all["tolerance"]["m"], 1e-5);
    // Two solids: one must be named.
    let e = call(&mut s, "blends.get", json!({"part": "p"})).unwrap_err();
    assert_eq!(e.details["param"], "solid", "{e}");
    let c = must(
        &mut s,
        "blends.get",
        json!({"part": "p", "solid": "c", "below": "3.5 mm"}),
    );
    assert_eq!(c["blends"], json!([]), "{c}");
    // Below, as the delete takes it.
    let listed = must(
        &mut s,
        "blends.get",
        json!({"part": "p", "solid": "b", "below": "3.5 mm"}),
    );
    add(
        &mut s,
        json!({"type": "delete_blend", "id": "d", "target": "b", "below": "3.5 mm"}),
    );
    let rep = must(&mut s, "inspect", json!({"part": "p", "feature": "d"}))["delete_blend"].clone();
    let faces = |v: &Value| {
        let mut f: Vec<String> = v["blends"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|b| b["faces"].as_array().unwrap().clone())
            .map(|f| f.as_str().unwrap().to_owned())
            .collect();
        f.sort();
        f
    };
    assert_eq!(faces(&listed), faces(&rep));
    // Cached by body and tolerance: the delete shows b anew (one more
    // body recognised), and asking again recognises nothing.
    let n = s.blends.len();
    must(&mut s, "blends.get", json!({"part": "p", "solid": "b"}));
    assert_eq!(s.blends.len(), n + 1);
    must(&mut s, "blends.get", json!({"part": "p", "solid": "b"}));
    assert_eq!(s.blends.len(), n + 1);
    // A tolerance outside the kernel's range is its refusal.
    let e = call(
        &mut s,
        "blends.get",
        json!({"part": "p", "solid": "b", "tolerance": "1 mm"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::TweakFailed, Some("bad_tolerance")),
        "{e}"
    );
}

/// A boss on a plate with its foot filleted and a filleted pocket beside
/// it, round its whole rim: deleting every blend at most 1.5 mm leaves
/// the sharp boss and
/// pocket: the plate 40 × 30 × 10, the boss π × 4² × 6, the pocket
/// 10 × 6 × 4 mm out of it.
#[test]
fn a_bosss_foot_and_a_pockets_rim_are_defeatured() {
    let (_dir, mut s) = boxed();
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"x": 40, "y": 30, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "operation": "join", "targets": ["b"],
               "params": {"radius": 4, "height": 6}, "placement": {"origin": [12, 15, 10]}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "k", "operation": "cut", "targets": ["b"],
               "params": {"x": 10, "y": 6, "z": 4}, "placement": {"origin": [25, 12, 6]}}),
    );
    let j = body(&mut s, "k");
    let foot = j["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["curve"]["type"] == "circle"
                && (e["curve"]["frame"]["origin"][2].as_f64().unwrap() - 0.010).abs() < 1e-12
        })
        .map(|e| format!("p/b/{}", e["name"].as_str().unwrap()))
        .expect("the boss's foot");
    // The pocket's whole rim: one of its edges alone runs off the top at
    // its ends (the kernel's overflow).
    let rim = [
        edge(&j, "b", [25.0, 12.0, 10.0], [35.0, 12.0, 10.0]),
        edge(&j, "b", [35.0, 12.0, 10.0], [35.0, 18.0, 10.0]),
        edge(&j, "b", [35.0, 18.0, 10.0], [25.0, 18.0, 10.0]),
        edge(&j, "b", [25.0, 18.0, 10.0], [25.0, 12.0, 10.0]),
    ];
    let mut edges = vec![foot];
    edges.extend(rim);
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 1, "edges": edges}),
    );
    add(
        &mut s,
        json!({"type": "delete_blend", "id": "d", "below": "1.5 mm"}),
    );
    near(
        volume(&mut s, "d", "b"),
        40.0 * 30.0 * 10.0 + PI * 16.0 * 6.0 - 10.0 * 6.0 * 4.0,
        1e-6,
    );
    report(&mut s, "d");
}

/// A sketch on a chamfer's face reads it; a delete of the chamfer moved
/// in before the sketch takes the face away: the sketch fails, naming the
/// delete.
#[test]
fn a_sketch_on_a_deleted_chamfer_fails_naming_the_delete() {
    let (_dir, mut s) = boxed();
    let lip = edge(&body(&mut s, "b"), "b", [0.0, 0.0, 20.0], [0.0, 30.0, 20.0]);
    add(
        &mut s,
        json!({"type": "chamfer", "id": "c", "d": 4, "edges": [lip]}),
    );
    let bevel = body(&mut s, "c")["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let z = &f["surface"]["frame"]["z"];
            f["surface"]["type"] == "plane"
                && z[0].as_f64().unwrap() != 0.0
                && z[2].as_f64().unwrap() != 0.0
        })
        .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
        .unwrap();
    add(
        &mut s,
        json!({"type": "sketch", "id": "s", "face": bevel.clone()}),
    );
    add(
        &mut s,
        json!({"type": "delete_blend", "id": "d", "faces": [bevel]}),
    );
    must(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "d", "after": "c"}),
    );
    let e = failure(&mut s, "s");
    assert_eq!(e.code, Code::ReferenceUnresolved, "{e}");
    assert_eq!(e.details["removed_by"], "d", "{e}");
}

/// A lid mated flat on the box's top: the top is the fillet's neighbour
/// and keeps its name and plane through every size the resize gives the
/// fillet, so the lid stays where it was.
#[test]
fn a_mate_to_a_kept_face_holds_across_a_resize() {
    let (_dir, mut s) = boxed();
    let rim = edge(
        &body(&mut s, "b"),
        "b",
        [40.0, 0.0, 20.0],
        [40.0, 30.0, 20.0],
    );
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 4, "edges": [rim]}),
    );
    let round = cylinders(&body(&mut s, "f"), "b");
    add(
        &mut s,
        json!({"type": "resize_blend", "id": "r",
               "groups": [{"radius": "3 mm", "faces": [round[0]]}]}),
    );
    let top = plane(&body(&mut s, "r"), "b", 2, 1.0, 20.0);
    must(&mut s, "part.create", json!({"part": "lid"}));
    add(
        &mut s,
        json!({"part": "lid", "type": "box", "id": "c", "params": {"x": 10, "y": 10, "z": 2}}),
    );
    let under = plane(
        &must(&mut s, "inspect", json!({"part": "lid", "feature": "c"}))["bodies"][0],
        "c",
        2,
        -1.0,
        0.0,
    )
    .replacen("p/", "", 1);
    must(
        &mut s,
        "part.create",
        json!({"part": "asm", "kind": "assembly"}),
    );
    add(
        &mut s,
        json!({"part": "asm", "type": "instance", "id": "base", "of": "p", "fixed": true}),
    );
    add(
        &mut s,
        json!({"part": "asm", "type": "instance", "id": "top", "of": "lid"}),
    );
    add(
        &mut s,
        json!({"part": "asm", "type": "mate", "id": "on", "form": "planar",
               "a": format!("asm/base/{}", top.replacen("p/", "", 1)),
               "b": format!("asm/top/{under}")}),
    );
    let lid_z = |s: &mut Session| -> f64 {
        let r = must(s, "rebuild", json!({"part": "asm"}));
        let asm = r["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["part"] == "asm")
            .unwrap()
            .clone();
        let i = asm["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "top")
            .unwrap()
            .clone();
        i["instance"]["pose"]["origin_m"][2].as_f64().unwrap()
    };
    let z0 = lid_z(&mut s);
    assert!((z0 - 0.020).abs() < 1e-9, "{z0}");
    for r in ["2 mm", "5 mm"] {
        must(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "r", "groups": [{"radius": r, "faces": [round[0]]}]}),
        );
        assert!((lid_z(&mut s) - z0).abs() < 1e-12, "{r}");
    }
}

#[test]
fn blend_edits_refuse_with_their_hints() {
    let (_dir, mut s) = boxed();
    let b = body(&mut s, "b");
    let top = plane(&b, "b", 2, 1.0, 20.0);
    // A face of no blend.
    add(
        &mut s,
        json!({"type": "delete_blend", "id": "d", "faces": [top.clone()]}),
    );
    let e = failure(&mut s, "d");
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::TweakFailed, Some("not_a_blend")),
        "{e}"
    );
    assert!(
        e.details["hint"]
            .as_str()
            .unwrap()
            .contains("ryucad blends"),
        "{e}"
    );
    must(&mut s, "feature.delete", json!({"part": "p", "id": "d"}));
    // A fillet too large for its box: lost.
    let rim = edge(&b, "b", [40.0, 0.0, 20.0], [40.0, 30.0, 20.0]);
    add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": 4, "edges": [rim]}),
    );
    let round = cylinders(&body(&mut s, "f"), "b");
    add(
        &mut s,
        json!({"type": "resize_blend", "id": "r",
               "groups": [{"radius": "25 mm", "faces": [round[0]]}]}),
    );
    let e = failure(&mut s, "r");
    assert_eq!(e.details["reason"], "blend_lost", "{e}");
    assert!(
        e.details["hint"].as_str().unwrap().contains("delete_blend"),
        "{e}"
    );
    // Two faces of one blend, or here one face, in two groups: refused
    // when written.
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "r", "groups": [
            {"radius": "3 mm", "faces": [round[0]]}, {"radius": "2 mm", "faces": [round[0]]}]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
    // A tolerance out of range: refused when written.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "delete_blend", "id": "t", "below": "1 mm",
               "tolerance": "1 mm"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
}

#[test]
fn blend_edits_build_alike_on_one_thread_and_eight() {
    let (_dir, mut s) = boxed();
    rounded(&mut s, 3.0);
    add(
        &mut s,
        json!({"type": "delete_blend", "id": "d", "below": "3.5 mm"}),
    );
    let part = Id::parse("p", "part").unwrap();
    let p = s.project().part(&part).unwrap().clone();
    let on = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| ryucad::rebuild::rebuild_part(&p))
    };
    assert_eq!(on(1).hash, on(8).hash);
}

/// The rounded box (3 mm fillets on every edge) written to STEP in
/// millimetres, in `dir`.
fn rounded_step(dir: &std::path::Path) -> std::path::PathBuf {
    let (_d, mut s) = boxed();
    rounded(&mut s, 3.0);
    let out = dir.join("rounded.step");
    must(
        &mut s,
        "export",
        json!({"part": "p", "out": out.to_str().unwrap(), "unit": "mm",
               "time_stamp": "2000-01-01T00:00:00"}),
    );
    out
}

/// Imported as a solid it defeatures to the box. Imported as a
/// reference, nothing may edit it: converted, it does.
#[test]
fn a_rounded_box_through_step_is_defeatured() {
    let files = tempfile::tempdir().unwrap();
    let step = rounded_step(files.path());
    for solid in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        ryucad::server::init(dir.path(), None).unwrap();
        let mut s = Session::open(dir.path()).unwrap();
        let v = must(
            &mut s,
            "import",
            json!({"path": step.to_str().unwrap(), "part": "p", "solid": solid}),
        );
        let imp = v["parts"][0]["features"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        if solid {
            let listed = must(&mut s, "blends.get", json!({"part": "p"}));
            assert_eq!(listed["blends"].as_array().unwrap().len(), 12, "{listed}");
        } else {
            assert_eq!(
                call(&mut s, "blends.get", json!({"part": "p"}))
                    .unwrap_err()
                    .code,
                Code::ReferenceBody
            );
            let e = call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "delete_blend", "id": "d", "below": "3.5 mm"}),
            )
            .unwrap_err();
            assert_eq!(e.code, Code::ReferenceBody, "{e}");
            assert!(
                e.details["hint"]
                    .as_str()
                    .unwrap()
                    .contains("import convert"),
                "{e}"
            );
            must(
                &mut s,
                "import.convert",
                json!({"part": "p", "feature": imp}),
            );
        }
        add(
            &mut s,
            json!({"type": "delete_blend", "id": "d", "below": "3.5 mm"}),
        );
        near(volume(&mut s, "d", &imp), 40.0 * 30.0 * 20.0, 1e-6);
    }
}

/// The committed NIST corpus (ryustep's `tests/corpus`), each file
/// imported as solids, each solid's blends at most 2 mm deleted under the
/// default edit budget: every outcome is a body that passes the kernel's
/// checks, or a typed refusal (`tweak_failed` but for `kernel`, or
/// `edit_budget_exceeded`), never a panic or an untyped error. Prints the
/// counts for the area doc and the spent units for the budget (Task 23).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: edits run to the edit budget"
)]
fn the_committed_corpus_defeatures_or_refuses_typed() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryustep/tests/corpus");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&corpus)
        .unwrap()
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension()? == "stp").then_some(p)
        })
        .collect();
    files.sort();
    assert_eq!(files.len(), 10, "the committed corpus: {files:?}");
    let (mut built_n, mut none) = (0usize, 0usize);
    let mut refused: std::collections::BTreeMap<String, usize> = Default::default();
    for f in &files {
        let dir = tempfile::tempdir().unwrap();
        ryucad::server::init(dir.path(), None).unwrap();
        let mut s = Session::open(dir.path()).unwrap();
        let v = must(
            &mut s,
            "import",
            json!({"path": f.to_str().unwrap(), "solid": true}),
        );
        for part in v["parts"].as_array().unwrap() {
            if part["kind"] != "part" {
                continue;
            }
            let pid = part["part"].as_str().unwrap().to_owned();
            let bodies = must(&mut s, "inspect", json!({"part": pid}))["bodies"].clone();
            for (k, b) in bodies.as_array().into_iter().flatten().enumerate() {
                let solid = b["solid"].as_str().unwrap().to_owned();
                let listed = must(
                    &mut s,
                    "blends.get",
                    json!({"part": pid, "solid": solid, "below": "2 mm"}),
                );
                if listed["blends"].as_array().is_none_or(Vec::is_empty) {
                    none += 1;
                    continue;
                }
                let id = format!("defeature{k}");
                must(
                    &mut s,
                    "feature.add",
                    json!({"part": pid, "type": "delete_blend",
                    "id": id, "target": solid, "below": "2 mm"}),
                );
                let p = Id::parse(&pid, "part").unwrap();
                let r = s.rebuilt(&p).unwrap();
                let (_, out) = r.features.iter().find(|(i, _)| i.as_str() == id).unwrap();
                match out {
                    Ok(o) => {
                        for (_, b) in o.solids() {
                            assert!(
                                ryukei::check::check(&b.body).is_empty(),
                                "{f:?} {pid}/{solid}"
                            );
                            assert_eq!(
                                ryukei::check::check_intersections(&b.body),
                                [],
                                "{f:?} {pid}/{solid}"
                            );
                        }
                        if let ryucad::rebuild::Output::Solids(m) = o
                            && let Some(rep) = &m.report
                        {
                            println!(
                                "edit spent corpus {}/{pid}/{solid}: {}",
                                f.display(),
                                rep["spent"]
                            );
                        }
                        built_n += 1;
                    }
                    Err(e) => {
                        let reason = match e.code {
                            Code::TweakFailed => {
                                e.details["reason"].as_str().unwrap_or("").to_owned()
                            }
                            Code::EditBudgetExceeded => "edit_budget_exceeded".to_owned(),
                            c => panic!("{f:?} {pid}/{solid}: untyped {c:?}: {e}"),
                        };
                        assert!(
                            reason != "kernel" && !reason.is_empty(),
                            "{f:?} {pid}/{solid}: {e}"
                        );
                        *refused.entry(reason).or_default() += 1;
                    }
                }
            }
        }
    }
    println!("corpus defeature: built {built_n}, nothing below 2 mm {none}, refused {refused:?}");
}
