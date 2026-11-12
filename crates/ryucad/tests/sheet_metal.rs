//! Sheet metal through the protocol (plan 16.A): plates and bent sections
//! from sketches against their closed forms; an edge flange against its
//! closed form; a tray of four flanges against the kernel's own call;
//! names kept across edits of every size, a hole on a flange following
//! them; refusals in the agent's terms. Every body passes the kernel's
//! checks, the self-intersection check among them; unfolds against
//! developed lengths, a slot cut across a bend line in the flat and
//! refolded, a partial unfold, names kept through unfold and refold, and a
//! closed tube's bend loop cleared by a rip (plan 16.B).

mod common;

use common::{call, must, project};
use ryucad::doc::id::Id;
use ryucad::error::Code;
use ryucad::proto::Request;
use ryucad::session::Session;
use serde_json::{Value, json};

fn add(s: &mut Session, f: Value) -> Value {
    let mut f = f;
    f["part"] = json!("p");
    must(s, "feature.add", f)
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
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": id, "ops": ops}),
    );
}

/// Solid `solid` as feature `f` left it: checked, and its volume in mm³
/// certified (the true volume within `bound` of `value`).
fn certified(s: &mut Session, f: &str, solid: &str) -> ryukei::measure::Value {
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
    ryukei::measure::mass(b, 1.0, &ryukei::measure::MassOptions::default())
        .unwrap()
        .volume
}

fn holds(v: ryukei::measure::Value, mm3: f64) {
    assert!(v.holds(mm3 * 1e-9, 1e-15), "{v:?} against {mm3} mm³");
}

/// A sheet's section area: `t` times its straight lengths, plus `t |φ|
/// (r + t / 2)` a corner.
fn section(t: f64, straight: f64, corners: &[(f64, f64)]) -> f64 {
    t * straight
        + corners
            .iter()
            .map(|(phi, r)| t * phi.abs() * (r + t / 2.0))
            .sum::<f64>()
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
    chain(
        &mut s,
        "s",
        "xy",
        &[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]],
        true,
    );
    base(&mut s, "plate", "s", json!({}));
    holds(certified(&mut s, "plate", "plate"), 60.0 * 40.0 * 1.5);
}

#[test]
fn an_l_a_u_and_a_z_from_open_sketches() {
    let (_dir, mut s) = project();
    let (t, r, d) = (1.5, 1.0, 20.0);
    // L: the material on the walk's left; a right turn, away from it.
    chain(
        &mut s,
        "l",
        "xy",
        &[[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]],
        false,
    );
    base(&mut s, "bl", "l", json!({"depth": "20 mm"}));
    holds(
        certified(&mut s, "bl", "bl"),
        d * section(t, (40.0 - r) + (30.0 - r), &[(Q, r)]),
    );
    // U, 100 mm along x: two left turns, towards it.
    chain(
        &mut s,
        "u",
        "xy",
        &[[100.0, 30.0], [100.0, 0.0], [140.0, 0.0], [140.0, 30.0]],
        false,
    );
    base(&mut s, "bu", "u", json!({"depth": "20 mm"}));
    let lose = r + t;
    let straight = (30.0 - lose) + (40.0 - 2.0 * lose) + (30.0 - lose);
    holds(
        certified(&mut s, "bu", "bu"),
        d * section(t, straight, &[(Q, r), (Q, r)]),
    );
    // Z, 200 mm along x: one each way.
    chain(
        &mut s,
        "z",
        "xy",
        &[[200.0, 0.0], [220.0, 0.0], [220.0, 25.0], [245.0, 25.0]],
        false,
    );
    base(&mut s, "bz", "z", json!({"depth": "20 mm"}));
    let straight = (20.0 - lose) + (25.0 - r - lose) + (25.0 - r);
    holds(
        certified(&mut s, "bz", "bz"),
        d * section(t, straight, &[(Q, r), (-Q, r)]),
    );
    // Reversed, the L's material is on the right: the corner turns towards
    // it, both lines losing r + t.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "bl", "reverse": true}),
    );
    holds(
        certified(&mut s, "bl", "bl"),
        d * section(t, (40.0 - lose) + (30.0 - lose), &[(Q, r)]),
    );
}

/// A plate `plate` 60 × 40 × 1.5 mm and its right top edge (x = 60,
/// z = 1.5) with its far corner (y = 40).
fn plated() -> (tempfile::TempDir, Session, String, String) {
    let (dir, mut s) = project();
    chain(
        &mut s,
        "s",
        "xy",
        &[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]],
        true,
    );
    base(&mut s, "plate", "s", json!({}));
    let b = body(&mut s, "plate");
    let rim = edge(&b, "plate", [60.0, 0.0, 1.5], [60.0, 40.0, 1.5]);
    let corner = format!("p/plate/{}", vertex(&b, [60.0, 40.0, 1.5]));
    (dir, s, rim, corner)
}

#[test]
fn an_edge_flange_makes_an_l_of_its_closed_form() {
    let (_dir, mut s, rim, _) = plated();
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": [rim], "angle": "90 deg", "length": "20 mm"}),
    );
    let (w, a, t, r, l) = (40.0, 60.0, 1.5, 1.0, 20.0);
    let want = w * t * (a - (r + t))
        + w * core::f64::consts::FRAC_PI_4 * ((r + t) * (r + t) - r * r)
        + w * t * (l - (r + t));
    holds(certified(&mut s, "f", "plate"), want);
    // Measured from the bend's end, the flange's straight length is the
    // length itself.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "f", "measured": "tangent"}),
    );
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
    chain(
        &mut s,
        "s",
        "xy",
        &[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]],
        true,
    );
    base(
        &mut s,
        "plate",
        "s",
        json!({"relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}),
    );
    let b = body(&mut s, "plate");
    let z = 1.5;
    let corners = [
        [0.0, 0.0, z],
        [60.0, 0.0, z],
        [60.0, 40.0, z],
        [0.0, 40.0, z],
    ];
    let rims: Vec<String> = (0..4)
        .map(|i| edge(&b, "plate", corners[i], corners[(i + 1) % 4]))
        .collect();
    let far = format!("p/plate/{}", vertex(&b, corners[2]));
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": rims, "angle": "90 deg",
                       "length": "15 mm", "setback": "5 mm",
                       "ends": [{"vertex": far, "setback": "8 mm"}]}),
    );
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap().clone();
    let plate = Id::parse("plate", "feature").unwrap();
    let ours = r
        .output(&Id::parse("f", "feature").unwrap())
        .unwrap()
        .solid(&plate)
        .unwrap()
        .body
        .clone();
    let base_body = r
        .output(&plate)
        .unwrap()
        .solid(&plate)
        .unwrap()
        .body
        .clone();
    // The kernel's own call.
    use ryukei::sheet_metal::{EdgeFlange, FlangePosition, Measure, Relief};
    let name = |t: &str| ryucad::doc::reference::parse_name(t.rsplit('/').next().unwrap()).unwrap();
    let far_name = name(&far);
    let flanges: Vec<EdgeFlange> = rims
        .iter()
        .map(|e| {
            let n = name(e);
            let eid = match base_body.by_name(n) {
                Some(ryukei::topo::EntityId::Edge(e)) => e,
                _ => unreachable!(),
            };
            let x = base_body.edge(eid).unwrap();
            let ends = [
                base_body.vertex(x.start).unwrap().name,
                base_body.vertex(x.end).unwrap().name,
            ];
            let setbacks = ends.map(|v| if v == far_name { 8e-3 } else { 5e-3 });
            EdgeFlange {
                edge: n,
                side: None,
                angle: Q,
                length: 15e-3,
                measured: Measure::OuterMould,
                radius: 1e-3,
                position: FlangePosition::MaterialInside,
                setbacks,
                reliefs: [Relief::Rectangular {
                    width: 1e-3,
                    depth: 5e-4,
                }; 2],
            }
        })
        .collect();
    let key = ryucad::rebuild::op_key(&part, &Id::parse("f", "feature").unwrap());
    let (theirs, _) =
        ryukei::sheet_metal::flange(key, &base_body, &flanges, &Default::default()).unwrap();
    assert_eq!(ours.dump(), theirs.dump());
    assert_eq!(ryukei::check::check_intersections(&ours), []);
    let model = ryukei::sheet_metal::recognize(&ours, &Default::default()).unwrap();
    assert_eq!(model.bends.len(), 4);
}

#[test]
fn names_last_across_every_size_and_a_hole_on_a_flange_follows() {
    let (_dir, mut s, rim, _) = plated();
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": [rim], "angle": "90 deg", "length": "20 mm"}),
    );
    // The flange's outer side: a plane facing +x, a face of the flange.
    let b = body(&mut s, "f");
    let outer = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["origin"]["part"] == "flange"
                && f["surface"]["type"] == "plane"
                && (f["surface"]["frame"]["z"][0].as_f64().unwrap()
                    * if f["reversed"] == true { -1.0 } else { 1.0 })
                    == 1.0
        })
        .map(|f| format!("p/plate/{}", f["name"].as_str().unwrap()))
        .expect("the flange's outer side");
    // A 3 mm hole through it, from a sketch on that face.
    add(&mut s, json!({"type": "sketch", "id": "hs", "face": outer}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "hs", "ops": [
        {"op": "add", "id": "c", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "ci", "type": "circle", "center": "c", "r": 1.5}]}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "hole", "sketch": "hs", "operation": "cut",
                       "targets": ["plate"], "extent": {"type": "through_all", "both": true}}),
    );
    let names = |s: &mut Session| -> std::collections::BTreeSet<String> {
        body(s, "f")["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect()
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
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": [rim.clone()], "angle": "90 deg",
                       "length": "20 mm", "ends": [{"vertex": corner.clone(), "setback": "5 mm"}]}),
    );
    let e = failed(&mut s, "f");
    assert_eq!(
        (e["code"].as_str(), e["details"]["reason"].as_str()),
        (Some("sheet_metal_failed"), Some("relief_needed")),
        "{e}"
    );
    assert!(
        e["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("--end-relief"),
        "{e}"
    );
    must(&mut s, "feature.delete", json!({"part": "p", "id": "f"}));
    // Two flanges meeting at a corner, not set back.
    let b = body(&mut s, "plate");
    let near = edge(&b, "plate", [60.0, 40.0, 1.5], [0.0, 40.0, 1.5]);
    add(
        &mut s,
        json!({"type": "flange", "id": "g", "edges": [rim.clone(), near], "angle": "90 deg",
                       "length": "20 mm"}),
    );
    let e = failed(&mut s, "g");
    assert_eq!(e["details"]["reason"], "flanges_meet", "{e}");
    must(&mut s, "feature.delete", json!({"part": "p", "id": "g"}));
    // An edge between two walls.
    let up = edge(&b, "plate", [60.0, 0.0, 0.0], [60.0, 0.0, 1.5]);
    add(
        &mut s,
        json!({"type": "flange", "id": "h", "edges": [up], "angle": "90 deg", "length": "20 mm"}),
    );
    let e = failed(&mut s, "h");
    assert_eq!(e["details"]["reason"], "not_a_flange_edge", "{e}");
    must(&mut s, "feature.delete", json!({"part": "p", "id": "h"}));
    // A block with a boss isn't sheet metal: its flange says so, naming the
    // face that stands out of the sheet.
    add(
        &mut s,
        json!({"type": "box", "id": "blk", "params": {"x": 40, "y": 30, "z": 2},
                       "placement": {"origin": [0, 100, 0]}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "params": {"radius": 3, "height": 10},
                       "placement": {"origin": [20, 115, 0]}, "operation": "join", "targets": ["blk"]}),
    );
    let k = body(&mut s, "boss");
    let top = edge(&k, "blk", [40.0, 100.0, 2.0], [40.0, 130.0, 2.0]);
    add(
        &mut s,
        json!({"type": "flange", "id": "i", "target": "blk", "edges": [top], "angle": "90 deg",
                       "length": "10 mm", "radius": "1 mm"}),
    );
    let e = failed(&mut s, "i");
    assert_eq!(e["details"]["reason"], "out_of_slab", "{e}");
    assert!(e["details"]["entities"][0]["reference"].is_string(), "{e}");
}

#[test]
fn the_sheet_report_gives_the_thickness_and_bend_table() {
    let (_dir, mut s, rim, _) = plated();
    // A plate: its thickness, no bends.
    let v = must(&mut s, "sheet.get", json!({"part": "p"}));
    assert_eq!(
        (
            v["schema"].as_u64(),
            v["solid"].as_str(),
            v["state"].as_str()
        ),
        (Some(1), Some("plate"), Some("folded")),
        "{v}"
    );
    assert!(
        (v["thickness_m"].as_f64().unwrap() - 1.5e-3).abs() < 1e-12,
        "{v}"
    );
    assert_eq!(v["bends"], json!([]));
    assert_eq!(v["tree"], true);
    // A flange: one bend, its developed length θ (r + k t).
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": [rim], "angle": "90 deg", "length": "20 mm"}),
    );
    let v = must(&mut s, "sheet.get", json!({"part": "p", "solid": "plate"}));
    let b = &v["bends"][0];
    assert!(
        (b["radius_m"].as_f64().unwrap() - 1e-3).abs() < 1e-12,
        "{v}"
    );
    assert!(
        (b["angle_deg"].as_f64().unwrap() - 90.0).abs() < 1e-9,
        "{v}"
    );
    let want = Q * (1e-3 + 0.44 * 1.5e-3);
    assert!(
        (b["developed_m"].as_f64().unwrap() - want).abs() < 1e-12,
        "{v}"
    );
    assert_eq!(v["allowance_from"], "plate");
    // Its bend face resolves as any face does.
    let r = b["bend"].as_str().unwrap();
    must(&mut s, "measure", json!({"entities": [r]}));
    // By a deduction: 2 (r + t) tan(θ / 2) - bd.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "plate", "allowance": {"deduction": "2.4 mm"}}),
    );
    let v = must(&mut s, "sheet.get", json!({"part": "p"}));
    let want = 2.0 * (1e-3 + 1.5e-3) - 2.4e-3;
    assert!(
        (v["bends"][0]["developed_m"].as_f64().unwrap() - want).abs() < 1e-12,
        "{v}"
    );
}

#[test]
fn the_sheet_report_refuses_what_isnt_sheet_metal() {
    let (_dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "blk", "params": {"x": 40, "y": 30, "z": 2}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "params": {"radius": 3, "height": 10},
                       "placement": {"origin": [20, 15, 0]}, "operation": "join", "targets": ["blk"]}),
    );
    let e = call(&mut s, "sheet.get", json!({"part": "p"})).unwrap_err();
    assert_eq!(e.code, Code::SheetMetalFailed, "{e}");
    // Two solids and none named.
    // A thin box, so recognition reads it as a plate (a cube's edges are
    // sharp bends).
    add(
        &mut s,
        json!({"type": "box", "id": "other", "params": {"x": 10, "y": 10, "z": 1},
                       "placement": {"origin": [100, 0, 0]}}),
    );
    let e = call(&mut s, "sheet.get", json!({"part": "p"})).unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
    // A plain box is a plate as far as recognition goes: no base flange
    // gives it an allowance.
    let v = must(&mut s, "sheet.get", json!({"part": "p", "solid": "other"}));
    assert_eq!(v["allowance"], Value::Null, "{v}");
}

/// Unfold `id` of `target` with `extra`'s fields.
fn unfold(s: &mut Session, id: &str, target: &str, extra: Value) -> Value {
    let mut v = json!({"type": "unfold", "id": id, "target": target});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    add(s, v)
}

fn same(a: ryukei::measure::Value, b: ryukei::measure::Value) {
    assert!(
        (a.value() - b.value()).abs() <= a.bound() + b.bound() + 1e-15,
        "{a:?} against {b:?}"
    );
}

#[test]
fn an_l_a_u_and_a_z_unfold_to_their_developed_lengths() {
    let (_dir, mut s) = project();
    let (t, r, d, k) = (1.5, 1.0, 20.0, 0.44);
    let ba = |phi: f64| phi.abs() * (r + k * t);
    let lose = r + t;
    for (sk, base_id, pts, straight, corners) in [
        (
            "l",
            "bl",
            vec![[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]],
            (40.0 - r) + (30.0 - r),
            vec![Q],
        ),
        (
            "u",
            "bu",
            vec![[100.0, 30.0], [100.0, 0.0], [140.0, 0.0], [140.0, 30.0]],
            (30.0 - lose) + (40.0 - 2.0 * lose) + (30.0 - lose),
            vec![Q, Q],
        ),
        (
            "z",
            "bz",
            vec![[200.0, 0.0], [220.0, 0.0], [220.0, 25.0], [245.0, 25.0]],
            (20.0 - lose) + (25.0 - r - lose) + (25.0 - r),
            vec![Q, -Q],
        ),
    ] {
        chain(&mut s, sk, "xy", &pts, false);
        base(&mut s, base_id, sk, json!({"depth": "20 mm"}));
        let u = format!("u_{base_id}");
        unfold(&mut s, &u, base_id, json!({}));
        let flat_len = straight + corners.iter().map(|p| ba(*p)).sum::<f64>();
        holds(certified(&mut s, &u, base_id), d * t * flat_len);
        let v = must(&mut s, "sheet.get", json!({"part": "p", "solid": base_id}));
        assert_eq!(v["state"], "flat", "{v}");
    }
}

#[test]
fn a_deduction_unfolds_as_the_k_factor_it_equals() {
    let (_dir, mut s, rim, _) = plated();
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": [rim], "angle": "90 deg", "length": "20 mm"}),
    );
    let (r, t, k) = (1e-3, 1.5e-3, 0.44);
    // bd = 2 (r + t) tan(θ / 2) - θ (r + k t), at θ = 90°.
    let bd = 2.0 * (r + t) - Q * (r + k * t);
    unfold(&mut s, "uk", "plate", json!({}));
    let by_k = certified(&mut s, "uk", "plate");
    must(&mut s, "feature.delete", json!({"part": "p", "id": "uk"}));
    unfold(
        &mut s,
        "ud",
        "plate",
        json!({"allowance": {"deduction": format!("{} mm", bd * 1e3)}}),
    );
    same(certified(&mut s, "ud", "plate"), by_k);
}

#[test]
fn a_tray_unfolds_to_its_plate_and_strips_keeping_its_volume() {
    let (_dir, mut s) = project();
    chain(
        &mut s,
        "s",
        "xy",
        &[[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]],
        true,
    );
    base(
        &mut s,
        "plate",
        "s",
        json!({"allowance": {"k_factor": "0.5"},
        "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}),
    );
    let b = body(&mut s, "plate");
    let z = 1.5;
    let corners = [
        [0.0, 0.0, z],
        [60.0, 0.0, z],
        [60.0, 40.0, z],
        [0.0, 40.0, z],
    ];
    let rims: Vec<String> = (0..4)
        .map(|i| edge(&b, "plate", corners[i], corners[(i + 1) % 4]))
        .collect();
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": rims, "angle": "90 deg",
                       "length": "15 mm", "setback": "5 mm"}),
    );
    unfold(&mut s, "u", "plate", json!({}));
    same(
        certified(&mut s, "u", "plate"),
        certified(&mut s, "f", "plate"),
    );
    let v = must(&mut s, "sheet.get", json!({"part": "p"}));
    assert_eq!(
        (v["state"].as_str(), v["unfolded"].as_array().map(Vec::len)),
        (Some("flat"), Some(4)),
        "{v}"
    );
    // Four bend lines, every one in the rebuild report.
    let rb = must(&mut s, "rebuild", json!({"part": "p"}));
    let u = rb["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == "u")
        .unwrap()
        .clone();
    assert_eq!(u["flat"]["bends"], 4, "{u}");
}

/// The plated plate flanged on its right edge and unfolded about its top
/// at a K-factor of a half; the plate's top face as a reference.
fn unfolded() -> (tempfile::TempDir, Session) {
    let (dir, mut s, rim, _) = plated();
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": [rim], "angle": "90 deg", "length": "20 mm"}),
    );
    let v = must(&mut s, "sheet.get", json!({"part": "p"}));
    // The plate's top: the side whose faces the plate's base flange made
    // and which lies at z = 1.5 mm.
    let b = body(&mut s, "f");
    let top = v["flanges"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["sides"].as_array().unwrap().clone())
        .flat_map(|side| side.as_array().unwrap().clone())
        .map(|f| f.as_str().unwrap().to_owned())
        .find(|f| {
            let n = f.rsplit('/').next().unwrap();
            b["faces"].as_array().unwrap().iter().any(|x| {
                x["name"] == n
                    && x["origin"]["feature"] == "plate"
                    && x["surface"]["type"] == "plane"
                    && (x["surface"]["frame"]["origin"][2].as_f64().unwrap() - 1.5e-3).abs() < 1e-12
            })
        })
        .expect("the plate's top");
    unfold(
        &mut s,
        "u",
        "plate",
        json!({"fixed_face": top, "allowance": {"k_factor": "0.5"}}),
    );
    (dir, s)
}

#[test]
fn a_slot_across_a_bend_line_in_the_flat_refolds_checked_clean() {
    let (_dir, mut s) = unfolded();
    // The strip runs from x = 57.5 for θ (r + t / 2): a 4 × 4 mm slot
    // across its middle crosses both bend lines.
    let ba = Q * 1.75;
    let mid = 57.5 + ba / 2.0;
    chain(
        &mut s,
        "slot_s",
        "xy",
        &[
            [mid - 2.0, 18.0],
            [mid + 2.0, 18.0],
            [mid + 2.0, 22.0],
            [mid - 2.0, 22.0],
        ],
        true,
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "slot", "sketch": "slot_s", "operation": "cut",
                       "targets": ["plate"], "extent": {"type": "through_all", "both": true}}),
    );
    let flat = certified(&mut s, "u", "plate");
    let cut = certified(&mut s, "slot", "plate");
    holds(cut, flat.value() * 1e9 - 4.0 * 4.0 * 1.5);
    add(
        &mut s,
        json!({"type": "refold", "id": "rf", "target": "plate", "unfold": "u"}),
    );
    // Checked clean (`certified` checks), with the cut flat's volume.
    same(certified(&mut s, "rf", "plate"), cut);
    let v = must(&mut s, "sheet.get", json!({"part": "p"}));
    assert_eq!(
        (v["state"].as_str(), v["bends"].as_array().map(Vec::len)),
        (Some("folded"), Some(1)),
        "{v}"
    );
    // The cut in the flat split the slot's walls at the bend lines, so
    // the refold kept every name the cut made, origins and all.
    let names = |s: &mut Session, f: &str| -> std::collections::BTreeSet<String> {
        body(s, f)["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names(&mut s, "rf"), names(&mut s, "slot"));
}

#[test]
fn names_last_through_unfold_and_refold_across_edits() {
    let (_dir, mut s) = unfolded();
    add(
        &mut s,
        json!({"type": "refold", "id": "rf", "target": "plate", "unfold": "u"}),
    );
    let names = |s: &mut Session, f: &str| -> std::collections::BTreeSet<String> {
        body(s, f)["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let folded = names(&mut s, "f");
    assert_eq!(names(&mut s, "u"), folded);
    assert_eq!(names(&mut s, "rf"), folded);
    for (id, edit) in [
        ("f", json!({"angle": "80 deg"})),
        ("f", json!({"length": "25 mm"})),
        ("plate", json!({"radius": "2 mm"})),
        ("plate", json!({"thickness": "1.2 mm"})),
        ("u", json!({"allowance": {"k_factor": "0.4"}})),
    ] {
        let mut e = edit.clone();
        e["part"] = json!("p");
        e["id"] = json!(id);
        must(&mut s, "feature.edit", e);
        let now = names(&mut s, "f");
        assert_eq!(names(&mut s, "u"), now, "{id}: {edit}");
        assert_eq!(names(&mut s, "rf"), now, "{id}: {edit}");
        assert_eq!(now, folded, "{id}: {edit}");
    }
}

#[test]
fn a_partial_unfold_leaves_the_rest_folded() {
    let (_dir, mut s, rim, _) = plated();
    let b = body(&mut s, "plate");
    let left = edge(&b, "plate", [0.0, 0.0, 1.5], [0.0, 40.0, 1.5]);
    add(
        &mut s,
        json!({"type": "flange", "id": "f", "edges": [rim, left], "angle": "90 deg",
                       "length": "20 mm"}),
    );
    let v = must(&mut s, "sheet.get", json!({"part": "p"}));
    let first = v["bends"][0]["bend"].as_str().unwrap().to_owned();
    unfold(&mut s, "u", "plate", json!({"only": [first]}));
    let v = must(&mut s, "sheet.get", json!({"part": "p"}));
    assert_eq!(
        (v["state"].as_str(), v["bends"].as_array().map(Vec::len)),
        (Some("partly"), Some(1)),
        "{v}"
    );
    // A partial unfold keeps the volume at a K-factor of a half too.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "plate", "allowance": {"k_factor": "0.5"}}),
    );
    same(
        certified(&mut s, "u", "plate"),
        certified(&mut s, "f", "plate"),
    );
}

/// Sketch `id` on xy: a square ring `side` mm across with its outer
/// corners rounded at `r + t` and its inner at `r`, about the same
/// centres, so it extrudes to a closed tube of four bends.
fn ring(s: &mut Session, id: &str, side: f64, r: f64, t: f64) {
    add(s, json!({"type": "sketch", "id": id, "plane": "xy"}));
    let ro = r + t;
    let c = [
        [ro, ro],
        [side - ro, ro],
        [side - ro, side - ro],
        [ro, side - ro],
    ];
    let mut ops = Vec::new();
    // Only the centres are fixed: fixing an arc's ends as well makes its
    // end's place on its circle redundant.
    let point = |ops: &mut Vec<Value>, name: String, at: [f64; 2]| {
        ops.push(json!({"op": "add", "id": name, "type": "point", "at": at}));
    };
    for (k, cc) in c.iter().enumerate() {
        point(&mut ops, format!("k{k}"), *cc);
        ops.push(json!({"op": "add", "type": "fix", "a": format!("k{k}")}));
    }
    // Each loop counterclockwise: side k runs from corner k to corner k+1,
    // `out_of[k]` from them, then the arc about corner k+1 turns a quarter
    // to the next side.
    let out_of = [[0.0, -1.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]];
    for (tag, rad) in [("o", ro), ("i", r)] {
        for k in 0..4 {
            let n = (k + 1) % 4;
            let a = [c[k][0] + out_of[k][0] * rad, c[k][1] + out_of[k][1] * rad];
            let b = [c[n][0] + out_of[k][0] * rad, c[n][1] + out_of[k][1] * rad];
            point(&mut ops, format!("{tag}a{k}"), a);
            point(&mut ops, format!("{tag}b{k}"), b);
        }
        for k in 0..4 {
            let n = (k + 1) % 4;
            ops.push(
                json!({"op": "add", "id": format!("{tag}l{k}"), "type": "line",
                            "start": format!("{tag}a{k}"), "end": format!("{tag}b{k}")}),
            );
            ops.push(
                json!({"op": "add", "id": format!("{tag}r{k}"), "type": "arc",
                            "center": format!("k{n}"), "start": format!("{tag}b{k}"),
                            "end": format!("{tag}a{n}")}),
            );
        }
    }
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": id, "ops": ops}),
    );
}

#[test]
fn a_closed_tube_is_a_bend_loop_until_a_rip_opens_it() {
    let (_dir, mut s) = project();
    ring(&mut s, "r", 40.0, 1.0, 1.5);
    add(
        &mut s,
        json!({"type": "extrude", "id": "tube", "sketch": "r",
                       "extent": {"type": "blind", "length": "20 mm"}}),
    );
    // No base flange founded it: an unfold gives its allowance.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "unfold", "id": "u", "target": "tube"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/allowance")),
        "{e}"
    );
    // A loop: refused, its hint to rip it. (A write that recognizes the
    // tube already refuses it so: either way, the same refusal.)
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "unfold", "id": "u",
        "target": "tube", "allowance": {"k_factor": "0.5"}}),
    );
    let e = match e {
        Err(e) => e,
        Ok(_) => {
            let part = Id::parse("p", "part").unwrap();
            let r = s.rebuilt(&part).unwrap();
            r.features
                .iter()
                .find(|(i, _)| i.as_str() == "u")
                .unwrap()
                .1
                .clone()
                .unwrap_err()
        }
    };
    assert_eq!(
        (e.code, e.details["reason"].as_str()),
        (Code::SheetMetalFailed, Some("bend_loop")),
        "{e}"
    );
    assert!(e.details["hint"].as_str().unwrap().contains("rip"), "{e}");
    let _ = call(&mut s, "feature.delete", json!({"part": "p", "id": "u"}));
    // A rip: a 0.5 mm slit through the bottom wall, the whole depth.
    add(
        &mut s,
        json!({"type": "box", "id": "rip", "params": {"x": 0.5, "y": 3, "z": 30},
                       "placement": {"origin": [19.75, -0.5, -5]},
                       "operation": "cut", "targets": ["tube"]}),
    );
    unfold(
        &mut s,
        "u",
        "tube",
        json!({"allowance": {"k_factor": "0.5"}}),
    );
    same(
        certified(&mut s, "u", "tube"),
        certified(&mut s, "rip", "tube"),
    );
    let v = must(&mut s, "sheet.get", json!({"part": "p", "solid": "tube"}));
    assert_eq!(v["state"], "flat", "{v}");
}

use ryucad::sheet_metal::flat::{self, From, Options};

/// The L of `an_l_a_u_and_a_z_from_open_sketches`, 20 mm deep, at a
/// K-factor of 0.44: runs of 39 and 29 mm and a quarter turn of 1 mm
/// inside, 1.5 mm thick.
fn l_part() -> (tempfile::TempDir, Session) {
    let (dir, mut s) = project();
    chain(
        &mut s,
        "l",
        "xy",
        &[[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]],
        false,
    );
    base(&mut s, "bl", "l", json!({"depth": "20 mm"}));
    (dir, s)
}

/// The L's flat length in mm: its runs and its bend's developed length.
fn l_flat_mm(k: f64) -> f64 {
    let (t, r) = (1.5, 1.0);
    (40.0 - r) + (30.0 - r) + Q * (r + k * t)
}

fn outline_mm2(f: &flat::FlatOf) -> f64 {
    f.pattern
        .loops
        .iter()
        .filter(|l| l.outer)
        .map(|l| l.area())
        .sum::<f64>()
        * 1e6
}

#[test]
fn a_folded_solid_is_unfolded_whole_in_memory() {
    let (_dir, mut s) = l_part();
    let f = flat::of(&mut s, "p", None, None, &Options::default())
        .unwrap()
        .1;
    assert_eq!(f.solid.as_str(), "bl");
    assert_eq!(f.from, From::Whole);
    assert!(
        (outline_mm2(&f) - 20.0 * l_flat_mm(0.44)).abs() < 1e-6,
        "{}",
        outline_mm2(&f)
    );
    assert_eq!(f.pattern.bend_lines.len(), 1);
    // The bend named as sheet.get names it.
    let sheet = must(&mut s, "sheet.get", json!({"part": "p"}));
    assert_eq!(f.bends[0], sheet["bends"][0]["bend"].as_str().unwrap());
    // Asked again: the same, from the session's cache.
    let again = flat::of(&mut s, "p", None, None, &Options::default())
        .unwrap()
        .1;
    assert!(std::sync::Arc::ptr_eq(&f, &again));
    // An override: the bend at a K-factor of a half.
    let o = Options {
        fixed: None,
        bends: vec![json!({"bend": f.bends[0], "k_factor": "0.5"})],
    };
    let half = flat::of(&mut s, "p", None, None, &o).unwrap().1;
    assert!((outline_mm2(&half) - 20.0 * l_flat_mm(0.5)).abs() < 1e-6);
}

#[test]
fn a_flat_in_force_is_redrawn_with_its_cuts() {
    let (_dir, mut s) = l_part();
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "unfold", "id": "u", "target": "bl"}),
    );
    // A 4 mm hole through the flat's first run, 10 mm from its end.
    let u = flat::of(&mut s, "p", None, None, &Options::default())
        .unwrap()
        .1;
    assert_eq!(u.from, From::Unfold(Id::parse("u", "feature").unwrap()));
    let before = outline_mm2(&u);
    let holes = |f: &flat::FlatOf| f.pattern.loops.iter().filter(|l| !l.outer).count();
    assert_eq!(holes(&u), 0);
    // The flat lies in y = -1.5 mm, so the hole is sketched on zx (normal
    // y), at x = 30 mm, z = 10 mm.
    add(&mut s, json!({"type": "sketch", "id": "h", "plane": "zx"}));
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "h", "ops": [
        {"op": "add", "id": "p1", "type": "point", "at": [10.0, 30.0]},
        {"op": "add", "type": "fix", "a": "p1"}]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "d", "sketch": "h", "diameter": "4 mm",
                       "depth": "through_all", "targets": ["bl"]}),
    );
    let cut = flat::of(&mut s, "p", None, None, &Options::default())
        .unwrap()
        .1;
    assert_eq!(holes(&cut), 1);
    assert_eq!(cut.pattern.bend_lines, u.pattern.bend_lines);
    assert!((outline_mm2(&cut) - before).abs() < 1e-9);
    // The unfold's own made this flat: no fixed face, no overrides.
    let o = Options {
        fixed: Some(format!("p/bl/{}", u.bends[0].rsplit('/').next().unwrap())),
        bends: vec![],
    };
    let e = flat::of(&mut s, "p", None, None, &o).unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
}

#[test]
fn a_flat_says_what_it_cant_take() {
    let (_dir, mut s) = project();
    chain(
        &mut s,
        "b",
        "xy",
        &[[0.0, 0.0], [30.0, 0.0], [30.0, 20.0], [0.0, 20.0]],
        true,
    );
    // A block made by an extrude: no base flange, no allowance.
    add(
        &mut s,
        json!({"type": "extrude", "id": "x", "sketch": "b", "extent": {"type": "blind", "length": "2 mm"}}),
    );
    let f = flat::of(&mut s, "p", None, None, &Options::default());
    // A plate: no bends, so nothing wants an allowance; its outline alone.
    let f = f.unwrap().1;
    assert!(f.pattern.bend_lines.is_empty());
    assert!((outline_mm2(&f) - 600.0).abs() < 1e-6);
    let (_dir, mut s) = l_part();
    let e = flat::of(&mut s, "p", Some("nope"), None, &Options::default()).unwrap_err();
    assert_eq!(e.code, Code::NotFound);
    let o = Options {
        fixed: None,
        bends: vec![json!({"bend": "p/bl/00000000000000000000000000000001", "k_factor": "0.5"})],
    };
    let e = flat::of(&mut s, "p", None, None, &o).unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/bends/0/bend"))
    );
}

#[test]
fn flat_get_answers_the_pattern_in_millimetres() {
    let (_dir, mut s) = l_part();
    let v = must(&mut s, "flat.get", json!({"part": "p"}));
    assert_eq!(
        (
            v["schema"].as_i64(),
            v["solid"].as_str(),
            v["from"].as_str()
        ),
        (Some(1), Some("bl"), Some("whole"))
    );
    assert_eq!(v["thickness_m"], 0.0015);
    let loops = v["loops"].as_array().unwrap();
    let outline: f64 = loops
        .iter()
        .filter(|l| l["outer"] == true)
        .map(|l| l["area_mm2"].as_f64().unwrap())
        .sum();
    assert!((outline - 20.0 * l_flat_mm(0.44)).abs() < 1e-6);
    for seg in loops.iter().flat_map(|l| l["segments"].as_array().unwrap()) {
        assert!(seg["edge"].as_str().unwrap().starts_with("p/bl/"), "{seg}");
        assert!(
            ["line", "arc", "circle", "ellipse", "spline"].contains(&seg["kind"].as_str().unwrap())
        );
        assert!(seg["polyline_mm"].as_array().unwrap().len() >= 2);
    }
    let b = &v["bend_lines"][0];
    let sheet = must(&mut s, "sheet.get", json!({"part": "p"}));
    assert_eq!(b["bend"], sheet["bends"][0]["bend"]);
    assert_eq!(
        b["note"],
        "UP 90° R1 K0.44".replace(
            "UP",
            b["direction"].as_str().unwrap().to_uppercase().as_str()
        )
    );
    assert!((b["developed_mm"].as_f64().unwrap() - Q * (1.0 + 0.44 * 1.5)).abs() < 1e-9);
    // An override by text, as the CLI sends it.
    let half = must(
        &mut s,
        "flat.get",
        json!({"part": "p", "bends": [{"bend": b["bend"], "k_factor": "0.5"}]}),
    );
    assert!(
        half["bend_lines"][0]["note"]
            .as_str()
            .unwrap()
            .ends_with("K0.5")
    );
}

/// The flat, its DXF and an override follow a parameter their allowance
/// reads: the folded body's hash doesn't move with it, so the session's
/// flats are keyed by the allowance's value, not its text.
#[test]
fn a_flat_follows_the_parameter_its_allowance_reads() {
    let (_dir, mut s) = project();
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"kf": "0.44"}}),
    );
    chain(
        &mut s,
        "l",
        "xy",
        &[[40.0, 0.0], [0.0, 0.0], [0.0, 30.0]],
        false,
    );
    base(
        &mut s,
        "bl",
        "l",
        json!({"depth": "20 mm", "allowance": {"k_factor": "kf"}}),
    );
    let note = |s: &mut Session, bends: Value| {
        let v = must(s, "flat.get", json!({"part": "p", "bends": bends}));
        let outline: f64 = v["loops"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["outer"] == true)
            .map(|l| l["area_mm2"].as_f64().unwrap())
            .sum();
        let b = &v["bend_lines"][0];
        (
            b["note"].as_str().unwrap().to_owned(),
            outline,
            b["bend"].clone(),
        )
    };
    let dxf = |s: &mut Session| {
        let req = Request::from_rpc("export", Some(json!({"part": "p", "format": "dxf"}))).unwrap();
        ryucad::server::answer_live(s, None, None, req)
            .unwrap()
            .binary
            .unwrap()
    };
    let (before, area, bend) = note(&mut s, json!([]));
    assert!(before.ends_with("K0.44"), "{before}");
    assert!((area - 20.0 * l_flat_mm(0.44)).abs() < 1e-6);
    let dxf_before = dxf(&mut s);
    let over = json!([{"bend": bend, "k_factor": "kf"}]);
    assert!(note(&mut s, over.clone()).0.ends_with("K0.44"));
    must(
        &mut s,
        "param.set",
        json!({"part": "p", "parameters": {"kf": "0.2"}}),
    );
    let (after, area, _) = note(&mut s, json!([]));
    assert!(after.ends_with("K0.2"), "{after}");
    assert!((area - 20.0 * l_flat_mm(0.2)).abs() < 1e-6);
    assert_ne!(dxf(&mut s), dxf_before);
    assert!(note(&mut s, over).0.ends_with("K0.2"));
}

#[test]
fn render_flat_draws_the_flat_and_its_bend_line() {
    let (_dir, mut s) = l_part();
    let req = Request::from_rpc(
        "render",
        Some(json!({"part": "p", "flat": "", "width": 400, "height": 300})),
    )
    .unwrap();
    let a = ryucad::server::answer_live(&mut s, None, None, req).unwrap();
    assert_eq!(
        (a.result["flat"].as_str(), a.result["from"].as_str()),
        (Some("bl"), Some("whole"))
    );
    let png = a.binary.expect("a PNG");
    let mut r = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut px = vec![0; r.output_buffer_size().expect("a size")];
    r.next_frame(&mut px).unwrap();
    let want = ryucad::render::BEND_UP
        .iter()
        .chain(&ryucad::render::BEND_DOWN)
        .copied()
        .collect::<Vec<f32>>();
    let close = |c: &[u8], k: usize| {
        (0..3).all(|i| (f32::from(c[i]) / 255.0 - want[3 * k + i]).abs() < 0.05)
    };
    assert!(
        px.chunks(4).any(|c| close(c, 0) || close(c, 1)),
        "a bend line's colour is drawn"
    );
    let req = Request::from_rpc(
        "render",
        Some(json!({"part": "p", "flat": "", "compare": "HEAD"})),
    )
    .unwrap();
    let e = ryucad::server::answer_live(&mut s, None, None, req).unwrap_err();
    assert_eq!(e.details["param"], "compare");
}

#[test]
fn mesh_get_meshes_the_flat_body_by_its_solid() {
    let (_dir, mut s) = l_part();
    // As JSON: one body, labelled by its solid, its faces the shown
    // body's names.
    let v = must(&mut s, "mesh.get", json!({"part": "p", "flat": ""}));
    assert_eq!(
        (v["flat"].as_str(), v["from"].as_str()),
        (Some("bl"), Some("whole"))
    );
    let bodies = v["bodies"].as_array().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["feature"], "bl");
    let folded = must(&mut s, "mesh.get", json!({"part": "p", "feature": "bl"}));
    let names = |b: &Value| -> std::collections::BTreeSet<String> {
        b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names(&bodies[0]), names(&folded["bodies"][0]));
    assert_ne!(bodies[0]["hash"], folded["bodies"][0]["hash"]);
    // As bytes: the same body, decoded as the window decodes it, its box
    // flat along the pattern's z.
    let req = Request::from_rpc(
        "mesh.get",
        Some(json!({"part": "p", "flat": "", "transfer": "compact"})),
    )
    .unwrap();
    let a = ryucad::server::answer_live(&mut s, None, None, req).unwrap();
    let wire = ryucad::mesh::decode(&a.binary.expect("bytes")).unwrap();
    assert_eq!(wire.len(), 1);
    assert_eq!(wire[0].feature, "bl");
    assert_eq!(wire[0].hash, bodies[0]["hash"]);
    // A solid that isn't the part's is refused as flat.get refuses it.
    let e = call(&mut s, "mesh.get", json!({"part": "p", "flat": "nope"})).unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::NotFound, "{e:?}");
}
