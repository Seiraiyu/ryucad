//! USD written and checked against the omniSolid schema by a reader of
//! the tests' own (`common::brep`).

mod common;

use common::brep::{Summary, check};
use ryuformats::Unit;
use ryuformats::usd::brep::{BrepError, brep_array};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;

fn tilted() -> Rigid {
    Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7)
        .then_after(&Rigid::translation(Vector3::new(0.01, -0.02, 0.03)))
}

fn sound(b: &Body, unit: Unit) -> Summary {
    let p = brep_array(b, "brep", unit, 1e-6).unwrap();
    check(&p)
}

#[test]
fn primitives_as_breparrays() {
    let k = OpKey(1);
    let t = tilted();
    let block = sound(
        &primitive::block(k, 0.01, 0.02, 0.03, &t).unwrap().0,
        Unit::Millimeter,
    );
    assert_eq!(block.regions, ["voidRegion", "solidRegion"]);
    assert_eq!(
        (block.shells, block.faces, block.edges, block.vertices),
        (2, 6, 12, 8)
    );
    assert_eq!(block.edgeuses, 24);
    assert_eq!(block.surfaces["BrepSurfacePlaneAPI"], 6);
    assert_eq!(block.curves["BrepCurve3dLineAPI"], 12);
    // A cylinder's side is cut along one seam: three faces, the side a
    // rectangle of its circles and the seam twice.
    let cyl = sound(
        &primitive::cylinder(k, 0.01, 0.02, &t).unwrap().0,
        Unit::Millimeter,
    );
    assert_eq!((cyl.faces, cyl.edges, cyl.edgeuses), (3, 3, 6));
    assert_eq!(cyl.surfaces["BrepSurfaceCylinderAPI"], 1);
    assert_eq!(cyl.rectangular, 1);
    assert_eq!((block.uvless, cyl.uvless), (0, 0));
    // A loop through a pole (a sphere's, a cone's apex) jumps along it in
    // the parameter plane: its edgeuses have no UV curves.
    for (b, kind, uvless) in [
        (
            primitive::cone(k, 0.01, 0.02, &t).unwrap().0,
            "BrepSurfaceConeAPI",
            1,
        ),
        (
            primitive::frustum(k, 0.01, 0.004, 0.02, &t).unwrap().0,
            "BrepSurfaceConeAPI",
            0,
        ),
        (
            primitive::sphere(k, 0.01, &t).unwrap().0,
            "BrepSurfaceSphereAPI",
            1,
        ),
        (
            primitive::torus(k, 0.02, 0.005, &t).unwrap().0,
            "BrepSurfaceTorusAPI",
            0,
        ),
    ] {
        for unit in [Unit::Millimeter, Unit::Meter] {
            let s = sound(&b, unit);
            assert_eq!((s.surfaces[kind], s.uvless), (1, uvless), "{kind}");
        }
    }
}

#[test]
fn holes_cavities_and_lumps() {
    let k = OpKey(1);
    let block = primitive::block(k, 0.04, 0.04, 0.02, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let pin = primitive::cylinder(k, 0.005, 0.05, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let ball = primitive::sphere(k, 0.004, &Rigid::IDENTITY).unwrap().0;
    let tool = |key: u128, body, x: f64, y: f64, z: f64| Tool {
        key: ToolKey(key),
        body,
        place: Rigid::translation(Vector3::new(x, y, z)),
    };
    // A plate with a hole: its top and bottom have a hole each.
    let (drilled, _) = boolean(
        OpKey(2),
        BooleanOp::Subtract,
        &block,
        &[tool(1, &pin, 0.02, 0.02, -0.01)],
    )
    .unwrap();
    let s = sound(&drilled, Unit::Millimeter);
    assert_eq!(s.faces, 7);
    assert_eq!(s.loops, 9);
    // A ball inside the block is a cavity: a void region of its own.
    let (hollow, _) = boolean(
        OpKey(3),
        BooleanOp::Subtract,
        &block,
        &[tool(2, &ball, 0.02, 0.02, 0.01)],
    )
    .unwrap();
    let s = sound(&hollow, Unit::Millimeter);
    assert_eq!(s.regions, ["voidRegion", "solidRegion", "voidRegion"]);
    assert_eq!(s.shells, 4);
    // Two blocks apart are two solid regions of one brep.
    let (two, _) = boolean(
        OpKey(4),
        BooleanOp::Unite,
        &block,
        &[tool(3, &block, 0.1, 0.0, 0.0)],
    )
    .unwrap();
    let s = sound(&two, Unit::Millimeter);
    assert_eq!(s.regions, ["voidRegion", "solidRegion", "solidRegion"]);
    assert_eq!(s.shells, 4);
}

#[test]
fn nurbs_bodies_as_breparrays() {
    let k = OpKey(1);
    for mut b in [
        primitive::block(k, 0.01, 0.02, 0.03, &tilted()).unwrap().0,
        primitive::cylinder(k, 0.01, 0.02, &tilted()).unwrap().0,
        primitive::sphere(k, 0.01, &tilted()).unwrap().0,
        primitive::torus(k, 0.02, 0.005, &tilted()).unwrap().0,
    ] {
        ryukei_testkit::nurbs::convert_body(&mut b).unwrap();
        let s = sound(&b, Unit::Millimeter);
        assert!(s.surfaces.contains_key("BrepSurfaceNurbAPI"));
        assert!(!s.surfaces.contains_key("BrepSurfacePlaneAPI"));
    }
}

#[test]
fn seeded_bodies_are_sound_or_refused() {
    let bodies = ryukei_testkit::race::bodies(16);
    let mut written = 0;
    for (i, b) in bodies.iter().enumerate() {
        match brep_array(b, "brep", Unit::Millimeter, 1e-6) {
            Ok(p) => {
                check(&p);
                written += 1;
            }
            Err(BrepError { reason, .. }) => assert_ne!(reason, "pcurve", "body {i}"),
        }
    }
    assert!(written >= bodies.len() - 2, "{written} of {}", bodies.len());
}
