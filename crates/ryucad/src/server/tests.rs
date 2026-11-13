use super::*;

use crate::doc::files::Manifest;
use crate::doc::id::Id;
use crate::doc::project::{MANIFEST, write_json};
use crate::error::Code;
use crate::live::{self, Live};
use crate::mesh;
use crate::notify::Topic;
use crate::proto::{Raw, Request};
use crate::session::Session;
use crate::units::Unit;
use crate::view::Camera;
use ryu_session::job::Job;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;

// The harness at its old path while ryucad 27 runs (design 541, "Paths
// kept"); 27.F removes it.
#[allow(unused_imports)]
pub(crate) use crate::testing::{
    RENDER_BUDGET, call, call_live, drain, fresh, render_pixels, with_datums, with_helix,
    with_part, with_sketch,
};

/// #676: a reference's faces that don't mesh are warned of in every
/// mesh answer, JSON, binary and compact alike, a part's and an
/// assembly's.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a large file: tests/step.rs reads it in release"
)]
fn every_mesh_answer_warns_of_a_references_unmeshed_faces() {
    // #676: NIST FTC 07 kept as a reference has faces that don't mesh;
    // the JSON answer warned of them, binary and compact (`--out`) didn't.
    let (_dir, mut s) = fresh();
    let file = format!(
        "{}/../ryustep/tests/corpus/nist_ftc_07_asme1_rd.stp",
        env!("CARGO_MANIFEST_DIR")
    );
    call(&mut s, "import", json!({"path": file, "part": "f"})).unwrap();
    let unmeshed = |v: &Value| -> Vec<Value> {
        v["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|w| w["code"] == "reference_faces_unmeshed")
            .cloned()
            .collect()
    };
    let want = unmeshed(&call(&mut s, "mesh.get", json!({"part": "f"})).unwrap());
    assert!(
        !want.is_empty(),
        "FTC 07's reference meshes whole now: pick another file"
    );
    for transfer in ["binary", "compact"] {
        let req = Request::from_rpc("mesh.get", Some(json!({"part": "f", "transfer": transfer})))
            .unwrap();
        let a = answer_live(&mut s, None, None, req).unwrap();
        assert_eq!(unmeshed(&a.result), want, "{transfer}");
    }
    // An assembly's occurrence of it warns as the part does, once for
    // the two instances sharing its body.
    call(
        &mut s,
        "part.create",
        json!({"part": "frame", "kind": "assembly"}),
    )
    .unwrap();
    for id in ["i", "j"] {
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": id, "of": "f"}),
        )
        .unwrap();
    }
    let got = unmeshed(&call(&mut s, "mesh.get", json!({"part": "frame"})).unwrap());
    assert_eq!(got, want, "json");
    for transfer in ["binary", "compact"] {
        let req = Request::from_rpc(
            "mesh.get",
            Some(json!({"part": "frame", "transfer": transfer})),
        )
        .unwrap();
        let a = answer_live(&mut s, None, None, req).unwrap();
        assert_eq!(unmeshed(&a.result), want, "assembly, {transfer}");
    }
}

/// #400: a render stops once its geometry passes its budget, naming
/// how many occurrences it drew; under a budget that holds it, it
/// draws as before.
#[test]
fn a_render_past_its_budget_is_refused_and_one_within_it_draws() {
    let (dir, mut s) = crate::assembly::tests::project();
    let add = |s: &mut Session, v: Value| call(s, "feature.add", v).unwrap();
    add(
        &mut s,
        json!({"part": "frame", "type": "instance", "id": "i", "of": "plate"}),
    );
    add(
        &mut s,
        json!({"part": "frame", "type": "instance_pattern", "id": "row",
               "instances": ["i"], "direction": "x", "spacing": "200 mm", "count": 200}),
    );
    let ask = |slot: &mut Option<Session>, budget: usize| {
        RENDER_BUDGET.with(|b| b.set(Some(budget)));
        let req = Request::from_rpc(
            "render",
            Some(json!({"part": "frame", "view": "iso", "width": 64, "height": 48})),
        )
        .unwrap();
        let a = handle_safely(slot, None, None, dir.path(), req);
        RENDER_BUDGET.with(|b| b.set(None));
        a
    };
    let mut slot = Some(Session::open(dir.path()).unwrap());
    let e = ask(&mut slot, 64 * 1024).unwrap_err();
    // The meshes it made count against its budget, and a refused
    // render leaves none of them in the session's mesh cache (#407).
    let held = &slot.as_ref().unwrap().caches.meshes;
    assert_eq!((held.len(), held.bytes()), (0, 0), "{e}");
    assert_eq!(e.code, Code::RenderTooLarge, "{e}");
    assert_eq!(e.details["occurrences"], 200);
    assert!(e.details["drawn"].as_u64().unwrap() < 200, "{e}");
    assert_eq!(e.details["budget"], 64 * 1024);
    assert!(e.message.contains("--hidden"), "{e}");
    let a = ask(&mut slot, 1 << 30).unwrap();
    assert!(a.binary.is_some());
    // One that draws keeps what it meshed (the plate, once).
    assert_eq!(slot.as_ref().unwrap().caches.meshes.len(), 1);
    drop(s);
}

#[test]
fn a_render_of_an_annotation_shows_its_view_and_marks() {
    let (_dir, mut s, view) = crate::annotation::tests::drilled();
    let boss = crate::annotation::tests::on(&view, [0.045, 0.02, 0.011]);
    let marks = crate::proto::MarksArg {
        part: "plate".into(),
        short: Vec::new(),
        list: vec![json!({"kind": "arrow", "in": "view", "xy_m": [[0.0, 0.0], boss]})],
    };
    let a = crate::annotation::create(
        &mut s,
        crate::annotation::By::agent("agent"),
        &[],
        "move it",
        Some(view.clone()),
        Some(&marks),
        &Default::default(),
    )
    .unwrap();
    let id = a["annotation"]["id"].as_str().unwrap().to_owned();
    let png = |s: &mut Session, p: Value| {
        let a = answer_live(s, None, None, Request::from_rpc("render", Some(p)).unwrap()).unwrap();
        (a.result.clone(), a.binary.unwrap())
    };
    let (r, marked) = png(
        &mut s,
        json!({"annotation": id, "width": 400, "height": 300}),
    );
    assert_eq!(r["camera"], json!(crate::view::Camera::from(&view)));
    // The same view without the annotation: the arrow is the difference.
    let camera = json!({"eye": ["120 mm", "-100 mm", "90 mm"], "target": ["30 mm", "20 mm", "2.5 mm"],
                        "up": [0, 0, 1], "projection": {"orthographic": {"height": "80 mm"}}});
    let (_, bare) = png(
        &mut s,
        json!({"part": "plate", "camera": camera, "width": 400, "height": 300}),
    );
    assert_ne!(marked, bare);
    // Asked with a camera too, it is a usage error.
    let e = answer_live(
        &mut s,
        None,
        None,
        Request::from_rpc("render", Some(json!({"annotation": id, "camera": camera}))).unwrap(),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::Usage);
    // An annotation with no view can't be rendered so.
    let plain = crate::annotation::create(
        &mut s,
        crate::annotation::By::agent("agent"),
        &[],
        "words only",
        None,
        None,
        &Default::default(),
    )
    .unwrap();
    let e = answer_live(
        &mut s,
        None,
        None,
        Request::from_rpc(
            "render",
            Some(json!({"annotation": plain["annotation"]["id"]})),
        )
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
}

#[test]
fn renders_without_marks_asked_for_are_unchanged() {
    let (_dir, mut s, view) = crate::annotation::tests::drilled();
    let render = |s: &mut Session, annotations: bool| {
        let p = json!({"part": "plate", "view": "iso", "width": 200, "height": 150, "annotations": annotations});
        answer_live(s, None, None, Request::from_rpc("render", Some(p)).unwrap())
            .unwrap()
            .binary
            .unwrap()
    };
    let before = render(&mut s, false);
    let marks = crate::proto::MarksArg {
        part: "plate".into(),
        short: Vec::new(),
        list: vec![json!({"kind": "line", "in": "view", "xy_m": [[0.0, 0.0], [0.01, 0.0]]})],
    };
    crate::annotation::create(
        &mut s,
        crate::annotation::By::agent("agent"),
        &[],
        "a line",
        Some(view),
        Some(&marks),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(render(&mut s, false), before);
    assert_ne!(
        render(&mut s, true),
        before,
        "--annotations draws open marks"
    );
}

#[test]
fn an_annotation_rendered_beside_the_open_ones_is_drawn_once() {
    let (_dir, mut s, view) = crate::annotation::tests::drilled();
    let boss = crate::annotation::tests::on(&view, [0.045, 0.02, 0.011]);
    let pin = |x: f64| {
        vec![crate::proto::PinArg {
            reference: None,
            part: Some("plate".into()),
            point: Some([x, 0.02, 0.005].map(|v| json!(format!("{v} m")))),
        }]
    };
    crate::annotation::create(
        &mut s,
        crate::annotation::By::agent("agent"),
        &pin(0.01),
        "older",
        Some(view.clone()),
        None,
        &Default::default(),
    )
    .unwrap();
    let marks = crate::proto::MarksArg {
        part: "plate".into(),
        short: Vec::new(),
        list: vec![json!({"kind": "arrow", "in": "view", "xy_m": [[0.0, 0.0], boss]})],
    };
    let asked = crate::annotation::create(
        &mut s,
        crate::annotation::By::agent("agent"),
        &pin(0.02),
        "move it",
        Some(view.clone()),
        Some(&marks),
        &Default::default(),
    )
    .unwrap();
    let id = asked["annotation"]["id"].as_str().unwrap().to_owned();
    let a = crate::annotation::one(&s, &id, None).unwrap();
    let scene = |s: &mut Session, all: bool| {
        let mut ask = RenderAsk::pick(
            "plate",
            crate::view::Camera::from(&view),
            Vec::new(),
            Default::default(),
        );
        ask.pick = false;
        ask.annotations = all;
        ask.annotation = Some(a.clone());
        render_scene(s, None, Vec::new(), ask).unwrap().g
    };
    // Alone: its arrow (a line and a head) and its pin by its place.
    let g = scene(&mut s, false);
    assert_eq!(g.overlay.len(), 2);
    let labels: Vec<&str> = g.labels.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(labels, ["1"]);
    // Beside the open ones: still drawn once, its pin under its number
    // (the second open), the older one's under its own.
    let g = scene(&mut s, true);
    assert_eq!(g.overlay.len(), 2, "its marks once");
    let mut labels: Vec<&str> = g.labels.iter().map(|l| l.text.as_str()).collect();
    labels.sort_unstable();
    assert_eq!(labels, ["1", "2"], "each pin once, one numbering");
}

#[test]
fn a_scene_for_a_fixed_camera_holds_the_tagged_model() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
    )
    .unwrap();
    let camera = crate::view::Camera {
        eye: [0.1, -0.1, 0.1],
        target: [0.005, 0.005, 0.005],
        up: [0.0, 0.0, 1.0],
        projection: crate::view::Projection::Orthographic { height: 0.05 },
    };
    let ask = RenderAsk::pick("p", camera.clone(), Vec::new(), Default::default());
    let scene = render_scene(&mut s, None, Vec::new(), ask).unwrap();
    assert_eq!(scene.camera, camera);
    assert_eq!(scene.g.tags.as_ref().unwrap().triangles.len(), 6);
    assert!(scene.g.labels.is_empty());
}

#[test]
fn a_tagged_scene_names_every_face_edge_and_vertex() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
    )
    .unwrap();
    let part = Id::parse("p", "part").unwrap();
    let (_, m) = meshes(&mut s, &part, None, mesh::Quality::Normal)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let m = m.unwrap();
    let mut g = crate::render::Geometry {
        tags: Some(Default::default()),
        ..Default::default()
    };
    draw_body(
        &mut g,
        &m,
        &ryukei::geom::Rigid::IDENTITY,
        "p/b",
        &BTreeMap::new(),
        (None, &BTreeMap::new(), None),
        (false, true),
    );
    let t = g.tags.unwrap();
    assert_eq!(t.triangles.len(), 6, "a run of triangles per face");
    assert_eq!(
        t.triangles.iter().map(|(r, _)| r.len()).sum::<usize>(),
        g.triangles.len()
    );
    assert_eq!(t.lines.len(), 12, "every edge");
    assert_eq!(t.points.len(), 8, "every vertex");
    assert!(
        t.names
            .iter()
            .all(|n| n.starts_with("p/b/") && n.len() == 4 + 32)
    );
}

#[test]
fn sheet_metal_through_the_protocol() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
    )
    .unwrap();
    let mut ops = Vec::new();
    for (i, at) in [[0, 0], [60, 0], [60, 40], [0, 40]].into_iter().enumerate() {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    for i in 1..=4 {
        ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                        "start": format!("p{i}"), "end": format!("p{}", i % 4 + 1)}));
    }
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": ops}),
    )
    .unwrap();
    // A plate; a bare K-factor is a number.
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "base_flange", "id": "plate",
        "sketch": "s", "thickness": "1.5 mm", "radius": "1 mm", "allowance": {"k_factor": 0.44}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["thickness"]["m"], 0.0015);
    assert_eq!(r["feature"]["allowance"]["k_factor"]["value"], "0.44");
    // The right top edge, by its vertices.
    let body = call(&mut s, "inspect", json!({"part": "p", "feature": "plate"})).unwrap()["bodies"]
        [0]
    .clone();
    let at = |p: [f64; 3]| -> String {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k] * 1e-3).abs() < 1e-9))
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .unwrap()
    };
    let (a, b) = (at([60.0, 0.0, 1.5]), at([60.0, 40.0, 1.5]));
    let edge = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .map(|e| format!("p/plate/{}", e["name"].as_str().unwrap()))
        .unwrap();
    // A flange: its target filled in; its length through an extent, as
    // the CLI's --length sends it; its radius its founder's.
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "flange", "id": "f",
        "edges": [edge], "angle": "90 deg", "extent": {"length": "20 mm"}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["target"], "plate", "{r}");
    assert_eq!(r["feature"]["length"]["m"], 0.02, "{r}");
    assert_eq!(r["feature"]["radius"]["m"], 0.001, "{r}");
    assert_eq!(
        r["feature"]["inherited"],
        json!(["radius", "relief"]),
        "{r}"
    );
    assert_eq!(r["feature"]["edges"], json!([edge]), "{r}");
    // An end set back with a relief, by vertex.
    let corner = format!("p/plate/{b}");
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "f",
        "ends": [{"vertex": corner, "setback": "5 mm",
                  "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}]}),
    )
    .unwrap();
    assert_eq!(r["feature"]["ends"][0]["vertex"], corner, "{r}");
    let rb = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    assert_eq!(rb["parts"][0]["failed"], 0, "{rb}");
    // Refusals made when written: a face where an edge goes, and an end
    // no edge of it has.
    let face = format!("p/plate/{}", body["faces"][0]["name"].as_str().unwrap());
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "f", "edges": [face]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    let far = format!("p/plate/{}", at([0.0, 0.0, 0.0]));
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "f",
        "ends": [{"vertex": far, "setback": "1 mm"}]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
    // What the kernel refuses only rebuild tells: set back with no relief.
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "f", "ends": [],
                                        "setback": "5 mm"}),
    )
    .unwrap();
    let rb = call(&mut s, "rebuild", json!({"part": "p"}));
    let e = rb.unwrap_err();
    let f = &e.details["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == "f")
        .unwrap()["error"];
    assert_eq!(
        (f["code"].as_str(), f["details"]["reason"].as_str()),
        (Some("sheet_metal_failed"), Some("relief_needed")),
        "{f}"
    );
    // Deleting the base flange while the flange reads it.
    let e = call(
        &mut s,
        "feature.delete",
        json!({"part": "p", "id": "plate"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::HasDependents, "{e}");
}

#[test]
fn unfolds_and_refolds_through_the_protocol() {
    let (_dir, mut s) = fresh();
    crate::sheet_metal::tests::tray(&mut s, "p");
    let sheet = call(&mut s, "sheet.get", json!({"part": "p"})).unwrap();
    let bend = sheet["bends"][0]["bend"].as_str().unwrap().to_owned();
    let side = sheet["flanges"][0]["sides"][0][0]
        .as_str()
        .unwrap()
        .to_owned();
    // An unfold: its target filled in, its fixed face written `fixed`,
    // its allowance its founder's until it has its own.
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "unfold", "id": "u",
        "fixed_face": side, "bends": [{"bend": bend, "deduction": "2.2 mm"}]}),
    )
    .unwrap();
    assert_eq!(r["feature"]["target"], "plate", "{r}");
    assert_eq!(r["feature"]["fixed"], side, "{r}");
    assert_eq!(r["feature"]["bends"][0]["bend"], bend, "{r}");
    assert_eq!(r["feature"]["bends"][0]["deduction"]["m"], 0.0022, "{r}");
    assert_eq!(
        r["feature"]["allowance"]["k_factor"]["value"], "0.44",
        "{r}"
    );
    assert_eq!(r["feature"]["inherited"], json!(["allowance"]), "{r}");
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "u",
        "allowance": {"k_factor": "0.5"}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["inherited"], json!([]), "{r}");
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "u", "allowance": ""}),
    )
    .unwrap();
    assert_eq!(r["feature"]["inherited"], json!(["allowance"]), "{r}");
    // The rebuild report gives the flat.
    let rb = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    let u = rb["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == "u")
        .unwrap()
        .clone();
    assert_eq!(u["flat"]["bends"], 4, "{u}");
    assert_eq!(u["flat"]["partial"], false, "{u}");
    assert!(u["flat"]["box_m"][1][0].as_f64().unwrap() > 0.06, "{u}");
    // A refold, its unfold named.
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "refold", "id": "rf", "unfold": "u", "only": [bend]}),
    )
    .unwrap();
    assert_eq!(r["feature"]["unfold"], "u", "{r}");
    assert_eq!(r["feature"]["only"], json!([bend]), "{r}");
    let rb = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    assert_eq!(rb["parts"][0]["failed"], 0, "{rb}");
    // Refused when written: a side face where a bend goes; an unfold
    // the refold doesn't fold by; the unfold deleted while the refold
    // reads it.
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "u", "only": [side]}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/only/0")),
        "{e}"
    );
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "rf", "unfold": "f"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/unfold")),
        "{e}"
    );
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "u"})).unwrap_err();
    assert_eq!(e.code, Code::HasDependents, "{e}");
    // A field another type takes is refused.
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "rf", "angle": "90 deg"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
}

#[test]
fn a_split_by_a_tool_is_checked_when_written_and_reports_its_sides() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        call(s, "feature.add", f)
    };
    add(
        &mut s,
        json!({"type": "box", "id": "h", "params": {"x": 10, "y": 10, "z": 10}}),
    )
    .unwrap();
    add(
        &mut s,
        json!({"type": "box", "id": "t", "params": {"x": 12, "y": 12, "z": 10},
               "placement": {"origin": [-1, -1, 5]}}),
    )
    .unwrap();
    // Its own target, and a body the part hasn't, are refused.
    let e = add(
        &mut s,
        json!({"type": "split", "id": "x", "target": "h", "tool": "h"}),
    )
    .unwrap_err();
    assert_eq!(e.details["pointer"], "/tool/body", "{e}");
    let e = add(
        &mut s,
        json!({"type": "split", "id": "x", "target": "h", "tool": "zz"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::NotFound, "{e}");
    // Faces of two bodies.
    let face = |s: &mut Session, f: &str| {
        let v = call(s, "inspect", json!({"part": "p", "feature": f})).unwrap();
        format!(
            "p/{f}/{}",
            v["bodies"][0]["faces"][0]["name"].as_str().unwrap()
        )
    };
    let (fh, ft) = (face(&mut s, "h"), face(&mut s, "t"));
    let e = add(
        &mut s,
        json!({"type": "split", "id": "x", "target": "h", "tool_faces": [fh, ft]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::TargetsMixed, "{e}");
    add(
        &mut s,
        json!({"type": "split", "id": "sp", "target": "h", "tool": "t"}),
    )
    .unwrap();
    let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    let line = r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "sp")
        .unwrap()
        .clone();
    assert_eq!(line["ok"], true, "{line}");
    assert_eq!(line["split"]["tool"], json!({"body": "t"}), "{line}");
    assert_eq!(line["split"]["tool_kind"], "solid", "{line}");
    let sides = line["split"]["sides"].as_array().unwrap();
    assert_eq!(sides.len(), 2, "{line}");
    assert_eq!(
        sides[0],
        json!({"side": "behind", "solid": "h", "lumps": 1, "kind": "solid"})
    );
}

#[test]
fn a_mirror_of_a_sheet_is_new_and_an_offset_of_one_a_sheet() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
    )
    .unwrap();
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}]}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "blind", "length": "20 mm"}}),
    )
    .unwrap();
    // Left out, the operation is new for a sheet.
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "mirror", "id": "m", "solid": "w", "plane": "zx"}),
    )
    .unwrap();
    let shown = call(&mut s, "feature.show", json!({"part": "p", "id": "m"})).unwrap();
    assert_eq!(shown["operation"], "new", "{shown}");
    assert!(shown.get("targets").is_none(), "{shown}");
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "offset", "id": "o", "target": "w", "distance": "2 mm"}),
    )
    .unwrap();
    let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    let solids = r["parts"][0]["solids"].as_array().unwrap();
    for id in ["m", "w"] {
        let b = solids
            .iter()
            .find(|x| x["solid"] == id)
            .unwrap_or_else(|| panic!("{r}"));
        assert_eq!(b["kind"], "sheet", "{b}");
        assert_eq!(b["ok"], true, "{b}");
    }
}

#[test]
fn mirrors_and_splits_through_the_protocol() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        call(s, "feature.add", f)
    };
    add(
        &mut s,
        json!({"type": "box", "id": "half", "params": {"x": 10, "y": 4, "z": 2}}),
    )
    .unwrap();
    // Left out, a mirror joins its source.
    let v = add(
        &mut s,
        json!({"type": "mirror", "id": "m", "solid": "half", "plane": "yz"}),
    )
    .unwrap();
    assert_eq!(
        (&v["feature"]["operation"], &v["feature"]["targets"]),
        (&json!("join"), &json!(["half"]))
    );
    let f = call(&mut s, "feature.show", json!({"part": "p", "id": "m"})).unwrap();
    assert_eq!(f["plane"], "yz");
    // Its rebuild report says where the plane is and what the copy did.
    let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    let m = &r["parts"][0]["features"][1];
    assert_eq!(m["ok"], true, "{m}");
    assert_eq!(m["mirror"]["plane"]["normal"], json!([1.0, 0.0, 0.0]));
    // A new copy about zx, as a solid of its own.
    add(
        &mut s,
        json!({"type": "mirror", "id": "c", "solid": "half", "plane": "zx",
                       "operation": "new"}),
    )
    .unwrap();
    let v = call(&mut s, "feature.show", json!({"part": "p", "id": "c"})).unwrap();
    assert_eq!(v["operation"], "new");
    assert!(v.get("targets").is_none());
    // A split by a datum: both sides, each with its lumps.
    add(
        &mut s,
        json!({"type": "plane", "id": "cut", "by": "offset", "from": "yz",
                       "distance": "-2 mm"}),
    )
    .unwrap();
    add(
        &mut s,
        json!({"type": "split", "id": "sp", "target": "half", "plane": "p/cut"}),
    )
    .unwrap();
    let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    let sp = r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "sp")
        .unwrap()
        .clone();
    assert_eq!(sp["ok"], true, "{sp}");
    assert_eq!(sp["solids"], json!(["half", "sp"]));
    assert_eq!(
        sp["split"]["sides"],
        json!([{"side": "behind", "solid": "half", "lumps": 1, "kind": "solid"},
               {"side": "front", "solid": "sp", "lumps": 1, "kind": "solid"}])
    );
    // Refusals at write time: a plane that isn't one, a solid that isn't.
    let e = add(
        &mut s,
        json!({"type": "split", "id": "bad", "target": "half", "plane": "z"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["role"].as_str()),
        (Code::ReferenceWrongKind, Some("plane"))
    );
    let e = add(
        &mut s,
        json!({"type": "mirror", "id": "bad", "solid": "cut", "plane": "yz"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    // Keeping one side while something reads the side in front.
    add(
        &mut s,
        json!({"type": "box", "id": "on", "operation": "join", "targets": ["sp"],
                       "params": {"x": 1, "y": 1, "z": 1}}),
    )
    .unwrap();
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "sp", "keep": "behind"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::HasDependents, "{e}");
    // Edited: flipped; and a mirror cut from another solid.
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "sp", "flip": true}),
    )
    .unwrap();
    let v = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "c", "operation": "cut",
                                                "targets": ["half"]}),
    )
    .unwrap();
    assert_eq!(v["feature"]["targets"], json!(["half"]));
}

/// Each face of `feature`'s body, as `p/<feature>/<name>`.
fn inspect_faces(s: &mut Session, feature: &str) -> Vec<Value> {
    let v = call(s, "inspect", json!({"part": "p", "feature": feature})).unwrap();
    v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| json!(format!("p/{feature}/{}", f["name"].as_str().unwrap())))
        .collect()
}

#[test]
fn sews_and_thickens_of_solids_are_refused_in_a_part_of_solids() {
    let (_dir, mut s) = fresh();
    let must = |s: &mut Session, m: &str, v: Value| call(s, m, v).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    for (id, at) in [("a", 0), ("b", 20)] {
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": id,
            "params": {"x": 10, "y": 10, "z": 10}, "placement": {"origin": [at, 0, 0]}}),
        );
    }
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sew", "sheets": ["a", "b"]}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["wanted"].as_str()),
        (Code::WrongBodyKind, Some("sheet")),
        "{e}"
    );
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "thicken", "sheet": "a", "front": "1 mm"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["role"].as_str()),
        (Code::WrongBodyKind, Some("from")),
        "{e}"
    );
    // Nothing was written.
    let v = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
    assert_eq!(v["features"].as_array().map(Vec::len), Some(2), "{v}");
}

#[test]
fn sews_and_thickens_are_refused_when_written() {
    let (_dir, mut s) = fresh();
    let must = |s: &mut Session, m: &str, v: Value| call(s, m, v).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b",
        "params": {"x": 10, "y": 10, "z": 10}}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "c",
        "params": {"x": 1, "y": 1, "z": 1}, "placement": {"origin": [20, 0, 0]}}),
    );
    let faces = inspect_faces(&mut s, "b");
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extract", "id": "x",
        "faces": [faces[0].clone()]}),
    );
    // A sew of a solid: wrong_body_kind, naming it.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sew",
        "sheets": ["x", "c"]}),
    )
    .unwrap_err();
    assert_eq!(
        (
            e.code,
            e.details["solid"].as_str(),
            e.details["wanted"].as_str()
        ),
        (Code::WrongBodyKind, Some("c"), Some("sheet")),
        "{e}"
    );
    // A thicken of a solid by name: wrong_body_kind too.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "thicken",
        "sheet": "c", "front": "1 mm"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["role"].as_str()),
        (Code::WrongBodyKind, Some("from")),
        "{e}"
    );
    // A thicken whose group names a face it doesn't thicken.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "thicken",
        "from_faces": [faces[0].clone()], "front": "1 mm",
        "face_sides": [{"front": "2 mm", "back": "0", "faces": [faces[1].clone()]}]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam, "{e}");
    // After a thicken consumes x, nothing acts on it.
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extract", "id": "y",
        "faces": [faces[1].clone()]}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "thicken", "id": "t",
        "sheet": "x", "front": "1 mm"}),
    );
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sew",
        "sheets": ["x", "y"]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::BodyConsumed, "{e}");
}

#[test]
fn fills_and_extracts_through_the_protocol() {
    let (_dir, mut s) = fresh();
    let must = |s: &mut Session, m: &str, v: Value| call(s, m, v).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b",
        "params": {"x": 10, "y": 10, "z": 10}}),
    );
    let b = inspect_faces(&mut s, "b");
    let x = must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extract", "id": "x",
        "faces": [b[0].clone()]}),
    );
    assert_eq!(x["feature"]["type"], "extract", "{x}");
    // A face of another body beside it is targets_mixed.
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "c",
        "params": {"x": 1, "y": 1, "z": 1}, "placement": {"origin": [20, 0, 0]}}),
    );
    let c = inspect_faces(&mut s, "c");
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extract",
        "faces": [b[0].clone(), c[0].clone()]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::TargetsMixed, "{e}");
    // A fill's sides as text; its tolerance and angle written with defaults.
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
        {"op": "add", "id": "p1", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "p2", "type": "point", "at": [10, 0]},
        {"op": "add", "id": "p3", "type": "point", "at": [0, 10]},
        {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
        {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p1"}]}),
    );
    let f = must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "fill", "id": "f",
        "sides": [{"curve": "p/s/l1"}, {"curve": "p/s/l2"}, {"curve": "p/s/l3"}]}),
    );
    assert_eq!(f["feature"]["tolerance"]["value"], "0.001 mm", "{f}");
    // An edge side that names a face is refused when written.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "fill",
        "sides": [{"edge": b[0].clone()}, {"curve": "p/s/l2"}, {"curve": "p/s/l3"}]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
}

#[test]
fn surface_sweeps_and_planars_are_written_by_feature_add() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
    )
    .unwrap();
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}]}),
    )
    .unwrap();
    let v = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extrude", "id": "w", "sketch": "s", "surface": true,
               "extent": {"type": "blind", "length": "5 mm"}}),
    )
    .unwrap();
    assert_eq!(v["feature"]["surface"], true, "{v}");
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extrude", "sketch": "s", "surface": true,
               "operation": "join", "targets": ["w"],
               "extent": {"type": "blind", "length": "5 mm"}}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["pointer"].as_str()),
        (Code::InvalidParam, Some("/operation")),
        "{e}"
    );
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "planar", "sketch": "s"}),
    )
    .unwrap_err();
    assert_eq!(e.details["reason"], "open_profile", "{e}");
    let v = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "w", "surface": false}),
    );
    // An open line makes no solid: back from a surface, the extrude
    // is refused or fails, never written as a surface.
    if let Ok(v) = v {
        assert!(v["feature"].get("surface").is_none(), "{v}");
    }
}

#[test]
fn reports_and_inspect_say_what_kind_each_body_is() {
    let (_dir, mut s, _) = crate::sketch::tests::with_box();
    let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    let solid = &r["parts"][0]["solids"][0];
    assert_eq!(solid["kind"], "solid", "{r}");
    assert!(solid.get("boundaries").is_none(), "{r}");
    let i = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
    let body = &i["bodies"][0];
    assert_eq!(body["kind"], "solid", "{i}");
    let edges = body["edges"].as_array().unwrap();
    assert!(!edges.is_empty(), "{i}");
    assert!(edges.iter().all(|e| e.get("laminar").is_none()), "{i}");
}

#[test]
fn datums_are_cut_along_looked_at_hidden_and_inspected() {
    let (_dir, mut s, _) = with_datums();
    // A section on the datum plane, 8 mm up.
    let arg = crate::section::PlaneArg {
        plane: Some("p/d1".into()),
        ..Default::default()
    };
    let cut = arg.resolve(&mut s, "/planes/0").unwrap();
    assert_eq!((cut.origin_m[2], cut.normal), (0.008, [0.0, 0.0, 1.0]));
    let axis = crate::section::PlaneArg {
        plane: Some("p/a1".into()),
        ..Default::default()
    };
    assert_eq!(
        axis.resolve(&mut s, "/planes/0").unwrap_err().code,
        Code::InvalidParam
    );
    // Looking normal to it looks down; along the axis, along y.
    let (dir_d, _) = normal_to_face(&mut s, "p/d1").unwrap();
    assert_eq!(dir_d.map(f64::abs), [0.0, 0.0, 1.0]);
    let (dir_a, _) = normal_to_face(&mut s, "p/a1").unwrap();
    assert_eq!(dir_a.map(f64::abs), [0.0, 1.0, 0.0]);
    assert!(normal_to_face(&mut s, "p/q1").is_err());
    // Datums are leaves of the hidden set, as solids are, and a
    // highlight resolves one.
    assert_eq!(crate::visibility::reach(&mut s, "p/d1").unwrap(), ["p/d1"]);
    let all = crate::visibility::reach(&mut s, "p").unwrap();
    assert_eq!(all, ["p/b", "p/d1", "p/a1", "p/q1"]);
    let r = crate::entity::EntityRef::parse("p/d1").unwrap();
    assert_eq!(
        r.resolve(&mut s).unwrap(),
        crate::entity::Target::Datum("plane")
    );
    // Inspect gives a datum's geometry, and the model lists them.
    let v = call(&mut s, "inspect", json!({"part": "p", "feature": "a1"})).unwrap();
    assert_eq!(v["datum"]["datum"]["direction"], json!([0.0, 1.0, 0.0]));
    let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
    assert_eq!(v["datums"].as_array().unwrap().len(), 3, "{v}");
}

#[test]
fn datums_are_measured_in_closed_form() {
    let (_dir, mut s, top) = with_datums();
    // One datum measures as itself; two, and a datum and a face, in
    // closed form.
    let m = call(&mut s, "measure", json!({"entities": ["p/q1"]})).unwrap();
    assert_eq!(m["kind"], "datum point");
    let m = call(&mut s, "measure", json!({"entities": ["p/q1", "p/d1"]})).unwrap();
    assert_eq!(m["distance"]["m"], 0.005);
    assert_eq!(m["distance"]["exact"], true);
    let m = call(&mut s, "measure", json!({"entities": ["p/a1", top]})).unwrap();
    assert_eq!(m["distance"]["m"], 0.005);
    assert_eq!(m["angle"]["value"], "0 deg", "{m}");
    let e = call(&mut s, "measure", json!({"entities": ["p/d1", "p/b"]})).unwrap_err();
    assert_eq!(e.details["reason"], "datum_against_body");
}

#[test]
fn paths_are_hidden_highlighted_inspected_and_measured() {
    let (_dir, mut s) = with_helix();
    // Leaves of the hidden set, as datums are, and highlighted alike.
    assert_eq!(crate::visibility::reach(&mut s, "p/h").unwrap(), ["p/h"]);
    let all = crate::visibility::reach(&mut s, "p").unwrap();
    assert_eq!(all, ["p/b", "p/h", "p/w"]);
    let r = crate::entity::EntityRef::parse("p/w").unwrap();
    assert_eq!(
        r.resolve(&mut s).unwrap(),
        crate::entity::Target::Datum("path")
    );
    // Inspect gives a helix's numbers, and the model lists both.
    let v = call(&mut s, "inspect", json!({"part": "p", "feature": "h"})).unwrap();
    assert_eq!(v["helix"]["helix"]["helix"]["turns"], 10.0, "{v}");
    let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
    assert_eq!(v["paths"].as_array().unwrap().len(), 2, "{v}");
    assert!(v.get("datums").is_none());
    // Measured alone, a path is its length, certified.
    let m = call(&mut s, "measure", json!({"entities": ["p/w"]})).unwrap();
    assert_eq!(m["kind"], "path");
    let want = 10.0
        * (0.002f64 * 0.002 + 0.016 * 0.016 * core::f64::consts::PI * core::f64::consts::PI).sqrt();
    let got = m["length"]["m"].as_f64().unwrap();
    let bound = m["length"]["bound_m"].as_f64().unwrap();
    assert!((got - want).abs() < 1e-6 + bound, "{m}");
    assert_eq!(m["segments"], 1);
    let m = call(&mut s, "measure", json!({"entities": ["p/h"]})).unwrap();
    assert_eq!(m["kind"], "helix");
    assert_eq!(m["turns"], 10.0);
    let e = call(&mut s, "measure", json!({"entities": ["p/w", "p/b"]})).unwrap_err();
    assert_eq!(e.details["reason"], "path_against_entity");
}

#[test]
fn renders_draw_annotations_only_when_asked() {
    use crate::render::ANNOTATION;
    let (dir, mut s, face) = crate::annotation::tests::plate();
    crate::annotation::tests::put(
        &mut s,
        &crate::annotation::tests::comment(
            "aaaaaaaaaa",
            vec![crate::annotation::Pin::Entity {
                reference: face,
                point_m: Some([0.0, 0.0, 0.005]),
            }],
        ),
    );
    drop(s);
    let count = |annotations: bool| {
        let mut p = json!({"part": "plate", "view": "iso", "width": 200, "height": 150});
        if annotations {
            p["annotations"] = json!(true);
        }
        let px = ANNOTATION.map(|x| (x * 255.0 + 0.5) as u8);
        let s = Session::open(dir.path()).unwrap();
        render_pixels(dir.path(), s, p)
            .into_iter()
            .filter(|p| *p == px)
            .count()
    };
    assert_eq!(count(false), 0, "only when asked");
    assert!(count(true) > 5, "the pin's marker is drawn");
}

#[test]
fn renders_draw_datums_unless_told_not_to() {
    use crate::render::{DATUM, NOTE};
    let (dir, s, _) = with_datums();
    let count = |params: Value, c: crate::render::Rgb| {
        let px = c.map(|x| (x * 255.0 + 0.5) as u8);
        let s = Session::open(dir.path()).unwrap();
        render_pixels(dir.path(), s, params)
            .into_iter()
            .filter(|p| *p == px)
            .count()
    };
    let base = json!({"part": "p", "view": "front", "width": 160, "height": 120});
    assert!(count(base.clone(), DATUM) > 20);
    let mut off = base.clone();
    off["datums"] = json!(false);
    assert_eq!(count(off, DATUM), 0);
    // A highlighted datum takes its highlight's colour.
    let mut lit = base;
    lit["highlight"] = json!(["p/a1"]);
    assert!(count(lit, NOTE) > 5);
    drop(s);
}

#[test]
fn renders_draw_paths_and_helices_unless_told_not_to() {
    use crate::render::{NOTE, PATH};
    let (dir, s) = with_helix();
    let count = |params: Value, c: crate::render::Rgb| {
        let px = c.map(|x| (x * 255.0 + 0.5) as u8);
        let s = Session::open(dir.path()).unwrap();
        render_pixels(dir.path(), s, params)
            .into_iter()
            .filter(|p| *p == px)
            .count()
    };
    let base = json!({"part": "p", "view": "front", "width": 160, "height": 120});
    assert!(count(base.clone(), PATH) > 50);
    let mut off = base.clone();
    off["datums"] = json!(false);
    assert_eq!(count(off, PATH), 0);
    let mut hidden = base.clone();
    hidden["hidden"] = json!(["p/h", "p/w"]);
    assert_eq!(count(hidden, PATH), 0);
    let mut lit = base;
    lit["highlight"] = json!(["p/w"]);
    assert!(count(lit, NOTE) > 50);
    drop(s);
}

#[test]
fn renders_draw_sketches() {
    use crate::render::{NOTE, SKETCH_WELL};
    let (dir, mut s) = crate::sketch::tests::with_sketch();
    crate::sketch::tests::edit(&mut s, crate::sketch::tests::rectangle_ops()).unwrap();
    let px = |c: crate::render::Rgb| c.map(|x| (x * 255.0 + 0.5) as u8);
    let count = |params: Value, c: [u8; 3]| {
        let s = Session::open(dir.path()).unwrap();
        render_pixels(dir.path(), s, params)
            .into_iter()
            .filter(|p| *p == c)
            .count()
    };
    let base = json!({"view": "top", "width": 120, "height": 90});
    // Fully defined: drawn in the well-defined colour.
    assert!(count(base.clone(), px(SKETCH_WELL)) > 50);
    let mut off = base.clone();
    off["sketches"] = json!(false);
    assert_eq!(count(off, px(SKETCH_WELL)), 0);
    // A highlighted sketch entity takes its highlight's colour.
    let mut lit = base;
    lit["highlight"] = json!(["p/s/l1"]);
    assert!(count(lit, px(NOTE)) > 20);
    drop(s);
}

#[test]
fn init_once() {
    let dir = tempfile::tempdir().unwrap();
    let r = init(dir.path(), Some(Unit::In)).unwrap();
    assert_eq!(r["default_unit"], "in");
    assert_eq!(
        init(dir.path(), None).unwrap_err().code,
        Code::ProjectExists
    );
    let mut s = Session::open(dir.path()).unwrap();
    let e = call(&mut s, "project.init", json!({})).unwrap_err();
    assert_eq!(e.code, Code::ProjectExists);
}

#[test]
fn parts() {
    let (_dir, mut s) = fresh();
    let r = call(&mut s, "part.create", json!({"part": "bracket"})).unwrap();
    assert_eq!(r["part"], "bracket");
    let rev = r["revision"].as_str().unwrap().to_owned();
    let e = call(&mut s, "part.create", json!({"part": "bracket"})).unwrap_err();
    assert_eq!(e.code, Code::DuplicateId);
    let e = call(&mut s, "part.create", json!({"part": "Bracket"})).unwrap_err();
    assert_eq!(e.code, Code::InvalidId);
    let list = call(&mut s, "part.list", json!({})).unwrap();
    assert_eq!(
        list["parts"],
        json!([{"part": "bracket", "kind": "part", "features": 0, "revision": rev}])
    );
    assert_eq!(list["model"], json!(["bracket"]));
    // Once built, a part's summary: its hash and failures.
    let built = call(&mut s, "rebuild", json!({"part": "bracket"})).unwrap();
    let list = call(&mut s, "part.list", json!({})).unwrap();
    assert_eq!(list["parts"][0]["hash"], built["parts"][0]["hash"]);
    assert_eq!(list["parts"][0]["failed"], 0);
    let info = call(&mut s, "project.info", json!({})).unwrap();
    assert_eq!(info["parts"], json!(["bracket"]));
    assert_eq!(info["default_unit"], "mm");
    let e = call(
        &mut s,
        "part.delete",
        json!({"part": "bracket", "expected_revision": "nope"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.details["actual"].as_str()),
        (Code::Conflict, Some(rev.as_str()))
    );
    call(
        &mut s,
        "part.delete",
        json!({"part": "bracket", "expected_revision": rev}),
    )
    .unwrap();
    let e = call(&mut s, "part.delete", json!({"part": "bracket"})).unwrap_err();
    assert_eq!(e.code, Code::NotFound);
}

/// A part deleted is gone from the next answer though the watch keeps
/// no event, as when FSEvents hands the marker's event over before the
/// folder's (issue #263).
#[test]
fn a_deleted_part_is_gone_without_file_events() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), None).unwrap();
    let root = dir.path().canonicalize().unwrap();
    let marks = tempfile::tempdir().unwrap();
    let watch = ryu_session::fresh::Watch::start(&root, &marks.path().join("m"), |_| false);
    let mut s = Session::open(&root).unwrap().watched(watch);
    let listed = |s: &mut Session| -> Vec<Value> {
        let list = call(s, "part.list", json!({})).unwrap();
        list["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["part"].clone())
            .collect()
    };
    assert!(listed(&mut s).is_empty());
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    assert_eq!(listed(&mut s), [json!("p")]);
    call(&mut s, "part.delete", json!({"part": "p"})).unwrap();
    assert!(listed(&mut s).is_empty());
    let w = s.watch_status().unwrap();
    assert_eq!(w["watching"], true, "{w}");
}

#[test]
fn assemblies_are_parts_of_their_own_kind() {
    let (dir, mut s) = fresh();
    let r = call(
        &mut s,
        "part.create",
        json!({"part": "frame", "kind": "assembly"}),
    )
    .unwrap();
    assert_eq!(r["kind"], "assembly");
    let text = fs::read_to_string(dir.path().join("parts/frame/part.json")).unwrap();
    assert!(text.contains("\"part_kind\": \"assembly\""), "{text}");
    assert!(text.contains("\"format\": 2"), "{text}");
    let list = call(&mut s, "part.list", json!({})).unwrap();
    assert_eq!(list["parts"][0]["kind"], "assembly");
    let e = call(
        &mut s,
        "part.create",
        json!({"part": "x", "kind": "drawing"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    assert!(!dir.path().join("parts/x").exists());
}

fn ids(v: &Value) -> Vec<&str> {
    v["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect()
}

#[test]
fn add_list_show() {
    let (dir, mut s) = with_part();
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "cylinder", "params": {"radius": 5, "height": "2 in"},
               "placement": {"origin": [0, 0, "1 m"]}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["id"], "cylinder-1");
    assert_eq!(
        r["feature"]["params"]["radius"],
        json!({"value": "5 mm", "m": 0.005})
    );
    assert_eq!(
        r["feature"]["placement"]["origin"][2],
        json!({"value": "1 m", "m": 1.0})
    );
    call(&mut s, "feature.add", json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "ball", "first": true})).unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "after": "ball"}),
    )
    .unwrap();
    let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
    assert_eq!(ids(&list), ["ball", "sphere-1", "cylinder-1"]);
    let show = call(&mut s, "feature.show", json!({"part": "p", "id": "ball"})).unwrap();
    assert_eq!(show["type"], "sphere");
    let list = call(&mut s, "part.list", json!({})).unwrap();
    assert_eq!(show["revision"], list["parts"][0]["revision"]);
    // The file on disk is canonical and explicit.
    let text = fs::read_to_string(dir.path().join("parts/p/features/cylinder-1.json")).unwrap();
    assert!(crate::doc::canonical::is_canonical(&text));
    assert!(text.contains("\"5 mm\""));
}

#[test]
fn a_primitives_radius_may_come_as_the_radius_field() {
    let (_dir, mut s) = with_part();
    // `ryucad feature add p cylinder --radius 2 --param height=3`.
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "cylinder", "radius": "2", "params": {"height": 3}}),
    )
    .unwrap();
    assert_eq!(
        r["feature"]["params"]["radius"],
        json!({"value": "2 mm", "m": 0.002})
    );
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "cylinder-1", "radius": "4 mm"}),
    )
    .unwrap();
    let show = call(
        &mut s,
        "feature.show",
        json!({"part": "p", "id": "cylinder-1"}),
    )
    .unwrap();
    assert_eq!(show["params"]["radius"]["value"], "4 mm");
    // Both at once is one too many.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "radius": 1, "params": {"radius": 1}}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    assert_eq!(e.details["param"], "radius");
    // A box has no radius, and the refusal says where its sizes go.
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "radius": 1, "params": {"x": 1, "y": 1, "z": 1}}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    assert_eq!(e.details["param"], "radius");
    assert!(
        e.message.contains("(x, y, z)") && e.message.contains("--param NAME=QUANTITY"),
        "{}",
        e.message
    );
}

#[test]
fn add_refuses_bad_input_before_writing() {
    let (dir, mut s) = with_part();
    for (params, code) in [
        (
            json!({"part": "p", "type": "prism"}),
            Code::UnknownFeatureType,
        ),
        (
            json!({"part": "p", "type": "sphere", "params": {}}),
            Code::InvalidParam,
        ),
        (
            json!({"part": "p", "type": "sphere", "params": {"radius": "1 parsec"}}),
            Code::InvalidQuantity,
        ),
        (
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "placement": {"z": [0, 0, 0]}}),
            Code::InvalidPlacement,
        ),
        (
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "after": "ghost"}),
            Code::NotFound,
        ),
        (
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "after": "x", "first": true}),
            Code::Usage,
        ),
        (
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "Bad"}),
            Code::InvalidId,
        ),
        (
            json!({"part": "q", "type": "sphere", "params": {"radius": 1}}),
            Code::NotFound,
        ),
    ] {
        let e = call(&mut s, "feature.add", params.clone()).unwrap_err();
        assert_eq!(e.code, code, "{params}");
    }
    assert_eq!(fs::read_dir(dir.path().join("parts/p")).unwrap().count(), 1);
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
    )
    .unwrap();
    let e = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::DuplicateId);
}

#[test]
fn x_follows_z_when_left_out() {
    let (_dir, mut s) = with_part();
    let r = call(&mut s, "feature.add", json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "placement": {"z": [-2, 0, 0]}})).unwrap();
    assert_eq!(r["feature"]["placement"]["x"], json!([0, 1, 0]));
    let r = call(&mut s, "feature.add", json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "placement": {"z": [0, 1, 0]}})).unwrap();
    assert_eq!(r["feature"]["placement"]["x"], json!([1, 0, 0]));
    // Given explicitly, a parallel x is still refused.
    let e = call(&mut s, "feature.add", json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "placement": {"z": [1, 0, 0], "x": [3, 0, 0]}})).unwrap_err();
    assert_eq!(e.code, Code::InvalidPlacement);
}

#[test]
fn edit_merges() {
    let (_dir, mut s) = with_part();
    call(&mut s, "feature.add", json!({"part": "p", "type": "cone", "params": {"radius": 5, "height": 9, "top_radius": 2}, "id": "c"})).unwrap();
    let r = call(&mut s, "feature.edit", json!({"part": "p", "id": "c", "params": {"height": "1 cm", "top_radius": null}, "placement": {"x": [0, 1, 0]}})).unwrap();
    let f = &r["feature"];
    assert_eq!(f["params"]["height"]["value"], "1 cm");
    assert_eq!(f["params"]["radius"]["value"], "5 mm");
    assert!(f["params"].get("top_radius").is_none());
    assert_eq!(f["placement"]["x"], json!([0, 1, 0]));
    assert_eq!(f["placement"]["z"], json!([0, 0, 1]));
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "c", "params": {"radius": null}}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
}

#[test]
fn move_and_delete() {
    let (dir, mut s) = with_part();
    for id in ["a", "b", "c"] {
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": id}),
        )
        .unwrap();
    }
    let r = call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "a", "after": "c"}),
    )
    .unwrap();
    assert_eq!(r["features"], json!(["b", "c", "a"]));
    let r = call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "a", "first": true}),
    )
    .unwrap();
    assert_eq!(r["features"], json!(["a", "b", "c"]));
    for bad in [
        json!({"part": "p", "id": "a"}),
        json!({"part": "p", "id": "a", "after": "a"}),
    ] {
        assert_eq!(
            call(&mut s, "feature.move", bad).unwrap_err().code,
            Code::Usage
        );
    }
    call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap();
    let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
    assert_eq!(ids(&list), ["a", "c"]);
    assert!(!dir.path().join("parts/p/features/b.json").exists());
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap_err();
    assert_eq!(e.code, Code::NotFound);
}

#[test]
fn writes_check_the_revision() {
    let (dir, mut s) = with_part();
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
    )
    .unwrap();
    let rev = r["revision"].as_str().unwrap().to_owned();
    // Someone else edits the feature on disk.
    let path = dir.path().join("parts/p/features/a.json");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("\"1 mm\"", "\"2 mm\"");
    fs::write(&path, text).unwrap();
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "a", "params": {"radius": 3}, "expected_revision": rev}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::Conflict);
    let show = call(&mut s, "feature.show", json!({"part": "p", "id": "a"})).unwrap();
    assert_eq!(show["params"]["radius"]["value"], "2 mm");
}

#[test]
fn broken_features_are_listed_with_their_error() {
    let (dir, mut s) = with_part();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
    )
    .unwrap();
    fs::write(dir.path().join("parts/p/features/a.json"), "{").unwrap();
    fs::write(dir.path().join("parts/p/features/z.json"), "{}").unwrap();
    let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
    assert_eq!(list["features"][0]["ok"], false);
    assert_eq!(list["features"][0]["error"]["code"], "invalid_json");
    assert_eq!(list["orphans"], json!(["z"]));
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "a", "params": {}}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidJson);
}

#[test]
fn features_that_fail_to_build_are_listed_failed() {
    // Issue #123: a file that loads but doesn't build was listed ok.
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    let unit = |id: &str, x: i32| {
        json!({"part": "p", "type": "box", "id": id, "params": {"x": 10, "y": 10, "z": 10},
               "placement": {"origin": [x, 0, 0]}})
    };
    call(&mut s, "feature.add", unit("a", 0)).unwrap();
    // A cut far from the box takes nothing: its file loads, but it
    // fails to build.
    let mut cut = unit("c", 100);
    cut["operation"] = json!("cut");
    call(&mut s, "feature.add", cut).unwrap();
    // One after it on the same solid is blocked by it.
    let mut d = unit("d", 5);
    d["operation"] = json!("cut");
    call(&mut s, "feature.add", d).unwrap();
    let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
    let f = &list["features"];
    assert_eq!(f[0]["ok"], true);
    assert_eq!(f[1]["ok"], false);
    assert_eq!(f[1]["error"]["code"], "operation_no_effect");
    assert!(f[1].get("blocked").is_none());
    assert_eq!(f[2]["ok"], false);
    assert_eq!(f[2]["blocked"], true);
    // Moved onto the box, it builds.
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "c", "placement": {"origin": [5, 5, 0]}}),
    )
    .unwrap();
    let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
    assert!(
        list["features"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["ok"] == true)
    );
}

#[test]
fn operations_through_the_protocol() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        call(s, "feature.add", f)
    };
    let unit = |id: &str, x: i32| {
        json!({"type": "box", "id": id, "params": {"x": 10, "y": 10, "z": 10},
               "placement": {"origin": [x, 0, 0]}})
    };
    add(&mut s, unit("a", 0)).unwrap();
    // With one solid, a cut acts on it; the file says so.
    let mut cut = unit("c", 5);
    cut["operation"] = json!("cut");
    let r = add(&mut s, cut).unwrap();
    assert_eq!(r["feature"]["operation"], "cut");
    assert_eq!(r["feature"]["targets"], json!(["a"]));
    let show = call(&mut s, "feature.show", json!({"part": "p", "id": "c"})).unwrap();
    assert_eq!(show["targets"], json!(["a"]));
    // With two, it must say which.
    add(&mut s, unit("b", 20)).unwrap();
    let mut join = unit("j", 25);
    join["operation"] = json!("join");
    let e = add(&mut s, join.clone()).unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    assert_eq!(e.details["solids"], json!(["a", "b"]));
    join["targets"] = json!(["b"]);
    add(&mut s, join).unwrap();
    // What isn't a solid, or comes after, is refused.
    let mut bad = unit("x", 0);
    bad["operation"] = json!("cut");
    bad["targets"] = json!(["c"]);
    let e = add(&mut s, bad.clone()).unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    assert_eq!(e.details["solids"], json!(["a"]));
    bad["targets"] = json!(["b"]);
    bad["first"] = json!(true);
    assert_eq!(add(&mut s, bad).unwrap_err().code, Code::ReferenceForward);
    // A founder that stops founding leaves what acts on it with nothing.
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "b", "operation": "join", "targets": ["a"]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::HasDependents, "{e}");
    assert_eq!(e.details["features"], json!(["j"]));
    // A cut made new again drops its targets.
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "c", "operation": "new"}),
    )
    .unwrap();
    assert!(r["feature"].get("targets").is_none());
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "c", "operation": "cut", "targets": ["a"]}),
    )
    .unwrap();
    // A boolean takes its target as target, or as targets' one entry.
    let r = add(
        &mut s,
        json!({"type": "boolean", "id": "u", "op": "unite", "targets": ["a"],
               "tools": ["b"]}),
    )
    .unwrap();
    assert_eq!(r["feature"]["target"], "a");
    let show = call(&mut s, "feature.show", json!({"part": "p", "id": "u"})).unwrap();
    assert_eq!(show["tools"], json!(["b"]));
    let e = call(&mut s, "rebuild", json!({"part": "p"}));
    assert!(e.is_ok(), "{e:?}");
    // After it, b is the boolean's: nothing acts on it.
    let mut late = unit("late", 22);
    late["operation"] = json!("cut");
    late["targets"] = json!(["b"]);
    let e = add(&mut s, late).unwrap_err();
    assert_eq!(e.code, Code::BodyConsumed, "{e}");
    assert_eq!(e.details["consumed_by"], "u");
    // Nor can the boolean move before what acts on its tools.
    let e = call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "u", "after": "b"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::BodyConsumed, "{e}");
    // Fields a type doesn't take are refused.
    let e = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "u", "operation": "cut"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    let e = add(
        &mut s,
        json!({"type": "boolean", "op": "unite", "target": "a",
                               "targets": ["a"], "tools": ["c"]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::Usage);
    // A founder others act on can't go.
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "a"})).unwrap_err();
    assert_eq!(e.code, Code::HasDependents);
}

#[test]
fn fillets_and_chamfers_through_the_protocol() {
    use crate::sketch::tests::{edge_between, face_along, vertex_at, with_box};
    let (dir, mut s, body) = with_box();
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        call(s, "feature.add", f)
    };
    let edit = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        call(s, "feature.edit", f)
    };
    let edge = |a: [f64; 3], b: [f64; 3]| {
        let (a, b) = (vertex_at(&body, a), vertex_at(&body, b));
        format!("p/b/{}", edge_between(&body, &a, &b))
    };
    // The box's vertical edge at its far corner, and its top front edge.
    let up = edge([0.02, 0.01, 0.0], [0.02, 0.01, 0.005]);
    let front = edge([0.0, 0.0, 0.005], [0.02, 0.0, 0.005]);
    let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
    let r = add(
        &mut s,
        json!({"type": "fillet", "id": "f", "radius": "1 mm", "edges": [up]}),
    )
    .unwrap();
    assert_eq!(
        r["feature"]["groups"],
        json!([{"radius": {"value": "1 mm", "m": 0.001}, "edges": [up]}])
    );
    // The file keeps the reference as {feature, name}, and no default.
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("parts/p/features/f.json")).unwrap(),
    )
    .unwrap();
    let name = up.rsplit('/').next().unwrap();
    assert_eq!(
        file["groups"][0]["edges"],
        json!([{"feature": "b", "name": name}])
    );
    assert!(file.get("propagate").is_none());
    assert_eq!(file["format"], 3);
    // One group's fields edit it; propagate is written only when false.
    let r = edit(&mut s, json!({"id": "f", "radius": 2, "propagate": false})).unwrap();
    assert_eq!(r["feature"]["groups"][0]["radius"]["value"], "2 mm");
    assert_eq!(r["feature"]["propagate"], false);
    let r = edit(&mut s, json!({"id": "f", "propagate": true})).unwrap();
    assert!(r["feature"].get("propagate").is_none());
    // Several sizes are several groups, given whole.
    let groups = json!([{"radius": "2 mm", "edges": [up]},
                        {"radius": "0.5 mm", "edges": [front]}]);
    let r = edit(&mut s, json!({"id": "f", "groups": groups})).unwrap();
    assert_eq!(r["feature"]["groups"][1]["edges"], json!([front]));
    let e = edit(&mut s, json!({"id": "f", "radius": 3})).unwrap_err();
    assert_eq!(
        (e.code, e.details["param"].as_str()),
        (Code::InvalidParam, Some("groups"))
    );
    let e = edit(&mut s, json!({"id": "f", "radius": 3, "groups": groups})).unwrap_err();
    assert_eq!(e.code, Code::Usage);
    // A chamfer: equal unless it says otherwise; a new form keeps only
    // what it takes.
    // (The left back vertical edge, clear of the fillet: where a chamfer
    // would meet it the kernel has no corner yet.)
    let back = edge([0.0, 0.01, 0.0], [0.0, 0.01, 0.005]);
    let r = add(
        &mut s,
        json!({"type": "chamfer", "id": "c", "d": "0.5 mm", "edges": [back]}),
    )
    .unwrap();
    assert_eq!(r["feature"]["groups"][0]["form"], "equal");
    let r = edit(
        &mut s,
        json!({"id": "c", "form": "distance_angle", "angle": 30, "face": top}),
    )
    .unwrap();
    let g = &r["feature"]["groups"][0];
    assert_eq!(g["d"]["value"], "0.5 mm");
    assert_eq!(g["angle"]["value"], "30 deg");
    assert_eq!(g["face"], top);
    let r = edit(&mut s, json!({"id": "c", "form": "equal", "d": 1})).unwrap();
    assert!(r["feature"]["groups"][0].get("face").is_none());
    call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    // What can't be blended now is refused before anything is written.
    let e = add(
        &mut s,
        json!({"type": "fillet", "radius": 1, "edges": [top]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    // The fillet took the corner edge: naming it after says so.
    let e = add(
        &mut s,
        json!({"type": "fillet", "radius": 1, "edges": [up]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceUnresolved, "{e}");
    assert_eq!(e.details["removed_by"], "f");
    let e = add(
        &mut s,
        json!({"type": "fillet", "after": "b",
               "groups": [{"radius": 1, "edges": [up]}, {"radius": 2, "edges": [up]}]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::EdgeListedTwice, "{e}");
    add(
        &mut s,
        json!({"type": "box", "id": "o", "params": {"x": 1, "y": 1, "z": 1},
               "placement": {"origin": [30, 0, 0]}}),
    )
    .unwrap();
    let other = {
        let o = call(&mut s, "inspect", json!({"part": "p", "feature": "o"})).unwrap();
        let o = &o["bodies"][0];
        let (a, b) = (
            vertex_at(o, [0.03, 0.0, 0.0]),
            vertex_at(o, [0.03, 0.0, 0.001]),
        );
        format!("p/o/{}", edge_between(o, &a, &b))
    };
    let e = add(
        &mut s,
        json!({"type": "fillet", "radius": 0.1, "edges": [front, other]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::TargetsMixed, "{e}");
    // Fields a fillet doesn't take, and a box's founder others round.
    let e = add(
        &mut s,
        json!({"type": "fillet", "radius": 1, "d": 1, "edges": [front]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap_err();
    assert_eq!(e.code, Code::HasDependents);
    assert_eq!(e.details["features"], json!(["f", "c"]));
}

#[test]
fn holes_through_the_protocol() {
    use crate::sketch::tests::{edge_between, face_along, vertex_at};
    let (dir, mut s) = crate::hole::tests::drilled();
    let body =
        call(&mut s, "inspect", json!({"part": "p", "feature": "b"})).unwrap()["bodies"][0].clone();
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        call(s, "feature.add", f)
    };
    let edit = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        f["id"] = json!("h");
        call(s, "feature.edit", f)
    };
    // A hole cuts the part's one solid when it names none; the file
    // names it all the same, and writes no defaults.
    let r = add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "s", "points": ["p1"],
               "diameter": 4, "depth": {"blind": 3}}),
    )
    .unwrap();
    let h = &r["feature"];
    assert_eq!(h["targets"], json!(["b"]));
    assert_eq!(h["diameter"], json!({"value": "4 mm", "m": 0.004}));
    assert_eq!(h["depth"], json!({"blind": {"value": "3 mm", "m": 0.003}}));
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("parts/p/features/h.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        (&file["format"], &file["form"], &file["points"]),
        (&json!(3), &json!("simple"), &json!(["p1"]))
    );
    assert!(file.get("bottom").is_none());
    // A counterbore needs its own fields; given in part, it keeps the
    // rest; a new form drops what it doesn't take.
    let e = edit(&mut s, json!({"form": "counterbore"})).unwrap_err();
    assert_eq!(e.details["pointer"], "/counterbore", "{e}");
    let r = edit(
        &mut s,
        json!({"form": "counterbore", "counterbore": {"diameter": 8, "depth": 1}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["counterbore"]["diameter"]["value"], "8 mm");
    let r = edit(&mut s, json!({"counterbore": {"depth": 2}})).unwrap();
    let c = &r["feature"]["counterbore"];
    assert_eq!(
        (&c["diameter"]["value"], &c["depth"]["value"]),
        (&json!("8 mm"), &json!("2 mm"))
    );
    let r = edit(&mut s, json!({"form": "simple"})).unwrap();
    assert!(r["feature"].get("counterbore").is_none());
    // A drill point, then flat again; every free point, then p1 again.
    let r = edit(&mut s, json!({"bottom": {"drill": 118}})).unwrap();
    assert_eq!(r["feature"]["bottom"]["drill"]["value"], "118 deg");
    let r = edit(&mut s, json!({"bottom": "flat", "points": []})).unwrap();
    assert!(r["feature"].get("bottom").is_none() && r["feature"].get("points").is_none());
    // Up to the box's bottom face, given as text and kept as a file
    // holds it.
    let bottom = format!("p/b/{}", face_along(&body, [0.0, 0.0, -1.0]));
    let r = edit(
        &mut s,
        json!({"points": ["p1"], "depth": {"up_to": {"target": bottom, "offset": "-1"}}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["depth"]["up_to"]["target"], bottom);
    assert_eq!(r["feature"]["depth"]["up_to"]["offset"]["value"], "-1 mm");
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("parts/p/features/h.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(file["depth"]["up_to"]["target"]["feature"], "b");
    let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    assert!(
        r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["ok"] == true),
        "{r}"
    );
    // What can be judged now is refused before anything is written.
    let edge = {
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.0]),
            vertex_at(&body, [0.02, 0.0, 0.0]),
        );
        format!("p/b/{}", edge_between(&body, &a, &b))
    };
    for (f, code) in [
        (
            json!({"depth": {"up_to": {"target": edge}}}),
            Code::ReferenceWrongKind,
        ),
        (json!({"points": ["p9"]}), Code::NotFound),
        (json!({"points": ["l1"]}), Code::ReferenceWrongKind),
        (json!({"sketch": "b"}), Code::ReferenceWrongKind),
        (json!({"radius": 1}), Code::InvalidParam),
    ] {
        let e = edit(&mut s, f.clone()).unwrap_err();
        assert_eq!(e.code, code, "{f}: {e}");
    }
    // What it drills at, and its sketch, stay while it does.
    let e = call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [{"op": "remove", "id": "p1"}]}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, &e.details["features"]),
        (Code::HasDependents, &json!(["h"]))
    );
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "s"})).unwrap_err();
    assert_eq!(e.code, Code::HasDependents);
    // With two solids a hole names its targets.
    add(
        &mut s,
        json!({"type": "box", "id": "o", "params": {"x": 1, "y": 1, "z": 1},
               "placement": {"origin": [30, 0, 0]}}),
    )
    .unwrap();
    let e = add(
        &mut s,
        json!({"type": "hole", "sketch": "s", "diameter": 1, "depth": "through_all"}),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, &e.details["param"]),
        (Code::InvalidParam, &json!("targets"))
    );
}

#[test]
fn patterns_through_the_protocol() {
    use crate::sketch::tests::{face_along, vertex_at};
    let (dir, mut s) = crate::hole::tests::drilled();
    let body =
        call(&mut s, "inspect", json!({"part": "p", "feature": "b"})).unwrap()["bodies"][0].clone();
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        call(s, "feature.add", f)
    };
    let edit = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        f["id"] = json!("pat");
        call(s, "feature.edit", f)
    };
    let file = |dir: &tempfile::TempDir| -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("parts/p/features/pat.json")).unwrap(),
        )
        .unwrap()
    };
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "s", "points": ["p1"],
               "diameter": 2, "depth": {"blind": 3}}),
    )
    .unwrap();
    // A pattern acts on its seeds' targets when it names none.
    let r = add(
        &mut s,
        json!({"type": "pattern", "id": "pat", "features": ["h"], "direction": "x",
               "spacing": 3, "count": 3}),
    )
    .unwrap();
    let f = &r["feature"];
    assert_eq!(f["targets"], json!(["b"]));
    assert_eq!(f["count"], json!({"value": "3", "n": 3.0}));
    assert_eq!(f["spacing"], json!({"value": "3 mm", "m": 0.003}));
    assert_eq!(
        (&file(&dir)["form"], &file(&dir)["format"]),
        (&json!("linear"), &json!(3))
    );
    // A second direction, a skip, then none of either again.
    let r = edit(
        &mut s,
        json!({"direction2": "y", "spacing2": 3, "count2": 2, "reverse2": true,
               "skip": [[1, 1]]}),
    )
    .unwrap();
    assert_eq!(r["feature"]["skip"], json!([[1, 1]]));
    assert_eq!(r["feature"]["reverse2"], true);
    call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    let e = edit(&mut s, json!({"direction2": ""})).unwrap_err();
    assert_eq!(e.details["pointer"], "/skip/0", "{e}");
    let r = edit(&mut s, json!({"direction2": "", "skip": []})).unwrap();
    assert!(r["feature"].get("direction2").is_none() && r["feature"].get("skip").is_none());
    // A new form keeps only what it takes: round the z axis, then a
    // mirror in the box's top face, given as text.
    let r = edit(
        &mut s,
        json!({"form": "circular", "axis": "z", "count": 4, "angle": 90}),
    )
    .unwrap();
    assert!(r["feature"].get("direction").is_none());
    assert_eq!(r["feature"]["angle"]["value"], "90 deg");
    assert_eq!(r["feature"]["features"], json!(["h"]));
    let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
    let r = edit(&mut s, json!({"form": "mirror", "plane": top})).unwrap();
    assert_eq!(r["feature"]["plane"], top);
    assert!(r["feature"].get("count").is_none());
    assert_eq!(file(&dir)["plane"]["feature"], "b");
    // Rebuild reports give a pattern's form, for the window's icons.
    fn line<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
        match v {
            Value::Object(o) if o.get("id") == Some(&json!(id)) && o.contains_key("ok") => Some(v),
            Value::Object(o) => o.values().find_map(|x| line(x, id)),
            Value::Array(a) => a.iter().find_map(|x| line(x, id)),
            _ => None,
        }
    }
    // (The mirror cuts nothing, so the rebuild fails: its report is in
    // the refusal.)
    let r = match call(&mut s, "rebuild", json!({"part": "p"})) {
        Ok(r) => r,
        Err(e) => e.details.clone(),
    };
    assert_eq!(line(&r, "pat").unwrap()["form"], "mirror", "{r}");
    assert!(line(&r, "h").unwrap().get("form").is_none());
    // What can be judged now is refused before anything is written.
    let vertex = format!("p/b/{}", vertex_at(&body, [0.0, 0.0, 0.0]));
    for (f, code) in [
        (json!({"features": ["b"]}), Code::ReferenceWrongKind),
        (json!({"plane": vertex}), Code::ReferenceWrongKind),
        (json!({"features": ["nope"]}), Code::NotFound),
        (json!({"radius": 1}), Code::InvalidParam),
        // A bare word that isn't an origin axis is a feature of the
        // part: here, none.
        (
            json!({"form": "linear", "direction": "w", "spacing": 1, "count": 2}),
            Code::NotFound,
        ),
    ] {
        let e = edit(&mut s, f.clone()).unwrap_err();
        assert_eq!(e.code, code, "{f}: {e}");
    }
    // A new pattern whose seed can't give it targets says why (#102).
    for (seed, code) in [("nope", Code::NotFound), ("b", Code::ReferenceWrongKind)] {
        let e = add(
            &mut s,
            json!({"type": "pattern", "id": "bad", "features": [seed], "direction": "x",
                   "spacing": 3, "count": 3}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["seed"], &e.details["pointer"]),
            (code, &json!(seed), &json!("/features/0")),
            "{seed}: {e}"
        );
    }
    // What a form needs and wasn't given is a parameter to give (#98).
    for (f, param) in [
        (json!({"axis": "z", "count": 4}), "angle"),
        (json!({"axis": "z", "angle": 90}), "count"),
        (
            json!({"form": "linear", "spacing": 3, "count": 3}),
            "direction",
        ),
        (json!({"direction": "x", "count": 3}), "spacing"),
        (json!({"form": "mirror"}), "plane"),
    ] {
        let mut f = f;
        f["type"] = json!("pattern");
        f["features"] = json!(["h"]);
        let e = add(&mut s, f.clone()).unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::InvalidParam, &json!(param)),
            "{f}: {e}"
        );
    }
    // A new pattern's form, left out, is what its fields say.
    let r = add(
        &mut s,
        json!({"type": "pattern", "id": "turn", "features": ["h"], "axis": "z",
               "count": 2, "angle": 90}),
    )
    .unwrap();
    assert_eq!(r["feature"]["form"], "circular");
    call(&mut s, "feature.delete", json!({"part": "p", "id": "turn"})).unwrap();
    // A seed acting elsewhere than the pattern does.
    add(
        &mut s,
        json!({"type": "box", "id": "o", "params": {"x": 1, "y": 1, "z": 1},
               "placement": {"origin": [30, 0, 0]}, "first": true}),
    )
    .unwrap();
    let e = edit(&mut s, json!({"targets": ["o"]})).unwrap_err();
    assert_eq!(
        (e.code, &e.details["pointer"]),
        (Code::InvalidParam, &json!("/targets"))
    );
    // Its seeds stay while it repeats them.
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "h"})).unwrap_err();
    assert_eq!(
        (e.code, &e.details["features"]),
        (Code::HasDependents, &json!(["pat"]))
    );
}

#[test]
fn the_model_is_its_solids() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        call(s, "feature.add", f).unwrap()
    };
    add(
        &mut s,
        json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "c", "operation": "cut",
               "params": {"x": 4, "y": 20, "z": 20}, "placement": {"origin": [8, -5, -5]}}),
    );
    let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
    let part = &r["parts"][0];
    let solid = &part["solids"][0];
    assert_eq!(part["solids"].as_array().unwrap().len(), 1);
    assert_eq!(solid["solid"], "a");
    assert_eq!(solid["features"], json!(["a", "c"]));
    assert_eq!((&solid["ok"], &solid["shown"]), (&json!(true), &json!("c")));
    assert_eq!(part["features"][1]["solids"], json!(["a"]));
    assert_eq!(part["features"][1]["operation"], "cut");
    assert!(part["features"][0].get("operation").is_none());
    assert_eq!(solid["hash"], part["features"][1]["hash"]);
    // Meshes are the solids', named by founder.
    let m = call(&mut s, "mesh.get", json!({"part": "p"})).unwrap();
    assert_eq!(m["bodies"].as_array().unwrap().len(), 1);
    assert_eq!(m["bodies"][0]["feature"], "a");
    assert_eq!(m["bodies"][0]["hash"], solid["hash"]);
    let e = call(&mut s, "mesh.get", json!({"part": "p", "feature": "c"})).unwrap_err();
    assert_eq!(e.code, Code::ReferenceWrongKind);
    assert_eq!(e.details["solids"], json!(["a"]));
    // Inspect shows the model, or any feature's state.
    let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
    let b = &v["bodies"][0];
    assert_eq!((&b["solid"], &b["feature"]), (&json!("a"), &json!("c")));
    let cut_face = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["origin"]["feature"] == "c")
        .unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned();
    let own = call(&mut s, "inspect", json!({"part": "p", "feature": "a"})).unwrap();
    assert_eq!(own["bodies"][0]["counts"]["faces"], 6);
    // A reference names the solid, and finds what the model shows.
    let one = call(
        &mut s,
        "inspect",
        json!({"part": "p", "entity": format!("p/a/{cut_face}")}),
    )
    .unwrap();
    assert_eq!((&one["solid"], &one["kind"]), (&json!("a"), &json!("face")));
    let mut live = Live::new();
    let human = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human"}),
    )
    .unwrap()["participant"]
        .as_str()
        .unwrap()
        .to_owned();
    let picked = format!("p/a/{cut_face}");
    let r = call_live(
        &mut s,
        &mut live,
        Some(&human),
        "selection.set",
        json!({"entities": [picked]}),
    )
    .unwrap();
    assert_eq!(r["selection"], json!([picked]));
    // The cut's own ID names no solid.
    let e = call_live(
        &mut s,
        &mut live,
        Some(&human),
        "selection.set",
        json!({"entities": [format!("p/c/{cut_face}")]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::NotFound);
    assert_eq!(e.details["errors"][0]["details"]["solids"], json!(["a"]));
    // A tip that fails leaves the solid showing its last good state,
    // and failed.
    add(
        &mut s,
        json!({"type": "box", "id": "miss", "operation": "cut", "targets": ["a"],
               "params": {"x": 1, "y": 1, "z": 1}, "placement": {"origin": [50, 0, 0]}}),
    );
    let e = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
    let solid = &e.details["parts"][0]["solids"][0];
    assert_eq!(
        (&solid["ok"], &solid["shown"]),
        (&json!(false), &json!("c"))
    );
    assert_eq!(solid["error"]["code"], "operation_no_effect");
    let m = call(&mut s, "mesh.get", json!({"part": "p"})).unwrap();
    assert_eq!(m["bodies"][0]["hash"], solid["hash"]);
}

#[test]
fn rebuild_and_inspect() {
    let (_dir, mut s) = with_part();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "params": {"x": 1, "y": 2, "z": 3}, "id": "b"}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "s"}),
    )
    .unwrap();
    let r = call(&mut s, "rebuild", json!({})).unwrap();
    let part = &r["parts"][0];
    assert_eq!(part["failed"], 0);
    assert_eq!(part["features"][1]["type"], "sphere");
    assert_eq!(
        part["features"][0]["counts"],
        json!({"faces": 6, "edges": 12, "vertices": 8})
    );
    let all = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
    assert_eq!(all["bodies"].as_array().unwrap().len(), 2);
    let one = call(&mut s, "inspect", json!({"part": "p", "feature": "s"})).unwrap();
    assert_eq!(one["bodies"][0]["feature"], "s");
    let name = all["bodies"][0]["edges"][3]["name"].as_str().unwrap();
    let e = call(&mut s, "inspect", json!({"part": "p", "entity": name})).unwrap();
    assert_eq!(
        (e["feature"].as_str(), e["kind"].as_str()),
        (Some("b"), Some("edge"))
    );
    let e = call(&mut s, "inspect", json!({"part": "p", "entity": "0"})).unwrap_err();
    assert_eq!(e.code, Code::NotFound);
    for (list, kind) in [("faces", "face"), ("edges", "edge"), ("vertices", "vertex")] {
        let name = all["bodies"][0][list][0]["name"].as_str().unwrap();
        let e = call(&mut s, "inspect", json!({"part": "p", "entity": name})).unwrap();
        assert_eq!(e["kind"], kind);
        // The full reference, as Copy reference and `selection` give it.
        let full = format!("p/b/{name}");
        let e = call(&mut s, "inspect", json!({"part": "p", "entity": full})).unwrap();
        assert_eq!(
            (e["kind"].as_str(), e["feature"].as_str()),
            (Some(kind), Some("b"))
        );
    }
    let elsewhere = format!("q/b/{name}");
    let e = call(&mut s, "inspect", json!({"part": "p", "entity": elsewhere})).unwrap_err();
    assert_eq!(e.code, Code::NotFound);
    let wrong_feature = format!("p/s/{name}");
    let e = call(
        &mut s,
        "inspect",
        json!({"part": "p", "entity": wrong_feature}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::NotFound);
}

#[test]
fn sketches_are_added_on_a_plane() {
    let (dir, mut s) = with_part();
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch"}),
    )
    .unwrap();
    assert_eq!(
        r["feature"],
        json!({"id": "sketch-1", "type": "sketch", "plane": "xy",
               "entities": {}, "constraints": {}})
    );
    let text = fs::read_to_string(dir.path().join("parts/p/features/sketch-1.json")).unwrap();
    assert!(text.contains("\"format\": 2"));
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "side", "plane": "zx"}),
    )
    .unwrap();
    assert_eq!(r["feature"]["plane"], "zx");
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "up",
               "placement": {"origin": [0, 0, "5 mm"]}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["placement"]["origin"][2]["m"], 0.005);
    assert!(r["feature"].get("plane").is_none());
    // Moved to another plane, and off it again.
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "side", "placement": {"origin": [0, "1 mm", 0]}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["placement"]["z"], json!([0, 1, 0]));
    assert_eq!(r["feature"]["placement"]["x"], json!([0, 0, 1]));
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "side", "plane": "yz"}),
    )
    .unwrap();
    assert_eq!(r["feature"]["plane"], "yz");
    let show = call(&mut s, "feature.show", json!({"part": "p", "id": "side"})).unwrap();
    assert_eq!(show["plane"], "yz");
    for (method, params, code) in [
        // Not an origin plane: a feature of the part, which it lacks.
        (
            "feature.add",
            json!({"type": "sketch", "plane": "xz"}),
            Code::NotFound,
        ),
        (
            "feature.add",
            json!({"type": "sketch", "plane": "origin"}),
            Code::InvalidParam,
        ),
        (
            "feature.add",
            json!({"type": "sketch", "params": {"x": 1}}),
            Code::InvalidParam,
        ),
        (
            "feature.add",
            json!({"type": "sketch", "plane": "xy",
                               "placement": {"z": [0, 0, 1]}}),
            Code::Usage,
        ),
        (
            "feature.add",
            json!({"type": "sphere", "params": {"radius": 1},
                               "plane": "xy"}),
            Code::InvalidParam,
        ),
        (
            "feature.edit",
            json!({"id": "side", "params": {"x": 1}}),
            Code::InvalidParam,
        ),
    ] {
        let mut params = params;
        params["part"] = json!("p");
        let e = call(&mut s, method, params.clone()).unwrap_err();
        assert_eq!(e.code, code, "{params}");
    }
}

#[test]
fn sketches_rebuild_to_their_state() {
    let (dir, mut s) = with_sketch();
    let r = call(&mut s, "rebuild", json!({})).unwrap();
    let f = &r["parts"][0]["features"][0];
    assert_eq!(
        (&f["type"], &f["ok"], &f["sketch"]),
        (
            &json!("sketch"),
            &json!(true),
            &json!({"state": "under", "dof": 4, "stale": false})
        )
    );
    // A sketch has no body: no hash, no mesh, nothing to inspect.
    assert!(f.get("hash").is_none());
    assert!(r.get("warnings").is_none());
    let m = call(&mut s, "mesh.get", json!({"part": "p"})).unwrap();
    assert_eq!(m["bodies"], json!([]));
    let i = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
    assert_eq!(i["bodies"], json!([]));
    // A parameter edited behind ryucad's back makes the sketch stale.
    let params =
        |w: &str| json!({"format": 1, "kind": "ryucad.parameters", "parameters": {"width": w}});
    let path = dir.path().join("parts/p/parameters.json");
    write_json(&path, &params("15 mm")).unwrap();
    let r = call(&mut s, "rebuild", json!({})).unwrap();
    assert_eq!(r["parts"][0]["features"][0]["sketch"]["stale"], true);
    assert_eq!(r["warnings"][0]["code"], "sketch_stale");
    assert_eq!(r["warnings"][0]["feature"], "s");
    // One it can't solve fails alone.
    write_json(&path, &params("50 mm")).unwrap();
    let e = call(&mut s, "rebuild", json!({})).unwrap_err();
    let f = &e.details["parts"][0]["features"][0];
    assert_eq!(f["error"]["code"], "sketch_unsolved");
}

/// The rebuild report of feature `f` in part `p`.
fn built(s: &mut Session, f: &str) -> Value {
    let r = match call(s, "rebuild", json!({"part": "p"})) {
        Ok(r) => r,
        Err(e) => e.details,
    };
    r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == f)
        .unwrap()
        .clone()
}

#[test]
fn sketches_extrude_and_revolve_into_bodies() {
    let (dir, mut s) = with_sketch();
    let r = call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extrude", "sketch": "s",
               "extent": {"type": "blind", "length": "width / 4"}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["id"], "extrude-1");
    assert_eq!(
        r["feature"]["extent"]["length"],
        json!({"value": "width / 4", "m": 0.005, "expr": true})
    );
    let text = fs::read_to_string(dir.path().join("parts/p/features/extrude-1.json")).unwrap();
    assert!(
        !text.contains("reverse") && !text.contains("loops"),
        "{text}"
    );
    let b = built(&mut s, "extrude-1");
    assert_eq!(b["counts"], json!({"faces": 6, "edges": 12, "vertices": 8}));
    // About its left side, a full turn: a solid cylinder.
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s",
               "axis": "l4", "angle": "full"}),
    )
    .unwrap();
    let b = built(&mut s, "r");
    assert_eq!(b["ok"], true, "{b}");
    assert_eq!(b["counts"]["faces"], 3);
    // A quarter turn, both ways, has two more faces: its ends.
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "r", "angle": 90, "symmetric": true}),
    )
    .unwrap();
    assert_eq!(r["feature"]["angle"]["value"], "90 deg");
    assert_eq!(built(&mut s, "r")["counts"]["faces"], 5);
    // Reversed and symmetric extrudes, and back.
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "extrude-1", "reverse": true}),
    )
    .unwrap();
    assert_eq!(built(&mut s, "extrude-1")["ok"], true);
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "extrude-1", "reverse": false}),
    )
    .unwrap();
    assert!(r["feature"].get("reverse").is_none());
    let show = call(&mut s, "feature.show", json!({"part": "p", "id": "r"})).unwrap();
    assert_eq!(
        (show["axis"].as_str(), show["symmetric"].as_bool()),
        (Some("l4"), Some(true))
    );
}

#[test]
fn sweeps_refuse_what_cant_build() {
    let (dir, mut s) = with_sketch();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": 1}}),
    )
    .unwrap();
    let blind = json!({"type": "blind", "length": "5 mm"});
    for (params, code) in [
        (
            json!({"type": "extrude", "sketch": "ghost", "extent": blind}),
            Code::NotFound,
        ),
        (
            json!({"type": "extrude", "sketch": "ball", "extent": blind}),
            Code::ReferenceWrongKind,
        ),
        (
            json!({"type": "extrude", "sketch": "s", "extent": blind, "first": true}),
            Code::ReferenceForward,
        ),
        (
            json!({"type": "extrude", "sketch": "s", "extent": {"type": "blind", "length": 0}}),
            Code::InvalidExtent,
        ),
        (
            json!({"type": "extrude", "sketch": "s", "extent": blind, "axis": "l1"}),
            Code::InvalidParam,
        ),
        (
            json!({"type": "extrude", "sketch": "s"}),
            Code::InvalidDocument,
        ),
        (
            json!({"type": "revolve", "sketch": "s", "axis": "l4", "angle": "400 deg"}),
            Code::InvalidExtent,
        ),
    ] {
        let mut params = params;
        params["part"] = json!("p");
        let e = call(&mut s, "feature.add", params.clone()).unwrap_err();
        assert_eq!(e.code, code, "{params}: {e}");
    }
    for (params, code) in [
        (json!({"axis": "l9", "angle": 90}), Code::NotFound),
        (json!({"axis": "p1", "angle": 90}), Code::ReferenceWrongKind),
        (
            json!({"axis": "l4", "angle": 90, "loops": ["q"]}),
            Code::NotFound,
        ),
    ] {
        let mut params = params;
        params["part"] = json!("p");
        params["type"] = json!("revolve");
        params["sketch"] = json!("s");
        let e = call(&mut s, "feature.add", params.clone()).unwrap_err();
        assert_eq!(e.code, code, "{params}: {e}");
    }
    // What only the rebuild can tell, each in the sketch's terms: the
    // diagonal crosses the rectangle, and isn't on a loop.
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s", "axis": "l5",
                "angle": 90}),
    )
    .unwrap();
    let b = built(&mut s, "r");
    assert_eq!(b["error"]["code"], "profile_invalid");
    assert_eq!(b["error"]["details"]["entities"][1], "l5");
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "r", "loops": ["l5"]}),
    )
    .unwrap();
    let b = built(&mut s, "r");
    assert_eq!(b["error"]["code"], "profile_invalid");
    assert_eq!(b["error"]["details"]["entities"], json!(["l5"]));
    call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "r", "axis": "l4", "loops": ["l2"]}),
    )
    .unwrap();
    assert_eq!(built(&mut s, "r")["ok"], true);
    // A sketch that fails takes its sweeps with it.
    let params =
        |w: &str| json!({"format": 1, "kind": "ryucad.parameters", "parameters": {"width": w}});
    write_json(
        &dir.path().join("parts/p/parameters.json"),
        &params("50 mm"),
    )
    .unwrap();
    let b = built(&mut s, "r");
    assert_eq!(b["error"]["code"], "dependency_failed");
    assert_eq!(b["error"]["details"]["dependency"], "s");
    assert_eq!(b["error"]["details"]["cause"], "sketch_unsolved");
}

#[test]
fn inspect_says_where_sweeps_came_from() {
    let (_dir, mut s) = with_sketch();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                "extent": {"length": "5 mm"}}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s", "axis": "l4",
                "angle": 90}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "revolve", "id": "full", "sketch": "s", "axis": "l4",
                "angle": "full"}),
    )
    .unwrap();
    for f in ["e", "r", "full"] {
        let v = call(&mut s, "inspect", json!({"part": "p", "feature": f})).unwrap();
        let b = &v["bodies"][0];
        for list in ["faces", "edges", "vertices"] {
            for e in b[list].as_array().unwrap() {
                assert_eq!(e["origin"]["sketch"], "s", "{f} {list}: {e}");
                assert_eq!(e["origin"]["feature"], f, "{f} {list}: {e}");
            }
        }
    }
    let v = call(&mut s, "inspect", json!({"part": "p", "feature": "e"})).unwrap();
    let b = &v["bodies"][0];
    let origins: Vec<&Value> = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| &f["origin"])
        .collect();
    assert!(origins.contains(&&json!({"feature": "e", "sketch": "s", "entity": "l1"})));
    assert!(
        origins.contains(&&json!({"feature": "e", "sketch": "s", "cap": "start", "loop": "p1"}))
    );
    assert!(origins.contains(&&json!({"feature": "e", "sketch": "s", "cap": "end", "loop": "p1"})));
    let near = |a: &Value, want: [f64; 3]| {
        for k in 0..3 {
            assert!(
                (a[k].as_f64().unwrap() - want[k]).abs() < 1e-9,
                "{a} != {want:?}"
            );
        }
    };
    near(&b["bounds"]["min"], [0.0, 0.0, 0.0]);
    near(&b["bounds"]["max"], [0.02, 0.01, 0.005]);
    // An origin is found by name too.
    let side = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["origin"] == json!({"feature": "e", "sketch": "s", "entity": "l2"}))
        .unwrap();
    let one = call(
        &mut s,
        "inspect",
        json!({"part": "p", "entity": format!("p/e/{}", side["name"].as_str().unwrap())}),
    )
    .unwrap();
    assert_eq!(one["entity"]["origin"]["entity"], "l2");
    // Display meshes are sized to the body, not a metre.
    let m = call(
        &mut s,
        "mesh.get",
        json!({"part": "p", "feature": "e", "quality": "coarse"}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
    )
    .unwrap();
    let b = call(
        &mut s,
        "mesh.get",
        json!({"part": "p", "feature": "b", "quality": "coarse"}),
    )
    .unwrap();
    // The same box as a primitive: the same chord.
    let chord = m["bodies"][0]["params"]["chord"].as_f64().unwrap();
    let want = b["bodies"][0]["params"]["chord"].as_f64().unwrap();
    assert!((chord - want).abs() < 1e-9 * want, "{chord} vs {want}");
}

#[test]
fn sweeps_keep_their_sketch_before_them() {
    let (_dir, mut s) = with_sketch();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": 1}}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s", "axis": "l4",
                "angle": 90, "loops": ["l1"]}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                "extent": {"type": "symmetric", "length": "4 mm"}}),
    )
    .unwrap();
    let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "s"})).unwrap_err();
    assert_eq!(
        (e.code, &e.details["features"]),
        (Code::HasDependents, &json!(["r", "e"]))
    );
    let e = call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "s", "after": "e"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceForward);
    let e = call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "r", "first": true}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ReferenceForward);
    // Moving what nothing sweeps, or a sweep that stays behind its sketch, is fine.
    call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "ball", "first": true}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.move",
        json!({"part": "p", "id": "r", "after": "e"}),
    )
    .unwrap();
    // What a sweep names stays in its sketch.
    for gone in ["l4", "l1"] {
        let e = call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "remove", "id": gone}]}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["entity"]),
            (Code::HasDependents, &json!(gone))
        );
    }
    // An extent's length alone keeps its type.
    let r = call(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "e", "extent": {"length": "8 mm"}}),
    )
    .unwrap();
    assert_eq!(r["feature"]["extent"]["type"], "symmetric");
    assert_eq!(r["feature"]["extent"]["length"]["m"], 0.008);
}

#[test]
fn a_failing_feature_fails_the_rebuild_but_reports_everything() {
    let (_dir, mut s) = with_part();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "ok"}),
    )
    .unwrap();
    call(&mut s, "feature.add", json!({"part": "p", "type": "torus", "params": {"major_radius": 1, "minor_radius": 1}, "id": "bad"})).unwrap();
    let e = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
    assert_eq!(e.code, Code::RebuildFailed);
    let features = &e.details["parts"][0]["features"];
    assert_eq!(features[0]["ok"], true);
    assert_eq!(features[1]["error"]["code"], "rebuild_failed");
    // Inspect still shows what did build.
    let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
    assert_eq!(v["bodies"].as_array().unwrap().len(), 1);
}

/// A rebuild's report is written as text (#533): the bytes a `Value`
/// of it gives, passing and failing, an assembly's occurrences in
/// their sorted place.
#[test]
fn a_rebuilds_text_is_its_values() {
    let (_dir, mut s) = with_part();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "params": {"x": 1, "y": 2, "z": 3}, "id": "b"}),
    )
    .unwrap();
    call(
        &mut s,
        "part.create",
        json!({"part": "frame", "kind": "assembly"}),
    )
    .unwrap();
    for id in ["i1", "i2"] {
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "of": "p", "id": id}),
        )
        .unwrap();
    }
    let raw = |s: &mut Session, params: Value| {
        let req = Request::from_rpc("rebuild", Some(params)).unwrap();
        answer_live(s, None, None, req).unwrap().raw.expect("text")
    };
    let Raw::Result(text) = raw(&mut s, json!({})) else {
        panic!("the project rebuilds")
    };
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(text, v.to_string());
    assert_eq!(
        v["parts"][0]["occurrences"].as_array().unwrap().len(),
        2,
        "{v}"
    );
    assert_eq!(v, call(&mut s, "rebuild", json!({})).unwrap());
    call(&mut s, "feature.add", json!({"part": "p", "type": "torus", "params": {"major_radius": 1, "minor_radius": 1}, "id": "bad"})).unwrap();
    for params in [json!({}), json!({"part": "p"})] {
        let Raw::Error { code, details, .. } = raw(&mut s, params.clone()) else {
            panic!("a feature fails")
        };
        assert_eq!(code, Code::RebuildFailed);
        let v: Value = serde_json::from_str(&details).unwrap();
        assert_eq!(details, v.to_string());
        assert_eq!(v, call(&mut s, "rebuild", params).unwrap_err().details);
    }
}

#[test]
fn check_format_reports_every_kind_of_problem() {
    let (dir, mut s) = with_part();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
    )
    .unwrap();
    assert_eq!(
        call(&mut s, "check_format", json!({})).unwrap()["clean"],
        true
    );
    let root = dir.path();
    let a = root.join("parts/p/features/a.json");
    let compact = serde_json::to_string(
        &crate::doc::canonical::parse(&fs::read_to_string(&a).unwrap()).unwrap(),
    )
    .unwrap();
    fs::write(&a, compact).unwrap();
    fs::write(
        root.join("parts/p/features/orphan.json"),
        fs::read_to_string(&a).unwrap(),
    )
    .unwrap();
    fs::write(root.join("parts/p/features/broken.json"), "{").unwrap();
    fs::write(root.join("parts/p/stray.txt"), "").unwrap();
    // ryupdm's files are known, and must be canonical too.
    fs::write(root.join("parts/p/properties.json"), "{\"name\":\"x\"}").unwrap();
    fs::write(root.join("ryupdm.json"), "{\"format\":1}").unwrap();
    let r = call(&mut s, "check_format", json!({})).unwrap();
    assert_eq!(r["clean"], false);
    for f in [
        "parts/p/features/a.json",
        "parts/p/properties.json",
        "ryupdm.json",
    ] {
        assert!(
            r["non_canonical"].as_array().unwrap().contains(&json!(f)),
            "{f}: {r}"
        );
    }
    assert_eq!(
        r["orphans"],
        json!([
            "parts/p/features/broken.json",
            "parts/p/features/orphan.json"
        ])
    );
    assert_eq!(r["errors"][0]["code"], "invalid_json");
    assert_eq!(r["unknown"], json!(["parts/p/stray.txt"]));
}

#[test]
fn a_panic_is_an_internal_error_and_drops_the_session() {
    let (dir, _) = fresh();
    let mut slot = None;
    let r = handle_safely(&mut slot, None, None, dir.path(), Request::PartList {})
        .unwrap()
        .result;
    assert_eq!(r["parts"], json!([]));
    assert!(slot.is_some());
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let e = guarded::<Value>(&mut slot, dir.path(), false, |_| panic!("boom")).unwrap_err();
    std::panic::set_hook(prev);
    assert_eq!(
        (e.code, e.details["panic"].as_str()),
        (Code::Internal, Some("boom"))
    );
    assert!(slot.is_none());
    assert!(handle_safely(&mut slot, None, None, dir.path(), Request::PartList {}).is_ok());
}

#[test]
fn daemon_requests_need_a_daemon() {
    let (_dir, mut s) = fresh();
    let e = call(&mut s, "daemon.status", json!({})).unwrap_err();
    assert_eq!(e.code, Code::Usage);
}
#[test]
fn annotations_are_announced_with_their_derived_states() {
    let (_dir, mut s, face) = crate::annotation::tests::plate();
    let mut live = Live::new();
    let sub = live.hub.subscribe("p-x", &[Topic::Annotations]);
    // The first reading is what the subscription starts from.
    crate::server::publish(&mut live, &mut s);
    let id = call_live(
        &mut s,
        &mut live,
        None,
        "annotation.create",
        json!({"pins": [{"ref": face}], "comment": "here"}),
    )
    .unwrap()["annotation"]["id"]
        .clone();
    let notes = drained(&sub);
    let n = notes
        .iter()
        .find(|v| v["method"] == "annotation.changed")
        .expect("told");
    assert_eq!(n["params"]["annotations"][0]["id"], id);
    assert_eq!(n["params"]["annotations"][0]["outdated"], false);
    // The model moving the pin away is news too.
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.delete",
        json!({"part": "plate", "id": "slab"}),
    )
    .unwrap();
    let notes = drained(&sub);
    let n = notes
        .iter()
        .find(|v| v["method"] == "annotation.changed")
        .expect("told");
    assert_eq!(n["params"]["annotations"][0]["outdated"], true);
    call_live(
        &mut s,
        &mut live,
        None,
        "annotation.delete",
        json!({"id": id}),
    )
    .unwrap();
    let notes = drained(&sub);
    let n = notes
        .iter()
        .find(|v| v["method"] == "annotation.changed")
        .expect("told");
    assert_eq!(n["params"]["removed"], json!([id]));
}

#[test]
fn a_participant_sets_its_own_mode() {
    let (_dir, mut s) = fresh();
    let mut live = Live::new();
    let h = live.hello(live::Kind::Human, Some("Dana"), None).unwrap();
    assert_eq!(live.get(&h).unwrap().view.mode, live::Mode::View);
    let v = call_live(
        &mut s,
        &mut live,
        Some(&h),
        "participant.mode",
        json!({"mode": "annotate"}),
    )
    .unwrap();
    assert_eq!(v, json!({"participant": h, "mode": "annotate"}));
    assert_eq!(live.get(&h).unwrap().to_json()["mode"], "annotate");
    // No request sets another's: an agent sets only its own.
    call_live(
        &mut s,
        &mut live,
        None,
        "participant.mode",
        json!({"mode": "edit"}),
    )
    .unwrap();
    assert_eq!(live.get(&h).unwrap().view.mode, live::Mode::Annotate);
    let e = call(&mut s, "participant.mode", json!({"mode": "edit"})).unwrap_err();
    assert_eq!(e.code, Code::NoLiveSession);
    let e = call_live(
        &mut s,
        &mut live,
        Some(&h),
        "participant.mode",
        json!({"mode": "write"}),
    )
    .unwrap_err();
    // An unknown mode is refused as any unknown variant is.
    assert_eq!(e.code, Code::Usage);
}

#[test]
fn a_humans_writes_are_refused_outside_their_mode_and_an_agents_never() {
    let (_dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    let mut live = Live::new();
    let h = live.hello(live::Kind::Human, Some("Dana"), None).unwrap();
    let add = json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 1, "z": 1}});
    let note = json!({"pins": [], "comment": "here"});
    let e = call_live(&mut s, &mut live, Some(&h), "feature.add", add.clone()).unwrap_err();
    assert_eq!(e.code, Code::ParticipantMode);
    assert_eq!(e.details["mode"], "view");
    assert_eq!(e.details["needs"], json!(["edit"]));
    let e = call_live(
        &mut s,
        &mut live,
        Some(&h),
        "annotation.create",
        note.clone(),
    )
    .unwrap_err();
    assert_eq!(e.details["needs"], json!(["annotate", "edit"]));
    // A mark is an annotation write like any other.
    let marked = json!({"pins": [], "comment": "here",
                        "camera": {"eye": [1, -1, 1], "target": [0, 0, 0], "up": [0, 0, 1],
                                   "projection": {"perspective": {"fov_deg": 40}}},
                        "marks": {"part": "p", "list": [{"kind": "line", "in": "view",
                                                         "xy_m": [[0, 0], [0.01, 0]]}]}});
    let e = call_live(&mut s, &mut live, Some(&h), "annotation.create", marked).unwrap_err();
    assert_eq!(e.code, Code::ParticipantMode);
    // Reading is never refused.
    call_live(&mut s, &mut live, Some(&h), "part.list", json!({})).unwrap();
    call_live(
        &mut s,
        &mut live,
        Some(&h),
        "participant.mode",
        json!({"mode": "annotate"}),
    )
    .unwrap();
    call_live(
        &mut s,
        &mut live,
        Some(&h),
        "annotation.create",
        note.clone(),
    )
    .unwrap();
    assert_eq!(
        call_live(&mut s, &mut live, Some(&h), "feature.add", add.clone())
            .unwrap_err()
            .code,
        Code::ParticipantMode
    );
    call_live(
        &mut s,
        &mut live,
        Some(&h),
        "participant.mode",
        json!({"mode": "edit"}),
    )
    .unwrap();
    call_live(&mut s, &mut live, Some(&h), "feature.add", add.clone()).unwrap();
    call_live(
        &mut s,
        &mut live,
        Some(&h),
        "annotation.create",
        note.clone(),
    )
    .unwrap();
    // An agent writes in any mode, and with no hello.
    let a = live.hello(live::Kind::Agent, Some("bot"), None).unwrap();
    call_live(
        &mut s,
        &mut live,
        Some(&a),
        "participant.mode",
        json!({"mode": "view"}),
    )
    .unwrap();
    call_live(
        &mut s,
        &mut live,
        Some(&a),
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"x": 2}}),
    )
    .unwrap();
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"x": 3}}),
    )
    .unwrap();
    // Undo is a write.
    call_live(
        &mut s,
        &mut live,
        Some(&h),
        "participant.mode",
        json!({"mode": "annotate"}),
    )
    .unwrap();
    assert_eq!(
        call_live(&mut s, &mut live, Some(&h), "undo", json!({}))
            .unwrap_err()
            .code,
        Code::ParticipantMode
    );
}

#[test]
fn a_view_mode_humans_dependency_fetch_is_refused_before_it_fetches() {
    use ryu_session::Serve;
    let (_dir, s) = fresh();
    let mut slot = Some(s);
    let mut live = Live::new();
    let h = live.hello(live::Kind::Human, Some("Dana"), None).unwrap();
    let a = live.hello(live::Kind::Agent, Some("bot"), None).unwrap();
    let add =
        Request::from_rpc("deps.add", Some(json!({"url": "file:///nowhere/lib.git"}))).unwrap();
    let job = Job::default();
    // An agent's fetch is prepared without the lock; the human's isn't
    // prepared at all, since `handle` refuses it.
    assert!(crate::Cad::prepare(&mut slot, &live, Some(&a), &add, &job).is_some());
    assert!(crate::Cad::prepare(&mut slot, &live, Some(&h), &add, &job).is_none());
    let update = Request::from_rpc("deps.update", Some(json!({}))).unwrap();
    assert!(crate::Cad::prepare(&mut slot, &live, Some(&h), &update, &job).is_none());
    call_live(
        slot.as_mut().unwrap(),
        &mut live,
        Some(&h),
        "participant.mode",
        json!({"mode": "edit"}),
    )
    .unwrap();
    assert!(crate::Cad::prepare(&mut slot, &live, Some(&h), &add, &job).is_some());
}

#[test]
fn an_export_never_writes_over_a_project_file_whoever_asks() {
    // A human in View mode may export (the window does), so `out` may
    // never be one of the project's files: export is never logged.
    let (dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 1, "z": 1}}),
    )
    .unwrap();
    for d in [
        "releases/p",
        "imports",
        "annotations",
        ".git",
        "out",
        "Parts/p",
        "Imports",
    ] {
        std::fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    // Windows can't make a folder whose name ends in a space or a dot,
    // so `parts. /p` is made and judged only elsewhere.
    if cfg!(not(windows)) {
        std::fs::create_dir_all(dir.path().join("parts. /p")).unwrap();
    }
    let mut live = Live::new();
    let h = live.hello(live::Kind::Human, Some("Dana"), None).unwrap();
    let a = live.hello(live::Kind::Agent, Some("bot"), None).unwrap();
    let export =
        |out: &std::path::Path| json!({"part": "p", "format": "stl", "out": out.to_str().unwrap()});
    for rel in [
        "ryucad.json",
        ".gitattributes",
        "parts/p/part.json",
        "parts/p/features/b.json",
        "parts/p/new.json",
        "releases/p/A.json",
        "imports/x.step",
        "annotations/x.json",
        ".git/config",
        // Windows and macOS fold case, and Windows drops trailing dots
        // and spaces: refused on every platform.
        "RYUCAD.JSON",
        "Parts/p/part.json",
        "Imports/x.step",
        "ryucad.json.",
        "ryucad.json ",
        "parts. /p/part.json",
        // An NTFS stream names the file it's on (#581).
        "ryucad.json::$DATA",
    ]
    .into_iter()
    .filter(|rel| cfg!(not(windows)) || !rel.starts_with("parts. "))
    {
        let out = dir.path().join(rel);
        let before = std::fs::read(&out).ok();
        for who in [Some(h.as_str()), Some(a.as_str()), None] {
            let e = call_live(&mut s, &mut live, who, "export", export(&out)).expect_err(rel);
            assert_eq!(e.code, Code::InvalidParam, "{rel}: {e:?}");
            assert_eq!(e.details["param"], "out", "{rel}");
        }
        assert_eq!(std::fs::read(&out).ok(), before, "{rel} unchanged");
    }
    // Elsewhere in the project folder, or outside it, View may export.
    let beside = tempfile::tempdir().unwrap();
    for out in [
        dir.path().join("p.stl"),
        dir.path().join("out/p.stl"),
        beside.path().join("p.stl"),
    ] {
        call_live(&mut s, &mut live, Some(&h), "export", export(&out)).unwrap();
        assert!(out.exists(), "{}", out.display());
    }
}

#[test]
fn an_export_never_writes_over_a_project_file_through_an_alias_the_file_system_resolves() {
    // #581: APFS folds Unicode case (`ſ` is `s`) and NTFS answers to
    // 8.3 short names, past the spelled check's ASCII folding. An
    // `out` the file system resolves to a project file is refused;
    // one it doesn't is a new file, and the project's is untouched.
    let (dir, mut s) = fresh();
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 1, "z": 1}}),
    )
    .unwrap();
    let project = dir.path().join("ryucad.json");
    let before = std::fs::read(&project).unwrap();
    #[allow(unused_mut)]
    let mut aliases = vec!["ryucad.j\u{17f}on".to_owned()];
    #[cfg(windows)]
    {
        let short = short_name(&project);
        // CI's Windows runners make 8.3 names on their temp volume;
        // a volume without them can't alias this way.
        assert!(
            short != "ryucad.json" || std::env::var_os("CI").is_none(),
            "no 8.3 name for {}: the case #581 needs isn't tested",
            project.display()
        );
        aliases.push(short);
    }
    for alias in aliases {
        let out = dir.path().join(&alias);
        let resolves = out.exists();
        let r = call(
            &mut s,
            "export",
            json!({"part": "p", "format": "stl", "out": out.to_str().unwrap()}),
        );
        if resolves {
            let e = r.expect_err(&alias);
            assert_eq!(e.code, Code::InvalidParam, "{alias}: {e:?}");
            assert_eq!(e.details["param"], "out", "{alias}");
        } else {
            r.unwrap();
        }
        assert_eq!(
            std::fs::read(&project).unwrap(),
            before,
            "{alias}: ryucad.json unchanged"
        );
    }
}

/// The 8.3 short name Windows gives `path`'s file, or its own name
/// where the volume makes none.
#[cfg(windows)]
fn short_name(path: &std::path::Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let long: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: `long` is NUL-terminated, and `buf.len()` is the room
    // `buf` gives the answer.
    let n = unsafe { GetShortPathNameW(long.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    assert!(
        n > 0 && (n as usize) < buf.len(),
        "GetShortPathNameW: {}",
        std::io::Error::last_os_error()
    );
    let short = std::path::PathBuf::from(String::from_utf16(&buf[..n as usize]).unwrap());
    short.file_name().unwrap().to_string_lossy().into_owned()
}

/// Every queued notification of `sub`, as JSON.
fn drained(sub: &crate::notify::Subscriber) -> Vec<Value> {
    sub.drain()
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn without_a_daemon_nobody_is_attached() {
    let (_dir, mut s) = fresh();
    let e = call(&mut s, "session.hello", json!({"kind": "agent"})).unwrap_err();
    assert_eq!(e.code, Code::NoLiveSession);
    let r = call(&mut s, "participant.list", json!({})).unwrap();
    assert_eq!(r, json!({"participants": []}));
    let e = call(&mut s, "app.open", json!({})).unwrap_err();
    assert_eq!(e.code, Code::NoLiveSession);
}

/// #755: a daemon's requests check only the paths file events name;
/// none checks every file (107,500 at design 192's benchmark).
#[test]
fn a_watched_daemon_s_requests_check_only_what_events_name() {
    let (_dir, s) = fresh();
    let marks = tempfile::tempdir().unwrap();
    let mut s = s.with_watch(&marks.path().join("m"));
    let mut live = Live::new();
    call_live(&mut s, &mut live, None, "part.create", json!({"part": "p"})).unwrap();
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 1, "z": 1}}),
    )
    .unwrap();
    let full = |s: &Session| s.watch_status().unwrap()["full_checks"].as_u64().unwrap();
    let before = full(&s);
    for _ in 0..20 {
        call_live(
            &mut s,
            &mut live,
            None,
            "feature.list",
            json!({"part": "p"}),
        )
        .unwrap();
        call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
    }
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.edit",
        json!({"part": "p", "id": "b", "params": {"x": 2}}),
    )
    .unwrap();
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.list",
        json!({"part": "p"}),
    )
    .unwrap();
    let w = s.watch_status().unwrap();
    assert_eq!(w["watching"], true, "{w}");
    assert_eq!(full(&s), before, "a request checked every file: {w}");
    assert!(w["narrowed_checks"].as_u64().unwrap() >= 42, "{w}");
}

#[test]
fn hello_registers_a_participant() {
    let (_dir, mut s) = fresh();
    let mut live = Live::new();
    let r = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human", "name": "Dana"}),
    )
    .unwrap();
    let id = r["participant"].as_str().unwrap().to_owned();
    assert_eq!(
        r["session"]["participants"],
        json!([{"id": id, "kind": "human", "name": "Dana", "status": null, "selection": [],
                "camera": null, "highlights": [], "compare": null, "focus": null, "hidden": [],
                "sections": [], "explode": null, "motion": null, "mode": "view"}])
    );
    let list = call_live(&mut s, &mut live, Some(&id), "participant.list", json!({})).unwrap();
    assert_eq!(list, r["session"]);
}

#[test]
fn sketch_entities_are_selected_highlighted_and_framed() {
    let (_dir, mut s) = with_sketch();
    let mut live = Live::new();
    let human = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human"}),
    )
    .unwrap()["participant"]
        .as_str()
        .unwrap()
        .to_owned();
    let me = Some(human.as_str());
    let r = call_live(
        &mut s,
        &mut live,
        me,
        "selection.set",
        json!({"entities": ["p/s/l1", "p/s/p3"]}),
    )
    .unwrap();
    assert_eq!(r["selection"], json!(["p/s/l1", "p/s/p3"]));
    call_live(
        &mut s,
        &mut live,
        me,
        "highlight.set",
        json!({"entities": ["p/s/l2"], "style": "pending"}),
    )
    .unwrap();
    let e = call_live(
        &mut s,
        &mut live,
        me,
        "selection.set",
        json!({"entities": ["p/s/l9"]}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::NotFound);
    // Framing a line takes its two ends.
    let r = call_live(
        &mut s,
        &mut live,
        me,
        "camera.set",
        json!({"fit": ["p/s/l1"]}),
    )
    .unwrap();
    assert!(r["camera"]["target"][0].as_f64().unwrap() > 0.0);
    // An entity that goes drops out of the selection.
    call_live(
        &mut s,
        &mut live,
        None,
        "sketch.edit",
        json!({"part": "p", "sketch": "s", "ops": [{"op": "remove", "id": "l3"},
                                                        {"op": "remove", "id": "p3"},
                                                        {"op": "remove", "id": "l2"},
                                                        {"op": "remove", "id": "l5"}]}),
    )
    .unwrap();
    let r = call_live(&mut s, &mut live, None, "selection.get", json!({})).unwrap();
    assert_eq!(r["selection"], json!(["p/s/l1"]));
}

#[test]
fn selections_follow_the_model() {
    let (_dir, mut s) = with_part();
    let mut live = Live::new();
    for id in ["a", "b"] {
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": id, "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap();
    }
    let face = |s: &mut Session, f: &str| {
        let v = call(s, "inspect", json!({"part": "p", "feature": f})).unwrap();
        format!(
            "p/{f}/{}",
            v["bodies"][0]["faces"][0]["name"].as_str().unwrap()
        )
    };
    let (fa, fb) = (face(&mut s, "a"), face(&mut s, "b"));
    let human = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human"}),
    )
    .unwrap()["participant"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = call_live(
        &mut s,
        &mut live,
        Some(&human),
        "selection.set",
        json!({"entities": [fa, fb]}),
    )
    .unwrap();
    assert_eq!(r["selection"], json!([fa, fb]));
    // Anyone asking without naming a participant gets the human's.
    let r = call_live(&mut s, &mut live, None, "selection.get", json!({})).unwrap();
    assert_eq!(
        (&r["participant"], &r["selection"]),
        (&json!(human), &json!([fa, fb]))
    );
    // A reference that doesn't resolve is refused, never matched.
    let e = call_live(
        &mut s,
        &mut live,
        Some(&human),
        "selection.set",
        json!({"entities": ["p/a/0123456789abcdef0123456789abcdef"], "mode": "add"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::NotFound);
    // Editing keeps names, so the selection stays; deleting drops it.
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.edit",
        json!({"part": "p", "id": "a", "params": {"x": 2}}),
    )
    .unwrap();
    let sel = |s: &mut Session, live: &mut Live| {
        call_live(s, live, None, "selection.get", json!({})).unwrap()["selection"].clone()
    };
    assert_eq!(sel(&mut s, &mut live), json!([fa, fb]));
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.delete",
        json!({"part": "p", "id": "a"}),
    )
    .unwrap();
    assert_eq!(sel(&mut s, &mut live), json!([fb]));
    // Removing works even for references that no longer resolve.
    call_live(
        &mut s,
        &mut live,
        Some(&human),
        "selection.set",
        json!({"entities": [fa, fb], "mode": "remove"}),
    )
    .unwrap();
    assert_eq!(sel(&mut s, &mut live), json!([]));
    // Without a daemon there is no selection to change.
    let e = call(&mut s, "selection.set", json!({"entities": []})).unwrap_err();
    assert_eq!(e.code, Code::NoLiveSession);
    let r = call(&mut s, "selection.get", json!({})).unwrap();
    assert_eq!(r, json!({"participant": null, "selection": []}));
}

/// The face of solid `b` of part `p` whose outward normal is `n`.
fn face_facing(s: &mut Session, n: [f64; 3]) -> String {
    let v = call(s, "inspect", json!({"part": "p", "feature": "b"})).unwrap();
    let face = v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let z = &f["surface"]["frame"]["z"];
            let k = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && (0..3).all(|i| (z[i].as_f64().unwrap() * k - n[i]).abs() < 1e-12)
        })
        .unwrap();
    format!("p/b/{}", face["name"].as_str().unwrap())
}

#[test]
fn cameras_restore_named_views_and_look_normal_to_faces() {
    let (dir, mut s) = with_part();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 20, "z": 10},
               "placement": {"origin": [30, 30, 5]}}),
    )
    .unwrap();
    let mut live = Live::new();
    let human = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human"}),
    )
    .unwrap()["participant"]
        .clone();
    let me = Some(human.as_str().unwrap());
    let set = |s: &mut Session, live: &mut Live, p: Value| call_live(s, live, me, "camera.set", p);
    // Every cube direction frames the model.
    for (name, _) in crate::view::STANDARD_VIEWS {
        let r = set(&mut s, &mut live, json!({"view": name})).unwrap();
        assert!(r["camera"]["eye"].is_array(), "{name}");
    }
    // Looking at the box and the origin beside it.
    let camera = json!({"eye": ["120 mm", "-80 mm", "80 mm"], "target": ["20 mm", "20 mm", "5 mm"],
                        "up": [0, 0, 1], "projection": {"orthographic": {"height": "120 mm"}}});
    let mine = set(&mut s, &mut live, json!({"camera": camera})).unwrap()["camera"].clone();
    // An agent saves what the one human sees.
    let saved = call_live(
        &mut s,
        &mut live,
        None,
        "view.save",
        json!({"part": "p", "view": "detail"}),
    )
    .unwrap();
    assert_eq!(saved["camera"], mine);
    set(&mut s, &mut live, json!({"view": "iso"})).unwrap();
    // Restored as saved, bit for bit: the project's model is one part.
    let r = set(&mut s, &mut live, json!({"view": "detail"})).unwrap();
    assert_eq!(r["camera"], mine);
    let r = set(&mut s, &mut live, json!({"view": "detail", "part": "p"})).unwrap();
    assert_eq!(r["camera"], mine);
    // Framing entities from a named view keeps its direction.
    let top = face_facing(&mut s, [0.0, 0.0, 1.0]);
    let r = set(&mut s, &mut live, json!({"view": "detail", "fit": [top]})).unwrap();
    assert_eq!(r["camera"]["up"], mine["up"]);
    assert_ne!(r["camera"]["eye"], mine["eye"]);
    let e = set(&mut s, &mut live, json!({"view": "side"})).unwrap_err();
    assert_eq!(
        (e.code, e.details["views"].clone()),
        (Code::ViewNotFound, json!(["detail"]))
    );
    // Normal to the top: looking down, the long edge (along x) level.
    let r = set(&mut s, &mut live, json!({"normal_to": top})).unwrap();
    let c: Camera = serde_json::from_value(r["camera"].clone()).unwrap();
    let (right, _, forward) = c.basis();
    assert!((forward[2] + 1.0).abs() < 1e-12, "{forward:?}");
    assert!((right[0].abs() - 1.0).abs() < 1e-12, "{right:?}");
    // The participant's projection is kept.
    assert!(matches!(
        c.projection,
        crate::view::Projection::Orthographic { .. }
    ));
    // The front face (normal -y): looking along +y with z up.
    let front = face_facing(&mut s, [0.0, -1.0, 0.0]);
    let r = set(&mut s, &mut live, json!({"normal_to": front})).unwrap();
    let c: Camera = serde_json::from_value(r["camera"].clone()).unwrap();
    let (right, up, forward) = c.basis();
    assert!((forward[1] - 1.0).abs() < 1e-12 && (right[0] - 1.0).abs() < 1e-12);
    assert!((up[2] - 1.0).abs() < 1e-12, "{up:?}");
    let e = set(&mut s, &mut live, json!({"normal_to": "p/b"})).unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    let e = set(
        &mut s,
        &mut live,
        json!({"normal_to": front, "view": "top"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::Usage);
    // A render restores the named view too, and draws the origin.
    let size = |v: Value| {
        let mut v = v;
        v["width"] = json!(320);
        v["height"] = json!(240);
        v
    };
    let named = render_pixels(dir.path(), s, size(json!({"view": "detail"})));
    let s = Session::open(dir.path()).unwrap();
    let again = render_pixels(dir.path(), s, size(json!({"camera": camera})));
    assert_eq!(named, again);
    let s = Session::open(dir.path()).unwrap();
    let with_origin = render_pixels(
        dir.path(),
        s,
        size(json!({"view": "detail", "origin": true})),
    );
    assert_ne!(with_origin, named);
    let red = |px: &[[u8; 3]]| {
        px.iter()
            .filter(|p| p[0] > 150 && p[1] < 90 && p[2] < 90)
            .count()
    };
    assert!(
        red(&with_origin) > red(&named),
        "the x axis is drawn red: {} against {}",
        red(&with_origin),
        red(&named)
    );
}

#[test]
fn cameras_highlights_and_status() {
    let (_dir, mut s) = with_part();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 5}}),
    )
    .unwrap();
    let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
    let face = format!(
        "p/s/{}",
        v["bodies"][0]["faces"][0]["name"].as_str().unwrap()
    );
    let mut live = Live::new();
    let human = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human"}),
    )
    .unwrap()["participant"]
        .clone();
    let me = Some(human.as_str().unwrap());
    let camera = json!({"eye": [0, "-100 mm", 0], "target": [0, 0, 0], "up": [0, 0, 1],
                        "projection": {"perspective": {"fov_deg": 30}}});
    let r = call_live(
        &mut s,
        &mut live,
        me,
        "camera.set",
        json!({"camera": camera}),
    )
    .unwrap();
    assert_eq!(r["camera"]["eye"], json!([0.0, -0.1, 0.0]));
    // Framing a face keeps the direction and looks at the face's box.
    let set = |s: &mut Session, live: &mut Live, p: Value| call_live(s, live, me, "camera.set", p);
    let r = set(&mut s, &mut live, json!({"fit": [face]})).unwrap();
    let (eye, target) = (&r["camera"]["eye"], &r["camera"]["target"]);
    assert!(eye[1].as_f64().unwrap() < target[1].as_f64().unwrap());
    assert!(target[2].as_f64().unwrap().abs() < 1e-3);
    let r = set(&mut s, &mut live, json!({"view": "top"})).unwrap();
    assert_eq!(r["camera"]["up"], json!([0.0, 1.0, 0.0]));
    // The participant's projection is kept.
    assert_eq!(r["camera"]["projection"]["perspective"]["fov_deg"], 30.0);
    let e = set(&mut s, &mut live, json!({"view": "top", "camera": camera})).unwrap_err();
    assert_eq!(e.code, Code::Usage);
    let r = set(&mut s, &mut live, json!({"camera": camera})).unwrap();
    assert_eq!(r["camera"]["eye"], json!([0.0, -0.1, 0.0]));
    let r = call_live(&mut s, &mut live, None, "camera.get", json!({})).unwrap();
    assert_eq!(
        (&r["participant"], &r["camera"]["eye"]),
        (&human, &json!([0.0, -0.1, 0.0]))
    );
    // An agent (no hello) highlights and reports status as itself.
    let r = call_live(
        &mut s,
        &mut live,
        None,
        "highlight.set",
        json!({"entities": [face], "style": "pending", "label": "next"}),
    )
    .unwrap();
    assert_eq!(
        r["highlights"],
        json!([{"style": "pending", "label": "next", "entities": [face]}])
    );
    let agent = r["participant"].clone();
    assert_ne!(agent, human);
    let r = call_live(
        &mut s,
        &mut live,
        None,
        "activity.set",
        json!({"status": "measuring", "entities": [face]}),
    )
    .unwrap();
    assert_eq!(
        r["status"],
        json!({"text": "measuring", "entities": [face]})
    );
    let list = call_live(&mut s, &mut live, None, "participant.list", json!({})).unwrap();
    let a = list["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == agent)
        .unwrap()
        .clone();
    assert_eq!(a["status"]["text"], "measuring");
    assert_eq!(a["highlights"][0]["label"], "next");
    let r = call_live(&mut s, &mut live, None, "highlight.clear", json!({})).unwrap();
    assert_eq!(r["highlights"], json!([]));
    for (method, params) in [
        ("camera.set", json!({"camera": camera})),
        ("highlight.set", json!({"entities": [face]})),
        ("highlight.clear", json!({})),
        ("activity.set", json!({"status": "x"})),
    ] {
        let e = call(&mut s, method, params).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession, "{method}");
    }
    let r = call(&mut s, "camera.get", json!({})).unwrap();
    assert_eq!(r, json!({"participant": null, "camera": null}));
}

#[test]
fn an_agent_puts_the_human_into_a_compare() {
    use crate::git::tests::git;
    let (dir, mut s) = fresh();
    let mut live = Live::new();
    let e = call_live(
        &mut s,
        &mut live,
        None,
        "compare.set",
        json!({"rev": "HEAD"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::NotAGitRepo);
    git(dir.path(), &["init", "-q", "-b", "main"]);
    call(&mut s, "part.create", json!({"part": "p"})).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "start"]);
    let human = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human", "name": "Dana"}),
    )
    .unwrap()["participant"]
        .clone();
    // An agent (no hello) sets it: it goes to the one human.
    let set =
        |s: &mut Session, live: &mut Live, p: Value| call_live(s, live, None, "compare.set", p);
    let r = set(&mut s, &mut live, json!({"rev": "main"})).unwrap();
    assert_eq!(r["participant"], human);
    assert_eq!(r["compare"]["rev"], "main");
    let commit = r["compare"]["commit"].as_str().unwrap().to_owned();
    assert_eq!(commit.len(), 40);
    let r = call_live(&mut s, &mut live, None, "compare.get", json!({})).unwrap();
    assert_eq!(
        r,
        json!({"participant": human, "compare": {"rev": "main", "commit": commit}})
    );
    let list = call_live(&mut s, &mut live, None, "participant.list", json!({})).unwrap();
    let of = |id: &Value| {
        list["participants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == *id)
            .unwrap()["compare"]
            .clone()
    };
    assert_eq!(of(&human), json!({"rev": "main"}));
    // Revisions that don't resolve, and the working files, are refused;
    // the compare stays as it was.
    for (p, code) in [
        (json!({"rev": "nowhere"}), Code::RevNotFound),
        (json!({"rev": "working"}), Code::InvalidParam),
        (json!({}), Code::Usage),
        (json!({"rev": "HEAD", "off": true}), Code::Usage),
        (
            json!({"off": true, "participant": "p-ffffffff"}),
            Code::NotFound,
        ),
    ] {
        let e = set(&mut s, &mut live, p.clone()).unwrap_err();
        assert_eq!(e.code, code, "{p}");
    }
    let r = call_live(&mut s, &mut live, None, "compare.get", json!({})).unwrap();
    assert_eq!(r["compare"]["rev"], "main");
    // Named, the agent compares itself; off ends the human's.
    let r = set(
        &mut s,
        &mut live,
        json!({"rev": "HEAD", "participant": human}),
    )
    .unwrap();
    assert_eq!(r["compare"]["rev"], "HEAD");
    let r = set(&mut s, &mut live, json!({"off": true})).unwrap();
    assert_eq!(r, json!({"participant": human, "compare": null}));
    // Without a daemon nobody compares.
    let e = call(&mut s, "compare.set", json!({"rev": "HEAD"})).unwrap_err();
    assert_eq!(e.code, Code::NoLiveSession);
    let r = call(&mut s, "compare.get", json!({})).unwrap();
    assert_eq!(r, json!({"participant": null, "compare": null}));
}

#[test]
fn the_log_records_changes_and_who_made_them() {
    let (dir, mut s) = with_part();
    let mut live = Live::new();
    let human = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human", "name": "Dana"}),
    )
    .unwrap()["participant"]
        .clone();
    // A person's window writes the model in Direct edit.
    call_live(
        &mut s,
        &mut live,
        human.as_str(),
        "participant.mode",
        json!({"mode": "edit"}),
    )
    .unwrap();
    call_live(
        &mut s,
        &mut live,
        human.as_str(),
        "feature.add",
        json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
    )
    .unwrap();
    // Reads and refused writes aren't logged.
    call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
    call_live(&mut s, &mut live, None, "part.create", json!({"part": "p"})).unwrap_err();
    // An edit behind the daemon's back is logged as external.
    let mut other = Session::open(dir.path()).unwrap();
    call(
        &mut other,
        "feature.edit",
        json!({"part": "p", "id": "s", "params": {"radius": 2}}),
    )
    .unwrap();
    call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
    let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
    let e = log["entries"].as_array().unwrap();
    assert_eq!(e.len(), 2);
    assert_eq!(e[0]["kind"], "change");
    assert_eq!(e[0]["method"], "feature.add");
    assert_eq!(e[0]["participant"]["name"], "Dana");
    assert_eq!(e[0]["params"]["id"], "s");
    assert_eq!(
        e[0]["files"],
        json!([{"path": "parts/p/features/s.json", "change": "created"},
               {"path": "parts/p/part.json", "change": "modified"}])
    );
    assert_eq!(e[1]["kind"], "external");
    assert_eq!(e[1]["participant"], Value::Null);
    assert_eq!(
        e[1]["files"],
        json!([{"path": "parts/p/features/s.json", "change": "modified"}])
    );
    let page = call_live(&mut s, &mut live, None, "activity.log", json!({"limit": 1})).unwrap();
    assert_eq!(page["entries"][0]["seq"], e[1]["seq"]);
    let r = call(&mut s, "activity.log", json!({})).unwrap();
    assert_eq!(r, json!({"entries": []}));
}

#[test]
fn undo_and_redo_go_through_the_files() {
    let (dir, mut s) = with_part();
    let mut live = Live::new();
    let human = call_live(
        &mut s,
        &mut live,
        None,
        "session.hello",
        json!({"kind": "human"}),
    )
    .unwrap()["participant"]
        .as_str()
        .unwrap()
        .to_owned();
    let h = Some(human.as_str());
    // A person's window writes the model in Direct edit.
    call_live(
        &mut s,
        &mut live,
        h,
        "participant.mode",
        json!({"mode": "edit"}),
    )
    .unwrap();
    let radius = |s: &mut Session| {
        call(s, "feature.show", json!({"part": "p", "id": "s"}))
            .map(|v| v["params"]["radius"]["value"].clone())
    };
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.add",
        json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
    )
    .unwrap();
    call_live(
        &mut s,
        &mut live,
        h,
        "feature.edit",
        json!({"part": "p", "id": "s", "params": {"radius": 2}}),
    )
    .unwrap();
    // The human undoes their own edit, then redoes it.
    let r = call_live(&mut s, &mut live, h, "undo", json!({})).unwrap();
    assert_eq!(r["entry"]["kind"], "undo");
    assert_eq!(radius(&mut s).unwrap(), "1 mm");
    assert!(r["revisions"]["p"].is_string());
    let e = call_live(&mut s, &mut live, h, "undo", json!({})).unwrap_err();
    assert_eq!(e.code, Code::NothingToUndo);
    call_live(&mut s, &mut live, h, "redo", json!({})).unwrap();
    assert_eq!(radius(&mut s).unwrap(), "2 mm");
    let e = call_live(&mut s, &mut live, h, "redo", json!({})).unwrap_err();
    assert_eq!(e.code, Code::NothingToUndo);
    // A redo can itself be undone.
    call_live(&mut s, &mut live, h, "undo", json!({})).unwrap();
    assert_eq!(radius(&mut s).unwrap(), "1 mm");
    call_live(&mut s, &mut live, h, "redo", json!({})).unwrap();
    // The agent can't undo its add while the human's edit stands on
    // top of it, by default or by number.
    let e = call_live(&mut s, &mut live, None, "undo", json!({})).unwrap_err();
    assert_eq!(e.code, Code::UndoConflict);
    assert_eq!(e.details["files"], json!(["parts/p/features/s.json"]));
    let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
    let add = log["entries"][0]["seq"].as_u64().unwrap();
    let e = call_live(&mut s, &mut live, None, "undo", json!({"entry": add})).unwrap_err();
    assert_eq!(e.code, Code::UndoConflict);
    // An edit behind the daemon's back can't be undone, and blocks
    // undoing what it overwrote.
    let mut other = Session::open(dir.path()).unwrap();
    call(
        &mut other,
        "feature.edit",
        json!({"part": "p", "id": "s", "params": {"radius": 3}}),
    )
    .unwrap();
    call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
    let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
    let entries = log["entries"].as_array().unwrap();
    let external = entries.last().unwrap()["seq"].as_u64().unwrap();
    let e = call_live(&mut s, &mut live, None, "undo", json!({"entry": external})).unwrap_err();
    assert_eq!(e.code, Code::NothingToUndo);
    let e = call_live(&mut s, &mut live, h, "undo", json!({})).unwrap_err();
    assert_eq!(e.code, Code::UndoConflict);
    // Undoing a part's creation removes its folder too.
    call_live(&mut s, &mut live, None, "part.create", json!({"part": "q"})).unwrap();
    assert!(dir.path().join("parts/q").is_dir());
    call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
    assert!(!dir.path().join("parts/q").exists());
    // Undoing a deletion brings every file back.
    call_live(&mut s, &mut live, None, "part.delete", json!({"part": "p"})).unwrap();
    call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
    assert_eq!(radius(&mut s).unwrap(), "3 mm");
    let e = call(&mut s, "undo", json!({})).unwrap_err();
    assert_eq!(e.code, Code::NoLiveSession);
}

#[test]
fn setting_a_preference_tells_subscribers() {
    crate::config::tests::with_config(|_| {
        let (_dir, mut s) = fresh();
        let mut live = Live::new();
        let sub = live.hub.subscribe("p-x", &[Topic::Project]);
        call_live(
            &mut s,
            &mut live,
            None,
            "config.set",
            json!({"key": "daemon.poll_ms", "value": 100}),
        )
        .unwrap();
        assert_eq!(live.poll_ms, 100);
        live.hub.send(Topic::Project, "marker", &json!({}));
        let got = drain(&sub);
        assert_eq!(got[0].0, "config.changed");
        assert_eq!(
            got[0].1,
            json!({"key": "daemon.poll_ms", "scope": "user", "value": 100, "default": false})
        );
        // A project setting is logged like any file change; a
        // preference isn't in the project, so it isn't.
        call_live(
            &mut s,
            &mut live,
            None,
            "config.set",
            json!({"key": "default_unit", "value": "cm"}),
        )
        .unwrap();
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let e = log["entries"].as_array().unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(
            e[0]["files"],
            json!([{"path": "ryucad.json", "change": "modified"}])
        );
    });
}

#[test]
fn setting_the_unit_keeps_the_project_id() {
    crate::config::tests::with_config(|_| {
        let (dir, mut s) = fresh();
        let id = crate::doc::id::ProjectId::from_bytes([9; 16]);
        let m = Manifest {
            project: Some(id.clone()),
            ..Manifest::new(Unit::Mm)
        };
        write_json(&dir.path().join(MANIFEST), &m.to_json()).unwrap();
        call(
            &mut s,
            "config.set",
            json!({"key": "default_unit", "value": "in"}),
        )
        .unwrap();
        let text = fs::read_to_string(dir.path().join(MANIFEST)).unwrap();
        assert_eq!(
            crate::doc::canonical::parse(&text).unwrap(),
            json!({"default_unit": "in", "format": 2, "kind": "ryucad.project",
                   "project": id})
        );
        assert_eq!(s.project().manifest.project, Some(id));
    });
}

#[test]
fn editing_one_feature_meshes_one_body() {
    crate::config::tests::with_config(|_| {
        let (_dir, mut s) = with_part();
        for (id, r) in [("a", 1), ("b", 2)] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "sphere", "id": id, "params": {"radius": r}}),
            )
            .unwrap();
        }
        let m = call(
            &mut s,
            "mesh.get",
            json!({"part": "p", "quality": "coarse"}),
        )
        .unwrap();
        assert_eq!(m["quality"], "coarse");
        assert_eq!(m["bodies"][1]["feature"], "b");
        assert_eq!(s.caches.meshes.len(), 2);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "a", "params": {"radius": 3}}),
        )
        .unwrap();
        let again = call(
            &mut s,
            "mesh.get",
            json!({"part": "p", "quality": "coarse"}),
        )
        .unwrap();
        assert_eq!(s.caches.meshes.len(), 3);
        assert_eq!(again["bodies"][1], m["bodies"][1]);
        assert_ne!(again["bodies"][0]["hash"], m["bodies"][0]["hash"]);
        // The user's display quality is the default.
        let v = call(&mut s, "mesh.get", json!({"part": "p", "feature": "b"})).unwrap();
        assert_eq!(
            (v["quality"].as_str(), v["bodies"].as_array().unwrap().len()),
            (Some("normal"), 1)
        );
        let e = call(&mut s, "mesh.get", json!({"part": "p", "feature": "z"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    });
}

#[test]
fn meshes_are_made_before_the_lock_is_taken() {
    crate::config::tests::with_config(|_| {
        let (dir, s) = with_part();
        let mut slot = Some(s);
        for (id, r) in [("a", 1), ("b", 2)] {
            call(
                slot.as_mut().unwrap(),
                "feature.add",
                json!({"part": "p", "type": "sphere", "id": id, "params": {"radius": r}}),
            )
            .unwrap();
        }
        let req = |rev: Option<&str>| {
            Request::from_rpc(
                "mesh.get",
                Some(
                    json!({"part": "p", "quality": "coarse", "transfer": "binary",
                            "rev": rev}),
                ),
            )
            .unwrap()
        };
        // A revision's meshes are made under the lock, as before.
        assert!(prepare(&mut slot, &req(Some("HEAD"))).is_none());
        let job = prepare(&mut slot, &req(None)).expect("two meshes to make");
        let put = job();
        let s = slot.as_mut().unwrap();
        assert_eq!(s.caches.meshes.len(), 0);
        put(s);
        assert_eq!(s.caches.meshes.len(), 2);
        assert!(prepare(&mut slot, &req(Some("working"))).is_none());
        // The answer is the one a session that meshed itself gives.
        let mut a = handle_safely(&mut slot, None, None, dir.path(), req(None)).unwrap();
        assert_eq!(slot.as_ref().unwrap().caches.meshes.len(), 2);
        slot.as_mut().unwrap().caches.meshes = mesh::Cache::default();
        let mut b = handle_safely(&mut slot, None, None, dir.path(), req(None)).unwrap();
        assert_eq!(a.result["binary"]["encoding"], mesh::ENCODING);
        let bytes = a.binary.take().unwrap();
        assert_eq!(Some(&bytes), b.binary.take().as_ref());
        assert_eq!(a.result, b.result);
        // `compact` answers the same meshes as `ryucad.mesh.2`, smaller.
        let compact = Request::from_rpc(
            "mesh.get",
            Some(json!({"part": "p", "quality": "coarse", "transfer": "compact"})),
        )
        .unwrap();
        let c = handle_safely(&mut slot, None, None, dir.path(), compact).unwrap();
        assert_eq!(c.result["binary"]["encoding"], mesh::COMPACT);
        let small = c.binary.unwrap();
        assert!(small.len() < bytes.len());
        let (one, two) = (mesh::decode(&bytes).unwrap(), mesh::decode(&small).unwrap());
        assert_eq!(one.len(), two.len());
        for (x, y) in one.iter().zip(&two) {
            let (x, y) = (x.mesh.as_ref().unwrap(), y.mesh.as_ref().unwrap());
            assert_eq!((&x.positions, &x.triangles), (&y.positions, &y.triangles));
        }
    });
}

#[test]
fn a_repair_is_logged_and_can_be_undone() {
    let (dir, mut s) = with_part();
    let mut live = Live::new();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sphere", "id": "a", "params": {"radius": 1}}),
    )
    .unwrap();
    let text = fs::read_to_string(dir.path().join("parts/p/features/a.json")).unwrap();
    fs::write(dir.path().join("parts/p/features/b.json"), text).unwrap();
    let r = call_live(&mut s, &mut live, None, "doctor", json!({})).unwrap();
    let adopt = r["issues"][0]["repairs"][0]["id"].clone();
    assert!(adopt.as_str().unwrap().starts_with("orphan_feature:adopt:"));
    call_live(
        &mut s,
        &mut live,
        None,
        "doctor.repair",
        json!({"repairs": [adopt]}),
    )
    .unwrap();
    assert_eq!(
        s.project()
            .part(&part_id("p").unwrap())
            .unwrap()
            .order()
            .len(),
        2
    );
    let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["method"], "doctor.repair");
    call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
    assert_eq!(
        s.project()
            .part(&part_id("p").unwrap())
            .unwrap()
            .order()
            .len(),
        1
    );
}

#[test]
fn a_label_made_official_is_news() {
    use crate::assembly::tests::{place_in, released};
    use crate::git::tests::git;
    let (dir, mut s) = released();
    let r = dir.path();
    call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
    place_in(r, "frame", "b", "bolt", json!({"label": "B"}), [0.0; 3]);
    let mut live = Live::new();
    let mut slot = Some(s);
    settle(&mut slot, &mut live, r);
    let sub = live.hub.subscribe("p-x", &[Topic::Model]);
    let mark = |live: &mut Live| live.hub.send(Topic::Model, "marker", &json!({}));
    // The first watched tick keys assemblies by their results; the
    // next has nothing to say.
    tick(&mut slot, &mut live, r);
    mark(&mut live);
    drain(&sub);
    tick(&mut slot, &mut live, r);
    mark(&mut live);
    assert!(drain(&sub).is_empty());
    // No file the session reads changes; the release branch does.
    git(r, &["add", "."]);
    git(r, &["commit", "-q", "-m", "bolt B"]);
    tick(&mut slot, &mut live, r);
    mark(&mut live);
    let got = drain(&sub);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, "model.changed");
    let frame = got[0].1["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["part"] == "frame")
        .unwrap()
        .clone();
    let b = frame["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "b")
        .unwrap()
        .clone();
    assert_eq!(b["instance"]["pin"]["official"], true, "{b}");
}

#[test]
fn subscribers_hear_of_project_parameters_nothing_reads() {
    let (dir, s) = with_part();
    let mut live = Live::new();
    let mut slot = Some(s);
    settle(&mut slot, &mut live, dir.path());
    let sub = live.hub.subscribe("p-x", &[Topic::Model, Topic::Project]);
    let mark = |live: &mut Live| live.hub.send(Topic::Model, "marker", &json!({}));
    let mut s = slot.take().unwrap();
    call_live(
        &mut s,
        &mut live,
        None,
        "param.set",
        json!({"scope": "project", "parameters": {"spare": "2 mm"}}),
    )
    .unwrap();
    mark(&mut live);
    let got = drain(&sub);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, "project.changed");
    assert_eq!(got[0].1["parameters"], json!({"spare": "2 mm"}));
}

#[test]
fn subscribers_hear_of_model_changes_once() {
    let (dir, s) = with_part();
    let mut live = Live::new();
    let mut slot = Some(s);
    // Changes from before the subscription aren't news.
    settle(&mut slot, &mut live, dir.path());
    let sub = live.hub.subscribe("p-x", &[Topic::Model, Topic::Project]);
    let mark = |live: &mut Live| live.hub.send(Topic::Model, "marker", &json!({}));
    let mut s = slot.take().unwrap();
    call_live(
        &mut s,
        &mut live,
        None,
        "feature.add",
        json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
    )
    .unwrap();
    mark(&mut live);
    let got = drain(&sub);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "model.changed");
    let part = &got[0].1["parts"][0];
    assert_eq!(part["features"][0]["id"], "s");
    assert_eq!(part["features"][0]["ok"], true);
    // Nothing changed: nothing is sent.
    call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
    let mut slot = Some(s);
    tick(&mut slot, &mut live, dir.path());
    mark(&mut live);
    assert!(drain(&sub).is_empty());
    // An edit behind the daemon's back is found by the tick.
    let mut other = Session::open(dir.path()).unwrap();
    call(
        &mut other,
        "feature.edit",
        json!({"part": "p", "id": "s", "params": {"radius": 2}}),
    )
    .unwrap();
    tick(&mut slot, &mut live, dir.path());
    mark(&mut live);
    let got = drain(&sub);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "model.changed");
    // A deleted part is reported as removed.
    call(&mut other, "part.delete", json!({"part": "p"})).unwrap();
    tick(&mut slot, &mut live, dir.path());
    mark(&mut live);
    let got = drain(&sub);
    assert_eq!(got[0].0, "project.changed");
    assert_eq!(got[0].1["parts"], json!([]));
    assert_eq!(got[1].1["removed"], json!(["p"]));
}

/// ryucad 25.B: the window fetches an inserted solid's mesh by its key,
/// one body per request as it fetches any solid's.
#[test]
fn mesh_get_takes_an_inserted_solids_key() {
    let (_dir, mut s) = crate::insert::tests::boarded();
    let all = call(&mut s, "mesh.get", json!({"part": "enclosure"})).unwrap();
    let u1 = all["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["feature"] == "pcb/u1")
        .cloned()
        .unwrap_or_else(|| panic!("{all}"));
    let one = call(
        &mut s,
        "mesh.get",
        json!({"part": "enclosure", "feature": "pcb/u1"}),
    )
    .unwrap();
    assert_eq!(one["bodies"].as_array().unwrap().len(), 1, "{one}");
    assert_eq!(one["bodies"][0]["feature"], "pcb/u1");
    assert_eq!(one["bodies"][0]["hash"], u1["hash"]);
    // A key below a feature that inserts nothing is refused, by name.
    let e = call(
        &mut s,
        "mesh.get",
        json!({"part": "enclosure", "feature": "nothing/u1"}),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::NotFound, "{e:?}");
}

/// ryucad 25.B: a deleted solid isn't the model's, but `mesh.get` with
/// `before` meshes it as that feature read it: a delete's ghost.
#[test]
fn mesh_get_before_meshes_a_deleted_solid_as_its_delete_read_it() {
    let (_dir, mut s) = crate::insert::tests::boarded();
    let shown = call(
        &mut s,
        "mesh.get",
        json!({"part": "enclosure", "feature": "pcb/u1"}),
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "enclosure", "type": "delete_body", "id": "d", "solids": ["pcb/u1"]}),
    )
    .unwrap();
    let all = call(&mut s, "mesh.get", json!({"part": "enclosure"})).unwrap();
    assert!(
        all["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["feature"] != "pcb/u1"),
        "{all}"
    );
    let ghost = call(
        &mut s,
        "mesh.get",
        json!({"part": "enclosure", "feature": "pcb/u1", "before": "d"}),
    )
    .unwrap();
    assert_eq!(ghost["bodies"][0]["feature"], "pcb/u1", "{ghost}");
    assert_eq!(ghost["bodies"][0]["hash"], shown["bodies"][0]["hash"]);
    // The window asks for it compact, as bytes.
    let req = Request::from_rpc(
        "mesh.get",
        Some(
            json!({"part": "enclosure", "feature": "pcb/u1", "before": "d", "transfer": "compact"}),
        ),
    )
    .unwrap();
    let a = answer_live(&mut s, None, None, req).unwrap();
    let bodies = mesh::decode(&a.binary.expect("bytes")).unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].feature, "pcb/u1");
    // `before` meshes one solid: beside `flat` or `features` it is refused
    // by name, by JSON and as bytes alike, never ignored.
    for extra in [json!({"flat": ""}), json!({"features": ["lid"]})] {
        for transfer in [None, Some("compact")] {
            let mut params = json!({"part": "enclosure", "feature": "pcb/u1", "before": "d"});
            for (k, v) in extra.as_object().unwrap() {
                params[k] = v.clone();
            }
            if let Some(t) = transfer {
                params["transfer"] = json!(t);
            }
            let req = Request::from_rpc("mesh.get", Some(params.clone())).unwrap();
            let e = match answer_live(&mut s, None, None, req) {
                Err(e) => e,
                Ok(_) => panic!("{params}: answered"),
            };
            assert_eq!(
                (e.code, &e.details["param"]),
                (Code::InvalidParam, &json!("before")),
                "{params}: {e:?}"
            );
        }
    }
    // `before` needs the solid it reads, and a feature of the part.
    for (params, code) in [
        (
            json!({"part": "enclosure", "before": "d"}),
            Code::InvalidParam,
        ),
        (
            json!({"part": "enclosure", "feature": "pcb/u1", "before": "nope"}),
            Code::NotFound,
        ),
    ] {
        let e = call(&mut s, "mesh.get", params.clone()).unwrap_err();
        assert_eq!(e.code, code, "{params}: {e:?}");
    }
}

#[test]
fn renders_draw_thread_rings_unless_told_not_to() {
    let (_dir, mut s) = crate::hole::tests::drilled();
    crate::hole::tests::put(
        &mut s,
        "h",
        json!({"format": 23, "points": ["p1"], "form": "simple",
               "thread": {"standard": "iso_metric", "size": "M3"}, "depth": "through_all"}),
    );
    let png = |s: &mut Session, threads: bool| {
        let p = json!({"part": "p", "width": 200, "height": 150, "threads": threads});
        let a = answer_live(s, None, None, Request::from_rpc("render", Some(p)).unwrap()).unwrap();
        a.binary.unwrap()
    };
    let with = png(&mut s, true);
    assert_ne!(with, png(&mut s, false));
    // A part with no tapped hole renders the same either way.
    crate::hole::tests::put(
        &mut s,
        "h",
        json!({"points": ["p1"], "form": "simple", "diameter": 3, "depth": "through_all"}),
    );
    assert_eq!(png(&mut s, true), png(&mut s, false));
}
