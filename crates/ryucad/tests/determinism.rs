//! Bit-exact determinism of everything ryucad computes from files. Every CI
//! platform and CPU level must reproduce `tests/golden/rebuild.txt`:
//! expressions, rebuilt bodies, display meshes, the pixels of renders, and
//! solved sketches and the bodies swept from them. After
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
    golden.finish();
}
