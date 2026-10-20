//! Bit-exact determinism of everything ryucad computes from files. Every CI
//! platform and CPU level must reproduce `tests/golden/rebuild.txt`:
//! rebuilt bodies, display meshes, and the pixels of renders. After
//! a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryucad --test determinism` and review the
//! diff like any other change.

use ryucad::doc::files::{Feature, FeatureType, PartFile, Placement};
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
        let ty = FeatureType::ALL[i % 5];
        let q = |g: &mut Lcg| Quantity::parse(&format!("{} mm", decimal(g)), Unit::Mm).unwrap();
        let mut params = std::collections::BTreeMap::new();
        for name in ty.required() {
            params.insert(name.to_string(), q(&mut g));
        }
        if ty == FeatureType::Torus {
            // The tube must be thinner than the ring.
            let major = params["major_radius"].meters();
            let minor = Quantity::parse(&format!("{} m", major / 3.0), Unit::Mm).unwrap();
            params.insert("minor_radius".into(), minor);
        }
        if ty == FeatureType::Cone && i % 2 == 0 {
            params.insert("top_radius".into(), q(&mut g));
        }
        let id = Id::parse(&format!("f{i}"), "feature").unwrap();
        let feature = Feature {
            ty,
            params,
            placement: random_placement(&mut g),
        };
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
        s.str(&b.as_ref().unwrap().body.dump());
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

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/rebuild.txt"
    ));
    units_section(&mut golden);
    placement_section(&mut golden);
    rebuild_section(&mut golden);
    display_section(&mut golden);
    render_section(&mut golden);
    golden.finish();
}
