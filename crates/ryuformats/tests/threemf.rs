//! 3MF written and read back by independent readers (`zip`,
//! `roxmltree`): the package's parts, the model's unit and metadata, one
//! object per product, watertight meshes and the placed volume.

mod common;

use common::{read_3mf, scene, scene_volume};
use ryuformats::threemf::{self, ThreeMf};
use ryuformats::{Scene, Unit};

fn written(scene: &Scene, opts: &ThreeMf) -> Vec<u8> {
    let mut out = Vec::new();
    threemf::write(scene, opts, &mut out).unwrap();
    out
}

fn options(unit: Unit) -> ThreeMf {
    ThreeMf {
        unit,
        metadata: vec![
            ("Title".into(), "Base".into()),
            ("Application".into(), "ryucad test".into()),
            ("CreationDate".into(), "2026-10-28T00:00:00".into()),
            ("ryucad:chord".into(), "0.01 mm".into()),
        ],
    }
}

#[test]
fn a_package_reads_back_with_its_volume() {
    let s = scene();
    for unit in Unit::ALL {
        let f = read_3mf(&written(&s, &options(unit)));
        assert_eq!(
            f.entries,
            ["[Content_Types].xml", "_rels/.rels", "3D/3dmodel.model"]
        );
        assert!(f.content_types.contains("Extension=\"model\""));
        assert!(f.rels.contains("Target=\"/3D/3dmodel.model\""));
        assert_eq!(f.unit, unit.name());
        assert_eq!(f.metadata, options(unit).metadata);
        assert!(f.meshes_watertight(), "{unit:?}");
        // The rod is one object, used twice; the base's two solids are
        // objects of their own under the base's.
        let named: Vec<(&str, Option<&str>)> = f
            .objects
            .values()
            .map(|o| (o.name.as_str(), o.partnumber.as_deref()))
            .collect();
        assert_eq!(
            named,
            [
                ("Rod", Some("R-1")),
                ("plate", None),
                ("ball", None),
                ("Base", Some("A-1"))
            ]
        );
        assert_eq!(f.items, [4]);
        assert_eq!(f.objects[&4].components.len(), 4);
        let (got, want) = (f.volume(), scene_volume(&s, unit));
        assert!(
            (got - want).abs() <= 1e-5 * want,
            "{unit:?}: {got} against {want}"
        );
    }
}

#[test]
fn the_same_scene_gives_the_same_bytes() {
    let a = written(&scene(), &options(Unit::Millimeter));
    let b = written(&scene(), &options(Unit::Millimeter));
    assert_eq!(a, b);
    assert_ne!(a, written(&scene(), &options(Unit::Meter)));
}
