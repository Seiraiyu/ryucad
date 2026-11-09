//! Bit-exact determinism of everything ryucad computes from files. Every CI
//! platform and CPU level must reproduce `tests/golden/rebuild.txt`:
//! expressions, rebuilt bodies, display meshes, the pixels of renders, and
//! solved sketches and the bodies swept from them, features that refer
//! to other features' faces, edges and vertices, solids that features
//! join, cut and intersect, holes and patterns, how sketches are drawn,
//! what diffs say, what merges make, the content hashes release records
//! name, where assemblies place their parts, the bodies and names of
//! imported STEP files, the bytes of exported STEP, STL, 3MF and USD
//! files, cameras:
//! the cube's views, named views and views normal to faces, renders
//! in colour, translucent, with an assembly's overrides, and the disk
//! cache's keys and entries under a release's identity (what lets a
//! release on one platform read what another wrote), and datum planes,
//! axes and points with what is built on them. After
//! a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryucad --test determinism` and review the
//! diff like any other change.

use ryucad::doc::files::{Feature, PartFile, Placement, Primitive, PrimitiveType};
use ryucad::doc::id::Id;
use ryucad::doc::project::Part;
use ryucad::rebuild::{placement, rebuild_part};
use ryucad::units::{Quantity, Unit};
use ryukei::geom::Point3;
use ryukei_testkit::{Golden, Lcg, Section};

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
            combine: ryucad::doc::solid::Combine::NEW,
        });
        order.push(id.clone());
        features.insert(id, Ok(feature));
    }
    let part = Part {
        id: Id::parse("golden", "part").unwrap(),
        file: Ok(PartFile::new(order)),
        parameters: None,
        env: ryucad::expr::Env::empty(),
        project_reads: Default::default(),
        uses: Default::default(),
        acts: Default::default(),
        imports: Default::default(),
        views: Default::default(),
        appearance: None,
        explodes: Default::default(),
        drawings: Default::default(),
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

/// The disk cache under a fixed release's identity: the identity, a key
/// of each kind, and every entry a rebuild and its meshes write (sorted by
/// their bytes, since a feature's key holds the versions a release stamps,
/// and its payload doesn't). Every platform must write the same bytes, or
/// a remote cache couldn't serve one platform what another filled.
fn cache_section(golden: &mut Golden) {
    use ryucad::cache::{Disk, Identity, Kind, released_identity};
    let mut s = golden.section("cache");
    let bytes = released_identity("0.7.0", "0.7.0", "0.7.0", "0123456789ab");
    s.str(&blake3::Hash::from(bytes).to_hex());
    let identity = Identity {
        bytes,
        shared: true,
    };
    let (dir, _) = showcase();
    let disk = std::sync::Arc::new(Disk::open_as(dir.path().join("cache"), identity));
    for kind in Kind::ALL {
        s.str(&disk.key(kind, b"golden input"));
    }
    let mut session = ryucad::session::Session::open(dir.path())
        .unwrap()
        .with_disk(Some(std::sync::Arc::clone(&disk)));
    let part = Id::parse("show", "part").unwrap();
    ryucad::server::meshes(&mut session, &part, None, ryucad::mesh::Quality::Coarse).unwrap();
    for kind in Kind::ALL {
        let keys = disk.keys_of(kind, &bytes);
        let mut entries: Vec<(u128, usize)> = keys
            .iter()
            .map(|k| {
                let e = disk.get_entry(kind, k).unwrap();
                (md(&e), e.len())
            })
            .collect();
        entries.sort();
        s.str(&format!("{} {}", kind.as_str(), entries.len()));
        for (h, len) in entries {
            s.str(&format!("{h:x} {len}"));
        }
        // A mesh's key is the body's hash and the parameters alone.
        if kind == Kind::Mesh {
            for k in keys {
                s.str(&k);
            }
        }
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

/// The showcase from the cube's edges and corners, dimetric and
/// trimetric, a named view with the origin drawn, and cameras normal to
/// a tilted face and to a cone's axis.
fn views_section(golden: &mut Golden) {
    let mut s = golden.section("views");
    let (dir, mut session) = showcase();
    let call = |session: &mut ryucad::session::Session,
                live: &mut ryucad::live::Live,
                m: &str,
                p: serde_json::Value| {
        ryucad::server::handle_live(
            session,
            Some(live),
            None,
            ryucad::proto::Request::from_rpc(m, Some(p)).unwrap(),
        )
        .unwrap()
    };
    let mut live = ryucad::live::Live::new();
    // The cylinder's flat ends lean with its axis; the cone is revolved.
    let faces = |session: &mut ryucad::session::Session, live: &mut ryucad::live::Live, f: &str| {
        let v = call(
            session,
            live,
            "inspect",
            serde_json::json!({"part": "show", "feature": f}),
        );
        v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| {
                (
                    x["surface"]["type"].as_str().unwrap().to_owned(),
                    format!("show/{f}/{}", x["name"].as_str().unwrap()),
                )
            })
            .collect::<Vec<_>>()
    };
    let end = faces(&mut session, &mut live, "cylinder")
        .into_iter()
        .find(|(t, _)| t == "plane")
        .unwrap()
        .1;
    let side = faces(&mut session, &mut live, "cone")
        .into_iter()
        .find(|(t, _)| t == "cone")
        .unwrap()
        .1;
    for face in [end, side] {
        let c = call(
            &mut session,
            &mut live,
            "camera.set",
            serde_json::json!({"normal_to": face}),
        );
        for k in ["eye", "target", "up"] {
            for x in c["camera"][k].as_array().unwrap() {
                s.f64(x.as_f64().unwrap());
            }
        }
    }
    call(
        &mut session,
        &mut live,
        "view.save",
        serde_json::json!({"part": "show", "view": "detail",
                           "camera": {"eye": ["90 mm", "-60 mm", "70 mm"], "target": ["30 mm", "20 mm", "10 mm"],
                                      "up": [0, 0, 1], "projection": {"orthographic": {"height": "110 mm"}}}}),
    );
    let renders = [
        serde_json::json!({"view": "front_top_left"}),
        serde_json::json!({"view": "back_bottom_right"}),
        serde_json::json!({"view": "top_right"}),
        serde_json::json!({"view": "dimetric"}),
        serde_json::json!({"view": "trimetric"}),
        serde_json::json!({"view": "detail", "origin": true}),
    ];
    for mut params in renders {
        params["width"] = serde_json::json!(320);
        params["height"] = serde_json::json!(240);
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

/// The showcase coloured: the part, one solid translucent over another,
/// the box's faces each its own colour, the cone in its material's
/// colour; and an assembly placing it twice, one occurrence recoloured
/// and its torus made translucent. Renders of both, the theme's render
/// beside them, and `appearance.get`'s answers.
fn appearance_section(golden: &mut Golden) {
    let mut s = golden.section("appearance");
    let (dir, mut session) = showcase();
    let call = |session: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(
            session,
            ryucad::proto::Request::from_rpc(m, Some(p)).unwrap(),
        )
        .unwrap()
    };
    call(
        &mut session,
        "materials.set",
        serde_json::json!({"name": "brass", "density": "8.5 g/cm^3", "colour": "#b5a642",
                           "default": true}),
    );
    call(
        &mut session,
        "appearance.set",
        serde_json::json!({"entities": ["show"], "colour": "#4a78c2"}),
    );
    call(
        &mut session,
        "appearance.set",
        serde_json::json!({"entities": ["show/sphere"], "opacity": 0.35}),
    );
    call(
        &mut session,
        "appearance.set",
        serde_json::json!({"entities": ["show/cone"], "material": true}),
    );
    let v = call(
        &mut session,
        "inspect",
        serde_json::json!({"part": "show", "feature": "box"}),
    );
    let palette = [
        "#d03030", "#30a050", "#e0a020", "#8040c0", "#20a0c0", "#c06080",
    ];
    for (f, colour) in v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .zip(palette)
    {
        call(
            &mut session,
            "appearance.set",
            serde_json::json!({"entities": [format!("show/box/{}", f["name"].as_str().unwrap())],
                               "colour": colour, "opacity": 0.8}),
        );
    }
    call(
        &mut session,
        "part.create",
        serde_json::json!({"part": "rack", "kind": "assembly"}),
    );
    for (id, x) in [("one", 0), ("two", 100)] {
        call(
            &mut session,
            "feature.add",
            serde_json::json!({"part": "rack", "type": "instance", "id": id, "of": "show",
                               "placement": {"origin": [x, 0, 0]}}),
        );
    }
    call(
        &mut session,
        "appearance.set",
        serde_json::json!({"entities": ["rack/two"], "colour": "#606060"}),
    );
    call(
        &mut session,
        "appearance.set",
        serde_json::json!({"entities": ["rack/two/torus"], "opacity": 0.5}),
    );
    for part in ["show", "rack"] {
        let v = call(
            &mut session,
            "appearance.get",
            serde_json::json!({"part": part}),
        );
        s.str(&ryucad::doc::canonical::to_string(&v["solids"]));
    }
    let renders = [
        serde_json::json!({"part": "show"}),
        serde_json::json!({"part": "show", "view": "front_top_left"}),
        serde_json::json!({"part": "show", "appearance": false}),
        serde_json::json!({"part": "rack"}),
    ];
    for mut params in renders {
        params["width"] = serde_json::json!(320);
        params["height"] = serde_json::json!(240);
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

/// Renders of `session`'s project, each `params` at 320×240, hashed into
/// `s`; returns the session, opened again after each.
fn renders(
    s: &mut Section<'_>,
    dir: &std::path::Path,
    mut session: ryucad::session::Session,
    renders: Vec<serde_json::Value>,
) -> ryucad::session::Session {
    for mut params in renders {
        params["width"] = serde_json::json!(320);
        params["height"] = serde_json::json!(240);
        let req = ryucad::proto::Request::from_rpc("render", Some(params)).unwrap();
        let a = ryucad::server::handle_safely(&mut Some(session), None, None, dir, req).unwrap();
        session = ryucad::session::Session::open(dir).unwrap();
        let png = a.binary.unwrap();
        let mut r = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        s.str(&format!("{:x}", md(&buf[..info.buffer_size()])));
    }
    session
}

/// The showcase cut: by an origin plane, by two planes from another
/// side, askew, in orthographic, translucent faces and all, and by a
/// named view that saved its planes.
fn sections_section(golden: &mut Golden) {
    let mut s = golden.section("sections");
    let (dir, mut session) = showcase();
    ryucad::server::handle(
        &mut session,
        ryucad::proto::Request::from_rpc(
            "appearance.set",
            Some(serde_json::json!({"entities": ["show/sphere"], "opacity": 0.4})),
        )
        .unwrap(),
    )
    .unwrap();
    let views = dir.path().join("parts/show/views");
    std::fs::create_dir_all(&views).unwrap();
    ryucad::doc::project::write_json(
        &views.join("cut.json"),
        &serde_json::json!({"eye_m": [0.25, -0.2, 0.15], "format": 2, "fov_deg": 30,
                            "kind": "ryucad.view", "projection": "perspective",
                            "sections": [{"origin_m": [0, 0.01, 0], "normal": [0, 1, 0]}],
                            "target_m": [0.03, 0.02, 0.01], "up": [0, 0, 1]}),
    )
    .unwrap();
    session.refresh().unwrap();
    renders(
        &mut s,
        dir.path(),
        session,
        vec![
            serde_json::json!({"part": "show", "section": [{"plane": "xy", "offset": 12}]}),
            serde_json::json!({"part": "show", "view": "front_top_left",
                               "section": [{"plane": "zx", "offset": 20, "flip": true},
                                           {"plane": "yz", "offset": 30}]}),
            serde_json::json!({"part": "show",
                               "section": [{"origin": [0, 0, 15], "normal": [1, 1, 0.5]}]}),
            serde_json::json!({"part": "show", "section": [{"plane": "xy", "offset": 8}],
                               "camera": {"eye": [200, -150, 120], "target": [30, 20, 10],
                                          "up": [0, 0, 1],
                                          "projection": {"orthographic": {"height": 140}}}}),
            serde_json::json!({"part": "show", "view": "cut"}),
        ],
    );
}

/// A part `dt`: a box, a datum by every construction that reads it, a
/// sketch on an angled plane extruded, a ring revolved about a datum axis,
/// and a pocket mirrored in a midplane: each datum's report and every
/// solid's hash, rebuilt on one thread and on four; and renders with the
/// datums, a highlighted one, and without them.
fn datums_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("datums");
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut session = ryucad::session::Session::open(dir.path()).unwrap();
    call(&mut session, "part.create", json!({"part": "dt"}));
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("dt");
        call(s, "feature.add", f);
    };
    add(
        &mut session,
        json!({"type": "box", "id": "b", "params": {"x": 30, "y": 20, "z": 8}}),
    );
    let face =
        |s: &mut ryucad::session::Session, n| format!("dt/b/{}", face_along(s, "dt", "b", n));
    let top = face(&mut session, [0.0, 0.0, 1.0]);
    let end = face(&mut session, [1.0, 0.0, 0.0]);
    let start = face(&mut session, [-1.0, 0.0, 0.0]);
    let body = call(
        &mut session,
        "inspect",
        json!({"part": "dt", "feature": "b"}),
    )["bodies"][0]
        .clone();
    let corner = |p: [f64; 3]| {
        let v = body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-12))
            .unwrap();
        format!("dt/b/{}", v["name"].as_str().unwrap())
    };
    // `edge_between` names the blended part's solids.
    let front_top =
        edge_between(&body, "b", [0.0, 0.0, 0.008], [0.03, 0.0, 0.008]).replacen("bl/", "dt/", 1);
    for f in [
        json!({"type": "plane", "id": "off", "by": "offset", "from": top, "distance": "12.5 mm"}),
        json!({"type": "plane", "id": "tilt", "by": "angle", "from": top, "about": front_top,
               "angle": "17 deg"}),
        json!({"type": "plane", "id": "tri", "by": "three_points",
               "points": [corner([0.0, 0.0, 0.0]), corner([0.03, 0.02, 0.0]),
                          corner([0.0, 0.02, 0.008])]}),
        json!({"type": "plane", "id": "par", "by": "parallel", "from": "yz",
               "through": corner([0.03, 0.0, 0.0])}),
        json!({"type": "plane", "id": "ap", "by": "axis_point", "axis": "z",
               "point": corner([0.03, 0.02, 0.008])}),
        json!({"type": "plane", "id": "nt", "by": "normal_to", "curve": front_top, "at": "end"}),
        json!({"type": "plane", "id": "mid", "by": "midplane", "between": [start, end]}),
        json!({"type": "plane", "id": "wedge", "by": "midplane", "between": [top, "dt/tilt"]}),
        json!({"type": "axis", "id": "diag", "by": "two_points",
               "points": [corner([0.0, 0.0, 0.0]), corner([0.03, 0.02, 0.008])]}),
        json!({"type": "axis", "id": "hinge", "by": "planes", "between": ["dt/tilt", "dt/tri"]}),
        json!({"type": "axis", "id": "up", "by": "normal", "plane": "dt/tilt",
               "point": corner([0.03, 0.02, 0.008])}),
        json!({"type": "axis", "id": "zz", "by": "of", "axis": "z", "flip": true}),
        json!({"type": "point", "id": "q1", "by": "coordinates",
               "coordinates": ["1.5 mm", "2.25 mm", "3 mm"], "in": "dt/tilt"}),
        json!({"type": "point", "id": "q2", "by": "along", "curve": front_top, "from": "end",
               "fraction": "0.3"}),
        json!({"type": "point", "id": "q3", "by": "intersection", "between": ["dt/diag", "dt/off"]}),
        json!({"type": "point", "id": "q4", "by": "projection", "point": "dt/q1", "onto": "dt/hinge"}),
        json!({"type": "point", "id": "q5", "by": "midpoint", "points": ["dt/q2", "dt/q3"]}),
        json!({"type": "sketch", "id": "s", "plane": "dt/tilt"}),
    ] {
        add(&mut session, f);
    }
    let rect = |at: [f64; 2], [w, h]: [f64; 2]| {
        let mut ops: Vec<serde_json::Value> = Vec::new();
        for (i, p) in [
            at,
            [at[0] + w, at[1]],
            [at[0] + w, at[1] + h],
            [at[0], at[1] + h],
        ]
        .into_iter()
        .enumerate()
        {
            ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": p}));
            ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
        }
        for i in 1..=4 {
            ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                            "start": format!("p{i}"), "end": format!("p{}", i % 4 + 1)}));
        }
        ops
    };
    call(
        &mut session,
        "sketch.edit",
        json!({"part": "dt", "sketch": "s", "ops": rect([4.0, 3.0], [6.0, 4.0])}),
    );
    add(
        &mut session,
        json!({"type": "extrude", "id": "e", "sketch": "s", "extent": {"length": 3}}),
    );
    add(
        &mut session,
        json!({"type": "sketch", "id": "u", "plane": "zx"}),
    );
    call(
        &mut session,
        "sketch.edit",
        json!({"part": "dt", "sketch": "u", "ops": rect([0.0, 40.0], [5.0, 3.0])}),
    );
    add(
        &mut session,
        json!({"type": "revolve", "id": "r", "sketch": "u", "axis": "dt/zz", "angle": "full"}),
    );
    add(
        &mut session,
        json!({"type": "box", "id": "c", "operation": "cut", "targets": ["b"],
               "params": {"x": 4, "y": 4, "z": 3}, "placement": {"origin": [22, 8, 5]}}),
    );
    add(
        &mut session,
        json!({"type": "pattern", "id": "m", "form": "mirror", "features": ["c"],
               "plane": "dt/mid"}),
    );
    let part = Id::parse("dt", "part").unwrap();
    let p = session.project().part(&part).unwrap().clone();
    let mut answers = Vec::new();
    for threads in [1, 4] {
        let r = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| rebuild_part(&p));
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let lines: Vec<String> = r
            .features
            .iter()
            .map(|(id, o)| {
                let o = o.as_ref().unwrap();
                match o.datum() {
                    Some(d) => format!("{id} {}", ryucad::doc::canonical::to_string(&d.report())),
                    None => format!("{id} {}", o.hash()),
                }
            })
            .collect();
        answers.push(lines);
    }
    assert_eq!(answers[0], answers[1], "one thread and four");
    for l in &answers[0] {
        s.str(l);
    }
    renders(
        &mut s,
        dir.path(),
        session,
        vec![
            json!({"part": "dt", "view": "iso"}),
            json!({"part": "dt", "view": "front", "highlight": ["dt/tilt"]}),
            json!({"part": "dt", "view": "iso", "datums": false}),
        ],
    );
}

/// Paths and helices (plan 13.B): a filleted box's rim from one edge
/// propagated, a path of sketch curves across two sketches, helices right
/// and left, tapered and from a start point, a path along one, and datums
/// along, normal to and through them; each feature's report, on one
/// thread and four, and renders drawing them.
fn paths_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("paths");
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut session = ryucad::session::Session::open(dir.path()).unwrap();
    call(&mut session, "part.create", json!({"part": "pt"}));
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("pt");
        call(s, "feature.add", f);
    };
    add(
        &mut session,
        json!({"type": "box", "id": "b", "params": {"x": 30, "y": 20, "z": 8}}),
    );
    let body = call(
        &mut session,
        "inspect",
        json!({"part": "pt", "feature": "b"}),
    )["bodies"][0]
        .clone();
    let edge = |a: [f64; 3], b: [f64; 3]| edge_between(&body, "b", a, b).replacen("bl/", "pt/", 1);
    let uprights: Vec<String> = [(0.0, 0.0), (0.03, 0.0), (0.03, 0.02), (0.0, 0.02)]
        .iter()
        .map(|(x, y)| edge([*x, *y, 0.0], [*x, *y, 0.008]))
        .collect();
    let front = edge([0.0, 0.0, 0.008], [0.03, 0.0, 0.008]);
    add(
        &mut session,
        json!({"type": "fillet", "id": "f", "radius": "3 mm", "edges": uprights}),
    );
    add(
        &mut session,
        json!({"type": "sketch", "id": "s1", "plane": "xy"}),
    );
    call(
        &mut session,
        "sketch.edit",
        json!({"part": "pt", "sketch": "s1", "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [-10, -10]},
            {"op": "add", "id": "p2", "type": "point", "at": [5, -10]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"}
        ]}),
    );
    // On zx moved to y = -10 mm: its x along z, its y along x.
    add(
        &mut session,
        json!({"type": "plane", "id": "y10", "by": "offset", "from": "zx", "distance": "-10 mm"}),
    );
    add(
        &mut session,
        json!({"type": "sketch", "id": "s2", "plane": "pt/y10"}),
    );
    call(
        &mut session,
        "sketch.edit",
        json!({"part": "pt", "sketch": "s2", "ops": [
            {"op": "add", "id": "c", "type": "point", "at": [4, 5]},
            {"op": "add", "id": "a", "type": "point", "at": [4, 9]},
            {"op": "add", "id": "z", "type": "point", "at": [0, 5]},
            {"op": "add", "id": "a1", "type": "arc", "center": "c", "start": "a", "end": "z"}
        ]}),
    );
    for f in [
        json!({"type": "path", "id": "rim", "segments": [{"curve": front, "propagate": true}]}),
        json!({"type": "helix", "id": "h1", "axis": "z", "radius": "6 mm", "pitch": "1.5 mm",
               "turns": "4.25"}),
        json!({"type": "point", "id": "st", "by": "coordinates",
               "coordinates": ["40 mm", "5 mm", "0"]}),
        json!({"type": "axis", "id": "ax", "by": "normal", "plane": "xy",
               "point": "pt/st"}),
        json!({"type": "helix", "id": "h2", "axis": "pt/ax", "radius": "4 mm", "height": "12 mm",
               "turns": 3, "hand": "left", "taper": "-7.5 deg"}),
        json!({"type": "path", "id": "along", "segments": ["pt/h1"], "reverse": true}),
        json!({"type": "point", "id": "q1", "by": "along", "curve": "pt/rim", "from": "start",
               "fraction": "0.4"}),
        json!({"type": "plane", "id": "n1", "by": "normal_to", "curve": "pt/h2", "at": "end"}),
        json!({"type": "plane", "id": "z3", "by": "offset", "from": "xy", "distance": "3 mm"}),
        json!({"type": "point", "id": "q2", "by": "intersection", "between": ["pt/along", "pt/z3"]}),
        json!({"type": "axis", "id": "a2", "by": "of", "axis": "pt/h2"}),
    ] {
        add(&mut session, f);
    }
    // A path of sketch curves across the two sketches, added last: its
    // arc's end meets the line where it ends.
    add(
        &mut session,
        json!({"type": "path", "id": "w", "segments": ["pt/s1/l1", "pt/s2/a1"]}),
    );
    let part = Id::parse("pt", "part").unwrap();
    let p = session.project().part(&part).unwrap().clone();
    let mut answers = Vec::new();
    for threads in [1, 4] {
        let r = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| rebuild_part(&p));
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let lines: Vec<String> = r
            .features
            .iter()
            .map(|(id, o)| {
                let o = o.as_ref().unwrap();
                match (o.datum(), o.path()) {
                    (Some(d), _) => {
                        format!("{id} {}", ryucad::doc::canonical::to_string(&d.report()))
                    }
                    (_, Some(m)) => format!(
                        "{id} {} {}",
                        m.hash,
                        ryucad::doc::canonical::to_string(&m.report(&part))
                    ),
                    _ => format!("{id} {}", o.hash()),
                }
            })
            .collect();
        answers.push(lines);
    }
    assert_eq!(answers[0], answers[1], "one thread and four");
    for l in &answers[0] {
        s.str(l);
    }
    renders(
        &mut s,
        dir.path(),
        session,
        vec![
            json!({"part": "pt", "view": "iso"}),
            json!({"part": "pt", "view": "top", "highlight": ["pt/rim"]}),
            json!({"part": "pt", "view": "iso", "datums": false}),
        ],
    );
}

/// Sweeps along paths (plan 14.A): a circle up a line, a square twisted
/// and scaled up another, a circle along a path of a line and a tangent
/// arc, and a box's top face swept up and joined to it; each sweep's body
/// by hash, passing the self-intersection check, with its report, on one
/// thread and four, and a render.
fn sweeps_along_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("sweeps_along");
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut session = ryucad::session::Session::open(dir.path()).unwrap();
    call(&mut session, "part.create", json!({"part": "pt"}));
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("pt");
        call(s, "feature.add", f);
    };
    let sketch =
        |s: &mut ryucad::session::Session, id: &str, plane: &str, ops: serde_json::Value| {
            add(s, json!({"type": "sketch", "id": id, "plane": plane}));
            call(
                s,
                "sketch.edit",
                json!({"part": "pt", "sketch": id, "ops": ops}),
            );
        };
    let line = |a: [f64; 2], b: [f64; 2]| {
        json!([
            {"op": "add", "id": "a", "type": "point", "at": a},
            {"op": "add", "id": "b", "type": "point", "at": b},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}
        ])
    };
    add(
        &mut session,
        json!({"type": "box", "id": "b", "params": {"x": 20, "y": 20, "z": 6}}),
    );
    sketch(
        &mut session,
        "c",
        "xy",
        json!([
            {"op": "add", "id": "o", "type": "point", "at": [40, 0]},
            {"op": "add", "id": "ci", "type": "circle", "center": "o", "r": 3}
        ]),
    );
    sketch(&mut session, "up", "zx", line([0.0, 40.0], [25.0, 40.0]));
    sketch(
        &mut session,
        "sq",
        "xy",
        json!([
            {"op": "add", "id": "p1", "type": "point", "at": [-42, -2]},
            {"op": "add", "id": "p2", "type": "point", "at": [-38, -2]},
            {"op": "add", "id": "p3", "type": "point", "at": [-38, 2]},
            {"op": "add", "id": "p4", "type": "point", "at": [-42, 2]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "id": "l4", "type": "line", "start": "p4", "end": "p1"}
        ]),
    );
    sketch(&mut session, "up2", "zx", line([0.0, -40.0], [20.0, -40.0]));
    // A bend on zx moved to y = -40 mm: up z from the y axis, then a
    // quarter turn towards +x (sketch x is z, sketch y is x).
    add(
        &mut session,
        json!({"type": "plane", "id": "y40", "by": "offset", "from": "zx", "distance": "-40 mm"}),
    );
    add(
        &mut session,
        json!({"type": "sketch", "id": "bend", "plane": "pt/y40"}),
    );
    call(
        &mut session,
        "sketch.edit",
        json!({"part": "pt", "sketch": "bend", "ops": [
            {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "c", "type": "point", "at": [10, 15]},
            {"op": "add", "id": "e", "type": "point", "at": [25, 15]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"},
            {"op": "add", "id": "a1", "type": "arc", "center": "c", "start": "b", "end": "e"}
        ]}),
    );
    sketch(
        &mut session,
        "c2",
        "xy",
        json!([
            {"op": "add", "id": "o", "type": "point", "at": [0, -40]},
            {"op": "add", "id": "ci", "type": "circle", "center": "o", "r": 2}
        ]),
    );
    add(
        &mut session,
        json!({"type": "path", "id": "pb", "segments": ["pt/bend/l1", "pt/bend/a1"]}),
    );
    let top = face_along(&mut session, "pt", "b", [0.0, 0.0, 1.0]);
    sketch(&mut session, "rise", "zx", line([6.0, 10.0], [16.0, 10.0]));
    for f in [
        json!({"type": "sweep", "id": "w1", "sketch": "c", "path": "pt/up/l1"}),
        json!({"type": "sweep", "id": "w2", "sketch": "sq", "path": "pt/up2/l1",
               "twist": "120 deg", "scale": "0.6"}),
        json!({"type": "sweep", "id": "w3", "sketch": "c2", "path": "pb"}),
        json!({"type": "sweep", "id": "w4", "face": format!("pt/b/{top}"), "path": "pt/rise/l1",
               "operation": "join", "targets": ["b"]}),
    ] {
        add(&mut session, f);
    }
    let part = Id::parse("pt", "part").unwrap();
    let p = session.project().part(&part).unwrap().clone();
    let mut answers = Vec::new();
    for threads in [1, 4] {
        let r = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| rebuild_part(&p));
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let mut lines = Vec::new();
        for (id, o) in &r.features {
            let o = o.as_ref().unwrap();
            let Some(Ok(Feature::Sweep(w))) = p.features.get(id) else {
                continue;
            };
            for (solid, b) in o.solids() {
                assert_eq!(ryukei::check::check_intersections(&b.body), [], "{id}");
                lines.push(format!("{id} {solid} {}", b.hash));
            }
            let d = ryucad::along::described(&p, id, w, &r).unwrap();
            lines.push(format!("{id} {}", ryucad::doc::canonical::to_string(&d)));
        }
        answers.push(lines);
    }
    assert_eq!(answers[0], answers[1], "one thread and four");
    for l in &answers[0] {
        s.str(l);
    }
    renders(
        &mut s,
        dir.path(),
        session,
        vec![json!({"part": "pt", "view": "iso"})],
    );
}

/// Lofts (plan 14.B): a square to a smaller one on a datum plane facing
/// back (turned over), the same pinned a quarter turn round by `loops`,
/// three circles on datum planes with end tangents, and a box's top face
/// lofted to a sketch above it and joined; each loft's body by hash,
/// passing the self-intersection check, with its report, on one thread
/// and four, and a render.
fn lofts_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("lofts");
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut session = ryucad::session::Session::open(dir.path()).unwrap();
    call(&mut session, "part.create", json!({"part": "pt"}));
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("pt");
        call(s, "feature.add", f);
    };
    let sketch =
        |s: &mut ryucad::session::Session, id: &str, plane: &str, ops: serde_json::Value| {
            add(s, json!({"type": "sketch", "id": id, "plane": plane}));
            call(
                s,
                "sketch.edit",
                json!({"part": "pt", "sketch": id, "ops": ops}),
            );
        };
    let square = |x: f64, y: f64, a: f64| {
        let h = a / 2.0;
        json!([
            {"op": "add", "id": "p1", "type": "point", "at": [x - h, y - h]},
            {"op": "add", "id": "p2", "type": "point", "at": [x + h, y - h]},
            {"op": "add", "id": "p3", "type": "point", "at": [x + h, y + h]},
            {"op": "add", "id": "p4", "type": "point", "at": [x - h, y + h]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "id": "l4", "type": "line", "start": "p4", "end": "p1"}
        ])
    };
    let circle = |x: f64, y: f64, r: f64| {
        json!([
            {"op": "add", "id": "o", "type": "point", "at": [x, y]},
            {"op": "add", "id": "ci", "type": "circle", "center": "o", "r": r}
        ])
    };
    let level = |s: &mut ryucad::session::Session, id: &str, mm: f64, flip: bool| {
        let mut f = json!({"type": "plane", "id": id, "by": "offset", "from": "xy",
                           "distance": format!("{mm} mm")});
        if flip {
            f["flip"] = json!(true);
        }
        add(s, f);
    };
    add(
        &mut session,
        json!({"type": "box", "id": "b", "params": {"x": 20, "y": 20, "z": 6}}),
    );
    sketch(&mut session, "q1", "xy", square(40.0, 0.0, 10.0));
    level(&mut session, "back", 12.0, true);
    // On a plane facing down, sketch y runs the other way.
    sketch(&mut session, "q2", "pt/back", square(40.0, 0.0, 5.0));
    sketch(&mut session, "c1", "xy", circle(-40.0, 0.0, 3.0));
    level(&mut session, "m", 10.0, false);
    sketch(&mut session, "c2", "pt/m", circle(-40.0, 0.0, 5.0));
    level(&mut session, "t", 20.0, false);
    sketch(&mut session, "c3", "pt/t", circle(-40.0, 0.0, 2.0));
    level(&mut session, "over", 16.0, false);
    sketch(&mut session, "q3", "pt/over", square(10.0, 10.0, 6.0));
    let top = face_along(&mut session, "pt", "b", [0.0, 0.0, 1.0]);
    for f in [
        json!({"type": "loft", "id": "f1", "sections": ["q1", "q2"]}),
        json!({"type": "loft", "id": "f2", "sections": [
            {"section": "q1", "loops": ["p1"]}, {"section": "q2", "loops": ["l2"]}]}),
        json!({"type": "loft", "id": "f3", "sections": ["c1", "c2", "c3"],
               "start_tangent": "normal", "end_tangent": "z", "end_weight": "0.5"}),
        json!({"type": "loft", "id": "f4", "sections": [format!("pt/b/{top}"), "q3"],
               "operation": "join", "targets": ["b"]}),
    ] {
        add(&mut session, f);
    }
    let part = Id::parse("pt", "part").unwrap();
    let p = session.project().part(&part).unwrap().clone();
    let mut answers = Vec::new();
    for threads in [1, 4] {
        let r = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| rebuild_part(&p));
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let mut lines = Vec::new();
        for (id, o) in &r.features {
            let o = o.as_ref().unwrap();
            let Some(Ok(Feature::Loft(l))) = p.features.get(id) else {
                continue;
            };
            for (solid, b) in o.solids() {
                assert_eq!(ryukei::check::check_intersections(&b.body), [], "{id}");
                lines.push(format!("{id} {solid} {}", b.hash));
            }
            let d = ryucad::loft::described(&p, id, l, &r).unwrap();
            lines.push(format!("{id} {}", ryucad::doc::canonical::to_string(&d)));
        }
        answers.push(lines);
    }
    assert_eq!(answers[0], answers[1], "one thread and four");
    for l in &answers[0] {
        s.str(l);
    }
    renders(
        &mut s,
        dir.path(),
        session,
        vec![json!({"part": "pt", "view": "iso"})],
    );
}

/// Surface sweeps and planar faces (ryucad 18.A): an extruded arc, a
/// revolved dome, an open line swept along a bend, two chains lofted, a
/// square's planar face; every body bit for bit on one thread and four,
/// each a sheet checking clean; then renders of them from in front and
/// behind (a sheet's back tinted) and cut by a section through them
/// (never capped).
fn surfaces_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("surfaces");
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut session = ryucad::session::Session::open(dir.path()).unwrap();
    call(&mut session, "part.create", json!({"part": "su"}));
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("su");
        call(s, "feature.add", f);
    };
    let sketch =
        |s: &mut ryucad::session::Session, id: &str, plane: &str, ops: serde_json::Value| {
            add(s, json!({"type": "sketch", "id": id, "plane": plane}));
            call(
                s,
                "sketch.edit",
                json!({"part": "su", "sketch": id, "ops": ops}),
            );
        };
    sketch(
        &mut session,
        "arc",
        "xy",
        json!([
            {"op": "add", "id": "o", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "q", "type": "point", "at": [0, 10]},
            {"op": "add", "id": "a1", "type": "arc", "center": "o", "start": "p", "end": "q"}
        ]),
    );
    add(
        &mut session,
        json!({"type": "extrude", "id": "wall", "sketch": "arc", "surface": true,
               "extent": {"type": "blind", "length": "20 mm"}}),
    );
    sketch(
        &mut session,
        "dome",
        "zx",
        json!([
            {"op": "add", "id": "o", "type": "point", "at": [0, -25]},
            {"op": "add", "id": "p", "type": "point", "at": [0, -17]},
            {"op": "add", "id": "q", "type": "point", "at": [8, -25]},
            {"op": "add", "id": "a1", "type": "arc", "center": "o", "start": "q", "end": "p"},
            {"op": "add", "id": "ax", "type": "line", "start": "o", "end": "q", "construction": true}
        ]),
    );
    add(
        &mut session,
        json!({"type": "revolve", "id": "cap", "sketch": "dome", "axis": "ax",
               "angle": "full", "surface": true}),
    );
    sketch(
        &mut session,
        "sq",
        "xy",
        json!([
            {"op": "add", "id": "p1", "type": "point", "at": [20, 0]},
            {"op": "add", "id": "p2", "type": "point", "at": [30, 0]},
            {"op": "add", "id": "p3", "type": "point", "at": [30, 10]},
            {"op": "add", "id": "p4", "type": "point", "at": [20, 10]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "id": "l4", "type": "line", "start": "p4", "end": "p1"}
        ]),
    );
    add(
        &mut session,
        json!({"type": "planar", "id": "pad", "sketch": "sq"}),
    );
    // An open line swept up and round a bend.
    sketch(
        &mut session,
        "band",
        "xy",
        json!([
            {"op": "add", "id": "a", "type": "point", "at": [38, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [42, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}
        ]),
    );
    sketch(
        &mut session,
        "bend",
        "zx",
        json!([
            {"op": "add", "id": "a", "type": "point", "at": [0, 40]},
            {"op": "add", "id": "b", "type": "point", "at": [10, 40]},
            {"op": "add", "id": "c", "type": "point", "at": [10, 55]},
            {"op": "add", "id": "e", "type": "point", "at": [25, 55]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"},
            {"op": "add", "id": "a1", "type": "arc", "center": "c", "start": "b", "end": "e"}
        ]),
    );
    add(
        &mut session,
        json!({"type": "path", "id": "pb", "segments": ["su/bend/l1", "su/bend/a1"]}),
    );
    add(
        &mut session,
        json!({"type": "sweep", "id": "ribbon", "sketch": "band", "path": "su/pb",
               "surface": true}),
    );
    // Two chains lofted: a line on xy and one 10 mm above, drawn back.
    sketch(
        &mut session,
        "la",
        "xy",
        json!([
            {"op": "add", "id": "a", "type": "point", "at": [-20, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [-10, 0]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}
        ]),
    );
    add(
        &mut session,
        json!({"type": "plane", "id": "up", "by": "offset", "from": "xy", "distance": "10 mm"}),
    );
    sketch(
        &mut session,
        "lb",
        "su/up",
        json!([
            {"op": "add", "id": "a", "type": "point", "at": [-8, 0]},
            {"op": "add", "id": "b", "type": "point", "at": [-20, 2]},
            {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}
        ]),
    );
    add(
        &mut session,
        json!({"type": "loft", "id": "skin", "sections": ["la", "lb"], "surface": true}),
    );
    let part = Id::parse("su", "part").unwrap();
    let p = session.project().part(&part).unwrap().clone();
    let mut answers = Vec::new();
    for threads in [1, 4] {
        let r = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| rebuild_part(&p));
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let mut lines = Vec::new();
        for (id, o) in &r.features {
            let o = o.as_ref().unwrap();
            for (solid, b) in o.solids() {
                assert_eq!(b.body.kind(), ryukei::topo::BodyKind::Sheet, "{id}");
                assert_eq!(ryukei::check::check_intersections(&b.body), [], "{id}");
                lines.push(format!("{id} {solid} {}", b.hash));
            }
        }
        lines.push(r.hash.clone());
        answers.push(lines);
    }
    assert_eq!(answers[0], answers[1], "one thread and four");
    for l in &answers[0] {
        s.str(l);
    }
    renders(
        &mut s,
        dir.path(),
        session,
        vec![
            json!({"part": "su", "view": "iso"}),
            json!({"part": "su", "view": "back_bottom_left"}),
            json!({"part": "su", "view": "iso", "section": [{"plane": "zx", "offset": 5}]}),
        ],
    );
}

/// A protocol call a section's builder makes.
type Call<'a> = dyn FnMut(&str, serde_json::Value) -> serde_json::Value + 'a;

/// A part `name`'s features, built through the protocol by `build` in a
/// project of its own, rebuilt on one thread and on four: each body's
/// dump, kind and boundaries (checked clean), and the rebuild report's
/// `fill` and `consumes` of each feature that has one (plan 18.B).
fn surface_part_section(
    golden: &mut Golden,
    name: &'static str,
    part: &str,
    build: fn(&mut Call<'_>),
) {
    use serde_json::json;
    let mut s = golden.section(name);
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut session = ryucad::session::Session::open(dir.path()).unwrap();
    let mut call = |m: &str, p: serde_json::Value| {
        ryucad::server::handle(
            &mut session,
            ryucad::proto::Request::from_rpc(m, Some(p)).unwrap(),
        )
        .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    call("part.create", json!({"part": part}));
    build(&mut call);
    let report = call("rebuild", json!({"part": part}));
    let p = session
        .project()
        .part(&Id::parse(part, "part").unwrap())
        .unwrap()
        .clone();
    let on = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| rebuild_part(&p))
    };
    let (one, four) = (on(1), on(4));
    assert_eq!(one.failed(), 0, "{:?}", one.features);
    assert_eq!(one.hash, four.hash, "one thread and four");
    for (id, o) in &one.features {
        let o = o.as_ref().unwrap();
        for (solid, b) in o.solids() {
            assert_eq!(ryukei::check::check_intersections(&b.body), [], "{id}");
            s.str(&format!(
                "{id} {solid} {} {} faces {} boundaries {}",
                b.hash,
                ryucad::kind::name(&b.body),
                b.body.faces().count(),
                ryucad::kind::boundaries(&b.body)
            ));
            s.str(&b.body.dump());
        }
    }
    for f in report["parts"][0]["features"].as_array().unwrap() {
        for key in ["fill", "consumes"] {
            if let Some(v) = f.get(key) {
                s.str(&format!(
                    "{} {key} {}",
                    f["id"],
                    ryucad::doc::canonical::to_string(v)
                ));
            }
        }
    }
    s.str(&one.hash);
}

/// The ops drawing a square of side `a` mm from `(x, y)`, lines `l1`..`l4`.
fn square_ops(x: f64, y: f64, a: f64) -> serde_json::Value {
    let p = [[x, y], [x + a, y], [x + a, y + a], [x, y + a]];
    let mut ops = Vec::new();
    for (i, q) in p.iter().enumerate() {
        ops.push(
            serde_json::json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": q}),
        );
    }
    for i in 1..=4 {
        ops.push(
            serde_json::json!({"op": "add", "id": format!("l{i}"), "type": "line",
                        "start": format!("p{i}"), "end": format!("p{}", i % 4 + 1)}),
        );
    }
    serde_json::Value::Array(ops)
}

/// Each face of `feature`'s body in part `part`, as `<part>/<feature>/<name>`.
fn faces_of(call: &mut Call<'_>, part: &str, feature: &str) -> Vec<String> {
    let v = call(
        "inspect",
        serde_json::json!({"part": part, "feature": feature}),
    );
    v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| format!("{part}/{feature}/{}", f["name"].as_str().unwrap()))
        .collect()
}

/// Fills (a pentagon, a quad tangent to a quarter cylinder sheet) and
/// extracts (a box's top, and all its faces as a closed sheet).
fn fills_section(golden: &mut Golden) {
    surface_part_section(golden, "fills", "fi", |call| {
        use serde_json::json;
        call(
            "feature.add",
            json!({"part": "fi", "type": "sketch", "id": "pent", "plane": "xy"}),
        );
        let mut ops = Vec::new();
        for k in 0..5 {
            let (sn, c) = ryukei::math::sin_cos((72.0 * k as f64).to_radians());
            ops.push(
                json!({"op": "add", "id": format!("p{}", k + 1), "type": "point",
                            "at": [10.0 * c, 10.0 * sn]}),
            );
        }
        for k in 0..5 {
            ops.push(json!({"op": "add", "id": format!("l{}", k + 1), "type": "line",
                            "start": format!("p{}", k + 1), "end": format!("p{}", (k + 1) % 5 + 1)}));
        }
        call(
            "sketch.edit",
            json!({"part": "fi", "sketch": "pent", "ops": ops}),
        );
        let sides: Vec<_> = (1..=5)
            .map(|k| json!({"curve": format!("fi/pent/l{k}")}))
            .collect();
        call(
            "feature.add",
            json!({"part": "fi", "type": "fill", "id": "f5", "sides": sides}),
        );
        call(
            "feature.add",
            json!({"part": "fi", "type": "box", "id": "b",
            "params": {"x": 10, "y": 20, "z": 5}, "placement": {"origin": [30, 0, 0]}}),
        );
        let faces = faces_of(call, "fi", "b");
        call(
            "feature.add",
            json!({"part": "fi", "type": "extract", "id": "x",
            "faces": [faces[0].clone()]}),
        );
        call(
            "feature.add",
            json!({"part": "fi", "type": "extract", "id": "xall",
            "faces": faces}),
        );
    });
}

/// A box's faces extracted and sewn back as a solid, and as a closed
/// sheet in a second sew of the same faces' twins.
fn sews_section(golden: &mut Golden) {
    surface_part_section(golden, "sews", "se", |call| {
        use serde_json::json;
        call(
            "feature.add",
            json!({"part": "se", "type": "box", "id": "b",
            "params": {"x": 10, "y": 20, "z": 5}}),
        );
        let faces = faces_of(call, "se", "b");
        for (pre, sew, solid) in [("x", "sw", true), ("y", "sh", false)] {
            let ids: Vec<String> = (0..faces.len()).map(|k| format!("{pre}{k}")).collect();
            for (k, f) in faces.iter().enumerate() {
                call(
                    "feature.add",
                    json!({"part": "se", "type": "extract", "id": ids[k],
                    "faces": [f]}),
                );
            }
            call(
                "feature.add",
                json!({"part": "se", "type": "sew", "id": sew, "sheets": ids,
                "make_solid": solid}),
            );
        }
    });
}

/// A square planar face thickened forward, its twin both ways, and a
/// box's top thickened and joined.
fn thickens_section(golden: &mut Golden) {
    surface_part_section(golden, "thickens", "th", |call| {
        use serde_json::json;
        for (sk, x) in [("a", 0.0), ("b", 20.0)] {
            call(
                "feature.add",
                json!({"part": "th", "type": "sketch", "id": sk, "plane": "xy"}),
            );
            call(
                "sketch.edit",
                json!({"part": "th", "sketch": sk, "ops": square_ops(x, 0.0, 10.0)}),
            );
        }
        call(
            "feature.add",
            json!({"part": "th", "type": "planar", "id": "fa", "sketch": "a"}),
        );
        call(
            "feature.add",
            json!({"part": "th", "type": "planar", "id": "fb", "sketch": "b"}),
        );
        call(
            "feature.add",
            json!({"part": "th", "type": "thicken", "id": "t1", "sheet": "fa",
            "front": "2 mm"}),
        );
        call(
            "feature.add",
            json!({"part": "th", "type": "thicken", "id": "t2", "sheet": "fb",
            "front": "1 mm", "back": "1 mm"}),
        );
        call(
            "feature.add",
            json!({"part": "th", "type": "box", "id": "bx",
            "params": {"x": 10, "y": 10, "z": 10}, "placement": {"origin": [40, 0, 0]}}),
        );
        let top = call("inspect", json!({"part": "th", "feature": "bx"}))["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["surface"]["type"] == "plane"
                    && f["surface"]["frame"]["origin"][2]
                        .as_f64()
                        .is_some_and(|z| z > 0.009)
            })
            .map(|f| format!("th/bx/{}", f["name"].as_str().unwrap()))
            .unwrap();
        call(
            "feature.add",
            json!({"part": "th", "type": "thicken", "id": "tj",
            "from_faces": [top], "front": "3 mm", "operation": "join", "targets": ["bx"]}),
        );
    });
}

/// An assembly of the showcase twice and a plate, exploded: its explode
/// as `explode.list` resolves it, and renders all the way, half way, by a
/// turn, and of a named view that saved it.
fn explodes_section(golden: &mut Golden) {
    let mut s = golden.section("explodes");
    let (dir, mut session) = showcase();
    let call = |session: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(
            session,
            ryucad::proto::Request::from_rpc(m, Some(p)).unwrap(),
        )
        .unwrap()
    };
    call(
        &mut session,
        "part.create",
        serde_json::json!({"part": "plate"}),
    );
    call(
        &mut session,
        "feature.add",
        serde_json::json!({"part": "plate", "type": "box", "id": "body",
                           "params": {"x": 200, "y": 80, "z": 5},
                           "placement": {"origin": [-20, -20, -5]}}),
    );
    call(
        &mut session,
        "part.create",
        serde_json::json!({"part": "rack", "kind": "assembly"}),
    );
    call(
        &mut session,
        "feature.add",
        serde_json::json!({"part": "rack", "type": "instance", "id": "base", "of": "plate"}),
    );
    for (id, x) in [("one", 0), ("two", 100)] {
        call(
            &mut session,
            "feature.add",
            serde_json::json!({"part": "rack", "type": "instance", "id": id, "of": "show",
                               "placement": {"origin": [x, 0, 0]}}),
        );
    }
    let v = call(
        &mut session,
        "explode.save",
        serde_json::json!({"part": "rack", "explode": "apart", "steps": [
            {"instances": ["one", "two"], "translate": [0, 0, 60]},
            {"instances": ["two"], "rotate": {"axis": [0, 1, 0], "angle": "45 deg"},
             "translate": [40, 0, 0]},
            {"instances": ["base"], "translate": [0, 0, -30]}
        ]}),
    );
    s.str(&ryucad::doc::canonical::to_string(&v["resolved"]));
    let views = dir.path().join("parts/rack/views");
    std::fs::create_dir_all(&views).unwrap();
    ryucad::doc::project::write_json(
        &views.join("half.json"),
        &serde_json::json!({"eye_m": [0.4, -0.5, 0.35], "explode": {"fraction": 0.5, "id": "apart"},
                            "format": 2, "fov_deg": 35, "kind": "ryucad.view",
                            "projection": "perspective", "target_m": [0.08, 0.02, 0.03],
                            "up": [0, 0, 1]}),
    )
    .unwrap();
    session.refresh().unwrap();
    renders(
        &mut s,
        dir.path(),
        session,
        vec![
            serde_json::json!({"part": "rack", "explode": "apart"}),
            serde_json::json!({"part": "rack", "explode": "apart", "fraction": 0.5}),
            serde_json::json!({"part": "rack", "explode": "apart", "fraction": 0.6,
                               "view": "front"}),
            serde_json::json!({"part": "rack", "view": "half"}),
        ],
    );
}

/// The showcase committed, then its box widened and its sphere removed:
/// renders compared with the commit, of the commit framed with the
/// working files, and of the working files framed with the commit.
fn compare_section(golden: &mut Golden) {
    let mut s = golden.section("compare");
    let (dir, mut session) = showcase();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_AUTHOR_DATE", "2026-10-21T12:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-10-21T12:00:00Z")
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "showcase"]);
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap()).unwrap()
    };
    call(
        &mut session,
        "feature.edit",
        serde_json::json!({"part": "show", "id": "box", "params": {"x": 18}}),
    );
    call(
        &mut session,
        "feature.delete",
        serde_json::json!({"part": "show", "id": "sphere"}),
    );
    drop(session);
    for params in [
        serde_json::json!({"compare": "HEAD", "width": 320, "height": 240}),
        serde_json::json!({"rev": "HEAD", "frame": ["working"], "width": 320, "height": 240}),
        serde_json::json!({"frame": ["HEAD"], "compare": "HEAD", "view": "top",
                           "width": 320, "height": 240}),
    ] {
        let req = ryucad::proto::Request::from_rpc("render", Some(params)).unwrap();
        let a = ryucad::server::handle_safely(&mut None, None, None, dir.path(), req).unwrap();
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

/// A millimetre part of solids: a plate drilled through all by a
/// sketch's two circles, a boss joined to it, a cross bore through the
/// boss, and a second solid (a block cut by a sphere) united with it.
fn solid() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    call(&mut s, "part.create", json!({"part": "so"}));
    call(
        &mut s,
        "param.set",
        json!({"part": "so", "parameters": {"t": "7.9 mm", "bore": "3.1 mm"}}),
    );
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("so");
        call(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "box", "id": "plate", "params": {"x": "41.3 mm", "y": "28.7 mm", "z": "t"}}),
    );
    add(&mut s, json!({"type": "sketch", "id": "holes"}));
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "so", "sketch": "holes", "ops": [
            {"op": "add", "type": "point", "at": [7.3, 6.1]},
            {"op": "add", "type": "point", "at": [33.9, 21.7]},
            {"op": "add", "type": "fix", "a": "p1"},
            {"op": "add", "type": "fix", "a": "p2"},
            {"op": "add", "type": "circle", "center": "p1", "r": 2.2},
            {"op": "add", "type": "circle", "center": "p2", "r": 2.2},
            {"op": "add", "type": "radius", "a": "ci1", "value": "2.2 mm"},
            {"op": "add", "type": "equal", "a": "ci1", "b": "ci2"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "drill", "sketch": "holes", "operation": "cut",
               "extent": {"type": "through_all"}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "operation": "join",
               "params": {"radius": "6.35 mm", "height": "11.2 mm"},
               "placement": {"origin": ["20.6 mm", "14.3 mm", "t"]}}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "cross", "operation": "cut",
               "params": {"radius": "bore / 2", "height": "30 mm"},
               "placement": {"origin": ["5.6 mm", "14.3 mm", "t + 5.5 mm"],
                             "z": [1, 0, 0], "x": [0, 1, 0]}}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "block", "params": {"x": "9 mm", "y": "9 mm", "z": "t"},
               "placement": {"origin": ["41.3 mm", "9.1 mm", 0]}}),
    );
    add(
        &mut s,
        json!({"type": "sphere", "id": "dimple", "operation": "cut", "targets": ["block"],
               "params": {"radius": "3.3 mm"},
               "placement": {"origin": ["45.8 mm", "13.6 mm", "t"]}}),
    );
    add(
        &mut s,
        json!({"type": "boolean", "id": "whole", "op": "unite", "target": "plate",
               "tools": ["block"]}),
    );
    (dir, s)
}

/// Every state of every solid bit for bit, the part's hash, and the
/// meshes of the solids as they show.
fn solids_section(golden: &mut Golden) {
    let mut s = golden.section("solids");
    let (_dir, mut session) = solid();
    let part = Id::parse("so", "part").unwrap();
    let result = session.rebuilt(&part).unwrap().clone();
    assert_eq!(result.failed(), 0, "{:?}", result.features);
    for (id, o) in &result.features {
        let o = o.as_ref().unwrap();
        s.str(id.as_str());
        for (solid, b) in o.solids() {
            s.str(solid.as_str());
            s.str(&b.body.dump());
        }
        if o.solids().is_empty() {
            s.str(o.hash());
        }
    }
    s.str(&result.hash);
    let meshes =
        ryucad::server::meshes(&mut session, &part, None, ryucad::mesh::Quality::Coarse).unwrap();
    assert_eq!(meshes.len(), 1);
    s.str(&format!("{:x}", md(&ryucad::mesh::encode(&meshes))));
}

#[test]
fn solids_are_sound() {
    let (_dir, mut session) = solid();
    let part = Id::parse("so", "part").unwrap();
    let p = session.project().part(&part).unwrap().clone();
    let result = session.rebuilt(&part).unwrap();
    let solids = ryucad::solid::Solids::of(&p);
    for shown in result.shown(&solids) {
        let (f, b) = shown.state.unwrap();
        assert!(ryukei::check::check(&b.body).is_empty(), "{f}");
        assert_eq!(ryukei::check::check_intersections(&b.body), [], "{f}");
    }
}

/// The edge of an `inspect` body from the vertex at `a` to the one at `b`
/// (meters), as a reference into solid `solid` of part `bl`.
fn edge_between(body: &serde_json::Value, solid: &str, a: [f64; 3], b: [f64; 3]) -> String {
    let vertex = |p: [f64; 3]| {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-9))
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .unwrap_or_else(|| panic!("no vertex at {p:?}"))
    };
    let (a, b) = (vertex(a), vertex(b));
    let e = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .expect("an edge from a to b");
    format!("bl/{solid}/{}", e["name"].as_str().unwrap())
}

/// The name of the vertex of an `inspect` body at `p` (meters).
fn vertex_name(body: &serde_json::Value, p: [f64; 3]) -> String {
    body["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-9))
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| panic!("no vertex at {p:?}"))
}

/// The circle edge of an `inspect` body of radius `r` about a centre at
/// `c` (meters), as a reference into solid `solid` of part `bl`.
fn circle_at(body: &serde_json::Value, solid: &str, c: [f64; 3], r: f64) -> String {
    let e = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            let o = &e["curve"]["frame"]["origin"];
            e["curve"]["type"] == "circle"
                && (e["curve"]["radius"].as_f64().unwrap() - r).abs() < 1e-12
                && (0..3).all(|k| (o[k].as_f64().unwrap() - c[k]).abs() < 1e-12)
        })
        .unwrap_or_else(|| panic!("no circle at {c:?}"));
    format!("bl/{solid}/{}", e["name"].as_str().unwrap())
}

/// A millimetre part of blends: a block's vertical edges rounded, a boss
/// joined to it with its foot filled and its top bevelled at an angle,
/// the top rim bevelled all round from one edge, a pocket whose walls and
/// floor are rounded, and a drilled hole countersunk.
fn blended() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    call(&mut s, "part.create", json!({"part": "bl"}));
    call(
        &mut s,
        "param.set",
        json!({"part": "bl", "parameters": {"corner": "4.1 mm", "foot": "1.3 mm"}}),
    );
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("bl");
        call(s, "feature.add", f);
    };
    let body = |s: &mut ryucad::session::Session, f: &str| {
        call(s, "inspect", json!({"part": "bl", "feature": f}))["bodies"][0].clone()
    };
    let mm = |p: [f64; 3]| p.map(|x| x * 1e-3);
    add(
        &mut s,
        json!({"type": "box", "id": "base", "params": {"x": "40 mm", "y": "30 mm", "z": "10 mm"}}),
    );
    let b = body(&mut s, "base");
    let ups: Vec<String> = [[0.0, 0.0], [40.0, 0.0], [0.0, 30.0], [40.0, 30.0]]
        .iter()
        .map(|[x, y]| edge_between(&b, "base", mm([*x, *y, 0.0]), mm([*x, *y, 10.0])))
        .collect();
    add(
        &mut s,
        json!({"type": "fillet", "id": "corners", "radius": "corner", "edges": ups}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "operation": "join",
               "params": {"radius": "6 mm", "height": "8 mm"},
               "placement": {"origin": ["20 mm", "15 mm", "10 mm"]}}),
    );
    let foot = circle_at(&body(&mut s, "boss"), "base", mm([20.0, 15.0, 10.0]), 6e-3);
    add(
        &mut s,
        json!({"type": "fillet", "id": "foot", "radius": "foot", "edges": [foot]}),
    );
    let b = body(&mut s, "foot");
    let rim = edge_between(&b, "base", mm([4.1, 0.0, 10.0]), mm([35.9, 0.0, 10.0]));
    add(
        &mut s,
        json!({"type": "chamfer", "id": "rim", "d": "0.8 mm", "edges": [rim]}),
    );
    let lid = body(&mut s, "rim")["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["surface"]["type"] == "plane"
                && (f["surface"]["frame"]["origin"][2].as_f64().unwrap() - 0.018).abs() < 1e-12
        })
        .map(|f| format!("bl/base/{}", f["name"].as_str().unwrap()))
        .expect("the boss's top");
    let top = circle_at(&body(&mut s, "rim"), "base", mm([20.0, 15.0, 18.0]), 6e-3);
    add(
        &mut s,
        json!({"type": "chamfer", "id": "cap", "form": "distance_angle", "d": "0.7 mm",
               "angle": "35 deg", "face": lid, "edges": [top]}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "pocket", "operation": "cut",
               "params": {"x": "6 mm", "y": "5 mm", "z": "4 mm"},
               "placement": {"origin": ["6 mm", "3 mm", "6 mm"]}}),
    );
    let p = body(&mut s, "pocket");
    let walls: Vec<String> = [[6.0, 3.0], [12.0, 3.0], [6.0, 8.0], [12.0, 8.0]]
        .iter()
        .map(|[x, y]| edge_between(&p, "base", mm([*x, *y, 6.0]), mm([*x, *y, 10.0])))
        .collect();
    add(
        &mut s,
        json!({"type": "fillet", "id": "walls", "radius": "1 mm", "edges": walls}),
    );
    let w = body(&mut s, "walls");
    let floor = edge_between(&w, "base", mm([7.0, 3.0, 6.0]), mm([11.0, 3.0, 6.0]));
    add(
        &mut s,
        json!({"type": "fillet", "id": "floor", "radius": "0.4 mm", "edges": [floor]}),
    );
    add(&mut s, json!({"type": "sketch", "id": "holes"}));
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "bl", "sketch": "holes", "ops": [
            {"op": "add", "type": "point", "at": [32, 22]},
            {"op": "add", "type": "fix", "a": "p1"},
            {"op": "add", "type": "circle", "center": "p1", "r": 1.5},
            {"op": "add", "type": "radius", "a": "ci1", "value": "1.5 mm"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "extrude", "id": "drill", "sketch": "holes", "operation": "cut",
               "extent": {"type": "through_all"}}),
    );
    let mouth = circle_at(
        &body(&mut s, "drill"),
        "base",
        mm([32.0, 22.0, 10.0]),
        1.5e-3,
    );
    add(
        &mut s,
        json!({"type": "chamfer", "id": "sink", "d": "0.4 mm", "edges": [mouth]}),
    );
    (dir, s)
}

/// Every state of the blended part's solid bit for bit, the part's hash,
/// and its mesh.
fn blends_section(golden: &mut Golden) {
    let mut s = golden.section("blends");
    let (_dir, mut session) = blended();
    let part = Id::parse("bl", "part").unwrap();
    let result = session.rebuilt(&part).unwrap().clone();
    assert_eq!(result.failed(), 0, "{:?}", result.features);
    for (id, o) in &result.features {
        let o = o.as_ref().unwrap();
        s.str(id.as_str());
        for (solid, b) in o.solids() {
            s.str(solid.as_str());
            s.str(&b.body.dump());
        }
        if o.solids().is_empty() {
            s.str(o.hash());
        }
    }
    s.str(&result.hash);
    let meshes =
        ryucad::server::meshes(&mut session, &part, None, ryucad::mesh::Quality::Coarse).unwrap();
    assert_eq!(meshes.len(), 1);
    s.str(&format!("{:x}", md(&ryucad::mesh::encode(&meshes))));
}

#[test]
fn blended_solids_are_sound() {
    let (_dir, mut session) = blended();
    let part = Id::parse("bl", "part").unwrap();
    let result = session.rebuilt(&part).unwrap().clone();
    assert_eq!(result.failed(), 0, "{:?}", result.features);
    // Every state is a sound body; the one shown meets itself nowhere
    // (`ryucad check` runs the intersection check, with the mesh).
    for (f, o) in &result.features {
        for (_, b) in o.as_ref().unwrap().solids() {
            assert!(ryukei::check::check(&b.body).is_empty(), "{f}");
        }
    }
    let v = ryucad::server::handle(
        &mut session,
        ryucad::proto::Request::from_rpc("check", Some(serde_json::json!({"part": "bl"}))).unwrap(),
    );
    assert!(v.is_ok(), "{v:?}");
}

/// Part `sh`: a tray (a box `tray` with its uprights rounded, shelled open
/// at its top with a thicker floor) and a cylinder `cup` shelled outwards,
/// closed. Part `of`: a box `g` grown, a box `q` whose pocket's floor is
/// raised, a cylinder `r` grown.
fn bodied() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let add = |s: &mut ryucad::session::Session, part: &str, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!(part);
        call(s, "feature.add", f);
    };
    let body = |s: &mut ryucad::session::Session, part: &str, f: &str| {
        call(s, "inspect", json!({"part": part, "feature": f}))["bodies"][0].clone()
    };
    // The planar face of `b` (of `solid` in `part`) facing up at `z` meters.
    let top = |b: &serde_json::Value, part: &str, solid: &str, z: f64| {
        b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
                f["surface"]["type"] == "plane"
                    && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == 1.0
                    && (f["surface"]["frame"]["origin"][2].as_f64().unwrap() - z).abs() < 1e-12
            })
            .map(|f| format!("{part}/{solid}/{}", f["name"].as_str().unwrap()))
            .expect("a face facing up")
    };
    let mm = |p: [f64; 3]| p.map(|x| x * 1e-3);
    call(&mut s, "part.create", json!({"part": "sh"}));
    add(
        &mut s,
        "sh",
        json!({"type": "box", "id": "tray", "params": {"x": "40 mm", "y": "30 mm", "z": "20 mm"}}),
    );
    let b = body(&mut s, "sh", "tray");
    let ups: Vec<String> = [[0.0, 0.0], [40.0, 0.0], [0.0, 30.0], [40.0, 30.0]]
        .iter()
        .map(|[x, y]| {
            edge_between(&b, "tray", mm([*x, *y, 0.0]), mm([*x, *y, 20.0])).replace("bl/", "sh/")
        })
        .collect();
    add(
        &mut s,
        "sh",
        json!({"type": "fillet", "id": "round", "radius": "5 mm", "edges": ups}),
    );
    let rim = top(&body(&mut s, "sh", "round"), "sh", "tray", 0.02);
    let floor = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == -1.0
        })
        .map(|f| format!("sh/tray/{}", f["name"].as_str().unwrap()))
        .unwrap();
    add(
        &mut s,
        "sh",
        json!({"type": "shell", "id": "hollow", "target": "tray", "thickness": "2 mm",
               "open": [rim], "faces": [{"thickness": "4 mm", "faces": [floor]}]}),
    );
    add(
        &mut s,
        "sh",
        json!({"type": "cylinder", "id": "cup", "params": {"radius": "10 mm", "height": "20 mm"},
               "placement": {"origin": ["60 mm", "0 mm", "0 mm"]}}),
    );
    add(
        &mut s,
        "sh",
        json!({"type": "shell", "id": "skin", "target": "cup", "thickness": "1.5 mm",
               "direction": "outward"}),
    );
    call(&mut s, "part.create", json!({"part": "of"}));
    add(
        &mut s,
        "of",
        json!({"type": "box", "id": "g", "params": {"x": "20 mm", "y": "10 mm", "z": "5 mm"}}),
    );
    add(
        &mut s,
        "of",
        json!({"type": "offset", "id": "grow", "target": "g", "distance": "1 mm"}),
    );
    add(
        &mut s,
        "of",
        json!({"type": "box", "id": "q", "params": {"x": "40 mm", "y": "30 mm", "z": "20 mm"},
               "placement": {"origin": ["40 mm", "0 mm", "0 mm"]}}),
    );
    add(
        &mut s,
        "of",
        json!({"type": "box", "id": "k", "operation": "cut", "targets": ["q"],
               "params": {"x": "20 mm", "y": "10 mm", "z": "30 mm"},
               "placement": {"origin": ["50 mm", "10 mm", "15 mm"]}}),
    );
    let pocket = top(&body(&mut s, "of", "k"), "of", "q", 0.015);
    add(
        &mut s,
        "of",
        json!({"type": "offset", "id": "raise", "target": "q", "distance": "0 mm",
               "faces": [{"distance": "3 mm", "faces": [pocket]}]}),
    );
    add(
        &mut s,
        "of",
        json!({"type": "cylinder", "id": "r", "params": {"radius": "5 mm", "height": "10 mm"},
               "placement": {"origin": ["100 mm", "0 mm", "0 mm"]}}),
    );
    add(
        &mut s,
        "of",
        json!({"type": "offset", "id": "fat", "target": "r", "distance": "0.5 mm"}),
    );
    // Part `mi`: half a bracket mirrored and joined about yz, a copy of it
    // about a datum 60 mm along x as a solid of its own, and a peg's
    // mirror image cut from a plate.
    call(&mut s, "part.create", json!({"part": "mi"}));
    add(
        &mut s,
        "mi",
        json!({"type": "box", "id": "half",
        "params": {"x": "20 mm", "y": "10 mm", "z": "5 mm"}}),
    );
    add(
        &mut s,
        "mi",
        json!({"type": "box", "id": "up", "operation": "join", "targets": ["half"],
        "params": {"x": "5 mm", "y": "10 mm", "z": "15 mm"},
        "placement": {"origin": ["0 mm", "0 mm", "5 mm"]}}),
    );
    add(
        &mut s,
        "mi",
        json!({"type": "mirror", "id": "m", "solid": "half", "plane": "yz"}),
    );
    add(
        &mut s,
        "mi",
        json!({"type": "plane", "id": "far", "by": "offset", "from": "yz",
        "distance": "60 mm"}),
    );
    add(
        &mut s,
        "mi",
        json!({"type": "mirror", "id": "c", "solid": "half", "plane": "mi/far",
        "operation": "new"}),
    );
    add(
        &mut s,
        "mi",
        json!({"type": "box", "id": "plate",
        "params": {"x": "40 mm", "y": "20 mm", "z": "4 mm"},
        "placement": {"origin": ["-20 mm", "30 mm", "0 mm"]}}),
    );
    add(
        &mut s,
        "mi",
        json!({"type": "cylinder", "id": "peg",
        "params": {"radius": "2 mm", "height": "10 mm"},
        "placement": {"origin": ["10 mm", "40 mm", "-3 mm"]}}),
    );
    add(
        &mut s,
        "mi",
        json!({"type": "mirror", "id": "hole", "solid": "peg", "plane": "yz",
        "operation": "cut", "targets": ["plate"]}),
    );
    // Part `sp`: a housing split at a datum into two solids, a U split into
    // a solid of two lumps, and a trim.
    call(&mut s, "part.create", json!({"part": "sp"}));
    add(
        &mut s,
        "sp",
        json!({"type": "box", "id": "h",
        "params": {"x": "40 mm", "y": "30 mm", "z": "20 mm"}}),
    );
    add(
        &mut s,
        "sp",
        json!({"type": "plane", "id": "parting", "by": "offset", "from": "xy",
        "distance": "8 mm"}),
    );
    add(
        &mut s,
        "sp",
        json!({"type": "split", "id": "s", "target": "h", "plane": "sp/parting"}),
    );
    add(
        &mut s,
        "sp",
        json!({"type": "box", "id": "u",
        "params": {"x": "40 mm", "y": "30 mm", "z": "20 mm"},
        "placement": {"origin": ["60 mm", "0 mm", "0 mm"]}}),
    );
    add(
        &mut s,
        "sp",
        json!({"type": "box", "id": "slot", "operation": "cut", "targets": ["u"],
        "params": {"x": "20 mm", "y": "40 mm", "z": "20 mm"},
        "placement": {"origin": ["70 mm", "-5 mm", "5 mm"]}}),
    );
    add(
        &mut s,
        "sp",
        json!({"type": "split", "id": "prongs", "target": "u", "plane": "sp/parting",
        "flip": true}),
    );
    add(
        &mut s,
        "sp",
        json!({"type": "plane", "id": "end", "by": "offset", "from": "yz",
        "distance": "30 mm"}),
    );
    add(
        &mut s,
        "sp",
        json!({"type": "split", "id": "trim", "target": "s", "plane": "sp/end",
        "keep": "behind"}),
    );
    // Part `sm`: a plate with rectangular reliefs by default, a tray of four
    // flanges set back 5 mm, one corner 8 mm; and a Z bent section with a
    // deduction, one flange on its top edge.
    call(&mut s, "part.create", json!({"part": "sm"}));
    add(
        &mut s,
        "sm",
        json!({"type": "sketch", "id": "s", "plane": "xy"}),
    );
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
        json!({"part": "sm", "sketch": "s", "ops": ops}),
    );
    add(
        &mut s,
        "sm",
        json!({"type": "base_flange", "id": "plate", "sketch": "s",
        "thickness": "1.5 mm", "radius": "1 mm", "allowance": {"k_factor": "0.44"},
        "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}),
    );
    let b = body(&mut s, "sm", "plate");
    let z = 1.5;
    let c = [
        [0.0, 0.0, z],
        [60.0, 0.0, z],
        [60.0, 40.0, z],
        [0.0, 40.0, z],
    ];
    let rims: Vec<String> = (0..4)
        .map(|i| edge_between(&b, "plate", mm(c[i]), mm(c[(i + 1) % 4])).replace("bl/", "sm/"))
        .collect();
    let far = format!("sm/plate/{}", vertex_name(&b, mm(c[2])));
    add(
        &mut s,
        "sm",
        json!({"type": "flange", "id": "tray", "edges": rims, "angle": "90 deg",
        "length": "15 mm", "setback": "5 mm", "ends": [{"vertex": far, "setback": "8 mm"}]}),
    );
    add(
        &mut s,
        "sm",
        json!({"type": "sketch", "id": "zs", "plane": "xy"}),
    );
    let mut ops = Vec::new();
    for (i, at) in [[100, 0], [120, 0], [120, 25], [145, 25]]
        .into_iter()
        .enumerate()
    {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    for i in 1..=3 {
        ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                        "start": format!("p{i}"), "end": format!("p{}", i + 1)}));
    }
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "sm", "sketch": "zs", "ops": ops}),
    );
    add(
        &mut s,
        "sm",
        json!({"type": "base_flange", "id": "z", "sketch": "zs",
        "thickness": "1 mm", "radius": "2 mm", "depth": "30 mm", "allowance": {"deduction": "2.4 mm"}}),
    );
    // Face tweaks (plan 17.A): a boss slid along its plate, and a lid's
    // top made flush with a datum plane.
    call(&mut s, "part.create", json!({"part": "tw"}));
    add(
        &mut s,
        "tw",
        json!({"type": "box", "id": "plate",
        "params": {"x": "40 mm", "y": "30 mm", "z": "10 mm"}}),
    );
    add(
        &mut s,
        "tw",
        json!({"type": "box", "id": "boss", "operation": "join", "targets": ["plate"],
        "params": {"x": "10 mm", "y": "10 mm", "z": "5 mm"},
        "placement": {"origin": ["5 mm", "5 mm", "10 mm"]}}),
    );
    let j = body(&mut s, "tw", "boss");
    // The planar face of `b` whose outward normal along `axis` is `n`, at
    // `at` meters along it.
    let at = |b: &serde_json::Value, axis: usize, n: f64, at: f64| {
        b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
                f["surface"]["type"] == "plane"
                    && f["surface"]["frame"]["z"][axis].as_f64().unwrap() * sign == n
                    && (f["surface"]["frame"]["origin"][axis].as_f64().unwrap() - at).abs() < 1e-12
            })
            .map(|f| format!("tw/plate/{}", f["name"].as_str().unwrap()))
            .expect("the boss's face")
    };
    let boss = vec![
        at(&j, 0, -1.0, 0.005),
        at(&j, 0, 1.0, 0.015),
        at(&j, 1, -1.0, 0.005),
        at(&j, 1, 1.0, 0.015),
        at(&j, 2, 1.0, 0.015),
    ];
    add(
        &mut s,
        "tw",
        json!({"type": "move_face", "id": "slide", "by": "translate",
        "direction": "x", "distance": "4 mm", "faces": boss}),
    );
    add(
        &mut s,
        "tw",
        json!({"type": "box", "id": "lid", "params": {"x": "20 mm", "y": "20 mm", "z": "8 mm"},
        "placement": {"origin": ["60 mm", "0 mm", "0 mm"]}}),
    );
    let lid_top = top(&body(&mut s, "tw", "lid"), "tw", "lid", 0.008);
    add(
        &mut s,
        "tw",
        json!({"type": "plane", "id": "level", "by": "offset", "from": "xy",
        "distance": "6 mm"}),
    );
    add(
        &mut s,
        "tw",
        json!({"type": "replace_face", "id": "flush",
        "groups": [{"with": "tw/level", "faces": [lid_top]}]}),
    );
    (dir, s)
}

/// Every state of every solid of `part` bit for bit, built on one thread
/// and on four alike, each passing the self-intersection check, and the
/// part's hash.
fn bodied_section(golden: &mut Golden, name: &'static str, part: &str) {
    let mut s = golden.section(name);
    let (_dir, session) = bodied();
    let p = session
        .project()
        .part(&Id::parse(part, "part").unwrap())
        .unwrap()
        .clone();
    let on = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| ryucad::rebuild::rebuild_part(&p))
    };
    let (one, four) = (on(1), on(4));
    assert_eq!(one.failed(), 0, "{:?}", one.features);
    assert_eq!(one.hash, four.hash);
    for (id, o) in &one.features {
        let o = o.as_ref().unwrap();
        s.str(id.as_str());
        for (solid, b) in o.solids() {
            s.str(solid.as_str());
            s.str(&b.body.dump());
            assert_eq!(ryukei::check::check_intersections(&b.body), [], "{id}");
        }
        if o.solids().is_empty() {
            s.str(o.hash());
        }
    }
    s.str(&one.hash);
}

fn shells_section(golden: &mut Golden) {
    bodied_section(golden, "shells", "sh");
}

fn offsets_section(golden: &mut Golden) {
    bodied_section(golden, "offsets", "of");
}

fn tweaks_section(golden: &mut Golden) {
    bodied_section(golden, "tweaks", "tw");
}

fn mirrors_section(golden: &mut Golden) {
    bodied_section(golden, "mirrors", "mi");
}

fn splits_section(golden: &mut Golden) {
    bodied_section(golden, "splits", "sp");
}

fn sheet_metal_section(golden: &mut Golden) {
    bodied_section(golden, "sheet_metal", "sm");
}

/// The sheet metal part's `sheet.get` answers for both solids, canonical.
fn sheet_metal_report_section(golden: &mut Golden) {
    let mut s = golden.section("sheet_metal_report");
    let (_dir, mut session) = bodied();
    for solid in ["plate", "z"] {
        let v = ryucad::server::handle(
            &mut session,
            ryucad::proto::Request::from_rpc(
                "sheet.get",
                Some(serde_json::json!({"part": "sm", "solid": solid})),
            )
            .unwrap(),
        )
        .unwrap();
        s.str(&ryucad::doc::canonical::to_string(&v));
    }
}

/// Part `sf`: a tray of four flanges set back 5 mm on a 60 × 40 mm plate,
/// unfolded (`u`), a slot `c` cut across its right bend line in the flat
/// and refolded (`rf`); and a Z bent section `z`, unfolded (`uz`) and
/// left flat.
fn flattened() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("sf");
        call(s, "feature.add", f);
    };
    let points = |s: &mut ryucad::session::Session, sketch: &str, at: &[[i64; 2]], closed: bool| {
        let mut ops = Vec::new();
        for (i, p) in at.iter().enumerate() {
            ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": p}));
            ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
        }
        let lines = if closed { at.len() } else { at.len() - 1 };
        for i in 1..=lines {
            ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                            "start": format!("p{i}"), "end": format!("p{}", i % at.len() + 1)}));
        }
        call(
            s,
            "sketch.edit",
            json!({"part": "sf", "sketch": sketch, "ops": ops}),
        );
    };
    call(&mut s, "part.create", json!({"part": "sf"}));
    add(&mut s, json!({"type": "sketch", "id": "s", "plane": "xy"}));
    points(&mut s, "s", &[[0, 0], [60, 0], [60, 40], [0, 40]], true);
    add(
        &mut s,
        json!({"type": "base_flange", "id": "plate", "sketch": "s",
        "thickness": "1.5 mm", "radius": "1 mm", "allowance": {"k_factor": "0.44"},
        "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}),
    );
    let b = call(&mut s, "inspect", json!({"part": "sf", "feature": "plate"}))["bodies"][0].clone();
    let mm = |p: [f64; 3]| p.map(|x| x / 1000.0);
    let c = [
        [0.0, 0.0, 1.5],
        [60.0, 0.0, 1.5],
        [60.0, 40.0, 1.5],
        [0.0, 40.0, 1.5],
    ];
    let rims: Vec<String> = (0..4)
        .map(|i| edge_between(&b, "plate", mm(c[i]), mm(c[(i + 1) % 4])).replace("bl/", "sf/"))
        .collect();
    add(
        &mut s,
        json!({"type": "flange", "id": "tray", "edges": rims, "angle": "90 deg",
        "length": "15 mm", "setback": "5 mm"}),
    );
    // Held still about the plate: the flange all four bends meet.
    let sheet = call(&mut s, "sheet.get", json!({"part": "sf", "solid": "plate"}));
    let mut meets = std::collections::BTreeMap::<String, usize>::new();
    for b in sheet["bends"].as_array().unwrap() {
        for f in b["flanges"].as_array().unwrap() {
            *meets.entry(f.as_str().unwrap().to_owned()).or_default() += 1;
        }
    }
    let base = meets.iter().max_by_key(|(_, n)| **n).unwrap().0.clone();
    let base = sheet["flanges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["flange"] == base)
        .unwrap();
    let fixed = base["sides"][0][0].as_str().unwrap().to_owned();
    add(
        &mut s,
        json!({"type": "unfold", "id": "u", "target": "plate", "fixed_face": fixed}),
    );
    // Across the right bend line (57.5 mm in), short of the far one.
    add(
        &mut s,
        json!({"type": "box", "id": "c", "operation": "cut", "targets": ["plate"],
        "params": {"x": "3 mm", "y": "4 mm", "z": "5 mm"},
        "placement": {"origin": ["56 mm", "18 mm", "-2 mm"]}}),
    );
    add(
        &mut s,
        json!({"type": "refold", "id": "rf", "target": "plate", "unfold": "u"}),
    );
    add(&mut s, json!({"type": "sketch", "id": "zs", "plane": "xy"}));
    points(
        &mut s,
        "zs",
        &[[100, 0], [120, 0], [120, 25], [145, 25]],
        false,
    );
    add(
        &mut s,
        json!({"type": "base_flange", "id": "z", "sketch": "zs",
        "thickness": "1 mm", "radius": "2 mm", "depth": "30 mm",
        "allowance": {"deduction": "2.4 mm"}}),
    );
    add(&mut s, json!({"type": "unfold", "id": "uz", "target": "z"}));
    (dir, s)
}

/// [`flattened`]'s every feature's bodies (as `bodied_section` gives
/// them), `sheet.get` and `flat.get` of both solids, each one's DXF and
/// a 320 × 240 `render --flat` of the tray, on one thread and four
/// alike.
fn sheet_metal_flat_section(golden: &mut Golden) {
    let mut s = golden.section("sheet_metal_flat");
    let (dir, session) = flattened();
    let p = session
        .project()
        .part(&Id::parse("sf", "part").unwrap())
        .unwrap()
        .clone();
    let on = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| ryucad::rebuild::rebuild_part(&p))
    };
    let (one, four) = (on(1), on(4));
    assert_eq!(one.failed(), 0, "{:?}", one.features);
    assert_eq!(one.hash, four.hash);
    for (id, o) in &one.features {
        let o = o.as_ref().unwrap();
        s.str(id.as_str());
        for (solid, b) in o.solids() {
            s.str(solid.as_str());
            s.str(&b.body.dump());
            assert_eq!(ryukei::check::check_intersections(&b.body), [], "{id}");
        }
        if o.solids().is_empty() {
            s.str(o.hash());
        }
    }
    s.str(&one.hash);
    let mut session = Some(session);
    let mut answer = |threads: usize, m: &str, params: serde_json::Value| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let req = ryucad::proto::Request::from_rpc(m, Some(params)).unwrap();
        let a = pool
            .install(|| ryucad::server::answer_live(session.as_mut().unwrap(), None, None, req))
            .unwrap_or_else(|e| panic!("{m}: {e}"));
        // A fresh session for the next question, so nothing is cached.
        session = Some(ryucad::session::Session::open(dir.path()).unwrap());
        a
    };
    for solid in ["plate", "z"] {
        for m in ["sheet.get", "flat.get"] {
            let params = serde_json::json!({"part": "sf", "solid": solid});
            let (a, b) = (answer(1, m, params.clone()), answer(4, m, params));
            assert_eq!(a.result, b.result, "{m} {solid}: one thread and four");
            s.str(&ryucad::doc::canonical::to_string(&a.result));
        }
        let params = serde_json::json!({"part": "sf", "solid": solid, "format": "dxf"});
        let (a, b) = (
            answer(1, "export", params.clone()),
            answer(4, "export", params),
        );
        let (a, b) = (a.binary.unwrap(), b.binary.unwrap());
        assert_eq!(a, b, "DXF of {solid}: one thread and four");
        s.str(&format!("{:x}", md(&a)));
    }
    let params = serde_json::json!({"part": "sf", "flat": "plate", "width": 320, "height": 240});
    let png = answer(1, "render", params).binary.unwrap();
    let mut r = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    s.str(&format!("{:x}", md(&buf[..info.buffer_size()])));
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
    assert!(result.warnings(&part).is_empty());
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
        let p = session.project().part(&part).unwrap().clone();
        let r = ryucad::diff::Built::of(&p, session.rebuilt(&part).unwrap());
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
    let mut v = ryucad::diff::structural(&before, &after, &opts, &Default::default());
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
            .map(|(p, v)| ((*p).to_owned(), text(v).into()))
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
    working.insert(
        "parts/p/features/box-2.json".into(),
        text(&m.to_json()).into(),
    );
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
    let text = |v: &serde_json::Value| -> ryucad::activity::Bytes {
        ryucad::doc::canonical::to_string(v).into_bytes().into()
    };
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
                combine: ryucad::doc::solid::Combine::NEW,
            });
            let id = format!("f{i}");
            files.insert(format!("parts/p/features/{id}.json"), text(&f.to_json()));
            order.push(Id::parse(&id, "feature").unwrap());
        }
        files.insert(
            "parts/p/part.json".into(),
            text(&PartFile::new(order).to_json()),
        );
        if g.next_u64().is_multiple_of(2) {
            let p = json!({"format": 1, "kind": "ryucad.parameters",
                           "parameters": {"w": format!("{} mm", decimal(&mut g))}});
            files.insert("parts/p/parameters.json".into(), text(&p));
        }
        let part = Id::parse("p", "part").unwrap();
        s.str(&ryupdm::content::hash(&files, &part).unwrap());
    }
    // An attestation's canonical bytes and its signature under the test
    // key: Ed25519 is deterministic, so the tag message is the same
    // everywhere; and the verdicts on an approved, an unapproved, a
    // forged and an unknown key's message.
    {
        use ryupdm::attest::*;
        let mut g = Lcg::new(0x6174_7465);
        let hex40 = |g: &mut Lcg| {
            format!(
                "{:016x}{:016x}{:08x}",
                g.next_u64(),
                g.next_u64(),
                g.next_u64() as u32
            )
        };
        let rules = vec![ryupdm::approvals::Rule {
            parts: vec!["*".into()],
            count: 1,
            from: vec!["alice".into(), "bob".into()],
        }];
        let commit = hex40(&mut g);
        let a = Attestation {
            approvals: vec![Approval {
                at: "2026-11-02T09:14:03Z".into(),
                commit: commit.clone(),
                github: User {
                    id: 1234567,
                    login: "alice".into(),
                },
                review: 2154874321,
            }],
            commit: commit.clone(),
            label: "B".into(),
            missing: vec![],
            outcome: Outcome::Approved,
            part: Id::parse("bracket", "part").unwrap(),
            project: ryucad::doc::id::ProjectId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            proposed_by: User {
                id: 7654321,
                login: "dstonely".into(),
            },
            pull: Some(Pull {
                number: 42,
                repo: "acme/widgets".into(),
            }),
            record: format!("{}{}", hex40(&mut g), &hex40(&mut g)[..24]),
            record_kind: ryupdm::record::Kind::Release,
            rules: rules.clone(),
            rules_at: hex40(&mut g),
            signed_at: "2026-11-02T09:20:11Z".into(),
        };
        s.str(&a.canonical());
        let signed = sign(&a, &TEST_SEED);
        s.str(&signed);
        let keys = std::collections::BTreeMap::from([(test_key_id(), public_key(&TEST_SEED))]);
        let expect = Expect {
            project: &a.project,
            part: &a.part,
            label: &a.label,
            kind: a.record_kind,
            commit: &a.commit,
            record: &a.record,
            rules: &rules,
            rules_at: &a.rules_at,
            keys: &keys,
        };
        let mut u = a.clone();
        u.outcome = Outcome::Unapproved;
        u.approvals.clear();
        u.missing = vec![ryupdm::approvals::Unmet {
            rule: rules[0].clone(),
            needed: 1,
        }];
        for m in [
            signed.clone(),
            sign(&u, &TEST_SEED),
            signed.replacen("\"alice\"", "\"bob\"", 1),
            sign(&a, &[8; 32]),
        ] {
            let v = verify_message(m.as_bytes(), &expect);
            let clause = match &v {
                Verdict::Invalid(c) => c.name(),
                _ => "",
            };
            s.str(&format!("{} {clause}", v.status()));
        }
    }
}

/// BOMs of seeded projects of parts and assemblies (indented and flat, as
/// JSON and CSV) and where-used of each part: the same bytes everywhere,
/// since the service compares what any platform wrote.
fn boms_section(golden: &mut Golden) {
    use ryucad::doc::assembly::Rev;
    use ryucad::doc::project::Project;
    use ryupdm::bom;
    use ryupdm::structure::{Cache, Reader};
    let mut s = golden.section("boms");
    let text = |v: &serde_json::Value| ryucad::doc::canonical::to_string(v);
    let mut g = Lcg::new(0x626f_6d30);
    for _ in 0..64 {
        let bytes: Vec<u8> = (0..64).map(|_| g.next_u64() as u8).collect();
        let st = ryucad_doc::doc::fixtures::structure(&bytes);
        let project = Project::load_from(std::path::Path::new(""), &st.files).unwrap();
        let config = ryupdm::config::Config::read(&st.files).unwrap();
        let mut cache = Cache::new();
        let mut r = Reader::new(&st.files, &project, None, None, &mut cache);
        let states = std::collections::BTreeMap::new();
        for part in project.parts.keys() {
            let row = bom::tree(&mut r, part, &Rev::Working, None).unwrap();
            let rows = bom::flat(&row, &config);
            s.str(&text(&bom::row_json(&row, &states)));
            s.str(&bom::csv_tree(&row, &config));
            s.str(&bom::csv_flat(&rows, &config));
            let q = ryupdm::where_used::Query {
                part: part.clone(),
                project: None,
                rev: None,
                direct: false,
            };
            s.str(&text(&ryupdm::where_used::where_used(&mut r, &q, &config)));
        }
    }
}

/// A project of parts and assemblies: a pair of bolts, placed twice in a
/// frame, one turned, with a plate.
fn assembled() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    call(&mut s, "part.create", json!({"part": "bolt"}));
    call(
        &mut s,
        "feature.add",
        json!({"part": "bolt", "type": "cylinder", "id": "shank",
               "params": {"radius": "3.1 mm", "height": "17.3 mm"}}),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "bolt", "type": "cylinder", "id": "head", "operation": "join",
               "params": {"radius": "5.2 mm", "height": "4.1 mm"},
               "placement": {"origin": [0, 0, "17.3 mm"]}}),
    );
    call(&mut s, "part.create", json!({"part": "plate"}));
    call(
        &mut s,
        "feature.add",
        json!({"part": "plate", "type": "box", "id": "slab",
               "params": {"x": "61.7 mm", "y": "33.1 mm", "z": "6.3 mm"}}),
    );
    for (a, _) in [("pair", 0), ("frame", 1)] {
        call(
            &mut s,
            "part.create",
            json!({"part": a, "kind": "assembly"}),
        );
    }
    for (id, x) in [("left", "-7.9 mm"), ("right", "7.9 mm")] {
        call(
            &mut s,
            "feature.add",
            json!({"part": "pair", "type": "instance", "id": id, "of": "bolt",
                   "placement": {"origin": [x, 0, 0]}}),
        );
    }
    call(
        &mut s,
        "feature.add",
        json!({"part": "frame", "type": "instance", "id": "base", "of": "plate", "fixed": true}),
    );
    for (id, x) in [("p1", "15.4 mm"), ("p2", "46.3 mm")] {
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": id, "of": "pair",
                   "placement": {"origin": [x, "16.55 mm", "6.3 mm"]}}),
        );
    }
    call(
        &mut s,
        "assembly.move",
        json!({"part": "frame", "instance": "p2", "axis": [0, 0, 1], "angle": "37.3 deg"}),
    );
    (dir, s)
}

/// Every occurrence of the frame: its path, part, solids' bodies and the
/// bits of its pose; the model's and each part's rebuild hashes; and the
/// pixels of a render of the frame.
fn assemblies_section(golden: &mut Golden) {
    use ryucad::doc::assembly::Rev;
    let mut s = golden.section("assemblies");
    let (dir, mut session) = assembled();
    let frame = Id::parse("frame", "part").unwrap();
    let model = session.model(&frame, &Rev::Working).unwrap();
    assert_eq!(model.occurrences.len(), 5);
    for o in &model.occurrences {
        s.str(&ryucad::assembly::path_text(&o.path));
        s.str(o.part.as_str());
        for sd in &o.solids {
            s.str(sd.founder.as_str());
            s.str(&sd.built.hash);
        }
        let p = ryucad::assembly::rigid_pose(&o.pose);
        for x in p.origin_m.iter().chain(&p.z).chain(&p.x) {
            s.str(&format!("{:016x}", x.to_bits()));
        }
    }
    s.str(&model.hash);
    for part in ["bolt", "plate", "pair", "frame"] {
        let id = Id::parse(part, "part").unwrap();
        s.str(&session.rebuilt(&id).unwrap().hash);
    }
    let params = serde_json::json!({"part": "frame", "width": 320, "height": 240});
    let req = ryucad::proto::Request::from_rpc("render", Some(params)).unwrap();
    let a = ryucad::server::handle_safely(&mut Some(session), None, None, dir.path(), req).unwrap();
    let png = a.binary.unwrap();
    let mut r = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    s.str(&format!("{:x}", md(&buf[..info.buffer_size()])));
}

/// Measurement over the assembled frame: the mass properties of a bolt,
/// the plate and the whole frame in steel and brass, the frame's
/// clearances, and the distance and angle between two occurrences, each
/// answer whole on one thread and on four.
fn measure_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("measure");
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let (_dir, mut session) = assembled();
    call(
        &mut session,
        "materials.set",
        json!({"name": "steel", "density": "7850 kg/m^3", "default": true}),
    );
    call(
        &mut session,
        "materials.set",
        json!({"name": "brass", "density": "8.5 g/cm^3"}),
    );
    std::fs::write(
        session.root().join("parts/bolt/properties.json"),
        ryucad::doc::canonical::to_string(
            &json!({"format": 1, "kind": "ryupdm.properties", "material": "brass"}),
        ),
    )
    .unwrap();
    let mut answers = Vec::new();
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let mut fresh = ryucad::session::Session::open(session.root()).unwrap();
        let got = pool.install(|| {
            let mut out = Vec::new();
            for of in ["bolt", "plate", "frame", "frame/p2"] {
                out.push(call(&mut fresh, "mass", json!({"of": of})));
            }
            out.push(call(
                &mut fresh,
                "clearance",
                json!({"part": "frame", "below": "50 mm"}),
            ));
            out.push(call(
                &mut fresh,
                "measure",
                json!({"entities": ["frame/p1/left", "frame/p2/right"]}),
            ));
            out.push(call(
                &mut fresh,
                "measure",
                json!({"entities": ["bolt/shank"]}),
            ));
            out
        });
        answers.push(got);
    }
    assert_eq!(answers[0], answers[1], "one thread and four");
    assert!(answers[0][2]["mass"]["kg"].as_f64().unwrap() > 0.0);
    for v in &answers[0] {
        let mut v = v.clone();
        v.as_object_mut().unwrap().remove("revision");
        s.str(&ryucad::doc::canonical::to_string(&v));
    }
}

/// [`assembled`]'s frame, with one more bolt sunk into the plate and
/// leaning into the first pair, and the interference report on one
/// thread and on four: every pair's paths, the bits of its volume and
/// box, and the faces it touches; the pairs that touch or can't be
/// checked.
fn interference_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("interference");
    let (_dir, mut session) = assembled();
    let req = ryucad::proto::Request::from_rpc(
        "feature.add",
        Some(
            json!({"part": "frame", "type": "instance", "id": "sunk", "of": "bolt",
                    "placement": {"origin": ["11.3 mm", "13.7 mm", "2.9 mm"],
                                  "z": [0.21, -0.13, 1], "x": [1, 0, 0]}}),
        ),
    )
    .unwrap();
    ryucad::server::handle(&mut session, req).unwrap();
    let mut reports = Vec::new();
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let mut fresh = ryucad::session::Session::open(session.root()).unwrap();
        let v = pool.install(|| ryucad::interference::run(&mut fresh, "frame", &[]).unwrap());
        reports.push(v);
    }
    assert_eq!(reports[0], reports[1], "one thread and four");
    let v = &reports[0];
    let found = v["interferences"].as_array().unwrap();
    assert!(found.len() >= 2, "{v}");
    for i in found {
        s.str(i["a"].as_str().unwrap());
        s.str(i["b"].as_str().unwrap());
        s.str(&format!(
            "{:016x}",
            i["volume_m3"].as_f64().unwrap().to_bits()
        ));
        for k in ["min_m", "max_m"] {
            for x in i["box"][k].as_array().unwrap() {
                s.str(&format!("{:016x}", x.as_f64().unwrap().to_bits()));
            }
        }
        for side in ["a", "b"] {
            for f in i["faces"][side].as_array().unwrap() {
                s.str(f.as_str().unwrap());
            }
        }
    }
    for k in ["touching", "unchecked"] {
        s.str(&v[k].to_string());
    }
    s.str(&v["pairs"].to_string());
}

/// [`assembled`]'s frame (a pair of bolts placed twice, one turned),
/// [`blended`]'s part and [`swept`]'s, each exported as AP242 in
/// millimetres and AP214 in meters, on one thread and on four: the files'
/// digests.
fn exports_section(golden: &mut Golden) {
    type Fixture = fn() -> (tempfile::TempDir, ryucad::session::Session);
    let mut s = golden.section("exports");
    let fixtures: [(Fixture, &str); 3] = [(assembled, "frame"), (blended, "bl"), (swept, "sw")];
    for (make, part) in fixtures {
        let (_dir, mut session) = make();
        for (schema, unit) in [("ap242", "mm"), ("ap214", "m")] {
            let mut files = Vec::new();
            for threads in [1, 4] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                let params = serde_json::json!({"part": part, "schema": schema, "unit": unit,
                                                "time_stamp": "2026-10-27T00:00:00",
                                                "file_name": format!("{part}.step")});
                let req = ryucad::proto::Request::from_rpc("export", Some(params)).unwrap();
                let a = pool
                    .install(|| ryucad::server::answer_live(&mut session, None, None, req))
                    .unwrap_or_else(|e| panic!("{part} {schema}: {e}"));
                files.push(a.binary.unwrap());
            }
            assert_eq!(files[0], files[1], "{part} {schema}: one thread and four");
            s.str(&format!(
                "{:x}",
                md(ryukei_testkit::unversioned(std::str::from_utf8(&files[0]).unwrap()).as_bytes())
            ));
        }
    }
}

/// The frame of [`assembled`] (a plate and a pair of bolts placed twice,
/// the pair a subassembly) with its first pair repeated by an instance
/// pattern.
fn patterned() -> (tempfile::TempDir, ryucad::session::Session) {
    let (dir, mut s) = assembled();
    let req = ryucad::proto::Request::from_rpc(
        "feature.add",
        Some(
            serde_json::json!({"part": "frame", "type": "instance_pattern", "id": "row",
                                "form": "linear", "instances": ["p1"], "direction": "y",
                                "spacing": "41.3 mm", "count": 2}),
        ),
    )
    .unwrap();
    ryucad::server::handle(&mut s, req).unwrap();
    (dir, s)
}

/// A project of one part imported from a file OCCT wrote: a spline
/// revolved, which USD writes as a NURBS.
fn imported() -> (tempfile::TempDir, ryucad::session::Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ryustep/tests/data/occt/spline_revolve.stp");
    let req = ryucad::proto::Request::from_rpc(
        "import",
        Some(serde_json::json!({"solid": true, "path": path})),
    )
    .unwrap();
    ryucad::server::handle(&mut s, req).unwrap();
    (dir, s)
}

/// Every format but STEP (whose bytes `exports` pins) of an assembly with
/// a subassembly and an instance pattern, a blended part, a swept part
/// and an imported one: binary and ASCII STL, 3MF, usda, usdc and usdz,
/// each project built and exported on one thread and on four. The
/// version the files name is fixed (`export_stamped`), so a release's
/// stamp moves nothing; the chord is coarse so a debug build is quick.
fn formats_section(golden: &mut Golden) {
    type Fixture = fn() -> (tempfile::TempDir, ryucad::session::Session);
    let mut s = golden.section("formats");
    let fixtures: [(Fixture, &str); 4] = [
        (patterned, "frame"),
        (blended, "bl"),
        (swept, "sw"),
        // The import names its one part.
        (imported, ""),
    ];
    let asks: [(&str, bool); 6] = [
        ("stl", false),
        ("stl", true),
        ("3mf", false),
        ("usda", false),
        ("usdc", false),
        ("usdz", false),
    ];
    for (make, part) in fixtures {
        let mut runs = Vec::new();
        for threads in [1, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let (_dir, mut session) = pool.install(make);
            let part = match part {
                "" => session.project().parts.keys().next().unwrap().to_string(),
                p => p.to_owned(),
            };
            let mut files = Vec::new();
            for (format, ascii) in asks {
                let ask = ryucad::export::Ask {
                    part: part.clone(),
                    format: Some(format.into()),
                    time_stamp: Some("2026-10-30T00:00:00".into()),
                    chord: Some("0.05 mm".into()),
                    angle: Some("20 deg".into()),
                    ascii,
                    ..Default::default()
                };
                let meshes_only = ryucad::export::Ask {
                    brep: Some(false),
                    ..ask.clone()
                };
                match pool.install(|| ryucad::export::export_stamped(&mut session, ask, "0.0.0")) {
                    Ok(a) => files.push(a.binary.unwrap()),
                    // A face a BrepArray can't describe (none now: the
                    // blended part's rolled corner and the swept arch until
                    // #247, OCCT's spline revolution until #470): the
                    // refusal as it stands, and the stage with meshes alone.
                    Err(e) if e.code == ryucad::error::Code::ExportUnrepresentable => {
                        files.push(format!("{e}").into_bytes());
                        let a = pool
                            .install(|| {
                                ryucad::export::export_stamped(&mut session, meshes_only, "0.0.0")
                            })
                            .unwrap_or_else(|e| panic!("{part} {format} meshes: {e}"));
                        files.push(a.binary.unwrap());
                    }
                    Err(e) => panic!("{part} {format}: {e}"),
                }
            }
            runs.push(files);
        }
        assert!(runs[0] == runs[1], "{part:?}: one thread and four");
        for f in &runs[0] {
            s.str(&format!("{:x}", md(f)));
        }
    }
}

/// Files OCCT wrote (ryustep's `tests/data/occt`), each imported into one
/// project: every import feature's body, names and all, and where the
/// assembly file's assembly places its parts, built on one thread and on
/// four.
fn imports_section(golden: &mut Golden) {
    use ryucad::doc::assembly::Rev;
    let mut s = golden.section("imports");
    let files = [
        "box_ap214.stp",
        "cylinder.stp",
        "torus.stp",
        "plate_hole.stp",
        "nurbs_cylinder.stp",
        "spline_revolve.stp",
        "assembly.stp",
    ];
    let mut runs = Vec::new();
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        ryucad::server::init(dir.path(), None).unwrap();
        let mut session = ryucad::session::Session::open(dir.path()).unwrap();
        let mut out = Vec::new();
        for f in files {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../ryustep/tests/data/occt")
                .join(f);
            let params = serde_json::json!({"solid": true, "path": path});
            let req = ryucad::proto::Request::from_rpc("import", Some(params)).unwrap();
            pool.install(|| ryucad::server::handle(&mut session, req))
                .unwrap_or_else(|e| panic!("{f}: {e}"));
        }
        let parts: Vec<Id> = session.project().parts.keys().cloned().collect();
        for part in &parts {
            let r = pool.install(|| session.rebuilt(part).unwrap().clone());
            assert_eq!(r.failed(), 0, "{part}: {:?}", r.features);
            out.push(part.to_string());
            for (id, o) in &r.features {
                out.push(id.to_string());
                for (solid, b) in o.as_ref().unwrap().solids() {
                    out.push(solid.to_string());
                    out.push(b.body.dump());
                }
            }
            let model = session.model(part, &Rev::Working).unwrap();
            for o in &model.occurrences {
                out.push(ryucad::assembly::path_text(&o.path));
                let p = ryucad::assembly::rigid_pose(&o.pose);
                for x in p.origin_m.iter().chain(&p.z).chain(&p.x) {
                    out.push(format!("{:016x}", x.to_bits()));
                }
            }
        }
        runs.push(out);
    }
    assert_eq!(runs[0], runs[1], "one thread and four");
    for x in &runs[0] {
        s.str(x);
    }
}

/// Files OCCT wrote and the committed corpus's smallest NIST body, each
/// imported as a reference (design 364): every reference's body hash,
/// counts, its report's counts, its faces' names; its STEP export (the
/// file's own representation passed through) and binary STL; `mass` of
/// the closed ones and `measure` of each. Built on one thread and on four.
/// The larger NIST bodies are left out: a debug build references them in
/// seconds each.
fn reference_imports_section(golden: &mut Golden) {
    let mut s = golden.section("reference_imports");
    let files = [
        "../ryustep/tests/data/occt/box_ap214.stp",
        "../ryustep/tests/data/occt/cylinder.stp",
        "../ryustep/tests/data/occt/torus.stp",
        "../ryustep/tests/data/occt/plate_hole.stp",
        "../ryustep/tests/data/occt/nurbs_cylinder.stp",
        "../ryustep/tests/data/occt/spline_revolve.stp",
        "../ryustep/tests/corpus/nist_ctc_01_asme1_rd.stp",
    ];
    let mut runs = Vec::new();
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        ryucad::server::init(dir.path(), None).unwrap();
        let mut session = ryucad::session::Session::open(dir.path()).unwrap();
        let call = |session: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
            let req = ryucad::proto::Request::from_rpc(m, Some(p)).unwrap();
            pool.install(|| ryucad::server::handle(session, req))
        };
        call(
            &mut session,
            "materials.set",
            serde_json::json!({"name": "steel", "density": "7850 kg/m^3", "default": true}),
        )
        .unwrap();
        let mut out = Vec::new();
        for f in files {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(f);
            call(&mut session, "import", serde_json::json!({"path": path}))
                .unwrap_or_else(|e| panic!("{f}: {e}"));
        }
        let parts: Vec<Id> = session.project().parts.keys().cloned().collect();
        for part in &parts {
            let r = pool.install(|| session.rebuilt(part).unwrap().clone());
            assert_eq!(r.failed(), 0, "{part}: {:?}", r.features);
            out.push(part.to_string());
            for (id, o) in &r.features {
                let o = o.as_ref().unwrap();
                out.push(id.to_string());
                if let ryucad::rebuild::Output::Solids(m) = o
                    && let Some(rep) = &m.report
                {
                    out.push(rep["reference"].to_string());
                }
                for (solid, b) in o.solids() {
                    assert!(b.reference, "{part}/{solid}");
                    out.push(solid.to_string());
                    out.push(b.hash.clone());
                    let c = b.body.counts();
                    out.push(format!("{} {} {}", c.faces, c.edges, c.vertices));
                    let mut names: Vec<String> = b
                        .body
                        .faces()
                        .map(|f| b.body.face(f).unwrap().name.to_string())
                        .collect();
                    names.sort();
                    out.push(format!("{:x}", md(names.join(",").as_bytes())));
                    let of = format!("{part}/{solid}");
                    let v = call(
                        &mut session,
                        "measure",
                        serde_json::json!({"entities": [of]}),
                    )
                    .unwrap();
                    out.push(v["area"].to_string());
                    match call(&mut session, "mass", serde_json::json!({"of": of})) {
                        Ok(v) => out.push(format!("{} {}", v["volume"], v["mass"])),
                        Err(e) => out.push(format!("{:?} {}", e.code, e.details["reason"])),
                    }
                }
            }
            for format in ["step", "stl"] {
                let ask = ryucad::export::Ask {
                    part: part.to_string(),
                    format: Some(format.into()),
                    time_stamp: Some("2026-11-06T00:00:00".into()),
                    chord: (format == "stl").then(|| "0.05 mm".into()),
                    angle: (format == "stl").then(|| "20 deg".into()),
                    ..Default::default()
                };
                let a = pool
                    .install(|| ryucad::export::export_stamped(&mut session, ask, "0.0.0"))
                    .unwrap_or_else(|e| panic!("{part} {format}: {e}"));
                out.push(format!("{:x}", md(&a.binary.unwrap())));
            }
        }
        runs.push(out);
    }
    assert_eq!(runs[0], runs[1], "one thread and four");
    for x in &runs[0] {
        s.str(x);
    }
}

/// The planar face of `part`'s solid `solid` whose outward normal is `n`.
fn face_along(s: &mut ryucad::session::Session, part: &str, solid: &str, n: [f64; 3]) -> String {
    let req = ryucad::proto::Request::from_rpc(
        "inspect",
        Some(serde_json::json!({"part": part, "feature": solid})),
    )
    .unwrap();
    let v = ryucad::server::handle(s, req).unwrap();
    v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let z = &f["surface"]["frame"]["z"];
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && (0..3).all(|k| (z[k].as_f64().unwrap() * sign - n[k]).abs() < 1e-12)
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .expect("a face along n")
}

/// An assembly of mates, built through the protocol, each write keeping
/// it current: a plate fixed, a bolt on a revolute through its top driven
/// to 25°, three copies of it round the plate's z, a cap fastened up the
/// second copy, and a block on a planar mate on the plate's top.
fn mated() -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    for (part, ty, id, params) in [
        (
            "plate",
            "box",
            "slab",
            json!({"x": "93.1 mm", "y": "51.7 mm", "z": "6.3 mm"}),
        ),
        (
            "bolt",
            "cylinder",
            "shank",
            json!({"radius": "3.1 mm", "height": "17.3 mm"}),
        ),
        (
            "block",
            "box",
            "cube",
            json!({"x": "9.7 mm", "y": "11.3 mm", "z": "7.1 mm"}),
        ),
    ] {
        call(&mut s, "part.create", json!({"part": part}));
        call(
            &mut s,
            "feature.add",
            json!({"part": part, "type": ty, "id": id, "params": params}),
        );
    }
    call(
        &mut s,
        "part.create",
        json!({"part": "frame", "kind": "assembly"}),
    );
    for (id, of, at, fixed) in [
        ("base", "plate", ["0 mm", "0 mm", "0 mm"], true),
        ("pin", "bolt", ["53.3 mm", "-7.1 mm", "11.9 mm"], false),
        ("cap", "bolt", ["0 mm", "0 mm", "71.3 mm"], false),
        ("slide", "block", ["-11.3 mm", "4.4 mm", "23.1 mm"], false),
    ] {
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": id, "of": of, "fixed": fixed,
                   "placement": {"origin": at}}),
        );
    }
    let top = face_along(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
    let foot = face_along(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
    let under = face_along(&mut s, "block", "cube", [0.0, 0.0, -1.0]);
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("frame");
        call(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "mate", "id": "m1", "form": "revolute", "angle": "25 deg",
               "a": format!("frame/base/slab/{top}"),
               "a_offset": {"origin": ["21.7 mm", "13.1 mm", "0 mm"]},
               "b": format!("frame/pin/shank/{foot}"), "b_offset": {"flip": true}}),
    );
    add(
        &mut s,
        json!({"type": "instance_pattern", "id": "ring", "instances": ["pin"], "axis": "z",
               "count": 4, "angle": "360 deg"}),
    );
    add(
        &mut s,
        json!({"type": "mate", "id": "m2", "form": "fastened", "a": "frame/ring:2/pin",
               "a_offset": {"origin": ["0 mm", "0 mm", "23.9 mm"], "rotate": "15 deg"},
               "b": "frame/cap"}),
    );
    add(
        &mut s,
        json!({"type": "mate", "id": "m3", "form": "planar",
               "a": format!("frame/base/slab/{top}"), "b": format!("frame/slide/cube/{under}"),
               "b_offset": {"flip": true}}),
    );
    (dir, s)
}

/// Every occurrence of the mated assembly with its pose's bits, every
/// instance file as stored (solver state), the solve's report, and the
/// rebuild's hash.
fn mates_section(golden: &mut Golden) {
    use ryucad::doc::assembly::Rev;
    let mut s = golden.section("mates");
    let (dir, mut session) = mated();
    let frame = Id::parse("frame", "part").unwrap();
    let r = session.rebuilt(&frame).unwrap().clone();
    assert_eq!(r.failed(), 0, "{:?}", r.features);
    let solve = r.solve.as_ref().unwrap();
    assert!(solve.report.converged && !solve.stale());
    s.str(&solve.to_json().to_string());
    let model = session.model(&frame, &Rev::Working).unwrap();
    assert_eq!(model.occurrences.len(), 7);
    for o in &model.occurrences {
        s.str(&ryucad::assembly::path_text(&o.path));
        let p = ryucad::assembly::rigid_pose(&o.pose);
        for x in p.origin_m.iter().chain(&p.z).chain(&p.x) {
            s.str(&format!("{:016x}", x.to_bits()));
        }
    }
    for f in ["pin", "cap", "slide"] {
        let path = dir.path().join(format!("parts/frame/features/{f}.json"));
        s.str(&std::fs::read_to_string(path).unwrap());
    }
    s.str(&model.hash);
    s.str(&r.hash);
}

/// A millimetre part of holes and patterns: a plate drilled at a
/// sketch's points (a counterbore with a drill point, a countersink
/// through all, a plain blind hole), a grid of the
/// counterbore with an instance skipped, a boss with a bolt circle of
/// holes about its side, an instance skipped, and a pocket mirrored.
/// `holes_only` stops after the holes.
fn drilled(holes_only: bool) -> (tempfile::TempDir, ryucad::session::Session) {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = ryucad::session::Session::open(dir.path()).unwrap();
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    call(&mut s, "part.create", json!({"part": "hp"}));
    call(
        &mut s,
        "param.set",
        json!({"part": "hp", "parameters": {"bore": "3.3 mm", "n": "6"}}),
    );
    let add = |s: &mut ryucad::session::Session, f: serde_json::Value| {
        let mut f = f;
        f["part"] = json!("hp");
        call(s, "feature.add", f);
    };
    add(
        &mut s,
        json!({"type": "box", "id": "plate", "params": {"x": "60 mm", "y": "40 mm", "z": "10 mm"},
               "placement": {"origin": ["-30 mm", "-20 mm", "0 mm"]}}),
    );
    let top =
        call(&mut s, "inspect", json!({"part": "hp", "feature": "plate"}))["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
                f["surface"]["type"] == "plane"
                    && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == 1.0
            })
            .map(|f| format!("hp/plate/{}", f["name"].as_str().unwrap()))
            .unwrap();
    add(
        &mut s,
        json!({"type": "sketch", "id": "spots", "face": top}),
    );
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "hp", "sketch": "spots", "ops": [
            {"op": "add", "type": "point", "at": [-24, -14]},
            {"op": "add", "type": "fix", "a": "p1"},
            {"op": "add", "type": "point", "at": [24, -14]},
            {"op": "add", "type": "fix", "a": "p2"},
            {"op": "add", "type": "point", "at": [24, 14]},
            {"op": "add", "type": "fix", "a": "p3"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "cb", "sketch": "spots", "points": ["p1"],
               "form": "counterbore", "diameter": "bore",
               "counterbore": {"diameter": "6 mm", "depth": "3 mm"},
               "depth": {"blind": "7 mm"}, "bottom": {"drill": "118 deg"}}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "cs", "sketch": "spots", "points": ["p2"],
               "form": "countersink", "diameter": "4 mm",
               "countersink": {"diameter": "8 mm", "angle": "90 deg"},
               "depth": "through_all"}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "up", "sketch": "spots", "points": ["p3"],
               "diameter": "2.5 mm", "depth": {"blind": "4 mm"}}),
    );
    if holes_only {
        return (dir, s);
    }
    add(
        &mut s,
        json!({"type": "pattern", "id": "grid", "features": ["cb"], "direction": "x",
               "spacing": "8 mm", "count": 3, "direction2": "y", "spacing2": "8 mm",
               "count2": 2, "skip": [[2, 1]]}),
    );
    add(
        &mut s,
        json!({"type": "cylinder", "id": "boss", "operation": "join", "targets": ["plate"],
               "params": {"radius": "5 mm", "height": "4 mm"},
               "placement": {"origin": ["10 mm", "2 mm", "10 mm"]}}),
    );
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "hp", "sketch": "spots", "ops": [
            {"op": "add", "type": "point", "at": [19, 2]},
            {"op": "add", "type": "fix", "a": "p4"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "bolt", "sketch": "spots", "points": ["p4"],
               "diameter": "2 mm", "depth": {"blind": "5 mm"}}),
    );
    let side =
        call(&mut s, "inspect", json!({"part": "hp", "feature": "bolt"}))["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["surface"]["type"] == "cylinder"
                    && (f["surface"]["radius"].as_f64().unwrap() - 0.005).abs() < 1e-12
            })
            .map(|f| format!("hp/plate/{}", f["name"].as_str().unwrap()))
            .unwrap();
    add(
        &mut s,
        json!({"type": "pattern", "id": "circle", "features": ["bolt"], "axis": side,
               "count": "n", "angle": "360 deg", "skip": [3]}),
    );
    add(
        &mut s,
        json!({"type": "box", "id": "pocket", "operation": "cut", "targets": ["plate"],
               "params": {"x": "6 mm", "y": "8 mm", "z": "3 mm"},
               "placement": {"origin": ["-2 mm", "-16 mm", "7 mm"]}}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "mirror", "features": ["pocket"], "plane": "zx"}),
    );
    (dir, s)
}

/// Every state of a drilled part's solid bit for bit, the part's hash,
/// and its mesh.
fn drilled_section(golden: &mut Golden, name: &str, holes_only: bool) {
    let mut s = golden.section(name);
    let (_dir, mut session) = drilled(holes_only);
    let part = Id::parse("hp", "part").unwrap();
    let result = session.rebuilt(&part).unwrap().clone();
    assert_eq!(result.failed(), 0, "{:?}", result.features);
    for (id, o) in &result.features {
        let o = o.as_ref().unwrap();
        s.str(id.as_str());
        for (solid, b) in o.solids() {
            s.str(solid.as_str());
            s.str(&b.body.dump());
        }
        if o.solids().is_empty() {
            s.str(o.hash());
        }
    }
    s.str(&result.hash);
    let meshes =
        ryucad::server::meshes(&mut session, &part, None, ryucad::mesh::Quality::Coarse).unwrap();
    assert_eq!(meshes.len(), 1);
    s.str(&format!("{:x}", md(&ryucad::mesh::encode(&meshes))));
}

#[test]
fn drilled_solids_are_sound() {
    let (_dir, mut session) = drilled(false);
    let part = Id::parse("hp", "part").unwrap();
    let result = session.rebuilt(&part).unwrap().clone();
    assert_eq!(result.failed(), 0, "{:?}", result.features);
    for (f, o) in &result.features {
        for (_, b) in o.as_ref().unwrap().solids() {
            assert!(ryukei::check::check(&b.body).is_empty(), "{f}");
        }
    }
    let v = ryucad::server::handle(
        &mut session,
        ryucad::proto::Request::from_rpc("check", Some(serde_json::json!({"part": "hp"}))).unwrap(),
    );
    assert!(v.is_ok(), "{v:?}");
}

/// The library and project [`deps_section`] and [`deps_structure_section`]
/// read: a library repository (its bolt released as A, then thicker as B)
/// and a project placing A twice and B once, fetched and locked. Both
/// projects have fixed IDs, so what names them hashes alike.
fn deps_projects() -> (tempfile::TempDir, tempfile::TempDir) {
    use serde_json::json;
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_AUTHOR_DATE", "2026-10-29T12:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-10-29T12:00:00Z")
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let call = |s: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
        ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let fixed = |dir: &std::path::Path, id: u8| {
        let m = json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project",
                       "project": ryucad::doc::id::ProjectId::from_bytes([id; 16])});
        ryucad::doc::project::write_json(&dir.join("ryucad.json"), &m).unwrap();
    };
    let lib = tempfile::tempdir().unwrap();
    git(lib.path(), &["init", "-q", "-b", "main"]);
    ryucad::server::init(lib.path(), None).unwrap();
    fixed(lib.path(), 1);
    let mut l = ryucad::session::Session::open(lib.path()).unwrap();
    call(&mut l, "part.create", json!({"part": "bolt"}));
    call(
        &mut l,
        "feature.add",
        json!({"part": "bolt", "type": "cylinder", "id": "shank",
                                        "params": {"radius": 3, "height": 20}}),
    );
    call(
        &mut l,
        "props.set",
        json!({"part": "bolt", "properties": {"number": "B-6"}}),
    );
    git(lib.path(), &["add", "."]);
    git(lib.path(), &["commit", "-q", "-m", "bolt"]);
    call(&mut l, "release.propose", json!({"part": "bolt"}));
    git(lib.path(), &["add", "."]);
    git(lib.path(), &["commit", "-q", "-m", "bolt A"]);
    call(
        &mut l,
        "feature.edit",
        json!({"part": "bolt", "id": "shank", "params": {"radius": 4}}),
    );
    call(&mut l, "release.propose", json!({"part": "bolt"}));
    git(lib.path(), &["add", "."]);
    git(lib.path(), &["commit", "-q", "-m", "bolt B"]);
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    fixed(dir.path(), 2);
    let mut session = ryucad::session::Session::open(dir.path()).unwrap();
    session.set_deps_cache(ryupdm::deps::Cache::new(dir.path().join("cache")));
    let url = format!("file://{}", lib.path().display());
    call(
        &mut session,
        "deps.add",
        json!({"url": url, "name": "fasteners"}),
    );
    call(
        &mut session,
        "part.create",
        json!({"part": "frame", "kind": "assembly"}),
    );
    for (id, rev, x) in [
        ("a1", "A", "0 mm"),
        ("a2", "A", "20 mm"),
        ("b1", "B", "40 mm"),
    ] {
        call(
            &mut session,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": id,
            "of": "fasteners:bolt", "rev": rev, "placement": {"origin": [x, "0 mm", "0 mm"]}}),
        );
    }
    (lib, dir)
}

/// An assembly placing another project's releases: a library repository
/// (its bolt released as A, then thicker as B) and a project placing A
/// twice and B once, fetched and locked. Each occurrence's bodies, and the
/// assembly exported as STEP, on one thread and four. Commits aren't
/// hashed: a record names the build that made it.
fn deps_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("deps");
    let (_lib, dir) = deps_projects();
    let mut runs = Vec::new();
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let frame = ryucad::doc::id::Id::parse("frame", "part").unwrap();
        let mut fresh = ryucad::session::Session::open(dir.path()).unwrap();
        fresh.set_deps_cache(ryupdm::deps::Cache::new(dir.path().join("cache")));
        let (bodies, step) = pool.install(|| {
            let m = fresh
                .model(&frame, &ryucad::doc::assembly::Rev::Working)
                .unwrap();
            let bodies: Vec<String> = m
                .occurrences
                .iter()
                .flat_map(|o| o.solids.iter().map(|x| x.built.hash.clone()))
                .collect();
            let params = json!({"part": "frame", "schema": "ap242", "unit": "mm",
                                "time_stamp": "2026-10-29T00:00:00", "file_name": "frame.step"});
            let req = ryucad::proto::Request::from_rpc("export", Some(params)).unwrap();
            let a = ryucad::server::answer_live(&mut fresh, None, None, req).unwrap();
            (bodies, a.binary.unwrap())
        });
        runs.push((bodies, step));
    }
    assert_eq!(runs[0], runs[1], "one thread and four");
    for b in &runs[0].0 {
        s.str(b);
    }
    let step = std::str::from_utf8(&runs[0].1).unwrap();
    s.str(&format!(
        "{:x}",
        md(ryukei_testkit::unversioned(step).as_bytes())
    ));
}

/// [`deps_projects`]' frame through ryupdm: its indented and flat BOMs as
/// JSON and CSV, where the library's bolt is used, and what the project
/// places from other projects (`deps uses`), on one thread and four.
/// Commits are left out: a record names the build that made it.
fn deps_structure_section(golden: &mut Golden) {
    use serde_json::{Value, json};
    let mut s = golden.section("deps_structure");
    let (_lib, dir) = deps_projects();
    fn uncommitted(v: &mut Value) {
        match v {
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if k == "commit" {
                        *x = json!("-");
                    } else {
                        uncommitted(x);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(uncommitted),
            _ => {}
        }
    }
    let mut runs = Vec::new();
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let mut fresh = ryucad::session::Session::open(dir.path()).unwrap();
        fresh.set_deps_cache(ryupdm::deps::Cache::new(dir.path().join("cache")));
        let answers: Vec<Value> = pool.install(|| {
            [
                ("bom.get", json!({"part": "frame"})),
                ("bom.get", json!({"part": "frame", "flat": true})),
                ("bom.get", json!({"part": "frame", "format": "csv"})),
                (
                    "bom.get",
                    json!({"part": "frame", "flat": true, "format": "csv"}),
                ),
                (
                    "bom.where_used",
                    json!({"part": "fasteners:bolt", "no_dependents": true}),
                ),
                ("deps.uses", json!({})),
            ]
            .into_iter()
            .map(|(m, p)| {
                let req = ryucad::proto::Request::from_rpc(m, Some(p)).unwrap();
                let mut v =
                    ryucad::server::handle(&mut fresh, req).unwrap_or_else(|e| panic!("{m}: {e}"));
                uncommitted(&mut v);
                v
            })
            .collect()
        });
        runs.push(answers);
    }
    assert_eq!(runs[0], runs[1], "one thread and four");
    for v in &runs[0] {
        s.str(&ryucad::doc::canonical::to_string(v));
    }
}

/// Proposals over the showcase, stacked: each one's captured writes, and
/// each `annotation:` revision's rebuild, on one thread and on four.
fn annotations_section(golden: &mut Golden) {
    let mut s = golden.section("annotations");
    let run = || {
        let (_dir, mut session) = showcase();
        let call = |session: &mut ryucad::session::Session, m: &str, p: serde_json::Value| {
            ryucad::server::handle(
                session,
                ryucad::proto::Request::from_rpc(m, Some(p)).unwrap(),
            )
            .unwrap()
        };
        let mut out = Vec::new();
        for (method, params) in [
            (
                "feature.edit",
                serde_json::json!({"part": "show", "id": "box", "params": {"y": 24}}),
            ),
            (
                "feature.edit",
                serde_json::json!({"part": "show", "id": "box", "params": {"z": 35}}),
            ),
            (
                "feature.add",
                serde_json::json!({"part": "show", "type": "cylinder", "id": "peg",
                                                "params": {"radius": 2, "height": 8},
                                                "placement": {"origin": [5, 10, 30]}}),
            ),
            (
                "param.set",
                serde_json::json!({"part": "show", "parameters": {"k": "3 mm"}}),
            ),
        ] {
            let v = call(
                &mut session,
                "annotation.propose",
                serde_json::json!({"request": {"method": method, "params": params}}),
            );
            let a = &v["annotation"];
            out.push(ryucad::doc::canonical::to_string(&a["change"]["writes"]));
            out.push(ryucad::doc::canonical::to_string(&a["change"]["base"]));
            out.push(format!(
                "after {}",
                a["change"]["after"].as_array().unwrap().len()
            ));
            let rev = format!("annotation:{}", a["id"].as_str().unwrap());
            let r = call(
                &mut session,
                "rebuild",
                serde_json::json!({"part": "show", "rev": rev}),
            );
            out.push(ryucad::doc::canonical::to_string(&r["parts"][0]["hash"]));
        }
        out
    };
    let on = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(run)
    };
    let (one, four) = (on(1), on(4));
    assert_eq!(one, four);
    for line in &one {
        s.str(line);
    }
}

/// The showcase drawn (design 417, plan 9.A): its standard views, and
/// the top view with hidden lines dashed, laid out by ryucad on an ANSI B
/// sheet, and an ISO drawing in first angle: each drawing's
/// `drawing.get` with its items, and its PDF, its sheet's SVG and PNG,
/// built from nothing on one thread and on four.
fn drawings_section(golden: &mut Golden) {
    use serde_json::json;
    let mut s = golden.section("drawings");
    let mut answers = Vec::new();
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        answers.push(pool.install(|| {
            let (dir, session) = showcase();
            let mut session = Some(session);
            let call = |s: &mut Option<ryucad::session::Session>, m: &str, p: serde_json::Value| {
                ryucad::server::handle_safely(
                    s,
                    None,
                    None,
                    dir.path(),
                    ryucad::proto::Request::from_rpc(m, Some(p)).unwrap(),
                )
                .unwrap_or_else(|e| panic!("{m}: {e}"))
            };
            call(
                &mut session,
                "drawing.create",
                json!({"part": "show", "drawing": "main", "name": "Showcase", "views": "standard"}),
            );
            call(
                &mut session,
                "drawing.edit",
                json!({"part": "show", "drawing": "main",
                       "ops": [{"op": "set", "id": "v2", "fields": {"hidden": "dashed"}}]}),
            );
            call(
                &mut session,
                "drawing.create",
                json!({"part": "show", "drawing": "iso", "size": "iso_a3", "views": "standard"}),
            );
            call(
                &mut session,
                "drawing.edit",
                json!({"part": "show", "drawing": "iso",
                       "ops": [{"op": "set", "fields": {"standard": "iso"}}]}),
            );
            let mut lines = Vec::new();
            for d in ["main", "iso"] {
                let get = call(
                    &mut session,
                    "drawing.get",
                    json!({"part": "show", "drawing": d, "items": true}),
                );
                let mut get = get.result;
                get.as_object_mut().unwrap().remove("revision");
                lines.push(format!(
                    "{d} get {:x}",
                    md(ryucad::doc::canonical::to_string(&get).as_bytes())
                ));
                for (what, params) in [
                    (
                        "pdf",
                        json!({"part": "show", "drawing": d, "format": "pdf"}),
                    ),
                    (
                        "svg",
                        json!({"part": "show", "drawing": d, "format": "svg"}),
                    ),
                ] {
                    let a = call(&mut session, "drawing.export", params);
                    lines.push(format!("{d} {what} {:x}", md(&a.binary.unwrap())));
                }
                let a = call(
                    &mut session,
                    "drawing.render",
                    json!({"part": "show", "drawing": d, "scale": 1.0}),
                );
                lines.push(format!("{d} png {:x}", md(&a.binary.unwrap())));
            }
            lines
        }));
    }
    assert_eq!(answers[0], answers[1], "one thread and four");
    for l in &answers[0] {
        s.str(l);
    }
}

/// The golden file every section's line is in.
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/rebuild.txt");

/// One test per section (`section::<name>`), each checking only its own
/// line of the golden file, so no one test runs long and they run in
/// parallel (issue #441); `section::listed` keeps the file to these
/// sections. `UPDATE_GOLDEN=1` rewrites each test's line in place.
macro_rules! sections {
    ($golden:ident; $($name:ident => $make:expr),* $(,)?) => {
        mod section {
            use super::*;
            $(
                #[test]
                fn $name() {
                    let mut $golden = Golden::new(GOLDEN);
                    $make;
                    $golden.finish_part();
                }
            )*

            #[test]
            fn listed() {
                Golden::check_names(GOLDEN, &[$(stringify!($name)),*]);
            }
        }
    };
}

sections! {
    golden;
    units => units_section(&mut golden),
    expressions => expressions_section(&mut golden),
    placement => placement_section(&mut golden),
    rebuild => rebuild_section(&mut golden),
    display => display_section(&mut golden),
    render => render_section(&mut golden),
    compare => compare_section(&mut golden),
    sweeps => sweeps_section(&mut golden),
    references => references_section(&mut golden),
    solids => solids_section(&mut golden),
    blends => blends_section(&mut golden),
    shells => shells_section(&mut golden),
    offsets => offsets_section(&mut golden),
    tweaks => tweaks_section(&mut golden),
    mirrors => mirrors_section(&mut golden),
    splits => splits_section(&mut golden),
    sheet_metal => sheet_metal_section(&mut golden),
    sheet_metal_report => sheet_metal_report_section(&mut golden),
    sheet_metal_flat => sheet_metal_flat_section(&mut golden),
    holes => drilled_section(&mut golden, "holes", true),
    patterns => drilled_section(&mut golden, "patterns", false),
    sketch_drawing => sketch_drawing_section(&mut golden),
    diff => diff_section(&mut golden),
    merge => merge_section(&mut golden),
    pdm => pdm_section(&mut golden),
    boms => boms_section(&mut golden),
    assemblies => assemblies_section(&mut golden),
    mates => mates_section(&mut golden),
    interference => interference_section(&mut golden),
    measure => measure_section(&mut golden),
    imports => imports_section(&mut golden),
    reference_imports => reference_imports_section(&mut golden),
    exports => exports_section(&mut golden),
    formats => formats_section(&mut golden),
    views => views_section(&mut golden),
    appearance => appearance_section(&mut golden),
    sections => sections_section(&mut golden),
    explodes => explodes_section(&mut golden),
    cache => cache_section(&mut golden),
    deps => deps_section(&mut golden),
    deps_structure => deps_structure_section(&mut golden),
    datums => datums_section(&mut golden),
    paths => paths_section(&mut golden),
    sweeps_along => sweeps_along_section(&mut golden),
    lofts => lofts_section(&mut golden),
    surfaces => surfaces_section(&mut golden),
    fills => fills_section(&mut golden),
    sews => sews_section(&mut golden),
    thickens => thickens_section(&mut golden),
    annotations => annotations_section(&mut golden),
    drawings => drawings_section(&mut golden),
}
