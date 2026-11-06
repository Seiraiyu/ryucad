//! Sheet metal through the protocol (plan 16.A): plates and bent sections
//! from sketches against their closed forms; an edge flange against its
//! closed form; a tray of four flanges against the kernel's own call;
//! names kept across edits of every size, a hole on a flange following
//! them; refusals in the agent's terms. Every body passes the kernel's
//! checks, the self-intersection check among them.

use ryucad::doc::id::Id;
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

/// The name of the vertex of `body` at `p` (mm).
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


/// Sketch `id` on `plane` of points `pts` (mm), fixed, joined in order by
/// lines, closed back to the first when `closed`.
fn chain(s: &mut Session, id: &str, plane: &str, pts: &[[f64; 2]], closed: bool) {
    add(s, json!({"type": "sketch", "id": id, "plane": plane}));
    let mut ops = Vec::new();
    for (i, at) in pts.iter().enumerate() {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    let n = if closed { pts.len() } else { pts.len() - 1 };
    for i in 0..n {
        ops.push(json!({"op": "add", "id": format!("l{}", i + 1), "type": "line",
                        "start": format!("p{}", i + 1), "end": format!("p{}", (i + 1) % pts.len() + 1)}));
    }
    must(s, "sketch.edit", json!({"part": "p", "sketch": id, "ops": ops}));
}

/// Solid `solid` as feature `f` left it: checked, and its volume in mm³
/// certified (the true volume within `bound` of `value`).
fn certified(s: &mut Session, f: &str, solid: &str) -> ryukei::measure::Value {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r.features.iter().find(|(i, _)| i.as_str() == f).expect("listed");
    let out = out.as_ref().unwrap_or_else(|e| panic!("{f}: {e}"));
    let b = &out.solid(&Id::parse(solid, "feature").unwrap()).expect("the solid").body;
    assert!(ryukei::check::check(b).is_empty(), "{f}");
    assert_eq!(ryukei::check::check_intersections(b), [], "{f}");
    ryukei::measure::mass(b, 1.0, &ryukei::measure::MassOptions::default()).unwrap().volume
}

fn holds(v: ryukei::measure::Value, mm3: f64) {
    assert!(v.holds(mm3 * 1e-9, 1e-15), "{v:?} against {mm3} mm³");
}

/// A sheet's section area: `t` times its straight lengths, plus `t |φ|
/// (r + t / 2)` a corner.
fn section(t: f64, straight: f64, corners: &[(f64, f64)]) -> f64 {
    t * straight + corners.iter().map(|(phi, r)| t * phi.abs() * (r + t / 2.0)).sum::<f64>()
}

const Q: f64 = core::f64::consts::FRAC_PI_2;

fn base(s: &mut Session, id: &str, sketch: &str, extra: Value) -> Value {
    let mut v = json!({"type": "base_flange", "id": id, "sketch": sketch, "thickness": "1.5 mm",
                       "radius": "1 mm", "allowance": {"k_factor": "0.44"}});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    add(s, v)
}

#[test]
fn a_plate_is_its_sketch_by_its_thickness() {
    let (_dir, mut s) = project();
    chain(&mut s, "s", "xy", &[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]], true);
    base(&mut s, "plate", "s", json!({}));
    holds(certified(&mut s, "plate", "plate"), 60.0 * 40.0 * 1.5);
}

#[test]
fn an_l_a_u_and_a_z_from_open_sketches() {
    let (_dir, mut s) = project();
    let (t, r, d) = (1.5, 1.0, 20.0);
    // L: the material on the walk's left; a right turn, away from it.
    chain(&mut s, "l", "xy", &[[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]], false);
    base(&mut s, "bl", "l", json!({"depth": "20 mm"}));
    holds(certified(&mut s, "bl", "bl"), d * section(t, (40.0 - r) + (30.0 - r), &[(Q, r)]));
    // U, 100 mm along x: two left turns, towards it.
    chain(&mut s, "u", "xy", &[[100.0, 30.0], [100.0, 0.0], [140.0, 0.0], [140.0, 30.0]], false);
    base(&mut s, "bu", "u", json!({"depth": "20 mm"}));
    let lose = r + t;
    let straight = (30.0 - lose) + (40.0 - 2.0 * lose) + (30.0 - lose);
    holds(certified(&mut s, "bu", "bu"), d * section(t, straight, &[(Q, r), (Q, r)]));
    // Z, 200 mm along x: one each way.
    chain(&mut s, "z", "xy", &[[200.0, 0.0], [220.0, 0.0], [220.0, 25.0], [245.0, 25.0]], false);
    base(&mut s, "bz", "z", json!({"depth": "20 mm"}));
    let straight = (20.0 - lose) + (25.0 - r - lose) + (25.0 - r);
    holds(certified(&mut s, "bz", "bz"), d * section(t, straight, &[(Q, r), (-Q, r)]));
    // Reversed, the L's material is on the right: the corner turns towards
    // it, both lines losing r + t.
    must(&mut s, "feature.edit", json!({"part": "p", "id": "bl", "reverse": true}));
    holds(certified(&mut s, "bl", "bl"), d * section(t, (40.0 - lose) + (30.0 - lose), &[(Q, r)]));
}

/// A plate `plate` 60 × 40 × 1.5 mm and its right top edge (x = 60,
/// z = 1.5) with its far corner (y = 40).
fn plated() -> (tempfile::TempDir, Session, String, String) {
    let (dir, mut s) = project();
    chain(&mut s, "s", "xy", &[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]], true);
    base(&mut s, "plate", "s", json!({}));
    let b = body(&mut s, "plate");
    let rim = edge(&b, "plate", [60.0, 0.0, 1.5], [60.0, 40.0, 1.5]);
    let corner = format!("p/plate/{}", vertex(&b, [60.0, 40.0, 1.5]));
    (dir, s, rim, corner)
}

#[test]
fn an_edge_flange_makes_an_l_of_its_closed_form() {
    let (_dir, mut s, rim, _) = plated();
    add(&mut s, json!({"type": "flange", "id": "f", "edges": [rim], "angle": "90 deg", "length": "20 mm"}));
    let (w, a, t, r, l) = (40.0, 60.0, 1.5, 1.0, 20.0);
    let want = w * t * (a - (r + t))
        + w * core::f64::consts::FRAC_PI_4 * ((r + t) * (r + t) - r * r)
        + w * t * (l - (r + t));
    holds(certified(&mut s, "f", "plate"), want);
    // Measured from the bend's end, the flange's straight length is the
    // length itself.
    must(&mut s, "feature.edit", json!({"part": "p", "id": "f", "measured": "tangent"}));
    let want = w * t * (a - (r + t))
        + w * core::f64::consts::FRAC_PI_4 * ((r + t) * (r + t) - r * r)
        + w * t * l;
    holds(certified(&mut s, "f", "plate"), want);
}

#[test]
fn a_tray_of_four_flanges_is_the_kernels_own() {
    // Four flanges round the plate, set back 5 mm with rectangular reliefs
    // from the founder, one corner set back 8 mm: the body is bit for bit
    // what the kernel's own call makes with the same edges, ends mapped
    // by hand from each edge's direction.
    let (_dir, mut s) = project();
    chain(&mut s, "s", "xy", &[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]], true);
    base(&mut s, "plate", "s", json!({"relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}));
    let b = body(&mut s, "plate");
    let z = 1.5;
    let corners = [[0.0, 0.0, z], [60.0, 0.0, z], [60.0, 40.0, z], [0.0, 40.0, z]];
    let rims: Vec<String> = (0..4).map(|i| edge(&b, "plate", corners[i], corners[(i + 1) % 4])).collect();
    let far = format!("p/plate/{}", vertex(&b, corners[2]));
    add(&mut s, json!({"type": "flange", "id": "f", "edges": rims, "angle": "90 deg",
                       "length": "15 mm", "setback": "5 mm",
                       "ends": [{"vertex": far, "setback": "8 mm"}]}));
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap().clone();
    let plate = Id::parse("plate", "feature").unwrap();
    let ours = r.output(&Id::parse("f", "feature").unwrap()).unwrap().solid(&plate).unwrap().body.clone();
    let base_body = r.output(&plate).unwrap().solid(&plate).unwrap().body.clone();
    // The kernel's own call.
    use ryukei::sheet_metal::{EdgeFlange, FlangePosition, Measure, Relief};
    let name = |t: &str| ryucad::doc::reference::parse_name(t.rsplit('/').next().unwrap()).unwrap();
    let far_name = name(&far);
    let flanges: Vec<EdgeFlange> = rims
        .iter()
        .map(|e| {
            let n = name(e);
            let eid = match base_body.by_name(n) { Some(ryukei::topo::EntityId::Edge(e)) => e, _ => unreachable!() };
            let x = base_body.edge(eid).unwrap();
            let ends = [base_body.vertex(x.start).unwrap().name, base_body.vertex(x.end).unwrap().name];
            let setbacks = ends.map(|v| if v == far_name { 8e-3 } else { 5e-3 });
            EdgeFlange {
                edge: n, side: None, angle: Q, length: 15e-3, measured: Measure::OuterMould,
                radius: 1e-3, position: FlangePosition::MaterialInside, setbacks,
                reliefs: [Relief::Rectangular { width: 1e-3, depth: 5e-4 }; 2],
            }
        })
        .collect();
    let key = ryucad::rebuild::op_key(&part, &Id::parse("f", "feature").unwrap());
    let (theirs, _) = ryukei::sheet_metal::flange(key, &base_body, &flanges, &Default::default()).unwrap();
    assert_eq!(ours.dump(), theirs.dump());
    assert_eq!(ryukei::check::check_intersections(&ours), []);
    let model = ryukei::sheet_metal::recognize(&ours, &Default::default()).unwrap();
    assert_eq!(model.bends.len(), 4);
}

#[test]
fn names_last_across_every_size_and_a_hole_on_a_flange_follows() {
    let (_dir, mut s, rim, _) = plated();
    add(&mut s, json!({"type": "flange", "id": "f", "edges": [rim], "angle": "90 deg", "length": "20 mm"}));
    // The flange's outer side: a plane facing +x, a face of the flange.
    let b = body(&mut s, "f");
    let outer = b["faces"].as_array().unwrap().iter()
        .find(|f| f["origin"]["part"] == "flange" && f["surface"]["type"] == "plane"
                  && (f["surface"]["frame"]["z"][0].as_f64().unwrap()
                      * if f["reversed"] == true { -1.0 } else { 1.0 }) == 1.0)
        .map(|f| format!("p/plate/{}", f["name"].as_str().unwrap()))
        .expect("the flange's outer side");
    // A 3 mm hole through it, from a sketch on that face.
    add(&mut s, json!({"type": "sketch", "id": "hs", "face": outer}));
    must(&mut s, "sketch.edit", json!({"part": "p", "sketch": "hs", "ops": [
        {"op": "add", "id": "c", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "ci", "type": "circle", "center": "c", "r": 1.5}]}));
    add(&mut s, json!({"type": "extrude", "id": "hole", "sketch": "hs", "operation": "cut",
                       "targets": ["plate"], "extent": {"type": "through_all", "both": true}}));
    let names = |s: &mut Session| -> std::collections::BTreeSet<String> {
        body(s, "f")["faces"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_owned()).collect()
    };
    let first = names(&mut s);
    for (id, edit) in [
        ("f", json!({"angle": "80 deg"})),
        ("f", json!({"length": "25 mm"})),
        ("plate", json!({"radius": "2 mm"})),
        ("plate", json!({"thickness": "1.2 mm"})),
        ("plate", json!({"allowance": {"k_factor": "0.4"}})),
    ] {
        let mut e = edit.clone();
        e["part"] = json!("p");
        e["id"] = json!(id);
        must(&mut s, "feature.edit", e);
        assert_eq!(names(&mut s), first, "{id}: {edit}");
        // The hole still cuts: the part rebuilds with nothing failing.
        let part = Id::parse("p", "part").unwrap();
        let r = s.rebuilt(&part).unwrap();
        assert_eq!(r.failed(), 0, "{edit}: {:?}", r.features);
    }
}

#[test]
fn refusals_are_in_the_agents_terms() {
    let (_dir, mut s, rim, corner) = plated();
    let failed = |s: &mut Session, f: &str| -> Value {
        let part = Id::parse("p", "part").unwrap();
        let r = s.rebuilt(&part).unwrap();
        let (_, out) = r.features.iter().find(|(i, _)| i.as_str() == f).unwrap();
        let e = out.as_ref().unwrap_err();
        json!({"code": e.code, "details": e.details})
    };
    // A setback with no relief cuts into the base.
    add(&mut s, json!({"type": "flange", "id": "f", "edges": [rim.clone()], "angle": "90 deg",
                       "length": "20 mm", "ends": [{"vertex": corner.clone(), "setback": "5 mm"}]}));
    let e = failed(&mut s, "f");
    assert_eq!((e["code"].as_str(), e["details"]["reason"].as_str()),
               (Some("sheet_metal_failed"), Some("relief_needed")), "{e}");
    assert!(e["details"]["hint"].as_str().unwrap().contains("--end-relief"), "{e}");
    must(&mut s, "feature.delete", json!({"part": "p", "id": "f"}));
    // Two flanges meeting at a corner, not set back.
    let b = body(&mut s, "plate");
    let near = edge(&b, "plate", [60.0, 40.0, 1.5], [0.0, 40.0, 1.5]);
    add(&mut s, json!({"type": "flange", "id": "g", "edges": [rim.clone(), near], "angle": "90 deg",
                       "length": "20 mm"}));
    let e = failed(&mut s, "g");
    assert_eq!(e["details"]["reason"], "flanges_meet", "{e}");
    must(&mut s, "feature.delete", json!({"part": "p", "id": "g"}));
    // An edge between two walls.
    let up = edge(&b, "plate", [60.0, 0.0, 0.0], [60.0, 0.0, 1.5]);
    add(&mut s, json!({"type": "flange", "id": "h", "edges": [up], "angle": "90 deg", "length": "20 mm"}));
    let e = failed(&mut s, "h");
    assert_eq!(e["details"]["reason"], "not_a_flange_edge", "{e}");
    must(&mut s, "feature.delete", json!({"part": "p", "id": "h"}));
    // A block with a boss isn't sheet metal: its flange says so, naming the
    // face that stands out of the sheet.
    add(&mut s, json!({"type": "box", "id": "blk", "params": {"x": 40, "y": 30, "z": 2},
                       "placement": {"origin": [0, 100, 0]}}));
    add(&mut s, json!({"type": "cylinder", "id": "boss", "params": {"radius": 3, "height": 10},
                       "placement": {"origin": [20, 115, 0]}, "operation": "join", "targets": ["blk"]}));
    let k = body(&mut s, "boss");
    let top = edge(&k, "blk", [40.0, 100.0, 2.0], [40.0, 130.0, 2.0]);
    add(&mut s, json!({"type": "flange", "id": "i", "target": "blk", "edges": [top], "angle": "90 deg",
                       "length": "10 mm", "radius": "1 mm"}));
    let e = failed(&mut s, "i");
    assert_eq!(e["details"]["reason"], "out_of_slab", "{e}");
    assert!(e["details"]["entities"][0]["reference"].is_string(), "{e}");
}
