//! The OCCT files (`tests/data/occt`) as typed entities.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ryustep::part21;
use ryustep::schema::{Entities, Entity, Schema};

fn occt(name: &str) -> part21::File {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests/data/occt", name]
        .iter()
        .collect();
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    part21::parse(&bytes).unwrap()
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
fn schemas() {
    let schema = |name| Schema::of(&occt(name).header).unwrap();
    assert_eq!(schema("box_ap203.stp"), Schema::Ap203);
    assert_eq!(schema("box_ap214.stp"), Schema::Ap214);
    for name in FILES {
        if !name.starts_with("box_ap2") {
            assert_eq!(schema(name), Schema::Ap242, "{name}");
        }
    }
}

/// Every instance of every file decodes, and the entities left as
/// `Other` are only these: what isn't geometry, topology, units or the
/// product tree.
#[test]
fn every_instance_decodes() {
    let mut others = BTreeSet::new();
    for name in FILES {
        let e = Entities::decode(&occt(name));
        for &id in e.ids() {
            match e.get(id, id) {
                Ok(Entity::Other(kind)) => {
                    others.insert(kind.to_string());
                }
                Ok(_) => {}
                Err(err) => panic!("{name}: {err}"),
            }
        }
    }
    let expected = [
        "APPLICATION_CONTEXT",
        "APPLICATION_PROTOCOL_DEFINITION",
        "APPROVAL",
        "APPROVAL_DATE_TIME",
        "APPROVAL_PERSON_ORGANIZATION",
        "APPROVAL_ROLE",
        "APPROVAL_STATUS",
        "CALENDAR_DATE",
        "CC_DESIGN_APPROVAL",
        "CC_DESIGN_DATE_AND_TIME_ASSIGNMENT",
        "CC_DESIGN_PERSON_AND_ORGANIZATION_ASSIGNMENT",
        "CC_DESIGN_SECURITY_CLASSIFICATION",
        "COORDINATED_UNIVERSAL_TIME_OFFSET",
        "DATE_AND_TIME",
        "DATE_TIME_ROLE",
        "DESIGN_CONTEXT",
        "DIMENSIONAL_EXPONENTS",
        "LOCAL_TIME",
        "MECHANICAL_CONTEXT",
        "PERSON",
        "PERSON_AND_ORGANIZATION",
        "PERSON_AND_ORGANIZATION_ROLE",
        "ORGANIZATION",
        "PRODUCT_CATEGORY",
        "PRODUCT_CATEGORY_RELATIONSHIP",
        "PRODUCT_CONTEXT",
        "PRODUCT_DEFINITION_CONTEXT",
        "PRODUCT_RELATED_PRODUCT_CATEGORY",
        "SECURITY_CLASSIFICATION",
        "SECURITY_CLASSIFICATION_LEVEL",
    ];
    let expected: BTreeSet<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        others.difference(&expected).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "read as Other but not expected"
    );
    assert_eq!(
        expected.difference(&others).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "expected as Other but read"
    );
}

/// Every shape representation's context gives its units: millimetres,
/// or inches in `box_inch.stp`, radians, and OCCT's 1e-7 accuracy.
#[test]
fn units_of_every_shape() {
    for name in FILES {
        let e = Entities::decode(&occt(name));
        let mut seen = 0;
        for &id in e.ids() {
            let Ok(Entity::Representation { kind, context, .. }) = e.get(id, id) else {
                continue;
            };
            if !kind.ends_with("SHAPE_REPRESENTATION") {
                continue;
            }
            seen += 1;
            let u = e
                .units(id, *context)
                .unwrap_or_else(|err| panic!("{name}: {err}"));
            let length = if name == "box_inch.stp" { 0.0254 } else { 1e-3 };
            assert_eq!(u.length, length, "{name}");
            assert_eq!(u.angle, Some(1.0), "{name}");
            assert_eq!(u.uncertainty, Some(1e-7 * length), "{name}");
        }
        assert!(seen > 0, "{name}");
    }
}
