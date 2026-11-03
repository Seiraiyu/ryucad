//! Whole-solid mirrors against exact volumes, through the protocol: half
//! a bracket mirrored and joined into the whole one, as one solid; a copy
//! as a solid of its own with its source's names; mirrors about a datum
//! plane and about the solid's own face; a cut by a mirrored copy; the
//! copy following an edit of its source; refusals. Every result passes
//! the kernel's checks, the self-intersection check among them.

use ryucad::doc::id::Id;
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
    f["part"] = json!("p");
    must(s, "feature.add", f)
}

/// A project with part `p`, in millimetres.
fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    (dir, s)
}

/// What feature `f` leaves, in `inspect`.
fn body(s: &mut Session, f: &str) -> Value {
    must(s, "inspect", json!({"part": "p", "feature": f}))["bodies"][0].clone()
}

/// The volume in mm³ of solid `solid` as feature `f` left it, after the
/// kernel's checks, meshed finely.
fn volume(s: &mut Session, f: &str, solid: &str) -> f64 {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed");
    let out = out.as_ref().unwrap_or_else(|e| panic!("{f}: {e}"));
    let b = &out
        .solid(&Id::parse(solid, "feature").unwrap())
        .expect("the solid")
        .body;
    assert!(ryukei::check::check(b).is_empty(), "{f}");
    assert_eq!(ryukei::check::check_intersections(b), [], "{f}");
    let m = ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(2e-7, 0.05)).unwrap();
    assert!(m.faults.is_empty(), "{f}: {:?}", m.faults);
    ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9
}

/// `got` within a part in `rel` of `want`.
fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want,
        "{got} mm³ against {want} mm³"
    );
}

/// Why feature `f` failed in the part's rebuild.
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

/// The planar face of `body` whose outward normal along `axis` is `n`, at
/// `at` mm along it, as a reference into `solid`.
fn plane(body: &Value, solid: &str, axis: usize, n: f64, at: f64) -> String {
    let f = body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][axis].as_f64().unwrap() * sign == n
                && (f["surface"]["frame"]["origin"][axis].as_f64().unwrap() - at * 1e-3).abs()
                    < 1e-9
        })
        .unwrap_or_else(|| panic!("no face facing {n} at {at} mm along {axis}"));
    format!("p/{solid}/{}", f["name"].as_str().unwrap())
}

/// Half a bracket: a base 20 × 10 × 5 mm and an upright 5 × 10 × 15 mm on
/// it at x 0, joined, 1,750 mm³, its middle plane x = 0.
fn half(s: &mut Session) {
    add(
        s,
        json!({"type": "box", "id": "half", "params": {"x": 20, "y": 10, "z": 5}}),
    );
    add(
        s,
        json!({"type": "box", "id": "up", "operation": "join", "targets": ["half"],
                  "params": {"x": 5, "y": 10, "z": 15},
                  "placement": {"origin": [0, 0, 5]}}),
    );
}

/// The model's solids, by founder.
fn solids(s: &mut Session) -> Vec<String> {
    let r = must(s, "rebuild", json!({"part": "p"}));
    r["parts"][0]["solids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["solid"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn half_a_bracket_mirrored_is_the_whole_bracket_as_one_solid() {
    let (_dir, mut s) = project();
    half(&mut s);
    let v = add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "half", "plane": "yz"}),
    );
    assert_eq!(v["feature"]["operation"], "join");
    near(volume(&mut s, "m", "half"), 3500.0, 1e-9);
    assert_eq!(solids(&mut s), ["half"]);
    // The faces either side of the plane on one surface merged: the base's
    // bottom is one face, as is the top of the upright.
    let b = body(&mut s, "m");
    let bottoms = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == -1.0
        })
        .count();
    assert_eq!(bottoms, 1);
}

#[test]
fn a_new_copy_keeps_its_sources_names_and_follows_its_edits() {
    let (_dir, mut s) = project();
    half(&mut s);
    add(
        &mut s,
        json!({"type": "mirror", "id": "c", "solid": "half", "plane": "zx",
                       "operation": "new"}),
    );
    assert_eq!(solids(&mut s), ["half", "c"]);
    near(volume(&mut s, "c", "c"), 1750.0, 1e-9);
    let (src, copy) = (body(&mut s, "up"), body(&mut s, "c"));
    let names = |b: &Value| {
        let mut n: Vec<String> = b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect();
        n.sort();
        n
    };
    assert_eq!(names(&src), names(&copy));
    // The copy's faces say what they reflect.
    let first = copy["faces"][0]["name"].as_str().unwrap().to_owned();
    let i = must(
        &mut s,
        "inspect",
        json!({"part": "p", "entity": format!("p/c/{first}")}),
    );
    assert_eq!(
        i["entity"]["origin"],
        json!({"feature": "c", "mirror_of": first}),
        "{i}"
    );
    // The source's base made longer: the copy follows, renaming nothing.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "half", "params": {"x": 30}}),
    );
    near(volume(&mut s, "c", "c"), 2250.0, 1e-9);
    assert_eq!(names(&body(&mut s, "c")), names(&copy));
}

#[test]
fn mirrors_about_a_datum_plane_and_about_the_solids_own_face() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
    );
    // About its own +x face: a block twice as long, one solid.
    let end = plane(&body(&mut s, "b"), "b", 0, 1.0, 20.0);
    add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "b", "plane": end}),
    );
    near(volume(&mut s, "m", "b"), 2000.0, 1e-9);
    assert_eq!(solids(&mut s), ["b"]);
    // About a datum plane 50 mm along x, as a solid of its own, apart.
    add(
        &mut s,
        json!({"type": "plane", "id": "far", "by": "offset", "from": "yz",
                       "distance": "50 mm"}),
    );
    add(
        &mut s,
        json!({"type": "mirror", "id": "c", "solid": "b", "plane": "p/far",
                       "operation": "new"}),
    );
    near(volume(&mut s, "c", "c"), 2000.0, 1e-9);
    let r = must(&mut s, "inspect", json!({"part": "p", "feature": "c"}));
    let xs: Vec<f64> = r["bodies"][0]["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["point"][0].as_f64().unwrap())
        .collect();
    let lo = xs.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!((lo - 0.060).abs() < 1e-12, "{lo}");
}

#[test]
fn a_mirrored_copy_cuts_another_solid() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "plate", "params": {"x": 40, "y": 20, "z": 4},
                       "placement": {"origin": [-20, 0, 0]}}),
    );
    // A peg 4 × 4 × 10 at x 10 to 14, through the plate, as a solid of its own.
    add(
        &mut s,
        json!({"type": "box", "id": "peg", "params": {"x": 4, "y": 4, "z": 10},
                       "placement": {"origin": [10, 8, -3]}}),
    );
    add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "peg", "plane": "yz",
                       "operation": "cut", "targets": ["plate"]}),
    );
    // The plate less the peg's mirror image at x -14 to -10: 3,200 - 64.
    near(volume(&mut s, "m", "plate"), 3136.0, 1e-9);
    assert_eq!(solids(&mut s), ["plate", "peg"]);
}

#[test]
fn mirrors_say_what_they_cant_do() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
    );
    // An axis isn't a plane; a sketch is one.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "mirror", "id": "m",
                                              "solid": "b", "plane": "z"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["role"].as_str()),
        (Code::ReferenceWrongKind, Some("plane"))
    );
    // Mirrored in a plane of its own symmetry, a joined copy changes nothing.
    add(
        &mut s,
        json!({"type": "plane", "id": "mid", "by": "offset", "from": "yz",
                       "distance": "10 mm"}),
    );
    add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "b", "plane": "p/mid"}),
    );
    let e = failure(&mut s, "m");
    assert_eq!(e.code, Code::OperationNoEffect, "{e}");
    // A copy 400 m off leaves the size box.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "mid", "distance": "400 m"}),
    );
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "m", "operation": "new"}),
    );
    let e = failure(&mut s, "m");
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::MirrorFailed, Some("outside_size_box")),
        "{e}"
    );
}
