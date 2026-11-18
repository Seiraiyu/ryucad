//! Inputs the `shapes` fuzz target found, run as its body runs them.

use ryu_board::model::Library;
use ryu_board::shape::{feature_shape, symbol_shape};
use ryu_board::testing::{Lcg, seeded_features, seeded_symbol, shape_faults};

/// The fuzz target's body (`fuzz/fuzz_targets/shapes.rs`): every shape
/// the bytes seed is closed and simple, on one thread and eight alike.
fn shapes(data: &[u8]) {
    let mut seed = 0xcbf2_9ce4_8422_2325u64;
    for &b in data {
        seed = (seed ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    let mut g = Lcg::new(seed);
    for _ in 0..8 {
        if let Ok(s) = symbol_shape(&seeded_symbol(&mut g)) {
            let faults = shape_faults(&s);
            assert!(faults.is_empty(), "{faults:?}");
        }
    }
    let f = seeded_features(&mut g, 8, 64, 100_000_000);
    let lib = Library::default();
    let shapes = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                (0..f.features().len())
                    .map(|i| feature_shape(&f, i, &lib).clone())
                    .collect::<Vec<_>>()
            })
    };
    let one = shapes(1);
    for (i, s) in one.iter().enumerate() {
        if let Ok(s) = s {
            let faults = shape_faults(s);
            assert!(faults.is_empty(), "feature {i}: {faults:?}");
        }
    }
    assert_eq!(one, shapes(8));
}

/// #944: a DonutRect pad (496 by 1,327 µm, a 124 µm ring, corners rounded
/// 124.001 µm) turned 75.985°: its hole's corners are rounded 1 nm, and
/// each such arc placed under the turn crossed the hole's own contour.
#[test]
fn a_ring_whose_hole_corners_round_one_nanometre_turns_simple() {
    let mut data = vec![0x85];
    data.extend([0x40; 13]);
    data.extend([0xbc, 0xc7]);
    shapes(&data);
}
