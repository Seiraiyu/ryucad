//! Every writer of `ryuformats` over scenes from bytes: one to three
//! seeded bodies (primitives, sweeps and booleans' results, as
//! [`ryukei_testkit::blend_bytes::body`] makes them), meshed, put in parts
//! of one or two solids, placed by an assembly (on a coarse grid of
//! positions and quarter turns) directly or through a subassembly, in a
//! unit, and written as binary and ASCII STL, 3MF, usda, usdc and usdz.
//! ryuformats' own test readers read each back: STL's triangles, the
//! same both ways and enclosing the scene's volume; 3MF's package,
//! watertight by index and of the same volume; the usda read and written
//! again to the same text; the usdc read to the same layer; the usdz
//! holding the usdc's bytes. Every BrepArray must pass the schema's
//! checks, and a body the BrepArray can't describe must be a typed
//! refusal. Any panic is a bug.

#![no_main]

#[path = "../../crates/ryuformats/tests/common/mod.rs"]
#[allow(dead_code, unused_imports)]
mod common;

use libfuzzer_sys::fuzz_target;
use ryuformats::usd::layer::Prim;
use ryuformats::usd::stage::{self, StageError};
use ryuformats::{Child, Product, Scene, Solid, Unit};
use ryukei::geom::{Rigid, UnitVector3, Vector3};
use ryukei::mesh::{BodyMesh, MeshParams, tessellate};
use ryukei::topo::Body;
use ryukei_testkit::euler_script::Bytes;
use std::sync::Arc;

/// A body's mesh at a chord of 5 mm (the seeded bodies are about a
/// metre across), or `None` where it won't mesh within 200,000 triangles
/// or without faults (which `export` refuses before any writer runs).
fn meshed(b: &Body) -> Option<Arc<BodyMesh>> {
    let params = MeshParams {
        max_triangles: 200_000,
        ..MeshParams::new(5e-3, 0.5)
    };
    let m = tessellate(b, &params).ok()?;
    m.faults.is_empty().then(|| Arc::new(m))
}

/// A placement on a grid: a step of a body's size each way and a quarter
/// turn about one axis.
fn place(b: &mut Bytes) -> Rigid {
    let step = |b: &mut Bytes| (b.pick(5) as f64 - 2.0) * 0.5;
    let at = Rigid::translation(Vector3::new(step(b), step(b), step(b)));
    let axis = [UnitVector3::X, UnitVector3::Y, UnitVector3::Z][b.pick(3)];
    let turns = b.pick(4) as f64 * core::f64::consts::FRAC_PI_2;
    at.then_after(&Rigid::rotation(axis, turns))
}

/// Every prim, depth first.
fn prims(p: &Prim, out: &mut Vec<Prim>) {
    out.push(p.clone());
    for c in &p.children {
        prims(c, out);
    }
}

/// The scale of a placed volume sum: rounding each position to `f32`
/// moves each triangle's term by about this much times `f32`'s epsilon.
fn scale(triangles: &[common::Triangle]) -> f64 {
    triangles
        .iter()
        .map(|t| {
            t.iter()
                .map(|v| v.iter().map(|x| f64::from(*x).abs()).sum::<f64>())
                .product::<f64>()
        })
        .sum()
}

fuzz_target!(|data: &[u8]| {
    let mut rest = data;
    let mut bodies = Vec::new();
    while bodies.len() < 3 {
        let Some((body, r)) = ryukei_testkit::blend_bytes::body(rest) else {
            break;
        };
        rest = r;
        if let Some(mesh) = meshed(&body) {
            bodies.push((Arc::new(body), mesh));
        }
    }
    if bodies.is_empty() {
        return;
    }
    let mut b = Bytes::new(rest);
    let unit = Unit::ALL[b.pick(Unit::ALL.len())];
    // Parts of one or two solids, then a root placing each once or
    // twice, through a subassembly if the bytes say.
    let n = 1 + b.pick(2);
    let mut leaves: Vec<(Product, stage::Product)> = Vec::new();
    for k in 0..n {
        let mut p = Product {
            id: format!("P-{k}"),
            name: format!("Part {k}"),
            part: format!("p{k}"),
            ..Product::default()
        };
        let mut u = stage::Product {
            id: p.id.clone(),
            name: p.name.clone(),
            part: p.part.clone(),
            ..stage::Product::default()
        };
        for s in 0..1 + b.pick(2) {
            let (body, mesh) = &bodies[b.pick(bodies.len())];
            p.solids.push(Solid {
                name: format!("s{s}"),
                mesh: Arc::clone(mesh),
            });
            u.solids.push(stage::Solid {
                name: format!("s{s}"),
                body: Arc::clone(body),
                mesh: Some(Arc::clone(mesh)),
            });
        }
        leaves.push((p, u));
    }
    let through = b.pick(2) == 1;
    let first_leaf = if through { 2 } else { 1 };
    let mut children = Vec::new();
    for k in 0..n {
        for c in 0..1 + b.pick(2) {
            children.push(Child {
                product: first_leaf + k,
                place: place(&mut b),
                name: format!("i{k}-{c}"),
            });
        }
    }
    let root = |children: Vec<Child>| Product {
        id: "ROOT".into(),
        name: "Root".into(),
        part: "root".into(),
        children,
        ..Product::default()
    };
    let mut products = vec![];
    if through {
        products.push(root(vec![
            Child {
                product: 1,
                place: place(&mut b),
                name: "sub-a".into(),
            },
            Child {
                product: 1,
                place: place(&mut b),
                name: "sub-b".into(),
            },
        ]));
        products.push(Product {
            id: "SUB".into(),
            name: "Sub".into(),
            part: "sub".into(),
            children,
            ..Product::default()
        });
    } else {
        products.push(root(children));
    }
    let mut model: Vec<stage::Product> = products
        .iter()
        .map(|p| stage::Product {
            id: p.id.clone(),
            name: p.name.clone(),
            part: p.part.clone(),
            children: p.children.clone(),
            ..stage::Product::default()
        })
        .collect();
    for (p, u) in leaves {
        products.push(p);
        model.push(u);
    }
    let scene = Scene { products };
    let placed = scene.placed();
    let want: usize = placed.iter().map(|p| p.mesh.triangles.len()).sum();

    // STL, binary and ASCII: the same triangles, every one placed.
    let header = "ryucad 0.0.0 root mm".to_owned();
    let mut bin = Vec::new();
    let opts = ryuformats::stl::Stl {
        unit,
        ascii: false,
        header: header.clone(),
    };
    ryuformats::stl::write(&scene, &opts, &mut bin).unwrap();
    let (head, triangles) = common::read_binary_stl(&bin);
    assert!(head.starts_with(header.as_bytes()));
    assert_eq!(triangles.len(), want);
    let mut text = Vec::new();
    let opts = ryuformats::stl::Stl { ascii: true, ..opts };
    ryuformats::stl::write(&scene, &opts, &mut text).unwrap();
    let solids = common::read_ascii_stl(std::str::from_utf8(&text).unwrap());
    assert_eq!(solids.len(), placed.len());
    let ascii: Vec<common::Triangle> = solids.into_iter().flat_map(|(_, t)| t).collect();
    assert_eq!(ascii, triangles, "ASCII and binary disagree");
    let exact = common::scene_volume(&scene, unit);
    let slack = 1e-5 * scale(&triangles) + 1e-12;
    let v = common::volume(&triangles);
    assert!((v - exact).abs() <= slack, "STL {v} against {exact}");

    // 3MF: watertight objects, every component placed, the same volume.
    let opts = ryuformats::threemf::ThreeMf {
        unit,
        metadata: vec![("Title".into(), "Root".into())],
    };
    let mut package = Vec::new();
    ryuformats::threemf::write(&scene, &opts, &mut package).unwrap();
    let f = common::read_3mf(&package);
    assert_eq!(f.unit, unit.name());
    assert!(f.meshes_watertight());
    let v = f.volume();
    assert!((v - exact).abs() <= slack, "3MF {v} against {exact}");

    // USD: B-rep and meshes, else a typed refusal.
    let opts = stage::Options {
        unit,
        brep: true,
        mesh: true,
        pipe_tolerance: 1e-7,
        layer_data: vec![("version".into(), "0.0.0".into())],
    };
    let layer = match stage::layer(&stage::Model { products: model }, &opts) {
        Ok(l) => l,
        Err(StageError::Brep { .. }) => return,
        Err(e) => panic!("{e:?}"),
    };
    let text = ryuformats::usd::usda::to_string(&layer).unwrap();
    let back = common::usda::read(&text);
    assert_eq!(ryuformats::usd::usda::to_string(&back).unwrap(), text);
    let crate_bytes = ryuformats::usd::usdc::to_bytes(&layer).unwrap();
    assert_eq!(common::usdc::read(&crate_bytes), common::usdc::sorted(&layer));
    let mut z = Vec::new();
    ryuformats::usd::usdz::write(&layer, "root", &mut z).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&z)).unwrap();
    let mut entry = archive.by_name("root.usdc").unwrap();
    let mut inner = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut inner).unwrap();
    assert_eq!(inner, crate_bytes);
    let mut all = Vec::new();
    for p in &layer.prims {
        prims(p, &mut all);
    }
    for p in all
        .iter()
        .filter(|p| p.type_name.as_deref() == Some("BrepArray"))
    {
        common::brep::check(p);
    }
});
