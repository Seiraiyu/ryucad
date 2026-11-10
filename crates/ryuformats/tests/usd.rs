//! USD written and checked against the omniSolid schema by a reader of
//! the tests' own (`common::brep`).

mod common;

use common::brep::{Summary, check};
use ryuformats::usd::brep::brep_array;
use ryuformats::usd::stage::{Model, Options, Product, Solid, layer};
use ryuformats::usd::{usda, usdc, usdz};
use ryuformats::{Child, Unit};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use std::sync::Arc;

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

/// A torus face whose loop crosses the tube's seam but not the turn's
/// (a rolled fillet corner's, #247): the cut along the turn was judged by
/// an area the tube's unplaced jumps spoiled, and refused as `domain`.
#[test]
fn a_torus_patch_across_its_tube_seam() {
    let k = OpKey(1);
    let torus = primitive::torus(k, 0.006, 0.004, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let block = primitive::block(k, 0.006, 0.004, 0.004, &Rigid::IDENTITY)
        .unwrap()
        .0;
    // The block's corner at (-3, 7.5, -2) mm: the torus's outside, about
    // the y axis, a band a quarter of the tube high either side of its
    // outer equator.
    let tool = Tool {
        key: ToolKey(1),
        body: &block,
        place: Rigid::translation(Vector3::new(-0.003, 0.0075, -0.002)),
    };
    let (cap, _) = boolean(OpKey(2), BooleanOp::Intersect, &torus, &[tool]).unwrap();
    let s = sound(&cap, Unit::Millimeter);
    assert_eq!(s.surfaces["BrepSurfaceTorusAPI"], 1);
    assert_eq!(s.faces, 6);
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

/// An offset body's faces on offsets of splines are written as their
/// NURBS forms within the pipe tolerance, beside its spheres (its edges,
/// splines too, are filled by pipes, also NURBS).
#[test]
fn offset_faces_as_nurbs() {
    let (mut b, _) = primitive::block(OpKey(1), 0.01, 0.02, 0.03, &tilted()).unwrap();
    ryukei_testkit::nurbs::convert_body(&mut b).unwrap();
    let spec = ryukei::offset::OffsetSpec {
        distance: 0.002,
        faces: Vec::new(),
    };
    let (o, _) = ryukei::offset::offset_body(OpKey(2), &b, &spec).unwrap();
    assert!(o.faces().any(|f| matches!(
        o.surface(o.face(f).unwrap().surface),
        Some(ryukei::geom::Surface::Offset(_))
    )));
    let s = sound(&o, Unit::Millimeter);
    assert!(s.surfaces.contains_key("BrepSurfaceNurbAPI"));
    assert!(s.surfaces.contains_key("BrepSurfaceSphereAPI"));
}

#[test]
fn seeded_bodies_are_sound() {
    for (i, b) in ryukei_testkit::race::bodies(16).iter().enumerate() {
        let p = brep_array(b, "brep", Unit::Millimeter, 1e-6)
            .unwrap_or_else(|e| panic!("body {i}: {e}"));
        check(&p);
    }
}

/// Sweeps whose faces on extrusions and revolutions were refused `domain`
/// (issue #470; race bodies 16, 33 and 37, and OCCT's half turn of a
/// spline profile, which ryucad's `formats` golden exports): their curves
/// were placed in a band of each period before their loops were turned,
/// running with their edges, so a use against its edge was taken from its
/// far end, a period off the one before, and no cut left the loops a
/// positive area.
#[test]
fn swept_faces_place_their_curves_as_their_loops_run() {
    use ryukei_testkit::race::seed_bytes;
    use ryukei_testkit::sweep_bytes::sweep;
    for seed in [9, 22, 24] {
        let (b, _) = sweep(&seed_bytes(seed)).expect("it builds");
        for unit in [Unit::Millimeter, Unit::Meter] {
            let p = brep_array(&b, "brep", unit, 1e-7)
                .unwrap_or_else(|e| panic!("sweep {seed} in {unit:?}: {e}"));
            check(&p);
        }
    }
}

/// Race bodies whose BrepArrays broke the schema's rules (issue #469):
/// edges off their vertices where `heal::seamed` cut a seam from the
/// wrong place, or where an edge 5e-16 past its spline's end was written
/// over the whole spline; a torus's seam along a hole's edge, which left
/// the hole outside the band; a sphere face's range starting 3e-9 short
/// of its first turn; and, once their seams were right, extrusions whose
/// curves crossed their NURBS form's period start; and #247's last three:
/// two revolutions and a plane face's circle. Release only: blends
/// take seconds each in a debug build.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a minute in a debug build: CI runs it in release"
)]
fn race_bodies_469_are_sound() {
    use ryukei_testkit::race::{blend_body, boolean_body};
    // Race bodies 133, 196, 220, 365, 392, 479, 498, 587, 1054, 1086,
    // 1314, 1434 and 1592 (blends), 529, 1003, 1372, 488 and 980
    // (booleans).
    let blends = [
        75, 110, 122, 199, 215, 266, 276, 325, 584, 602, 728, 796, 894,
    ];
    let booleans = [292, 559, 763, 270, 546];
    let bodies = blends
        .iter()
        .map(|&k| (format!("blend {k}"), blend_body(k)))
        .chain(
            booleans
                .iter()
                .map(|&k| (format!("boolean {k}"), boolean_body(k))),
        );
    for (what, b) in bodies {
        let b = b.expect("it builds");
        let p = brep_array(&b, "brep", Unit::Millimeter, 1e-7)
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        check(&p);
    }
    // Two revolutions of closed profiles, race bodies 819 and 482, whose
    // near-meridian edges a fit by projection can't follow onto their
    // NURBS forms across the profile's period start: their curves are
    // carried through the forms' reparameterizations (#247).
    for seed in [455, 267] {
        let b = blend_body(seed).expect("it builds");
        let p = brep_array(&b, "brep", Unit::Millimeter, 1e-7)
            .unwrap_or_else(|e| panic!("blend {seed}: {e}"));
        check(&p);
    }
    // Race body 1187, a boolean whose plane face's circle was cut two
    // ulps short of a knot (#247).
    let b = boolean_body(657).expect("it builds");
    let p = brep_array(&b, "brep", Unit::Millimeter, 1e-7)
        .unwrap_or_else(|e| panic!("boolean 657: {e}"));
    check(&p);
}

/// Race bodies SMLib wouldn't import or measured wrongly (issue #620),
/// each now passing the checks that would have caught it: fifteen faces
/// a whole turn round starting away from a quarter turn, whose ranges
/// came out a rounding error over 360 degrees in SMLib's degrees (race
/// bodies 155, 200, 229, 272, 380, 427, 467, 481, 536, 539, 755, 1008,
/// 1265, 1372 and 1564); race body 1318's cylinder, one loop touching
/// itself at a vertex and covering the whole turn with no seam, now cut
/// there and seamed; and race body 469's torus, whose seam crossed a
/// hole that dipped past it between `heal::seamed`'s samples. Release
/// only: blends take seconds each in a debug build.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a minute in a debug build: CI runs it in release"
)]
fn race_bodies_620_are_sound() {
    use ryukei_testkit::race::{blend_body, boolean_body};
    let blends = [87, 259];
    let booleans = [
        113, 128, 151, 209, 233, 258, 267, 298, 299, 421, 562, 701, 731, 763, 881,
    ];
    let bodies = blends
        .iter()
        .map(|&k| (format!("blend {k}"), blend_body(k)))
        .chain(
            booleans
                .iter()
                .map(|&k| (format!("boolean {k}"), boolean_body(k))),
        );
    for (what, b) in bodies {
        let b = b.expect("it builds");
        let p = brep_array(&b, "brep", Unit::Millimeter, 1e-7)
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        check(&p);
    }
}

/// Every race body (`race::writable(1600)`) is written as a BrepArray
/// that passes the schema's checks, or refused where `REFUSED` lists it (none).
/// Release only (minutes even there): its own step in `full.yml`.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes even in release: full.yml runs it in release"
)]
fn the_race_is_sound_or_refused() {
    use rayon::prelude::*;
    // Race bodies refused, and why: none since #247's last three.
    const REFUSED: [(usize, &str); 0] = [];
    let bodies = ryukei_testkit::race::writable(1600);
    let outcomes: Vec<Result<(), String>> = bodies
        .par_iter()
        .enumerate()
        .map(
            |(i, b)| match brep_array(b, "brep", Unit::Millimeter, 1e-7) {
                Ok(p) => std::panic::catch_unwind(|| {
                    check(&p);
                })
                .map_err(|e| {
                    let why = e
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    format!("body {i} breaks the schema's rules: {why}")
                }),
                Err(e) if REFUSED.contains(&(i, e.reason)) => Ok(()),
                Err(e) => Err(format!("body {i}: {e}")),
            },
        )
        .collect();
    let wrong: Vec<String> = outcomes.into_iter().filter_map(Result::err).collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A frame placing a pin twice and a ring once, every solid with its
/// B-rep and its mesh.
fn frame() -> Model {
    let k = OpKey(1);
    let solid = |name: &str, b: Body| Solid {
        name: name.into(),
        mesh: Some(Arc::new(
            tessellate(&b, &MeshParams::new(1e-5, 0.26)).unwrap(),
        )),
        body: Arc::new(b),
        brep: true,
    };
    let at = |x: f64| Rigid::translation(Vector3::new(x, 0.0, 0.01));
    Model {
        products: vec![
            Product {
                id: "F-1".into(),
                name: "Frame".into(),
                part: "frame".into(),
                label: None,
                solids: vec![solid(
                    "plate",
                    primitive::block(k, 0.1, 0.05, 0.01, &Rigid::IDENTITY)
                        .unwrap()
                        .0,
                )],
                children: vec![
                    Child {
                        product: 1,
                        place: at(0.01),
                        name: "a".into(),
                    },
                    Child {
                        product: 1,
                        place: at(0.03),
                        name: "b".into(),
                    },
                    Child {
                        product: 2,
                        place: at(0.06).then_after(&tilted()),
                        name: "ring-1".into(),
                    },
                ],
            },
            Product {
                id: "P-1".into(),
                name: "Pin".into(),
                part: "pin".into(),
                label: Some("B".into()),
                solids: vec![solid(
                    "shank",
                    primitive::cylinder(k, 0.003, 0.02, &Rigid::IDENTITY)
                        .unwrap()
                        .0,
                )],
                children: vec![],
            },
            Product {
                id: "R-1".into(),
                name: "Ring \"7\"".into(),
                part: "ring".into(),
                label: None,
                solids: vec![solid(
                    "torus",
                    primitive::torus(k, 0.01, 0.002, &Rigid::IDENTITY)
                        .unwrap()
                        .0,
                )],
                children: vec![],
            },
        ],
    }
}

#[test]
fn usda_reads_back_to_the_same_layer() {
    let opts = Options {
        unit: Unit::Millimeter,
        brep: true,
        mesh: true,
        pipe_tolerance: 1e-7,
        layer_data: vec![("version".into(), "0.0.0".into())],
    };
    let l = layer(&frame(), &opts).unwrap();
    let text = usda::to_string(&l).unwrap();
    let back = common::usda::read(&text);
    assert_eq!(back, l);
    assert_eq!(usda::to_string(&back).unwrap(), text);
    // Every BrepArray in it is sound.
    fn breps<'a>(
        p: &'a ryuformats::usd::layer::Prim,
        out: &mut Vec<&'a ryuformats::usd::layer::Prim>,
    ) {
        if p.type_name.as_deref() == Some("BrepArray") {
            out.push(p);
        }
        for c in &p.children {
            breps(c, out);
        }
    }
    let mut found = Vec::new();
    for p in &back.prims {
        breps(p, &mut found);
    }
    assert_eq!(found.len(), 3);
    for b in found {
        check(b);
    }
}

#[test]
fn usdc_reads_back_to_the_same_layer() {
    let opts = Options {
        unit: Unit::Millimeter,
        brep: true,
        mesh: true,
        pipe_tolerance: 1e-7,
        layer_data: vec![
            ("version".into(), "0.0.0".into()),
            ("time_stamp".into(), "2026-10-29T00:00:00".into()),
        ],
    };
    let l = layer(&frame(), &opts).unwrap();
    let bytes = usdc::to_bytes(&l).unwrap();
    assert_eq!(&bytes[..8], b"PXR-USDC");
    // The same layer, the same bytes.
    assert_eq!(usdc::to_bytes(&l).unwrap(), bytes);
    let back = common::usdc::read(&bytes);
    assert_eq!(back, common::usdc::sorted(&l));
    // Read back and written again, the same bytes: dictionaries are
    // written sorted whatever order they were made in.
    assert_eq!(usdc::to_bytes(&back).unwrap(), bytes);
    // Binary is smaller than text: the frame's meshes and BrepArrays.
    let text = usda::to_string(&l).unwrap();
    assert!(
        bytes.len() * 2 < text.len(),
        "{} against {}",
        bytes.len(),
        text.len()
    );
}

#[test]
fn usdc_refuses_numbers_that_arent_finite() {
    use ryuformats::usd::layer::{Attribute, Value};
    let mut l = layer(
        &frame(),
        &Options {
            unit: Unit::Meter,
            brep: false,
            mesh: true,
            pipe_tolerance: 0.0,
            layer_data: vec![],
        },
    )
    .unwrap();
    l.prims[0]
        .attributes
        .push(Attribute::new("bad", Value::Double(f64::NAN)).unwrap());
    let e = usdc::to_bytes(&l).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn usdc_refuses_tokens_and_strings_holding_nul() {
    use ryuformats::usd::layer::{Attribute, DictValue, Value};
    // A STEP product name can decode `\X2\0000\X0\` to U+0000; the token
    // table is NUL-separated, so it would read back as other tokens.
    let base = layer(
        &frame(),
        &Options {
            unit: Unit::Meter,
            brep: false,
            mesh: true,
            pipe_tolerance: 0.0,
            layer_data: vec![],
        },
    )
    .unwrap();
    let mut string = base.clone();
    string.prims[0]
        .attributes
        .push(Attribute::new("bad", Value::String("a\0b".into())).unwrap());
    let mut token = base.clone();
    token.prims[0]
        .attributes
        .push(Attribute::new("bad", Value::Token("a\0b".into())).unwrap());
    let mut data = base.clone();
    data.prims[0]
        .meta
        .custom_data
        .0
        .push(("name".into(), DictValue::String("a\0b".into())));
    for l in [string, token, data] {
        let e = usdc::to_bytes(&l).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        let e = usdz::write(&l, "frame", &mut Vec::new()).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    }
    assert!(usdc::to_bytes(&base).is_ok());
}

#[test]
fn usdz_is_the_usdc_stored_and_aligned() {
    let opts = Options {
        unit: Unit::Centimeter,
        brep: true,
        mesh: true,
        pipe_tolerance: 1e-7,
        layer_data: vec![],
    };
    let l = layer(&frame(), &opts).unwrap();
    let mut bytes = Vec::new();
    usdz::write(&l, "frame", &mut bytes).unwrap();
    let mut again = Vec::new();
    usdz::write(&l, "frame", &mut again).unwrap();
    assert_eq!(bytes, again);
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    assert_eq!(z.len(), 1);
    let mut f = z.by_index(0).unwrap();
    assert_eq!(f.name(), "frame.usdc");
    assert_eq!(f.compression(), zip::CompressionMethod::Stored);
    assert_eq!(f.data_start() % usdz::ALIGN as u64, 0);
    let mut inner = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut inner).unwrap();
    assert_eq!(inner, usdc::to_bytes(&l).unwrap());
}
