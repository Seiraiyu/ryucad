//! Seeded symbols and features from bytes: every shape is closed, simple,
//! outer contours counterclockwise and holes clockwise, inside its box,
//! and a layer's shapes are the same bits on one thread and eight.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryu_board::model::Library;
use ryu_board::shape::{feature_shape, symbol_shape};
use ryu_board::testing::{Lcg, seeded_features, seeded_symbol, shape_faults};

fuzz_target!(|data: &[u8]| {
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
                let f = f.clone();
                (0..f.features().len())
                    .map(|i| feature_shape(&f, i, &lib).clone())
                    .collect::<Vec<_>>()
            })
    };
    let one = shapes(1);
    for s in one.iter().flatten() {
        let faults = shape_faults(s);
        assert!(faults.is_empty(), "{faults:?}");
    }
    assert_eq!(one, shapes(8));
});
