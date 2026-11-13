//! The product trees of the OCCT files (`tests/data/occt`).

use std::path::PathBuf;

use ryustep::part21;
use ryustep::products::{Products, Transform};
use ryustep::schema::{Brep, Entities, Entity};

fn occt(name: &str) -> (Entities, Products) {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests/data/occt", name]
        .iter()
        .collect();
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let e = Entities::decode(&part21::parse(&bytes).unwrap());
    let p = Products::build(&e);
    (e, p)
}

/// The kinds of B-rep item each file's one part holds.
#[test]
fn single_parts() {
    let cases = [
        ("box_ap203.stp", "MANIFOLD_SOLID_BREP"),
        ("box_ap214.stp", "MANIFOLD_SOLID_BREP"),
        ("box_ap242.stp", "MANIFOLD_SOLID_BREP"),
        ("box_inch.stp", "MANIFOLD_SOLID_BREP"),
        ("cavity.stp", "BREP_WITH_VOIDS"),
        ("cone.stp", "MANIFOLD_SOLID_BREP"),
        ("cylinder.stp", "MANIFOLD_SOLID_BREP"),
        ("elliptic_prism.stp", "MANIFOLD_SOLID_BREP"),
        ("nurbs_cylinder.stp", "MANIFOLD_SOLID_BREP"),
        ("offset_sheet.stp", "SHELL_BASED_SURFACE_MODEL"),
        ("plate_hole.stp", "MANIFOLD_SOLID_BREP"),
        ("sphere.stp", "MANIFOLD_SOLID_BREP"),
        ("spline_revolve.stp", "MANIFOLD_SOLID_BREP"),
        ("torus.stp", "MANIFOLD_SOLID_BREP"),
    ];
    for (name, kind) in cases {
        let (e, p) = occt(name);
        assert_eq!(p.faults, [], "{name}");
        assert_eq!(p.definitions.len(), 1, "{name}");
        assert_eq!(p.roots, [0], "{name}");
        assert_eq!(p.unplaced, [], "{name}");
        let d = &p.definitions[0];
        // OCCT numbers the products it writes: "part 3", "part 6", ...
        assert!(d.name.starts_with("part "), "{name}: {}", d.name);
        assert_eq!(d.children, [], "{name}");
        assert_eq!(d.shapes.len(), 1, "{name}");
        let s = &d.shapes[0];
        assert_eq!(s.breps.len(), 1, "{name}");
        assert_eq!(s.mapped, [], "{name}");
        let brep = e.get(s.representation, s.breps[0]).unwrap();
        assert_eq!(brep.kind(), kind, "{name}");
        assert!(e.units(s.representation, s.context).is_ok(), "{name}");
    }
}

#[test]
fn the_cavity_has_one_void() {
    let (e, p) = occt("cavity.stp");
    let s = &p.definitions[0].shapes[0];
    let Ok(Entity::Brep(Brep::Solid { voids, .. })) = e.get(0, s.breps[0]) else {
        panic!("a solid");
    };
    assert_eq!(voids.len(), 1);
    assert!(matches!(
        e.get(0, voids[0]),
        Ok(Entity::OrientedShell {
            orientation: false,
            closed: true,
            ..
        })
    ));
}

/// XDE wrote an assembly of a block placed twice and a pin turned a
/// quarter about z: three definitions, two of them used.
#[test]
fn the_assembly() {
    let (e, p) = occt("assembly.stp");
    assert_eq!(p.faults, []);
    let names: Vec<&str> = p.definitions.iter().map(|d| &*d.name).collect();
    assert_eq!(names, ["assembly", "block", "pin"]);
    assert_eq!(p.roots, [0]);
    let asm = &p.definitions[0];
    assert_eq!(asm.shapes, []);
    let children: Vec<usize> = asm.children.iter().map(|o| o.definition).collect();
    assert_eq!(children, [1, 1, 2]);
    // Each child's origin goes onto a placement in the assembly: the
    // second block 100 mm along x, the pin 50 mm along y.
    let origins: Vec<Vec<f64>> = asm
        .children
        .iter()
        .map(|o| {
            let Some(Transform::Items { child, parent }) = o.transform else {
                panic!("{o:?}");
            };
            assert_eq!(child, 11);
            let at = e.placement(0, parent).unwrap().location();
            e.point(0, at).unwrap().coords().to_vec()
        })
        .collect();
    assert_eq!(
        origins,
        [
            vec![0.0, 0.0, 0.0],
            vec![100.0, 0.0, 0.0],
            vec![0.0, 50.0, 0.0]
        ]
    );
    assert_eq!(p.definitions[1].shapes.len(), 1);
    assert_eq!(p.definitions[2].shapes.len(), 1);
}

fn external_fixture(name: &str) -> ryustep::Opened {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/external")
        .join(name);
    ryustep::open(&std::fs::read(path).unwrap()).unwrap()
}

/// A document as a definition names it: file, location, format.
type Named = (String, Option<String>, Option<String>);

/// What each definition names, by product id: (file, location, format).
fn externals(o: &ryustep::Opened) -> Vec<(String, Vec<Named>, bool)> {
    o.products
        .definitions
        .iter()
        .filter(|d| !d.external.is_empty())
        .map(|d| {
            (
                d.product_id.to_string(),
                d.external
                    .iter()
                    .map(|x| (x.name.clone(), x.location.clone(), x.format.clone()))
                    .collect(),
                d.externally_defined(),
            )
        })
        .collect()
}

/// OCCT's AP214 form: a document reference to an empty document, its
/// document product's definition listing the file, named by an external
/// identification; the document product itself is named by nothing.
#[test]
fn occt_external_references_name_their_files() {
    let o = external_fixture("occt_style.stp");
    let st = |s: &str| Some(s.to_owned());
    assert_eq!(
        externals(&o),
        [
            (
                "P1".to_owned(),
                vec![("bracket.stp".to_owned(), None, st("STEP AP214"))],
                true
            ),
            (
                "P2".to_owned(),
                vec![("parts/plate.stp".to_owned(), None, st("STEP AP214"))],
                true
            ),
        ]
    );
    let asm = o
        .products
        .definitions
        .iter()
        .find(|d| &*d.product_id == "ASM")
        .unwrap();
    assert!(asm.external.is_empty() && !asm.externally_defined());
    assert_eq!(o.held.len(), 0, "no geometry in the file");
}

#[test]
fn ap203_and_cax_if_external_references_name_their_files() {
    assert_eq!(
        externals(&external_fixture("ap203_style.stp")),
        [(
            "P1".to_owned(),
            vec![("bracket.stp".to_owned(), None, None)],
            true
        )]
    );
    assert_eq!(
        externals(&external_fixture("cax_if_style.stp")),
        [(
            "P1".to_owned(),
            vec![(
                "housing.stp".to_owned(),
                Some("sub/housing.stp".to_owned()),
                Some("ISO 10303-21".to_owned())
            )],
            true
        )]
    );
}

/// A file with geometry names no documents: nothing changes for it.
#[test]
fn files_with_their_own_geometry_name_no_documents() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/occt/assembly.stp");
    let o = ryustep::open(&std::fs::read(path).unwrap()).unwrap();
    assert!(o.products.definitions.iter().all(|d| d.external.is_empty()));
}

/// A document resolved stands for every instance it was read through
/// (design 448, plan 24.D): the document, its type, the representation
/// type and references naming it, and its property definitions with
/// their representations and items. CAx-IF's form uses #100 to #111.
#[test]
fn a_document_lists_the_instances_it_is_read_through() {
    let o = external_fixture("cax_if_style.stp");
    let d = o
        .products
        .definitions
        .iter()
        .find(|d| &*d.product_id == "P1")
        .unwrap();
    assert_eq!(d.external.len(), 1);
    assert_eq!(d.external[0].related, (100..=111).collect::<Vec<u64>>());
}

/// OCCT's multi-file assembly (24.D): every externally defined product
/// names a file OCCT wrote beside it.
#[test]
fn occts_multi_file_assembly_names_files_beside_it() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/occt/multi");
    let o = ryustep::open(&std::fs::read(dir.join("assembly.stp")).unwrap()).unwrap();
    let named: Vec<String> = o
        .products
        .definitions
        .iter()
        .filter(|d| d.externally_defined())
        .map(|d| d.external[0].name.clone())
        .collect();
    assert_eq!(named.len(), 2, "{named:?}");
    for n in &named {
        assert!(dir.join(n).is_file(), "{n}");
    }
}
