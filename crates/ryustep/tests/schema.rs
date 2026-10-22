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
        "ADVANCED_BREP_SHAPE_REPRESENTATION",
        "ADVANCED_FACE",
        "APPLICATION_CONTEXT",
        "APPLICATION_PROTOCOL_DEFINITION",
        "APPROVAL",
        "APPROVAL_DATE_TIME",
        "APPROVAL_PERSON_ORGANIZATION",
        "APPROVAL_ROLE",
        "APPROVAL_STATUS",
        "BREP_WITH_VOIDS",
        "CALENDAR_DATE",
        "CC_DESIGN_APPROVAL",
        "CC_DESIGN_DATE_AND_TIME_ASSIGNMENT",
        "CC_DESIGN_PERSON_AND_ORGANIZATION_ASSIGNMENT",
        "CC_DESIGN_SECURITY_CLASSIFICATION",
        "CLOSED_SHELL",
        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
        "CONVERSION_BASED_UNIT LENGTH_UNIT NAMED_UNIT",
        "COORDINATED_UNIVERSAL_TIME_OFFSET",
        "DATE_AND_TIME",
        "DATE_TIME_ROLE",
        "DEFINITIONAL_REPRESENTATION",
        "DESIGN_CONTEXT",
        "DIMENSIONAL_EXPONENTS",
        "EDGE_CURVE",
        "EDGE_LOOP",
        "FACE_BOUND",
        "GEOMETRIC_REPRESENTATION_CONTEXT GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT GLOBAL_UNIT_ASSIGNED_CONTEXT REPRESENTATION_CONTEXT",
        "GEOMETRIC_REPRESENTATION_CONTEXT PARAMETRIC_REPRESENTATION_CONTEXT REPRESENTATION_CONTEXT",
        "ITEM_DEFINED_TRANSFORMATION",
        "LENGTH_MEASURE_WITH_UNIT",
        "LENGTH_UNIT NAMED_UNIT SI_UNIT",
        "LOCAL_TIME",
        "MANIFOLD_SOLID_BREP",
        "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
        "MECHANICAL_CONTEXT",
        "NAMED_UNIT PLANE_ANGLE_UNIT SI_UNIT",
        "NAMED_UNIT SI_UNIT SOLID_ANGLE_UNIT",
        "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
        "OPEN_SHELL",
        "ORIENTED_CLOSED_SHELL",
        "ORIENTED_EDGE",
        "PERSON",
        "PERSON_AND_ORGANIZATION",
        "PERSON_AND_ORGANIZATION_ROLE",
        "ORGANIZATION",
        "PRODUCT",
        "PRODUCT_CATEGORY",
        "PRODUCT_CATEGORY_RELATIONSHIP",
        "PRODUCT_CONTEXT",
        "PRODUCT_DEFINITION",
        "PRODUCT_DEFINITION_CONTEXT",
        "PRODUCT_DEFINITION_FORMATION",
        "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
        "PRODUCT_DEFINITION_SHAPE",
        "PRODUCT_RELATED_PRODUCT_CATEGORY",
        "REPRESENTATION_RELATIONSHIP REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION SHAPE_REPRESENTATION_RELATIONSHIP",
        "SECURITY_CLASSIFICATION",
        "SECURITY_CLASSIFICATION_LEVEL",
        "SHAPE_DEFINITION_REPRESENTATION",
        "SHAPE_REPRESENTATION",
        "SHELL_BASED_SURFACE_MODEL",
        "UNCERTAINTY_MEASURE_WITH_UNIT",
        "VERTEX_LOOP",
        "VERTEX_POINT",
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
