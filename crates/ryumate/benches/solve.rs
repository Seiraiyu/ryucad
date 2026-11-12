//! Solving assemblies of 10 to 1,000 bodies: bolts fastened round a fixed
//! plate (each body its own piece), and chains of driven hinges (one
//! component, a block per link), from poses jostled off and from poses
//! that hold (which still diagnoses).

#[path = "../tests/common/mod.rs"]
mod common;

use common::{fixed, free, jostle, on, turned};
use divan::Bencher;
use ryukei_testkit::Lcg;
use ryumate::{Assembly, Kind, Mate, Pose, solve};

fn main() {
    divan::main();
}

/// `n` bolts fastened to a plate 1 cm apart, jostled by about a degree
/// and a tenth of a millimetre.
fn bolts(n: u32) -> Assembly {
    let mut g = Lcg::new(7);
    let mut asm = Assembly::default();
    asm.bodies.insert(0, fixed(Pose::IDENTITY));
    for k in 1..=n {
        let at = Pose {
            origin: [0.01 * f64::from(k % 40), 0.01 * f64::from(k / 40), 0.0],
            ..Pose::IDENTITY
        };
        asm.bodies.insert(k, free(at));
        asm.mates.insert(
            k,
            Mate {
                kind: Kind::Fastened,
                a: on(0, at),
                b: on(k, Pose::IDENTITY),
                limits: ryumate::Limits::NONE,
            },
        );
    }
    jostle(&mut asm, &mut g, 0.02, 1e-4);
    asm
}

/// A chain of `n` links on hinges driven to 10°, jostled likewise.
fn chain(n: u32) -> Assembly {
    let mut g = Lcg::new(8);
    let mut asm = Assembly::default();
    asm.bodies.insert(0, fixed(Pose::IDENTITY));
    let off = Pose {
        origin: [0.01, 0.0, 0.0],
        ..Pose::IDENTITY
    };
    let mut place = Pose::IDENTITY;
    for k in 1..=n {
        place = place.then_after(&off).then_after(&turned(
            [0.0, 0.0, 1.0],
            0.17453292519943295,
            [0.0; 3],
        ));
        asm.bodies.insert(k, free(place));
        asm.mates.insert(
            k,
            Mate {
                kind: Kind::Revolute {
                    angle: Some(0.17453292519943295),
                },
                a: on(k - 1, off),
                b: on(k, Pose::IDENTITY),
                limits: ryumate::Limits::NONE,
            },
        );
    }
    jostle(&mut asm, &mut g, 0.02, 1e-4);
    asm
}

#[divan::bench(args = [10, 100, 1000], sample_count = 10)]
fn bolts_jostled(b: Bencher, n: u32) {
    let asm = bolts(n);
    b.bench(|| solve(&asm).unwrap());
}

#[divan::bench(args = [10, 100, 1000], sample_count = 10)]
fn bolts_held(b: Bencher, n: u32) {
    let asm = bolts(n);
    let held = solve(&asm).unwrap().assembly(&asm);
    b.bench(|| solve(&held).unwrap());
}

#[divan::bench(args = [10, 100], sample_count = 10)]
fn chain_jostled(b: Bencher, n: u32) {
    let asm = chain(n);
    b.bench(|| solve(&asm).unwrap());
}

#[divan::bench(args = [10, 100], sample_count = 10)]
fn chain_held(b: Bencher, n: u32) {
    let asm = chain(n);
    let held = solve(&asm).unwrap().assembly(&asm);
    b.bench(|| solve(&held).unwrap());
}

/// `n` bolts on hinges round z, each stopped at ±6°, stored turned 0.2
/// rad: every one pinned in the second round.
fn bolts_with_stops(n: u32) -> Assembly {
    let mut asm = Assembly::default();
    asm.bodies.insert(0, fixed(Pose::IDENTITY));
    let stop = 6.0 * (core::f64::consts::PI / 180.0);
    for k in 1..=n {
        let o = [0.01 * f64::from(k % 40), 0.01 * f64::from(k / 40), 0.0];
        let at = Pose {
            origin: o,
            ..Pose::IDENTITY
        };
        asm.bodies.insert(k, free(turned([0.0, 0.0, 1.0], 0.2, o)));
        asm.mates.insert(
            k,
            Mate {
                kind: Kind::Revolute { angle: None },
                a: on(0, at),
                b: on(k, Pose::IDENTITY),
                limits: ryumate::Limits {
                    angle: Some(ryumate::Range {
                        min: Some(-stop),
                        max: Some(stop),
                    }),
                    distance: None,
                },
            },
        );
    }
    asm
}

#[divan::bench(args = [10, 100, 1000], sample_count = 10)]
fn bolts_stopped(b: Bencher, n: u32) {
    let asm = bolts_with_stops(n);
    b.bench(|| solve(&asm).unwrap());
}
