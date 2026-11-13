//! What a solve reports: redundant and conflicting mates, states, and
//! patterns' copies moving with their seeds.

mod common;

use common::*;
use ryumate::{
    Assembly, BodyState, Connector, Direction, Kind, Limits, Mate, MateError, MateState, Motion,
    On, Pattern, Pose, Range, State, solve, solve_whole,
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
            limits: ryumate::Limits::NONE,
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
            limits: ryumate::Limits::NONE,
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
            limits: ryumate::Limits::NONE,
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
            limits: ryumate::Limits::NONE,
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
            limits: ryumate::Limits::NONE,
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
            limits: ryumate::Limits::NONE,
        },
    );
    // Copy [0, 1] on the base: the seed goes 2 cm down y.
    let s = solve(&asm).unwrap();
    assert!(s.report.converged);
    assert!(same(s.poses[&2].origin, [0.0, -0.02, 0.0], 1e-10));
}

/// Two arms hinged on the base, one carrying a lid on a ball, the other
/// alone; a bolt circle on the base, a washer fastened to its second copy;
/// every free body nudged off where its mates hold.
fn two_groups() -> Assembly {
    let mut asm = bolt_circle();
    let off = |x: f64, y: f64| Pose {
        origin: [x, y, 0.0],
        ..Pose::IDENTITY
    };
    let hinge = |a, b, at: Pose| Mate {
        kind: revolute(),
        a: on(a, at),
        b: on(b, Pose::IDENTITY),
        limits: ryumate::Limits::NONE,
    };
    asm.bodies.insert(3, free(off(0.101, 0.002)));
    asm.bodies.insert(4, free(off(0.152, 0.001)));
    asm.bodies.insert(5, free(off(-0.098, 0.003)));
    asm.bodies.insert(6, free(off(0.0, 0.2)));
    asm.mates.insert(10, hinge(1, 3, off(0.1, 0.0)));
    asm.mates.insert(
        11,
        Mate {
            kind: Kind::Ball,
            a: on(3, off(0.05, 0.0)),
            b: on(4, Pose::IDENTITY),
            limits: ryumate::Limits::NONE,
        },
    );
    asm.mates.insert(12, hinge(1, 5, off(-0.1, 0.0)));
    asm.mates.insert(
        13,
        Mate {
            kind: Kind::Fastened,
            a: Connector {
                on: copy(2),
                frame: Pose::IDENTITY,
            },
            b: on(6, Pose::IDENTITY),
            limits: ryumate::Limits::NONE,
        },
    );
    asm
}

#[test]
fn a_component_solves_as_the_whole_does() {
    let asm = two_groups();
    let whole = solve(&asm).unwrap();
    assert!(whole.report.converged);
    // The lid's arm: the lid and the base (a constant), not the other
    // arm, the bolt or the washer.
    let c = asm.component(&[3]);
    assert_eq!(c.bodies.keys().copied().collect::<Vec<_>>(), [1, 3, 4]);
    assert_eq!(c.mates.keys().copied().collect::<Vec<_>>(), [10, 11]);
    assert!(c.patterns.is_empty());
    let part = solve(&c).unwrap();
    assert!(part.report.converged);
    for b in [3, 4] {
        assert_eq!(part.poses[&b], whole.poses[&b], "body {b}");
    }
    // The washer reaches the bolt through its copy, and the base the
    // pattern turns about.
    let c = asm.component(&[6]);
    assert_eq!(c.bodies.keys().copied().collect::<Vec<_>>(), [1, 2, 6]);
    assert_eq!(c.patterns.keys().copied().collect::<Vec<_>>(), [20]);
    let part = solve(&c).unwrap();
    for b in [2, 6] {
        assert_eq!(part.poses[&b], whole.poses[&b], "body {b}");
    }
    // From the base, fixed as it is, everything mated to it; two bodies
    // in different components give both.
    assert_eq!(asm.component(&[1]).bodies.len(), 6);
    let c = asm.component(&[4, 5]);
    assert_eq!(c.bodies.keys().copied().collect::<Vec<_>>(), [1, 3, 4, 5]);
    // A body nothing mates is alone.
    let mut lone = asm.clone();
    lone.bodies.insert(7, free(Pose::IDENTITY));
    assert_eq!(lone.component(&[7]).bodies.len(), 1);
    assert!(lone.component(&[7]).mates.is_empty());
}

/// A hinge about z between the fixed base and body 2, turned `at`.
fn turned_hinge(at: f64) -> ryumate::Assembly {
    common::pair(
        Kind::Revolute { angle: None },
        Pose::IDENTITY,
        Pose::IDENTITY,
        common::turned([0.0, 0.0, 1.0], at, [0.0; 3]),
    )
}

#[test]
fn a_mates_values_are_read_at_the_poses() {
    let v = ryumate::values(&turned_hinge(0.3)).unwrap();
    let a = v[&10].angle.unwrap();
    assert!((a - 0.3).abs() < 1e-15, "{a}");
    assert_eq!(v[&10].distance, None);
    // A slider's distance is b's origin along a's z.
    let s = common::pair(
        Kind::Slider { distance: None },
        Pose::IDENTITY,
        Pose::IDENTITY,
        common::moved([0.0, 0.0, 0.05]),
    );
    let d = ryumate::values(&s).unwrap()[&10].distance.unwrap();
    assert!((d - 0.05).abs() < 1e-17, "{d}");
    // A fastened mate has no free value.
    let f = common::pair(
        Kind::Fastened,
        Pose::IDENTITY,
        Pose::IDENTITY,
        Pose::IDENTITY,
    );
    assert!(!ryumate::values(&f).unwrap().contains_key(&10));
}

fn limited(
    mut asm: ryumate::Assembly,
    angle: Option<Range>,
    distance: Option<Range>,
) -> ryumate::Assembly {
    asm.mates.get_mut(&10).unwrap().limits = Limits { angle, distance };
    asm
}

fn angle_of(asm: &ryumate::Assembly, s: &ryumate::Solved) -> f64 {
    ryumate::values(&s.assembly(asm)).unwrap()[&10]
        .angle
        .unwrap()
}

#[test]
fn a_stored_pose_past_a_stop_is_pulled_back_to_it() {
    let r = Some(Range {
        min: Some(-0.1),
        max: Some(0.1),
    });
    let asm = limited(turned_hinge(0.3), r, None);
    let s = ryumate::solve(&asm).unwrap();
    assert!(s.report.converged, "{:?}", s.report);
    assert!(s.report.changed);
    assert!((angle_of(&asm, &s) - 0.1).abs() <= 1e-11);
    // Still one turn free the other way: a limit is no row.
    assert_eq!(s.report.bodies[&2].freedom.dof, 1);
    assert_eq!(s.report.mates[&10], MateState::Well);
    // At the stop it holds: the same bits again.
    let again = ryumate::solve(&s.assembly(&asm)).unwrap();
    assert!(!again.report.changed);
    assert_eq!(again.poses, s.poses);
}

#[test]
fn within_its_limits_nothing_changes_to_the_bit() {
    let r = Some(Range {
        min: Some(-0.1),
        max: Some(0.1),
    });
    for at in [0.05, -0.0999] {
        let plain = ryumate::solve(&turned_hinge(at)).unwrap();
        let s = ryumate::solve(&limited(turned_hinge(at), r, None)).unwrap();
        assert_eq!(s.poses, plain.poses);
        assert_eq!(s.report.iterations, plain.report.iterations);
    }
}

#[test]
fn ranges_across_the_cut_and_one_sided_stops() {
    let deg = |d: f64| d * (core::f64::consts::PI / 180.0);
    let range = Range {
        min: Some(deg(170.0)),
        max: Some(deg(190.0)),
    };
    let across = Some(range);
    let asm = limited(turned_hinge(deg(-175.0)), across, None);
    assert!(!ryumate::solve(&asm).unwrap().report.changed);
    let asm = limited(turned_hinge(deg(-160.0)), across, None);
    let s = ryumate::solve(&asm).unwrap();
    let a = ryumate::unwrap_angle(angle_of(&asm, &s), &range);
    assert!((a - deg(190.0)).abs() <= 1e-11, "{a}");
    let min_only = Some(Range {
        min: Some(-0.1),
        max: None,
    });
    let asm = limited(turned_hinge(-0.3), min_only, None);
    let s = ryumate::solve(&asm).unwrap();
    assert!((angle_of(&asm, &s) + 0.1).abs() <= 1e-11);
    assert!(
        !ryumate::solve(&limited(turned_hinge(2.0), min_only, None))
            .unwrap()
            .report
            .changed
    );
}

#[test]
fn a_slider_stops_at_its_distance() {
    let s = common::pair(
        Kind::Slider { distance: None },
        Pose::IDENTITY,
        Pose::IDENTITY,
        common::moved([0.0, 0.0, 0.05]),
    );
    let asm = limited(
        s,
        None,
        Some(Range {
            min: Some(0.0),
            max: Some(0.02),
        }),
    );
    let solved = ryumate::solve(&asm).unwrap();
    assert!(solved.report.converged);
    let d = ryumate::values(&solved.assembly(&asm)).unwrap()[&10]
        .distance
        .unwrap();
    assert!((d - 0.02).abs() <= 1e-10, "{d}");
}

#[test]
fn a_chain_of_stops_ends_within_as_many_rounds() {
    let mut asm = ryumate::Assembly::default();
    asm.bodies.insert(0, common::fixed(Pose::IDENTITY));
    let r = Some(Range {
        min: Some(-0.05),
        max: Some(0.05),
    });
    let off = common::moved([0.01, 0.0, 0.0]);
    let mut place = Pose::IDENTITY;
    for k in 1..=3u32 {
        place = place
            .then_after(&off)
            .then_after(&common::turned([0.0, 0.0, 1.0], 0.3, [0.0; 3]));
        asm.bodies.insert(k, common::free(place));
        asm.mates.insert(
            k,
            Mate {
                kind: Kind::Revolute { angle: None },
                a: common::on(k - 1, off),
                b: common::on(k, Pose::IDENTITY),
                limits: Limits {
                    angle: r,
                    distance: None,
                },
            },
        );
    }
    let s = ryumate::solve(&asm).unwrap();
    assert!(s.report.converged, "{:?}", s.report);
    let v = ryumate::values(&s.assembly(&asm)).unwrap();
    for k in 1..=3 {
        assert!(
            (v[&k].angle.unwrap() - 0.05).abs() <= 1e-11,
            "{k}: {:?}",
            v[&k]
        );
    }
}

#[test]
fn a_stop_that_cant_hold_conflicts_by_name() {
    // The arm is also fastened to a fixed body 0.5 rad round: within ±0.1
    // the hinge can't hold beside it.
    let mut asm = limited(
        turned_hinge(0.5),
        Some(Range {
            min: Some(-0.1),
            max: Some(0.1),
        }),
        None,
    );
    asm.bodies.insert(
        3,
        common::fixed(common::turned([0.0, 0.0, 1.0], 0.5, [0.0; 3])),
    );
    asm.mates.insert(
        11,
        Mate {
            kind: Kind::Fastened,
            a: common::on(3, Pose::IDENTITY),
            b: common::on(2, Pose::IDENTITY),
            limits: Limits::NONE,
        },
    );
    let s = ryumate::solve(&asm).unwrap();
    assert!(!s.report.converged);
    assert!(
        s.report
            .conflicting
            .iter()
            .any(|c| c.contains(&10) && c.contains(&11)),
        "{:?}",
        s.report
    );
}

#[test]
fn a_driven_value_is_never_stopped_by_ryumate() {
    let mut asm = limited(
        turned_hinge(0.0),
        Some(Range {
            min: Some(-0.1),
            max: Some(0.1),
        }),
        None,
    );
    asm.mates.get_mut(&10).unwrap().kind = Kind::Revolute { angle: Some(0.5) };
    let s = ryumate::solve(&asm).unwrap();
    assert!((angle_of(&asm, &s) - 0.5).abs() <= 1e-11);
}

#[test]
fn the_report_gives_each_mates_values_and_stops() {
    let r = Some(Range {
        min: Some(-0.1),
        max: Some(0.1),
    });
    let s = ryumate::solve(&limited(turned_hinge(0.3), r, None)).unwrap();
    let v = s.report.values[&10].angle.unwrap();
    assert!((v.value - 0.1).abs() <= 1e-11);
    assert_eq!(v.at, Some(ryumate::Stop::Max));
    let free = ryumate::solve(&turned_hinge(0.3)).unwrap();
    let v = free.report.values[&10].angle.unwrap();
    assert!((v.value - 0.3).abs() < 1e-15);
    assert_eq!(v.at, None);
}

/// A lid (body 1) carrying three places in its own frame: 20 mm along x,
/// 30 mm along y, and a quarter turn about its z; a screw (body 2, free)
/// 10 mm along x is the seed.
fn screws_on_a_lid(lid: Pose) -> Assembly {
    let mut asm = Assembly::default();
    asm.bodies.insert(1, free(lid));
    asm.bodies.insert(2, free(moved([0.01, 0.0, 0.0])));
    asm.patterns.insert(
        20,
        Pattern {
            seeds: vec![2],
            motion: Motion::Places {
                body: 1,
                places: vec![
                    moved([0.02, 0.0, 0.0]),
                    moved([0.0, 0.03, 0.0]),
                    turned([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2, [0.0; 3]),
                ],
            },
        },
    );
    asm
}

fn fastened_to(index: [u32; 2], other: u32) -> Mate {
    Mate {
        kind: Kind::Fastened,
        a: Connector {
            on: On::Copy {
                pattern: 20,
                seed: 2,
                index,
            },
            frame: Pose::IDENTITY,
        },
        b: on(other, Pose::IDENTITY),
        limits: Limits::NONE,
    }
}

#[test]
fn places_are_checked() {
    let mut asm = screws_on_a_lid(Pose::IDENTITY);
    assert_eq!(asm.validate(), Ok(()));
    asm.bodies.insert(3, fixed(Pose::IDENTITY));
    asm.mates.insert(10, fastened_to([3, 0], 3));
    assert_eq!(asm.validate(), Ok(()));
    // Three places: copies [1, 0] to [3, 0], one row.
    for index in [[4, 0], [1, 1]] {
        asm.mates.insert(10, fastened_to(index, 3));
        assert_eq!(
            asm.validate(),
            Err(MateError::UnknownCopy {
                mate: Some(10),
                pattern: 20,
                seed: 2
            })
        );
    }
    let mut asm = screws_on_a_lid(Pose::IDENTITY);
    if let Motion::Places { body, .. } = &mut asm.patterns.get_mut(&20).unwrap().motion {
        *body = 9;
    }
    assert_eq!(
        asm.validate(),
        Err(MateError::UnknownBody {
            mate: None,
            body: 9
        })
    );
    let mut asm = screws_on_a_lid(Pose::IDENTITY);
    if let Motion::Places { places, .. } = &mut asm.patterns.get_mut(&20).unwrap().motion {
        places[0].origin[0] = f64::NAN;
    }
    assert!(matches!(asm.validate(), Err(MateError::BadGeometry { .. })));
}
