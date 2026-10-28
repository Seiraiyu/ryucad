//! Shapes of seeded symbols and features: every one builds, every contour
//! is closed, simple, the right way round and inside the shape's box, and
//! one thread and eight build the same bits.

use ryu_board::geom::Point;
use ryu_board::model::{Board, Capabilities, Library, Source, Step};
use ryu_board::shape::symbol_shape;
use ryu_board::testing::{Golden, Lcg, seeded_features, seeded_symbol, shape_faults};

#[test]
fn every_seeded_symbol_has_a_sound_shape() {
    let mut g = Lcg::new(0x7379_6d62);
    for i in 0..2000 {
        let s = seeded_symbol(&mut g);
        let shape = symbol_shape(&s).unwrap_or_else(|e| panic!("{i}: {s:?}: {e:?}"));
        let faults = shape_faults(&shape);
        assert!(faults.is_empty(), "{i}: {s:?}: {faults:?}");
    }
}

fn board(features: ryu_board::model::Features) -> Board {
    Board {
        name: "seeded".into(),
        source: Source::default(),
        layers: vec![ryu_board::model::Layer {
            name: "top".into(),
            row: 1,
            context: ryu_board::model::Context::Board,
            kind: ryu_board::model::LayerKind::Signal,
            polarity: ryu_board::model::Polarity::Positive,
            side: Some(ryu_board::model::Side::Top),
            span: None,
            attributes: Default::default(),
        }],
        steps: vec![Step {
            name: "pcb".into(),
            datum: Point::ORIGIN,
            profile: None,
            repeats: vec![],
            layers: vec![features],
            components: vec![],
            packages: vec![],
            nets: vec![],
            attributes: Default::default(),
        }],
        board_step: 0,
        library: Library::default(),
        drills: vec![],
        stackup: None,
        attributes: Default::default(),
        capabilities: Capabilities {
            geometry: true,
            ..Capabilities::default()
        },
        faults: vec![],
        warnings: vec![],
    }
}

#[test]
fn every_seeded_feature_has_a_sound_shape() {
    let mut g = Lcg::new(0x6665_6174);
    let b = board(seeded_features(&mut g, 64, 3000, 50_000_000));
    for (i, s) in b.layer_shapes(0, 0).into_iter().enumerate() {
        let s = s.as_ref().unwrap_or_else(|e| panic!("{i}: {e:?}"));
        let faults = shape_faults(s);
        assert!(
            faults.is_empty(),
            "{i}: {:?}: {faults:?}",
            b.steps[0].layers[0].features()[i]
        );
    }
}

fn hash(threads: usize) -> String {
    let mut g = Lcg::new(0x7468_7264);
    let b = board(seeded_features(&mut g, 64, 3000, 50_000_000));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let shapes = pool.install(|| b.layer_shapes(0, 0));
    let mut golden = Golden::new("unused");
    {
        let mut s = golden.section("s");
        shapes.into_iter().for_each(|r| s.shape(r));
    }
    format!("{:?}", golden.sections_for_test())
}

#[test]
fn one_thread_and_eight_build_the_same_bits() {
    assert_eq!(hash(1), hash(8));
}
