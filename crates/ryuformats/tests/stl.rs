//! STL written and read back: watertight, enclosing the volume of the
//! placed meshes, in the unit asked for.

mod common;

use common::{read_ascii_stl, read_binary_stl, scene_volume, volume, watertight};
use ryuformats::stl::{self, Stl};
use ryuformats::{Child, Product, Scene, Solid, Unit};
use ryukei::geom::{Rigid, UnitVector3, Vector3};
use ryukei::mesh::MeshParams;
use ryukei::name::OpKey;
use std::sync::Arc;

fn meshed(b: ryukei::topo::Body) -> Arc<ryukei::mesh::BodyMesh> {
    Arc::new(ryukei::mesh::tessellate(&b, &MeshParams::new(1e-5, 0.26)).unwrap())
}

/// A plate with a ball on it, and a cylinder placed twice, the second
/// turned: curved faces, seams, poles.
fn scene() -> Scene {
    let k = OpKey(7);
    let plate = ryukei::primitive::block(k, 0.08, 0.05, 0.004, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let ball = ryukei::primitive::sphere(
        k,
        0.006,
        &Rigid::translation(Vector3::new(0.02, 0.02, 0.02)),
    )
    .unwrap()
    .0;
    let rod = ryukei::primitive::cylinder(k, 0.003, 0.03, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let solid = |name: &str, b| Solid {
        name: name.into(),
        mesh: meshed(b),
    };
    let turned = Rigid::translation(Vector3::new(0.06, 0.03, 0.01))
        .then_after(&Rigid::rotation(UnitVector3::X, 1.0));
    Scene {
        products: vec![
            Product {
                id: "A-1".into(),
                name: "Base".into(),
                part: "base".into(),
                solids: vec![solid("plate", plate), solid("ball", ball)],
                children: vec![
                    Child {
                        product: 1,
                        place: Rigid::translation(Vector3::new(0.01, 0.01, 0.004)),
                        name: "r1".into(),
                    },
                    Child {
                        product: 1,
                        place: turned,
                        name: "r2".into(),
                    },
                ],
            },
            Product {
                id: "R-1".into(),
                name: "Rod".into(),
                part: "rod".into(),
                solids: vec![solid("rod", rod)],
                children: vec![],
            },
        ],
    }
}

fn written(scene: &Scene, opts: &Stl) -> Vec<u8> {
    let mut out = Vec::new();
    stl::write(scene, opts, &mut out).unwrap();
    out
}

#[test]
fn binary_reads_back_watertight_with_its_volume() {
    let s = scene();
    for unit in Unit::ALL {
        let opts = Stl {
            unit,
            header: format!("ryucad test {}", unit.as_str()),
            ..Stl::default()
        };
        let (header, triangles) = read_binary_stl(&written(&s, &opts));
        assert!(header.starts_with(format!("ryucad test {}", unit.as_str()).as_bytes()));
        assert_eq!(triangles.len() as u64, s.triangles(0));
        assert!(watertight(&triangles), "{unit:?}");
        let (got, want) = (volume(&triangles), scene_volume(&s, unit));
        assert!(
            (got - want).abs() <= 1e-5 * want,
            "{unit:?}: {got} against {want}"
        );
    }
    // The same scene, the same bytes.
    assert_eq!(
        written(&s, &Stl::default()),
        written(&scene(), &Stl::default())
    );
}

#[test]
fn ascii_reads_back_as_the_binary_does() {
    let s = scene();
    let ascii = Stl {
        ascii: true,
        ..Stl::default()
    };
    let text = String::from_utf8(written(&s, &ascii)).unwrap();
    let solids = read_ascii_stl(&text);
    let names: Vec<&str> = solids.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["base/plate", "base/ball", "base/r1/rod", "base/r2/rod"]
    );
    for (name, t) in &solids {
        assert!(watertight(t), "{name}");
    }
    // Every vertex reads back to the bits the binary file holds.
    let all: Vec<_> = solids.into_iter().flat_map(|(_, t)| t).collect();
    let (_, binary) = read_binary_stl(&written(&s, &Stl::default()));
    let bits = |t: &[[[f32; 3]; 3]]| -> Vec<u32> {
        t.iter().flatten().flatten().map(|x| x.to_bits()).collect()
    };
    assert_eq!(bits(&all), bits(&binary));
}
