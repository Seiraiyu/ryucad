//! STL written and read back: watertight, enclosing the volume of the
//! placed meshes, in the unit asked for.

mod common;

use common::{read_ascii_stl, read_binary_stl, scene, scene_volume, volume, watertight};
use ryuformats::stl::{self, Stl};
use ryuformats::{Scene, Unit};

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

#[test]
fn a_sheets_open_mesh_is_written_as_it_is() {
    // STL needs nothing for a sheet: its triangles are written, and the
    // file is open where the sheet is.
    use ryukei::geom::{Frame, Plane, Surface};
    use ryukei::mesh::{MeshParams, tessellate};
    use ryukei::name::OpKey;
    use std::sync::Arc;
    let plane = Surface::Plane(Plane::new(Frame::WORLD));
    let (sheet, _) =
        ryukei::sheet::from_surface(OpKey::from_bytes(b"sq"), &plane, [[0.0, 0.01], [0.0, 0.01]])
            .unwrap();
    let mesh = tessellate(&sheet, &MeshParams::new(1e-5, 0.2)).unwrap();
    let n = mesh.triangles.len();
    let scene = Scene {
        products: vec![ryuformats::Product {
            id: "p".into(),
            name: "p".into(),
            part: "p".into(),
            solids: vec![ryuformats::Solid {
                name: "w".into(),
                mesh: Arc::new(mesh),
                surface: true,
            }],
            children: vec![],
        }],
    };
    let (_, triangles) = read_binary_stl(&written(&scene, &Stl::default()));
    assert_eq!(triangles.len(), n);
    assert!(!watertight(&triangles));
}
