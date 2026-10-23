//! Bit-exact determinism of everything ryucad computes from files. Every CI
//! platform and CPU level must reproduce `tests/golden/rebuild.txt`:
//! expressions, rebuilt bodies, display meshes, the pixels of renders, and
//! solved sketches and the bodies swept from them, features that refer
//! to other features' faces, edges and vertices, how sketches are drawn,
//! what diffs say, what merges make, and the content hashes release
//! records name. After
//! a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryucad --test determinism` and review the
//! diff like any other change.

use ryucad::doc::files::{Feature, PartFile, Placement, Primitive, PrimitiveType};
use ryucad::doc::id::Id;
use ryucad::doc::project::Part;
use ryucad::rebuild::{placement, rebuild_part};
use ryucad::units::{Quantity, Unit};
use ryukei::geom::Point3;
use ryukei_testkit::{Golden, Lcg};

/// A decimal string with up to 6 digits after the point, in 0.1 to 1000.
fn decimal(g: &mut Lcg) -> String {
    let whole = g.next_u64() % 1000;
    let frac = g.next_u64() % 1_000_000;
    format!("{}.{frac:06}", whole.max(1))
}

fn unit(g: &mut Lcg) -> Unit {
    Unit::ALL[(g.next_u64() % 6) as usize]
}

fn units_section(golden: &mut Golden) {
    let mut s = golden.section("units");
    let mut g = Lcg::new(0x756e_6974);
    for _ in 0..4096 {
        let text = format!("{} {}", decimal(&mut g), unit(&mut g));
        s.f64(Quantity::parse(&text, Unit::Mm).unwrap().meters());
    }
}

/// A random expression over `len`, `ang` and `n`, with every operator
/// and function, `depth` levels deep.
fn expression(g: &mut Lcg, depth: u32) -> String {
    const LEAVES: [&str; 6] = ["len", "ang", "n", "pi", "2.5 mm", "7 deg"];
    const FUNCTIONS: [&str; 11] = [
        "sin", "cos", "tan", "asin", "acos", "atan", "sqrt", "abs", "floor", "ceil", "round",
    ];
    if depth == 0 {
        return match g.next_u64() % 3 {
            0 => decimal(g),
            _ => LEAVES[(g.next_u64() % 6) as usize].to_owned(),
        };
    }
    let a = expression(g, depth - 1);
    match g.next_u64() % 8 {
        0 => format!("{a} + {}", expression(g, depth - 1)),
        1 => format!("{a} - {}", expression(g, depth - 1)),
        2 => format!("{a} * {}", expression(g, depth - 1)),
        3 => format!("{a} / ({})", expression(g, depth - 1)),
        4 => format!("({a})^{}", g.next_u64() % 4),
        5 => format!("{}({a})", FUNCTIONS[(g.next_u64() % 11) as usize]),
        6 => format!("atan2({a}, {})", expression(g, depth - 1)),
        _ => format!("max({a}, {})", expression(g, depth - 1)),
    }
}

/// Expressions evaluated, values or the code that refused them. Most mix
/// kinds and fail; the rest pin the arithmetic and `ryukei::math`.
fn expressions_section(golden: &mut Golden) {
    use ryucad::expr::{Expr, Val, no_parameters};
    use ryucad::units::Dim;
    let lookup = |name: &str| match name {
        "len" => Ok(Val::new(0.0125, Dim::LENGTH)),
        "ang" => Ok(Val::new(0.375, Dim::ANGLE)),
        "n" => Ok(Val::new(3.0, Dim::NUMBER)),
        _ => no_parameters(name),
    };
    let mut s = golden.section("expressions");
    let mut g = Lcg::new(0x6578_7072);
    for i in 0..4096 {
        let text = expression(&mut g, 1 + i % 4);
        match Expr::parse(&text).unwrap().eval(&lookup) {
            Ok(v) => {
                s.f64(v.x);
                s.str(&v.dim.name());
            }
            Err(e) => s.str(&serde_json::json!(e.code).to_string()),
        }
    }
}

fn random_placement(g: &mut Lcg) -> Placement {
    let mut p = Placement::identity(Unit::Mm);
    for o in &mut p.origin {
        *o = Quantity::parse(&format!("{} mm", g.next_f64(-1000.0, 1000.0)), Unit::Mm).unwrap();
    }
    p.z = [
        g.next_f64(-1.0, 1.0),
        g.next_f64(-1.0, 1.0),
        g.next_f64(-1.0, 1.0),
    ];
    p.x = [
        g.next_f64(-1.0, 1.0),
        g.next_f64(-1.0, 1.0),
        g.next_f64(-1.0, 1.0),
    ];
    p
}

fn placement_section(golden: &mut Golden) {
    let mut s = golden.section("placement");
    let mut g = Lcg::new(0x706c_6163);
    for _ in 0..1024 {
        let r = placement(&random_placement(&mut g)).unwrap();
        let q = r.apply_point(Point3::new(0.1, 0.2, 0.3));
        for x in r.rotation_matrix().iter().flatten().chain(&[q.x, q.y, q.z]) {
            s.f64(*x);
        }
    }
}

fn rebuild_section(golden: &mut Golden) {
    let mut s = golden.section("rebuild");
    let mut g = Lcg::new(0x7265_6275);
    let mut order = Vec::new();
    let mut features = std::collections::BTreeMap::new();
    for i in 0..120 {
        let ty = PrimitiveType::ALL[i % 5];
        let q = |g: &mut Lcg| Quantity::parse(&format!("{} mm", decimal(g)), Unit::Mm).unwrap();
        let mut params = std::collections::BTreeMap::new();
        for name in ty.required() {
            params.insert(name.to_string(), q(&mut g));
        }
        if ty == PrimitiveType::Torus {
            // The tube must be thinner than the ring.
            let major = params["major_radius"].meters();
            let minor = Quantity::parse(&format!("{} m", major / 3.0), Unit::Mm).unwrap();
            params.insert("minor_radius".into(), minor);
        }
        if ty == PrimitiveType::Cone && i % 2 == 0 {
            params.insert("top_radius".into(), q(&mut g));
        }
        let id = Id::parse(&format!("f{i}"), "feature").unwrap();
        let feature = Feature::Primitive(Primitive {
            ty,
            params,
            placement: random_placement(&mut g),
        });
        order.push(id.clone());
        features.insert(id, Ok(feature));
    }
    let part = Part {
        id: Id::parse("golden", "part").unwrap(),
        file: Ok(PartFile { features: order }),
        parameters: None,
        env: ryucad::expr::Env::empty(),
        uses: Default::default(),
        features,
        revision: String::new(),
    };
    let result = rebuild_part(&part);
    assert_eq!(result.failed(), 0);
    for (_, b) in &result.features {
        s.str(&b.as_ref().unwrap().body().unwrap().body.dump());
    }
    s.str(&result.hash);
}

/// A project with one of each primitive, placed apart and tilted.
fn showcase() -> (tempfile::TempDir, ryucad::session::Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap()).unwrap()
    };
    call(&mut s, "part.create", serde_json::json!({"part": "show"}));
    let features = [
        (
            "box",
            serde_json::json!({"x": 10, "y": 20, "z": 30}),
            [0, 0, 0],
            [0.0, 0.0, 1.0],
        ),
        (
            "cylinder",
            serde_json::json!({"radius": 6, "height": 25}),
            [30, 0, 0],
            [0.0, 0.3, 1.0],
        ),
        (
            "cone",
            serde_json::json!({"radius": 8, "height": 20, "top_radius": 3}),
            [60, 0, 0],
            [0.2, 0.0, 1.0],
        ),
        (
            "sphere",
            serde_json::json!({"radius": 9}),
            [0, 40, 10],
            [0.0, 0.0, 1.0],
        ),
        (
            "torus",
            serde_json::json!({"major_radius": 12, "minor_radius": 3}),
            [40, 40, 5],
            [1.0, 1.0, 1.0],
        ),
    ];
    for (ty, params, origin, z) in features {
        call(
            &mut s,
            "feature.add",
            serde_json::json!({"part": "show", "type": ty, "id": ty, "params": params,
                               "placement": {"origin": origin, "z": z}}),
        );
    }
    (dir, s)
}

fn display_section(golden: &mut Golden) {
    let mut s = golden.section("display");
    use ryucad::mesh::Quality;
    let mut g = Lcg::new(0x6469_7370);
    for _ in 0..256 {
        let d = g.next_f64(1e-9, 1000.0);
        for q in [Quality::Coarse, Quality::Normal, Quality::Fine] {
            let p = q.params(d);
            s.f64(p.chord);
            s.f64(p.angle);
        }
    }
    let (_dir, mut session) = showcase();
    let part = Id::parse("show", "part").unwrap();
    for q in [Quality::Coarse, Quality::Fine] {
        let meshes = ryucad::server::meshes(&mut session, &part, None, q).unwrap();
        s.str(&format!("{:x}", md(&ryucad::mesh::encode(&meshes))));
    }
}

/// A short digest of bytes.
fn md(bytes: &[u8]) -> u128 {
    u128::from_le_bytes(blake3::hash(bytes).as_bytes()[..16].try_into().unwrap())
}

fn render_section(golden: &mut Golden) {
    let mut s = golden.section("render");
    let (dir, mut session) = showcase();
    let face = {
        let v = ryucad::server::handle(
            &mut session,
            ryucad::proto::Request::from_rpc(
                "inspect",
                Some(serde_json::json!({"part": "show", "feature": "torus"})),
            )
            .unwrap(),
        )
        .unwrap();
        format!(
            "show/torus/{}",
            v["bodies"][0]["faces"][0]["name"].as_str().unwrap()
        )
    };
    let views = ["iso", "front", "top", "right"];
    for (i, view) in views.iter().enumerate() {
        let params = serde_json::json!({"view": view, "width": 320, "height": 240,
                                        "highlight": if i == 0 { vec![face.clone()] } else { vec![] },
                                        "labels": i == 0, "edges": i != 3});
        let req = ryucad::proto::Request::from_rpc("render", Some(params)).unwrap();
        let a =
            ryucad::server::handle_safely(&mut Some(session), None, None, dir.path(), req).unwrap();
        session = ryucad::session::Session::open(dir.path()).unwrap();
        let png = a.binary.unwrap();
        let mut r = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        s.str(&format!("{:x}", md(&buf[..info.buffer_size()])));
    }
}

/// A part with a sketch of each kind of curve, swept every way: a slot
/// with a round hole, extruded and revolved part way and all the way.
fn swept() -> (tempfile::TempDir, ryucad::session::Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap()).unwrap()
    };
    call(&mut s, "part.create", serde_json::json!({"part": "sw"}));
    call(
        &mut s,
        "param.set",
        serde_json::json!({"part": "sw", "parameters": {"len": "23.7 mm"}}),
    );
    call(
        &mut s,
        "feature.add",
        serde_json::json!({"part": "sw", "type": "sketch", "id": "slot",
                           "placement": {"origin": ["3 mm", "-2 mm", "1.5 mm"], "z": [0.2, 0.3, 1]}}),
    );
    let ops = serde_json::json!([
        {"op": "add", "id": "c1", "type": "point", "at": [8, 6]},
        {"op": "add", "id": "c2", "type": "point", "at": [30, 7]},
        {"op": "add", "id": "p1", "type": "point", "at": [8, 1]},
        {"op": "add", "id": "p2", "type": "point", "at": [30, 2]},
        {"op": "add", "id": "p3", "type": "point", "at": [30, 12]},
        {"op": "add", "id": "p4", "type": "point", "at": [8, 11]},
        {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "id": "a1", "type": "arc", "center": "c2", "start": "p2", "end": "p3"},
        {"op": "add", "id": "l2", "type": "line", "start": "p3", "end": "p4"},
        {"op": "add", "id": "a2", "type": "arc", "center": "c1", "start": "p4", "end": "p1"},
        {"op": "add", "id": "h", "type": "circle", "center": "c1", "r": 2},
        {"op": "add", "id": "ax", "type": "line", "construction": true, "start": "q1", "end": "q2"},
        {"op": "add", "id": "q1", "type": "point", "at": [0, -3]},
        {"op": "add", "id": "q2", "type": "point", "at": [0, 20]},
        {"op": "add", "type": "tangent", "a": "l1", "b": "a1"},
        {"op": "add", "type": "tangent", "a": "l2", "b": "a1"},
        {"op": "add", "type": "tangent", "a": "l2", "b": "a2"},
        {"op": "add", "type": "tangent", "a": "l1", "b": "a2"},
        {"op": "add", "type": "equal", "a": "a1", "b": "a2"},
        {"op": "add", "type": "radius", "a": "a1", "value": "5.25 mm"},
        {"op": "add", "type": "distance", "a": "c1", "b": "c2", "value": "len"},
        {"op": "add", "type": "angle", "a": "ax", "b": "l1", "value": "-80 deg"},
        {"op": "add", "type": "vertical", "a": "ax"},
        {"op": "add", "type": "fix", "a": "q1"},
        {"op": "add", "type": "fix", "a": "c1"}
    ]);
    // The axis's points come after the line that uses them in the batch.
    let mut ops = ops.as_array().unwrap().clone();
    let axis = ops.remove(11);
    ops.insert(13, axis);
    call(
        &mut s,
        "sketch.edit",
        serde_json::json!({"part": "sw", "sketch": "slot", "ops": ops}),
    );
    for f in [
        serde_json::json!({"type": "extrude", "id": "plate", "sketch": "slot",
                           "extent": {"type": "symmetric", "length": "4.4 mm"}}),
        serde_json::json!({"type": "extrude", "id": "boss", "sketch": "slot", "loops": ["h"],
                           "extent": {"length": "len / 3"}, "reverse": true}),
        serde_json::json!({"type": "revolve", "id": "ring", "sketch": "slot", "axis": "ax",
                           "angle": "full"}),
        serde_json::json!({"type": "revolve", "id": "arch", "sketch": "slot", "axis": "ax",
                           "angle": "137.5 deg", "symmetric": true, "loops": ["l1"]}),
    ] {
        let mut f = f;
        f["part"] = serde_json::json!("sw");
        call(&mut s, "feature.add", f);
    }
    (dir, s)
}

/// The solved sketch, every swept body bit for bit, and their display
/// meshes.
fn sweeps_section(golden: &mut Golden) {
    let mut s = golden.section("sweeps");
    let (dir, mut session) = swept();
    let text = std::fs::read_to_string(dir.path().join("parts/sw/features/slot.json")).unwrap();
    s.str(&text);
    let part = Id::parse("sw", "part").unwrap();
    let result = session.rebuilt(&part).unwrap().clone();
    assert_eq!(result.failed(), 0, "{:?}", result.features);
    for (_, o) in &result.features {
        match o.as_ref().unwrap().body() {
            Some(b) => s.str(&b.body.dump()),
            None => s.str(o.as_ref().unwrap().hash()),
        }
    }
    s.str(&result.hash);
    let meshes =
        ryucad::server::meshes(&mut session, &part, None, ryucad::mesh::Quality::Coarse).unwrap();
    s.str(&format!("{:x}", md(&ryucad::mesh::encode(&meshes))));
}

/// How the swept part's sketch is drawn: `sketch.get` at each quality,
/// and renders with its dimensions labelled and an entity highlighted.
fn sketch_drawing_section(golden: &mut Golden) {
    let mut s = golden.section("sketch_drawing");
    let (dir, mut session) = swept();
    for q in ["coarse", "normal", "fine"] {
        let v = ryucad::server::handle(
            &mut session,
            ryucad::proto::Request::from_rpc(
                "sketch.get",
                Some(serde_json::json!({"part": "sw", "sketch": "slot", "quality": q})),
            )
            .unwrap(),
        )
        .unwrap();
        s.str(&ryucad::doc::canonical::to_string(&v));
    }
    for (view, labels) in [("iso", true), ("top", false)] {
        let params = serde_json::json!({"part": "sw", "view": view, "width": 320, "height": 240,
                                        "highlight": ["sw/slot/a1"], "labels": labels});
        let req = ryucad::proto::Request::from_rpc("render", Some(params)).unwrap();
        let a =
            ryucad::server::handle_safely(&mut Some(session), None, None, dir.path(), req).unwrap();
        session = ryucad::session::Session::open(dir.path()).unwrap();
        let png = a.binary.unwrap();
        let mut r = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        s.str(&format!("{:x}", md(&buf[..info.buffer_size()])));
    }
}

/// The entities of `kind` of `feature`'s body in `inspect`.
fn listed(
    s: &mut ryucad::session::Session,
    part: &str,
    feature: &str,
    kind: &str,
) -> Vec<serde_json::Value> {
    let req = ryucad::proto::Request::from_rpc(
        "inspect",
        Some(serde_json::json!({"part": part, "feature": feature})),
    )
    .unwrap();
    let v = ryucad::server::handle(s, req).unwrap();
    v["bodies"][0][kind].as_array().unwrap().clone()
}

fn dot(a: &serde_json::Value, b: [f64; 3]) -> f64 {
    (0..3).map(|k| a[k].as_f64().unwrap() * b[k]).sum()
}

/// A part whose features refer to each other every way, off the axes: a
/// tilted box; a sketch on its top, placed by a corner and an edge, with
/// that corner's far neighbour and an edge projected; its extrude; a
/// cylinder's rim and the extrude's corner projected onto xy, extruded up
/// to that corner; and a sketch on zx revolved about an upright box's
/// edge.
fn referenced() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    call(&mut s, "part.create", json!({"part": "rf"}));
    call(
        &mut s,
        "param.set",
        json!({"part": "rf", "parameters": {"w": "11.3 mm"}}),
    );
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("rf");
        call(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": "31.3 mm", "y": "17.9 mm", "z": "6.1 mm"},
               "placement": {"origin": ["1 mm", "2 mm", "3 mm"], "z": [0.1, 0.2, 1]}}),
    );
    // The top: the face whose outward normal rises most; its corners and
    // edges are the points in its plane.
    let faces = listed(&mut s, "rf", "b", "faces");
    let normal = |f: &serde_json::Value| {
        let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
        let z = &f["surface"]["frame"]["z"];
        [0, 1, 2].map(|k| z[k].as_f64().unwrap() * sign)
    };
    let top = faces
        .iter()
        .max_by(|a, b| normal(a)[2].total_cmp(&normal(b)[2]))
        .unwrap();
    let (n, o) = (normal(top), top["surface"]["frame"]["origin"].clone());
    let on_top = |p: &serde_json::Value| (dot(p, n) - dot(&o, n)).abs() < 1e-12;
    let mut corners: Vec<serde_json::Value> = listed(&mut s, "rf", "b", "vertices")
        .into_iter()
        .filter(|v| on_top(&v["point"]))
        .collect();
    corners.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let name = |v: &serde_json::Value| v["name"].as_str().unwrap().to_owned();
    let (c0, c1) = (name(&corners[0]), name(&corners[1]));
    let mut edges: Vec<serde_json::Value> = listed(&mut s, "rf", "b", "edges")
        .into_iter()
        .filter(|e| corners.iter().any(|c| c["name"] == e["start"]))
        .filter(|e| corners.iter().any(|c| c["name"] == e["end"]))
        .collect();
    edges.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let (e0, e1) = (name(&edges[0]), name(&edges[1]));
    let r = |f: &str, n: &str| format!("rf/{f}/{n}");
    add(
        &mut s,
        json!({"type": "sketch", "id": "s", "face": r("b", &name(top)),
               "face_origin": r("b", &c0), "face_x": r("b", &e0)}),
    );
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "rf", "sketch": "s", "ops": [
            {"op": "add", "id": "k", "type": "point", "projected": r("b", &c1)},
            {"op": "add", "id": "rim", "type": "line", "construction": true,
             "projected": r("b", &e1)},
            {"op": "add", "type": "point", "at": [3, 3]},
            {"op": "add", "type": "point", "at": [12, 3.5]},
            {"op": "add", "type": "point", "at": [6, 9]},
            {"op": "add", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "type": "line", "start": "p4", "end": "p5"},
            {"op": "add", "type": "line", "start": "p5", "end": "p3"},
            {"op": "add", "type": "length", "a": "l1", "value": "w"},
            {"op": "add", "type": "parallel", "a": "l1", "b": "rim"},
            {"op": "add", "type": "distance", "a": "k", "b": "p5", "value": "4.7 mm"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "e", "sketch": "s", "extent": {"length": "4.2 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "c", "params": {"radius": "3.3 mm", "height": "7.7 mm"},
               "placement": {"origin": ["60 mm", "0 mm", "0 mm"]}}),
    );
    let rim = listed(&mut s, "rf", "c", "edges")
        .into_iter()
        .find(|e| e["curve"]["type"] == "circle" && e["curve"]["frame"]["origin"][2] != 0.0)
        .unwrap();
    let peak = listed(&mut s, "rf", "e", "vertices")
        .into_iter()
        .max_by(|a, b| {
            a["point"][2]
                .as_f64()
                .unwrap()
                .total_cmp(&b["point"][2].as_f64().unwrap())
        })
        .unwrap();
    add(&mut s, json!({"type": "sketch", "id": "t"}));
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "rf", "sketch": "t", "ops": [
            {"op": "add", "type": "circle", "projected": r("c", &name(&rim))},
            {"op": "add", "type": "point", "projected": r("e", &name(&peak))}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "f", "sketch": "t",
               "extent": {"type": "up_to", "target": r("e", &name(&peak)), "offset": "0.3 mm"}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "d", "params": {"x": "9 mm", "y": "4 mm", "z": "2 mm"},
               "placement": {"origin": ["-30 mm", "0 mm", "0 mm"]}}),
    );
    let along = listed(&mut s, "rf", "d", "edges")
        .into_iter()
        .find(|e| {
            e["curve"]["type"] == "line"
                && e["curve"]["dir"][0].as_f64().unwrap().abs() == 1.0
                && e["curve"]["origin"][1] == 0.0
                && e["curve"]["origin"][2] == 0.0
        })
        .unwrap();
    add(&mut s, json!({"type": "sketch", "id": "u", "plane": "zx"}));
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "rf", "sketch": "u", "ops": [
            {"op": "add", "type": "point", "at": [3, -28]},
            {"op": "add", "type": "point", "at": [7.5, -28]},
            {"op": "add", "type": "point", "at": [5, -23.25]},
            {"op": "add", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "type": "line", "start": "p3", "end": "p1"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "revolve", "id": "r", "sketch": "u", "axis": r("d", &name(&along)),
               "angle": "211.7 deg"}),
    );
    (dir, s)
}

/// Every sketch file as written, every body bit for bit, and their
/// display meshes.
fn references_section(golden: &mut Golden) {
    let mut s = golden.section("references");
    let (dir, mut session) = referenced();
    for f in ["s", "t", "u"] {
        let path = dir.path().join(format!("parts/rf/features/{f}.json"));
        s.str(&std::fs::read_to_string(path).unwrap());
    }
    let part = Id::parse("rf", "part").unwrap();
    let result = session.rebuilt(&part).unwrap().clone();
    assert_eq!(result.failed(), 0, "{:?}", result.features);
    assert!(result.warnings().is_empty());
    for (_, o) in &result.features {
        match o.as_ref().unwrap().body() {
            Some(b) => s.str(&b.body.dump()),
            None => s.str(o.as_ref().unwrap().hash()),
        }
    }
    s.str(&result.hash);
    let meshes =
        ryucad::server::meshes(&mut session, &part, None, ryucad::mesh::Quality::Coarse).unwrap();
    s.str(&format!("{:x}", md(&ryucad::mesh::encode(&meshes))));
}

#[test]
fn referenced_bodies_are_sound() {
    let (_dir, mut session) = referenced();
    let part = Id::parse("rf", "part").unwrap();
    let result = session.rebuilt(&part).unwrap();
    for (id, o) in &result.features {
        let Some(b) = o.as_ref().unwrap().body() else {
            continue;
        };
        assert!(ryukei::check::check(&b.body).is_empty(), "{id}");
        assert!(
            ryukei::check::check_intersections(&b.body).is_empty(),
            "{id}"
        );
    }
}

/// Every file of the referenced project and of an edit of it as textconv
/// prints them, the structural diff between them (every position listed)
/// and the model diff, bounds and all.
fn diff_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("diff");
    let (dir, mut session) = referenced();
    let part = Id::parse("rf", "part").unwrap();
    let built = |session: &mut ryucad::session::Session| {
        let r = session.rebuilt(&part).unwrap().features.clone();
        [("rf".to_owned(), r)].into_iter().collect()
    };
    let before = ryucad::activity::snapshot(dir.path()).unwrap();
    let from = built(&mut session);
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    call(
        &mut session,
        "param.set",
        json!({"part": "rf", "parameters": {"w": "12.7 mm"}}),
    );
    call(
        &mut session,
        "feature.edit",
        json!({"part": "rf", "id": "b", "params": {"x": "33.1 mm"}}),
    );
    let after = ryucad::activity::snapshot(dir.path()).unwrap();
    let to = built(&mut session);
    for files in [&before, &after] {
        for bytes in files.values() {
            s.str(&String::from_utf8(ryucad::diff::textconv(bytes)).unwrap());
        }
    }
    let opts = ryucad::diff::Options {
        positions: true,
        ..Default::default()
    };
    let mut v = ryucad::diff::structural(&before, &after, &opts);
    s.str(&ryucad::doc::canonical::to_string(&v));
    ryucad::diff::model(&mut v, &from, &to, &opts);
    s.str(&ryucad::doc::canonical::to_string(&v));
}

/// A sketch: a square's corners, sides and a length, each corner moved a
/// little by `g`.
fn merge_sketch(g: &mut Lcg) -> serde_json::Value {
    use serde_json::json;
    let mut e = serde_json::Map::new();
    for (i, [x, y]) in [[0.0, 0.0], [0.01, 0.0], [0.01, 0.01], [0.0, 0.01]]
        .iter()
        .enumerate()
    {
        let m = [x + g.next_f64(-1e-3, 1e-3), y + g.next_f64(-1e-3, 1e-3)];
        e.insert(format!("p{}", i + 1), json!({"type": "point", "m": m}));
        e.insert(
            format!("l{}", i + 1),
            json!({"type": "line", "start": format!("p{}", i + 1),
                   "end": format!("p{}", (i + 1) % 4 + 1)}),
        );
    }
    json!({"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "xy",
           "entities": e,
           "constraints": {"d1": {"type": "length", "a": "l1", "value": "10 mm"}}})
}

/// One random edit of a sketch: a point moved, a point added (one past the
/// largest, so two sides collide), a dimension changed, a side made
/// construction, or a line removed.
fn merge_edit(g: &mut Lcg, v: &mut serde_json::Value) {
    use serde_json::json;
    let n = (g.next_u64() % 4 + 1) as usize;
    match g.next_u64() % 5 {
        0 => {
            v["entities"][format!("p{n}")]["m"] =
                json!([g.next_f64(-0.02, 0.02), g.next_f64(-0.02, 0.02)])
        }
        1 => {
            let next = (1..)
                .map(|k| format!("p{k}"))
                .find(|k| v["entities"].get(k).is_none())
                .unwrap();
            v["entities"][next] = json!({"type": "point", "m": [g.next_f64(0.0, 0.02), 0.03]});
        }
        2 => v["constraints"]["d1"]["value"] = json!(format!("{} mm", decimal(g))),
        3 => v["entities"][format!("l{n}")]["construction"] = json!(true),
        _ => {
            v["entities"]
                .as_object_mut()
                .unwrap()
                .remove(&format!("l{n}"));
            if n == 1 {
                v["constraints"].as_object_mut().unwrap().remove("d1");
            }
        }
    }
}

/// Merged files: random edits of sketches, feature lists and parameters
/// on two sides, a file merged as lines, and the renumbering of a feature
/// both sides added, names mapped through two builds of it.
fn merge_section(golden: &mut Golden) {
    use ryucad::merge::bytes;
    use serde_json::json;
    let mut s = golden.section("merge");
    let text = |v: &serde_json::Value| ryucad::doc::canonical::to_string(v).into_bytes();
    let mut g = Lcg::new(0x6d65_7267);
    for _ in 0..256 {
        let base = merge_sketch(&mut g);
        let (mut ours, mut theirs) = (base.clone(), base.clone());
        for _ in 0..g.next_u64() % 4 {
            merge_edit(&mut g, &mut ours);
        }
        for _ in 0..g.next_u64() % 4 {
            merge_edit(&mut g, &mut theirs);
        }
        let (out, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
        s.str(std::str::from_utf8(&out).unwrap());
    }
    for _ in 0..64 {
        let base: Vec<String> = (0..6).map(|i| format!("f{i}")).collect();
        let side = |g: &mut Lcg, prefix: &str| {
            let mut l = base.clone();
            for k in 0..g.next_u64() % 4 {
                match g.next_u64() % 3 {
                    0 if !l.is_empty() => {
                        let i = (g.next_u64() % l.len() as u64) as usize;
                        l.remove(i);
                    }
                    1 if !l.is_empty() => {
                        let i = (g.next_u64() % l.len() as u64) as usize;
                        let f = l.remove(i);
                        let at = (g.next_u64() % (l.len() as u64 + 1)) as usize;
                        l.insert(at, f);
                    }
                    _ => {
                        let at = (g.next_u64() % (l.len() as u64 + 1)) as usize;
                        l.insert(at, format!("{prefix}{k}"));
                    }
                }
            }
            json!({"features": l, "format": 1, "kind": "ryucad.part"})
        };
        let part = json!({"features": base, "format": 1, "kind": "ryucad.part"});
        let (ours, theirs) = (side(&mut g, "o"), side(&mut g, "t"));
        let (out, _) = bytes(&text(&part), &text(&ours), &text(&theirs));
        s.str(std::str::from_utf8(&out).unwrap());
        let params = |g: &mut Lcg| {
            let mut p = serde_json::Map::new();
            for i in 0..4 {
                if g.next_u64().is_multiple_of(2) {
                    p.insert(format!("p{i}"), json!(format!("{} mm", decimal(g))));
                }
            }
            json!({"format": 1, "kind": "ryucad.parameters", "parameters": p})
        };
        let (b, o, t) = (params(&mut g), params(&mut g), params(&mut g));
        let (out, _) = bytes(&text(&b), &text(&o), &text(&t));
        s.str(std::str::from_utf8(&out).unwrap());
    }
    let (out, n) = ryucad::merge::text(b"a\nb\nc\nd\n", b"a\nB\nc\nd\ne\n", b"z\na\nX\nc\nd\n");
    s.str(std::str::from_utf8(&out).unwrap());
    s.u64(n as u64);
    // Both sides add box-2; theirs sketches on its far face. Renumbered,
    // the sketch follows theirs' box by the face's new name.
    let files = |list: &[(&str, serde_json::Value)]| -> ryucad::activity::Files {
        list.iter()
            .map(|(p, v)| ((*p).to_owned(), text(v)))
            .collect()
    };
    let manifest = json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"});
    let boxed = |x: &str| {
        json!({"format": 1, "kind": "ryucad.feature", "type": "box",
               "params": {"x": x, "y": "10 mm", "z": "10 mm"},
               "placement": {"origin": ["0 mm", "0 mm", "0 mm"], "x": [1, 0, 0], "z": [0, 0, 1]}})
    };
    let list = |f: &[&str]| json!({"features": f, "format": 1, "kind": "ryucad.part"});
    let ours = files(&[
        ("ryucad.json", manifest.clone()),
        ("parts/p/part.json", list(&["box-2"])),
        ("parts/p/features/box-2.json", boxed("5 mm")),
    ]);
    let alone = files(&[
        ("ryucad.json", manifest.clone()),
        ("parts/p/part.json", list(&["box-2"])),
        ("parts/p/features/box-2.json", boxed("30 mm")),
    ]);
    let root = std::path::Path::new("/golden");
    let project = ryucad::doc::project::Project::load_from(root, &alone).unwrap();
    let built = rebuild_part(&project.parts[&Id::parse("p", "part").unwrap()]);
    let body = &built.features[0].1.as_ref().unwrap().body().unwrap().body;
    let far = body
        .faces()
        .find(|f| {
            let face = body.face(*f).unwrap();
            body.surface(face.surface)
                .and_then(|s| s.frame())
                .is_some_and(|frame| frame.origin().x > 0.029)
        })
        .map(|f| body.face(f).unwrap().name.to_string())
        .unwrap();
    let sketch = json!({"format": 2, "kind": "ryucad.feature", "type": "sketch",
                        "face": {"feature": "box-2", "name": far},
                        "entities": {}, "constraints": {}});
    let theirs = files(&[
        ("ryucad.json", manifest.clone()),
        ("parts/p/part.json", list(&["box-2", "on"])),
        ("parts/p/features/box-2.json", boxed("30 mm")),
        ("parts/p/features/on.json", sketch.clone()),
    ]);
    let mut working = theirs.clone();
    let m = ryucad::merge::merge(None, &boxed("5 mm"), &boxed("30 mm"));
    working.insert("parts/p/features/box-2.json".into(), text(&m.to_json()));
    let c = &m.conflicts[""];
    let (r, changes) = ryucad::renumber::plan(
        root,
        &working,
        &ours,
        &theirs,
        "parts/p/features/box-2.json",
        "",
        c,
    )
    .unwrap();
    s.str(r.to());
    for ch in changes {
        s.str(&ch.path);
        s.str(&String::from_utf8(ch.content.unwrap_or_default()).unwrap());
    }
}

/// Content hashes of projects of random primitives, units and parameters,
/// written as ryucad writes them: what a release record names must be the
/// same everywhere.
fn pdm_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("pdm");
    let text = |v: &serde_json::Value| ryucad::doc::canonical::to_string(v).into_bytes();
    let mut g = Lcg::new(0x7064_6d30);
    for _ in 0..64 {
        let unit = unit(&mut g);
        let mut files = ryucad::activity::Files::new();
        files.insert(
            "ryucad.json".into(),
            text(&ryucad::doc::files::Manifest::new(unit).to_json()),
        );
        let mut order = Vec::new();
        for i in 0..1 + g.next_u64() % 4 {
            let ty = PrimitiveType::ALL[(g.next_u64() % 5) as usize];
            let params = ty
                .required()
                .iter()
                .map(|n| {
                    let q = format!("{} {}", decimal(&mut g), unit.as_str());
                    (n.to_string(), Quantity::parse(&q, unit).unwrap())
                })
                .collect();
            let f = Feature::Primitive(Primitive {
                ty,
                params,
                placement: random_placement(&mut g),
            });
            let id = format!("f{i}");
            files.insert(format!("parts/p/features/{id}.json"), text(&f.to_json()));
            order.push(Id::parse(&id, "feature").unwrap());
        }
        files.insert(
            "parts/p/part.json".into(),
            text(&PartFile { features: order }.to_json()),
        );
        if g.next_u64().is_multiple_of(2) {
            let p = json!({"format": 1, "kind": "ryucad.parameters",
                           "parameters": {"w": format!("{} mm", decimal(&mut g))}});
            files.insert("parts/p/parameters.json".into(), text(&p));
        }
        let part = Id::parse("p", "part").unwrap();
        s.str(&ryupdm::content::hash(&files, &part).unwrap());
    }
}

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/rebuild.txt"
    ));
    units_section(&mut golden);
    expressions_section(&mut golden);
    placement_section(&mut golden);
    rebuild_section(&mut golden);
    display_section(&mut golden);
    render_section(&mut golden);
    sweeps_section(&mut golden);
    references_section(&mut golden);
    sketch_drawing_section(&mut golden);
    diff_section(&mut golden);
    merge_section(&mut golden);
    pdm_section(&mut golden);
    golden.finish();
}
