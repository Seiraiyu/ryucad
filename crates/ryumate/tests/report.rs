//! What a solve reports: redundant and conflicting mates, states, and
//! patterns' copies moving with their seeds.

mod common;

use common::*;
use ryumate::{
    Assembly, BodyState, Connector, Direction, Kind, Mate, MateError, MateState, Motion, On,
    Pattern, Pose, State, solve, solve_whole,
};

fn revolute() -> Kind {
    Kind::Revolute { angle: None }
}

/// The base, and an arm on a revolute about z at the base's origin,
/// sitting where it holds.
fn hinge() -> Assembly {
    pair(revolute(), Pose::IDENTITY, Pose::IDENTITY, Pose::IDENTITY)
}

#[test]
fn a_mate_given_twice_is_redundant() {
    let mut asm = hinge();
    asm.mates.insert(11, asm.mates[&10]);
    let s = solve(&asm).unwrap();
    assert!(s.report.converged);
    assert_eq!(s.report.state, State::Over);
    assert_eq!(s.report.redundant, [vec![10, 11]]);
    assert!(s.report.conflicting.is_empty());
    assert_eq!(s.report.mates[&10], MateState::Redundant);
    assert_eq!(s.report.bodies[&2].state, BodyState::Over);
    // It still turns about z.
    assert_eq!(s.report.bodies[&2].freedom.dof, 1);
    assert_eq!(
        solve_whole(&asm).unwrap().report.redundant,
        s.report.redundant
    );
}

#[test]
fn a_ball_inside_a_revolute_is_redundant_and_two_axes_conflict() {
    // A ball at the hinge's centre says part of what the hinge says.
    let mut asm = hinge();
    asm.mates.insert(
        11,
        Mate {
            kind: Kind::Ball,
            a: on(1, Pose::IDENTITY),
            b: on(2, Pose::IDENTITY),
        },
    );
    let r = solve(&asm).unwrap().report;
    assert!(r.converged);
    assert_eq!(r.redundant, [vec![10, 11]]);
    // A second hinge about x through the same point: the arm can't turn
    // about both, but it can sit still, so this is redundant, not in
    // conflict, and nothing moves.
    let mut asm = hinge();
    let about_x = turned([0.0, 1.0, 0.0], std::f64::consts::FRAC_PI_2, [0.0; 3]);
    asm.mates.insert(
        11,
        Mate {
            kind: revolute(),
            a: on(1, about_x),
            b: on(2, about_x),
        },
    );
    let r = solve(&asm).unwrap().report;
    assert!(r.converged);
    assert_eq!(r.bodies[&2].freedom.dof, 0);
    // The same hinge 1 cm off along x: it can't hold with the first.
    let mut asm = hinge();
    let off = Pose {
        origin: [0.01, 0.0, 0.0],
        ..Pose::IDENTITY
    };
    asm.mates.insert(
        11,
        Mate {
            kind: revolute(),
            a: on(1, off),
            b: on(2, Pose::IDENTITY),
        },
    );
    let s = solve(&asm).unwrap();
    assert!(!s.report.converged);
    assert_eq!(s.report.conflicting, [vec![10, 11]]);
    assert_eq!(s.report.mates[&11], MateState::Conflicting);
    assert_eq!(s.report.bodies[&2].state, BodyState::Failing);
}

#[test]
fn mates_between_fixed_bodies_hold_or_conflict_alone() {
    let mut asm = hinge();
    asm.bodies.get_mut(&2).unwrap().fixed = true;
    let r = solve(&asm).unwrap().report;
    assert!(r.converged);
    assert_eq!(r.redundant, [vec![10]]);
    asm.bodies.get_mut(&2).unwrap().pose.origin = [0.0, 0.01, 0.0];
    let r = solve(&asm).unwrap().report;
    assert!(!r.converged);
    assert_eq!(r.conflicting, [vec![10]]);
    assert_eq!(r.mates[&10], MateState::Conflicting);
}

#[test]
fn what_names_nothing_is_refused() {
    let mut asm = hinge();
    asm.mates.get_mut(&10).unwrap().b.on = On::Body(7);
    assert_eq!(
        solve(&asm).unwrap_err(),
        MateError::UnknownBody {
            mate: Some(10),
            body: 7
        }
    );
}

/// A bolt (body 2, free) on the base, and a circular pattern of it about
/// the base's z: 6 copies a sixth of a turn apart.
fn bolt_circle() -> Assembly {
    let mut asm = Assembly::default();
    asm.bodies.insert(1, fixed(Pose::IDENTITY));
    asm.bodies.insert(
        2,
        free(Pose {
            origin: [0.05, 0.0, 0.0],
            ..Pose::IDENTITY
        }),
    );
    asm.patterns.insert(
        20,
        Pattern {
            seeds: vec![2],
            motion: Motion::Circular {
                axis: Direction::Connector(on(1, Pose::IDENTITY)),
                step: std::f64::consts::TAU / 6.0,
            },
        },
    );
    asm
}

fn copy(i: u32) -> On {
    On::Copy {
        pattern: 20,
        seed: 2,
        index: [i, 0],
    }
}

#[test]
fn copies_move_with_their_seed() {
    let asm = bolt_circle();
    let p = ryumate::copy_poses(&asm, &[(20, 2, [1, 0]), (20, 2, [3, 0])]);
    let c = std::f64::consts::FRAC_PI_3;
    assert!(same(
        p[0].origin,
        [
            0.05 * ryukei::math::cos(c),
            0.05 * ryukei::math::sin(c),
            0.0
        ],
        1e-15
    ));
    assert!(same(p[1].origin, [-0.05, 0.0, 0.0], 1e-15));
    // A nut fastened to copy 3, 2 mm up: it and the seed meet halfway
    // (both are free), the nut where copy 3 of the moved seed is.
    let mut asm = bolt_circle();
    let half = turned([0.0, 0.0, 1.0], std::f64::consts::PI, [0.0; 3]);
    asm.bodies.insert(
        3,
        free(Pose {
            origin: [-0.04, 0.001, 0.0],
            ..half
        }),
    );
    let up = Pose {
        origin: [0.0, 0.0, 0.002],
        ..Pose::IDENTITY
    };
    asm.mates.insert(
        10,
        Mate {
            kind: Kind::Fastened,
            a: Connector {
                on: copy(3),
                frame: up,
            },
            b: on(3, Pose::IDENTITY),
        },
    );
    let s = solve(&asm).unwrap();
    assert!(s.report.converged, "{:?}", s.report);
    let at = s.assembly(&asm);
    let copy3 = ryumate::copy_poses(&at, &[(20, 2, [3, 0])])[0].then_after(&up);
    assert!(same(s.poses[&3].origin, copy3.origin, 1e-10));
    assert!(s.poses[&2] != asm.bodies[&2].pose);
    // The nut is fixed by the copy, and the seed's freedom carries it:
    // the nut moves with the seed, so the nut itself has none of its own
    // left only once the seed is fixed.
    assert_eq!(s.report.bodies[&3].freedom.dof, 6);
    asm.bodies.get_mut(&2).unwrap().fixed = true;
    let s = solve(&asm).unwrap();
    assert_eq!(s.report.bodies[&3].freedom.dof, 0);
    // Fixing the nut instead moves the seed: copy 3 goes to the nut.
    let mut asm = bolt_circle();
    asm.bodies.insert(
        3,
        fixed(Pose {
            origin: [-0.06, 0.0, 0.0],
            ..half
        }),
    );
    asm.mates.insert(
        10,
        Mate {
            kind: Kind::Fastened,
            a: Connector {
                on: copy(3),
                frame: up,
            },
            b: on(3, Pose::IDENTITY),
        },
    );
    let s = solve(&asm).unwrap();
    assert!(s.report.converged);
    assert!(same(s.poses[&2].origin, [0.06, 0.0, -0.002], 1e-10));
    // With the nut the right way up but not turned as the copy is, the
    // frames start exactly opposite each other, where the residuals have
    // no direction: the seed is turned a little first, and turns the rest
    // of the half turn.
    asm.bodies.get_mut(&3).unwrap().pose = Pose {
        origin: [-0.06, 0.0, 0.0],
        ..Pose::IDENTITY
    };
    let s = solve(&asm).unwrap();
    assert!(s.report.converged, "{:?}", s.report);
    let at = s.assembly(&asm);
    let copy3 = ryumate::copy_poses(&at, &[(20, 2, [3, 0])])[0].then_after(&up);
    for k in 0..3 {
        assert!(same(copy3.axis(k), at.bodies[&3].pose.axis(k), 1e-11));
    }
    // Both fixed, nothing can turn: it conflicts alone, never holds.
    asm.bodies.get_mut(&2).unwrap().fixed = true;
    let r = solve(&asm).unwrap().report;
    assert!(!r.converged);
    assert_eq!(r.conflicting, [vec![10]]);
    assert!(r.redundant.is_empty());
    assert_eq!(r.mates[&10], MateState::Conflicting);
}

#[test]
fn a_linear_pattern_in_two_directions_places_its_grid() {
    let mut asm = Assembly::default();
    asm.bodies.insert(1, fixed(Pose::IDENTITY));
    asm.bodies.insert(2, free(Pose::IDENTITY));
    asm.patterns.insert(
        20,
        Pattern {
            seeds: vec![2],
            motion: Motion::Linear {
                direction: Direction::World([1.0, 0.0, 0.0]),
                spacing: 0.01,
                second: Some((
                    Direction::Connector(on(
                        1,
                        turned([1.0, 0.0, 0.0], -std::f64::consts::FRAC_PI_2, [0.0; 3]),
                    )),
                    0.02,
                )),
            },
        },
    );
    let p = ryumate::copy_poses(&asm, &[(20, 2, [2, 1])]);
    // The connector's z is the base's y.
    assert!(same(p[0].origin, [0.02, 0.02, 0.0], 1e-15));
    asm.mates.insert(
        10,
        Mate {
            kind: Kind::Fastened,
            a: Connector {
                on: On::Copy {
                    pattern: 20,
                    seed: 2,
                    index: [0, 1],
                },
                frame: Pose::IDENTITY,
            },
            b: on(1, Pose::IDENTITY),
        },
    );
    // Copy [0, 1] on the base: the seed goes 2 cm down y.
    let s = solve(&asm).unwrap();
    assert!(s.report.converged);
    assert!(same(s.poses[&2].origin, [0.0, -0.02, 0.0], 1e-10));
}
