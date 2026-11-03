//! Seeded sheet-metal parts (design 298, plan 11.D): plates with one to
//! four edge flanges, holes in the plate and the flanges, and holes and
//! slots cut across bend lines in the flat
//! ([`ryukei_testkit::sheet_metal_bytes`]). Each part unfolds, its flat
//! is cut, refolded and unfolded again:
//!
//! - with no cuts, the refolded part has the part's topology and names,
//!   its vertices within `1e-12` m (the fold map and its inverse round);
//! - unfolding the refolded part gives the cut flat as closely;
//! - every body passes `check` and `check_intersections` and meshes
//!   without faults;
//! - one thread and eight give the same bits.
//!
//! Release only: minutes in a debug build.

use ryukei_testkit::Lcg;
use ryukei_testkit::sheet_metal_bytes::{Case, Refused, Trip, case, judge, trip, typed};
use std::time::Instant;

const SEEDS: u64 = 64;

fn bytes(seed: u64) -> Vec<u8> {
    let mut g = Lcg::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5348_4d50);
    (0..96).map(|_| (g.next_u64() >> 56) as u8).collect()
}

fn on(threads: usize, data: &[u8]) -> Option<Result<(Case, Trip), Refused>> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    pool.install(|| {
        let c = case(data)?;
        Some(c.and_then(|c| trip(&c).map(|r| (c, r))))
    })
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn seeded_parts_unfold_cut_and_refold() {
    let mut refused = Vec::new();
    let mut wrong = Vec::new();
    let mut held = 0;
    for seed in 0..SEEDS {
        let data = bytes(seed);
        let t0 = Instant::now();
        let one = on(1, &data).expect("enough bytes");
        let secs = t0.elapsed().as_secs_f64();
        match one {
            Err(e) => {
                assert!(typed(&e), "seed {seed}: {e:?}");
                eprintln!(
                    "seed {seed}: refused at {} in {secs:.1} s: {:?}",
                    e.step, e.error
                );
                refused.push(seed);
            }
            Ok((c, r)) => {
                eprintln!(
                    "seed {seed}: {} bends, {} faces, {} cuts in {secs:.1} s",
                    c.model.bends.len(),
                    c.folded.faces().count(),
                    c.cuts.len()
                );
                if let Err(why) = judge(&c, &r) {
                    eprintln!("seed {seed}: {why}");
                    wrong.push(seed);
                    continue;
                }
                let Some(Ok((c8, r8))) = on(8, &data) else {
                    panic!("seed {seed}: eight threads refused");
                };
                assert_eq!(c.folded.dump(), c8.folded.dump(), "seed {seed}");
                assert_eq!(r.again.dump(), r8.again.dump(), "seed {seed}");
                assert_eq!(r.refolded.dump(), r8.refolded.dump(), "seed {seed}");
                held += 1;
            }
        }
    }
    eprintln!("{held} held, refused {refused:?}, wrong {wrong:?}");
    assert_eq!(wrong, Vec::<u64>::new());
    // Flanges that meet (8, 61), a flange running into another after the
    // booleans (12, 58), a hole drilled where the part no longer reads as
    // sheet metal (47): typed refusals of the seeds as drawn. A change
    // that moves this list moves what the seeds make; review it.
    assert_eq!(refused, [8, 12, 47, 58, 61]);
}

