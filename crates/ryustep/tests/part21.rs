//! Files OCCT wrote (`tests/data/occt`, made by `gen.cpp` there) read as
//! exchange structures.

use std::path::PathBuf;

use ryustep::part21::{self, Param, Record};

fn occt(name: &str) -> Vec<u8> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests/data/occt", name]
        .iter()
        .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

const FILES: [&str; 15] = [
    "assembly.stp",
    "box_ap203.stp",
    "box_ap214.stp",
    "box_ap242.stp",
    "box_inch.stp",
    "cavity.stp",
    "cone.stp",
    "cylinder.stp",
    "elliptic_prism.stp",
    "nurbs_cylinder.stp",
    "offset_sheet.stp",
    "plate_hole.stp",
    "sphere.stp",
    "spline_revolve.stp",
    "torus.stp",
];

#[test]
fn every_file_reads() {
    for name in FILES {
        let bytes = occt(name);
        let f = part21::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        // OCCT starts every instance on a line of its own.
        let text = String::from_utf8(bytes).unwrap();
        let count = text.lines().filter(|l| l.starts_with('#')).count();
        assert_eq!(f.instances.len(), count, "{name}");
        assert_eq!(f.header.file_name_field(5), Some("Open CASCADE 7.8"));
    }
}

#[test]
fn schemas_as_written() {
    let schema = |name| part21::parse(&occt(name)).unwrap().header.schemas()[0].to_owned();
    assert_eq!(schema("box_ap203.stp"), "CONFIG_CONTROL_DESIGN");
    assert_eq!(
        schema("box_ap214.stp"),
        "AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }"
    );
    // OCCT wraps the string over two lines and puts a point after the name.
    assert_eq!(
        schema("box_ap242.stp"),
        "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF. {1 0 10303 442 1 1 4 }"
    );
}

#[test]
fn a_sphere_instance_by_instance() {
    let f = part21::parse(&occt("sphere.stp")).unwrap();
    let simple = |id| match &f.get(id).unwrap().record {
        Record::Simple(p) => p.clone(),
        r => panic!("#{id}: {r:?}"),
    };
    let s = simple(22);
    assert_eq!(&*s.name, "SPHERICAL_SURFACE");
    assert_eq!(
        s.params,
        [Param::Str("".into()), Param::Ref(23), Param::Real(7.0)]
    );
    let p = simple(21);
    assert_eq!(
        p.params[1],
        Param::List(vec![
            Param::Real(4.286263797016e-16),
            Param::Real(-1.049831847863e-31),
            Param::Real(-7.0),
        ])
    );
    let Record::Complex(parts) = &f.get(27).unwrap().record else {
        panic!("#27 is complex");
    };
    let names: Vec<&str> = parts.iter().map(|p| &*p.name).collect();
    assert_eq!(
        names,
        [
            "GEOMETRIC_REPRESENTATION_CONTEXT",
            "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
            "GLOBAL_UNIT_ASSIGNED_CONTEXT",
            "REPRESENTATION_CONTEXT",
        ]
    );
    let u = simple(31);
    assert_eq!(
        u.params[0],
        Param::Typed("LENGTH_MEASURE".into(), Box::new(Param::Real(1e-7)))
    );
}

#[test]
fn one_thread_and_many_read_the_same_table() {
    for name in FILES {
        let bytes = occt(name);
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| part21::parse(&bytes).unwrap());
        assert_eq!(one, part21::parse(&bytes).unwrap(), "{name}");
    }
}
