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
            fill::outside(&f.polygons, &s.ask.area).is_empty(),
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

/// A tiled fill (ryueda 7.E) keeps every promise of a whole one: exactly
/// clear of obstacles by the clearance, inside the area, simple polygons
/// in each tile, the same spokes; one thread and eight alike; and it
/// covers what the whole fill covers, but for the seams' bound.
#[test]
fn tiled_fills_keep_the_whole_fills_promises() {
    let side = 3 * seeded_fill::MM;
    for seed in 0..SEEDS {
        let s = seeded(seed);
        let whole = fill::fill(&s.ask);
        let t = fill::fill_tiled(&s.ask, side, None);
        let all = t.polygons();
        for (k, o) in s.others.iter().enumerate() {
            assert_eq!(
                measure::closer(&all.body(), &o.body(), s.clearance),
                Verdict::No,
                "seed {seed}: the tiled fill within the clearance of obstacle {k}"
            );
        }
        assert!(
            poly::difference(&all, &s.ask.area).is_empty(),
            "seed {seed}"
        );
        for tile in &t.tiles {
            let faults = poly::faults(&tile.fill);
            assert!(
                faults.is_empty(),
                "seed {seed} tile {:?}: {faults:?}",
                tile.at
            );
        }
        assert_eq!(t.spokes, whole.spokes, "seed {seed}");
        let lost = poly::difference(
            &whole.polygons,
            &poly::grow(&all, 4 * fill::BOUND, &fill::opts()),
        );
        assert!(
            lost.is_empty(),
            "seed {seed}: the tiles lose {} polygons",
            lost.polygons.len()
        );
        let more = poly::difference(
            &all,
            &poly::grow(&whole.polygons, 4 * fill::BOUND, &fill::opts()),
        );
        assert!(
            more.is_empty(),
            "seed {seed}: the tiles add {} polygons",
            more.polygons.len()
        );
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| fill::fill_tiled(&s.ask, side, None))
        };
        assert_eq!(run(1), run(8), "seed {seed}");
    }
}

/// A refill after one obstacle grew by a micrometre (ryueda 7.E) fills
/// only the tiles whose inputs changed, reuses the rest, and equals a
/// tiled fill from nothing.
#[test]
fn a_refill_rebuilds_only_the_tiles_an_edit_reaches() {
    let side = 3 * seeded_fill::MM;
    let mut fewer = 0;
    for seed in 0..16u64 {
        let s = seeded(seed);
        let (first, all) = fill::fill_tiled_counted(&s.ask, side, None);
        assert_eq!(
            all,
            first.tiles.len(),
            "seed {seed}: from nothing, every tile"
        );
        let (same, none) = fill::fill_tiled_counted(&s.ask, side, Some(&first));
        assert_eq!((none, &same), (0, &first), "seed {seed}: nothing changed");
        let mut ask = s.ask.clone();
        if ask.obstacles.is_empty() {
            continue;
        }
        ask.obstacles[0] = poly::grow(&ask.obstacles[0], 1_000, &fill::opts()).into();
        let (again, built) = fill::fill_tiled_counted(&ask, side, Some(&first));
        assert_eq!(again, fill::fill_tiled(&ask, side, None), "seed {seed}");
        let changed = again
            .tiles
            .iter()
            .filter(|t| {
                first
                    .tiles
                    .iter()
                    .find(|u| u.at == t.at)
                    .is_none_or(|u| u.key != t.key)
            })
            .count();
        assert_eq!(
            built, changed,
            "seed {seed}: only the changed tiles are filled"
        );
        fewer += usize::from(built < first.tiles.len());
    }
    assert!(
        fewer >= 8,
        "{fewer} of 16 seeds refilled fewer than every tile"
    );
}
