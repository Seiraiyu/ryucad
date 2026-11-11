//! Styles through ryustep (design 448, plan 24.B): two committed files
//! ryustep writes, which ryucad's, the app's and the CLI's tests import
//! — `painted.stp`, a block red with one face blue, and
//! `painted_assembly.stp`, a rack placing the block twice (the second
//! green in that occurrence) and a pin half transparent — kept equal to
//! what ryustep writes now (`UPDATE_GOLDEN=1` rewrites them), and what
//! they read back as.

use ryukei::geom::{Rigid, Vector3};
use ryukei::name::{Name, OpKey};
use ryukei::topo::Body;
use ryustep::{
    BodyPaint, Child, LengthUnit, OccurrencePaint, Paint, Paints, Product, WriteModel,
    WriteOptions, write_styled,
};

fn block() -> Body {
    ryukei::primitive::block(OpKey(1), 0.01, 0.02, 0.03, &Rigid::IDENTITY)
        .unwrap()
        .0
}

fn pin() -> Body {
    ryukei::primitive::cylinder(OpKey(2), 0.002, 0.02, &Rigid::IDENTITY)
        .unwrap()
        .0
}

/// The block's first face by name: the one painted blue.
fn blue_face(b: &Body) -> Name {
    b.faces().map(|f| b.face(f).unwrap().name).min().unwrap()
}

const RED: Paint = Paint {
    colour: [255, 0, 0],
    opacity: 1.0,
};
const BLUE: Paint = Paint {
    colour: [0, 0, 255],
    opacity: 1.0,
};
const GREEN: Paint = Paint {
    colour: [0, 255, 0],
    opacity: 1.0,
};
const GLASS: Paint = Paint {
    colour: [0, 0, 255],
    opacity: 0.5,
};

fn opts(name: &str) -> WriteOptions {
    WriteOptions {
        unit: LengthUnit::Millimeter,
        file_name: name.into(),
        time_stamp: "2026-11-11T00:00:00".into(),
        ..Default::default()
    }
}

fn painted() -> Vec<u8> {
    let b = block();
    let face = blue_face(&b);
    let model = WriteModel {
        products: vec![Product {
            id: "block".into(),
            name: "block".into(),
            bodies: vec![b],
            names: vec!["body".into()],
            ..Default::default()
        }],
    };
    let paints = Paints {
        bodies: vec![BodyPaint {
            product: 0,
            body: 0,
            base: Some(RED),
            faces: vec![(face, BLUE)],
        }],
        occurrences: Vec::new(),
    };
    write_styled(&model, &[], &paints, &opts("painted.stp"))
        .unwrap()
        .bytes
}

fn painted_assembly() -> Vec<u8> {
    let at = |x: f64| Rigid::translation(Vector3::new(x, 0.0, 0.0));
    let model = WriteModel {
        products: vec![
            Product {
                id: "rack".into(),
                name: "rack".into(),
                children: vec![
                    Child {
                        product: 1,
                        place: at(0.0),
                        id: "1".into(),
                    },
                    Child {
                        product: 1,
                        place: at(0.05),
                        id: "2".into(),
                    },
                    Child {
                        product: 2,
                        place: at(0.1),
                        id: "3".into(),
                    },
                ],
                ..Default::default()
            },
            Product {
                id: "block".into(),
                name: "block".into(),
                bodies: vec![block()],
                names: vec!["body".into()],
                ..Default::default()
            },
            Product {
                id: "pin".into(),
                name: "pin".into(),
                bodies: vec![pin()],
                names: vec!["body".into()],
                ..Default::default()
            },
        ],
    };
    let paints = Paints {
        bodies: vec![
            BodyPaint {
                product: 1,
                body: 0,
                base: Some(RED),
                faces: Vec::new(),
            },
            BodyPaint {
                product: 2,
                body: 0,
                base: Some(GLASS),
                faces: Vec::new(),
            },
        ],
        occurrences: vec![OccurrencePaint {
            path: vec![(0, 1)],
            body: 0,
            paint: GREEN,
        }],
    };
    write_styled(&model, &[], &paints, &opts("painted_assembly.stp"))
        .unwrap()
        .bytes
}

fn kept(name: &str, bytes: &[u8]) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/styles")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    }
    let on_disk = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{name}: {e} (UPDATE_GOLDEN=1 writes it)"));
    assert!(on_disk == bytes, "{name} isn't what ryustep writes now: UPDATE_GOLDEN=1");
}

#[test]
fn the_committed_fixtures_are_what_ryustep_writes() {
    kept("painted.stp", &painted());
    kept("painted_assembly.stp", &painted_assembly());
}

#[test]
fn painted_reads_back_red_with_one_blue_face() {
    let o = ryustep::open(&painted()).unwrap();
    let s = o.body_styles(0);
    assert_eq!(s.body.unwrap().colour, Some([255, 0, 0]));
    assert_eq!(s.faces.len(), 1);
    assert_eq!(s.faces.values().next().unwrap().colour, Some([0, 0, 255]));
}

#[test]
fn painted_assembly_reads_back_its_occurrence_and_transparency() {
    let o = ryustep::open(&painted_assembly()).unwrap();
    let defs = &o.products.definitions;
    let held = |id: &str| {
        o.held
            .iter()
            .position(|h| h.definition.is_some_and(|d| &*defs[d].product_id == id))
            .unwrap()
    };
    assert_eq!(o.body_styles(held("block")).body.unwrap().colour, Some([255, 0, 0]));
    let pin = o.body_styles(held("pin")).body.unwrap();
    assert_eq!((pin.colour, pin.opacity), (Some([0, 0, 255]), Some(0.5)));
    let occ = o.occurrence_styles();
    assert_eq!(occ.len(), 1);
    assert_eq!(&*defs[o.products.roots[0]].children[occ[0].path[0].1].id, "2");
    assert_eq!(occ[0].style.colour, Some([0, 255, 0]));
}

fn occt(name: &str) -> ryustep::Opened {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/occt")
        .join(name);
    ryustep::open(&std::fs::read(path).unwrap()).unwrap()
}

/// OCCT's XDE colours: the block red with its top face blue, the pin
/// green and half transparent (`gen.cpp`).
#[test]
fn occt_colours_read_as_occt_wrote_them() {
    let o = occt("colours.stp");
    let defs = &o.products.definitions;
    let held = |name: &str| {
        o.held
            .iter()
            .position(|h| h.definition.is_some_and(|d| &*defs[d].name == name))
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let block = o.body_styles(held("block"));
    assert_eq!(block.body.unwrap().colour, Some([255, 0, 0]));
    assert_eq!(block.faces.len(), 1, "{block:?}");
    assert_eq!(block.faces.values().next().unwrap().colour, Some([0, 0, 255]));
    let pin = o.body_styles(held("pin")).body.unwrap();
    assert_eq!(pin.colour, Some([0, 255, 0]));
    assert_eq!(pin.opacity, Some(0.5));
    assert!(o.styles().failed.is_empty(), "{:?}", o.styles().failed);
}

/// One block placed twice, red in one place and blue in the other, by
/// XDE's instance colours: each occurrence reads as its colour, through
/// an occurrence style or the block's own.
#[test]
fn occt_occurrence_colours_read_per_occurrence() {
    let o = occt("colour_assembly.stp");
    let root = o.products.roots[0];
    let children = &o.products.definitions[root].children;
    assert_eq!(children.len(), 2);
    let block = children[0].definition;
    let k = o
        .held
        .iter()
        .position(|h| h.definition == Some(block))
        .unwrap();
    let own = o.body_styles(k).body;
    let occ = o.occurrence_styles();
    let colour = |n: usize| {
        occ.iter()
            .find(|x| x.path == [(root, n)] && matches!(x.target, ryustep::styles::Target::Body(_) | ryustep::styles::Target::Product))
            .map(|x| x.style)
            .or(own)
            .and_then(|s| s.colour)
    };
    let mut got = [colour(0), colour(1)];
    got.sort();
    assert_eq!(got, [Some([0, 0, 255]), Some([255, 0, 0])], "{occ:?} own {own:?}");
}
