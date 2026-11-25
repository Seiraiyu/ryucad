//! Fills of seeded asks: clear of other nets' copper by the clearance,
//! exactly; inside their area; simple polygons in order; the same on one
//! thread and eight; and, islands dropped, every polygon touching the
//! pour's own copper.

mod seeded_fill;

use ryu_copper::fill::{self, Fill};
use ryu_copper::measure::{self, Verdict};
use ryu_copper::poly::{self, Polygons};
use seeded_fill::seeded;

/// Debug builds run a quarter of the seeds.
const SEEDS: u64 = if cfg!(debug_assertions) { 16 } else { 64 };

fn each(f: &Fill) -> impl Iterator<Item = Polygons> + '_ {
    f.polygons.polygons.iter().map(|p| Polygons {
        polygons: vec![p.clone()],
        bound: f.polygons.bound,
    })
}

#[test]
fn fills_keep_their_clearance_exactly() {
    for seed in 0..SEEDS {
        let s = seeded(seed);
        let f = fill::fill(&s.ask);
        let faults = poly::faults(&f.polygons);
        assert!(faults.is_empty(), "seed {seed}: {faults:?}");
        let body = f.polygons.body();
        for (k, o) in s.others.iter().enumerate() {
            assert_eq!(
                measure::closer(&body, &o.body(), s.clearance),
                Verdict::No,
                "seed {seed}: the fill within the clearance of obstacle {k}"
            );
        }
        let area = s.ask.area.body();
        for p in each(&f) {
            assert_eq!(
                measure::contains(&area, p.polygons[0].outer[0]),
                Verdict::Yes,
                "seed {seed}: a polygon outside the area"
            );
        }
        assert!(
            poly::difference(&f.polygons, &s.ask.area).is_empty(),
            "seed {seed}: the fill leaves its area"
        );
    }
}

#[test]
fn fills_are_the_same_on_one_thread_and_eight() {
    for seed in 0..16 {
        let s = seeded(seed);
        let run = |threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| fill::fill(&s.ask))
        };
        assert_eq!(run(1), run(8), "seed {seed}");
    }
}

#[test]
fn islands_touch_their_copper() {
    for seed in 0..SEEDS {
        let mut s = seeded(seed);
        s.ask.keep_islands = false;
        let f = fill::fill(&s.ask);
        for (k, p) in each(&f).enumerate() {
            let body = p.body();
            assert!(
                s.ask
                    .own
                    .iter()
                    .any(|o| measure::touches(o, &body) != Verdict::No),
                "seed {seed}: polygon {k} touches none of the pour's copper"
            );
        }
    }
}
