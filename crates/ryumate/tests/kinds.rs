//! Each kind of mate removes exactly the freedoms it should, holds, and
//! leaves the motions it should, about the axes it should.

mod common;

use common::*;
use ryukei::math;
use ryukei_testkit::Lcg;
use ryumate::{Assembly, BodyState, Kind, MateState, Pose, State, solve};

/// A frame on the base and one on the arm, off their origins and turned.
fn frames() -> (Pose, Pose) {
    let a = turned([1.0, 0.0, 0.0], 0.3, [0.01, 0.02, 0.03]);
    let b = turned([0.0, 1.0, 0.0], -0.4, [0.005, 0.0, -0.01]);
    (a, b)
}

/// The arm where its frame is the base's, nudged off.
fn mated(kind: Kind, seed: u64) -> Assembly {
    let (a, b) = frames();
    let exact = a.then_after(&b.inverse());
    let mut g = Lcg::new(seed);
    pair(kind, a, b, nudged(&mut g, &exact, 0.2, 0.004))
}

/// Solves, checks it held and that a second solve gives the same bits,
/// and gives the solved assembly with the arm's report.
fn solved(asm: &Assembly) -> (Assembly, ryumate::BodyReport, ryumate::Report) {
    let s = solve(asm).unwrap();
    assert!(s.report.converged, "{:?}", s.report);
    assert!(s.report.changed);
    let at = s.assembly(asm);
    let again = solve(&at).unwrap();
    assert!(again.report.converged);
    assert!(!again.report.changed);
    assert_eq!(again.report.iterations, 0);
    assert_eq!(again.poses, s.poses);
    let arm = s.report.bodies[&2].clone();
    (at, arm, s.report)
}

fn frames_of(at: &Assembly) -> (Pose, Pose) {
    let m = &at.mates[&10];
    (world(at, &m.a), world(at, &m.b))
}

/// The distance of `p` from the line through `o` along unit `d`.
fn off_line(p: [f64; 3], o: [f64; 3], d: [f64; 3]) -> f64 {
    let v = sub(p, o);
    let along = dot(v, d);
    let w = [
        v[0] - along * d[0],
        v[1] - along * d[1],
        v[2] - along * d[2],
    ];
    math::sqrt(dot(w, w))
}

/// Whether unit `a` is `b` or `-b`.
fn along(a: [f64; 3], b: [f64; 3]) -> bool {
    (dot(a, b).abs() - 1.0).abs() < 1e-9
}

#[test]
fn fastened_frames_agree_and_nothing_moves() {
    let (at, arm, r) = solved(&mated(Kind::Fastened, 1));
    let (a, b) = frames_of(&at);
    for k in 0..3 {
        assert!(same(a.axis(k), b.axis(k), 1e-11));
    }
    assert!(same(a.origin, b.origin, 1e-10));
    assert_eq!((arm.freedom.dof, arm.state), (0, BodyState::Well));
    assert_eq!((r.dof, r.state), (0, State::Well));
    assert_eq!(r.mates[&10], MateState::Well);
}

#[test]
fn a_revolute_turns_about_its_axis() {
    let (at, arm, _) = solved(&mated(Kind::Revolute { angle: None }, 2));
    let (a, b) = frames_of(&at);
    assert!(same(a.origin, b.origin, 1e-10));
    assert!(same(a.axis(2), b.axis(2), 1e-11));
    let f = &arm.freedom;
    assert_eq!((f.dof, f.translations.len(), f.rotations.len()), (1, 0, 1));
    assert!(along(f.rotations[0].direction, a.axis(2)));
    assert!(off_line(f.rotations[0].through, a.origin, a.axis(2)) < 1e-9);
    assert_eq!(arm.state, BodyState::Under);
    // Driven: the turn from a's x to b's is the angle, and nothing moves.
    let (at, arm, _) = solved(&mated(Kind::Revolute { angle: Some(0.5) }, 3));
    let (a, b) = frames_of(&at);
    let phi = math::atan2(dot(b.axis(0), a.axis(1)), dot(b.axis(0), a.axis(0)));
    assert!((phi - 0.5).abs() < 1e-11, "{phi}");
    assert_eq!(arm.freedom.dof, 0);
}

#[test]
fn a_slider_slides_along_its_axis() {
    let (at, arm, _) = solved(&mated(Kind::Slider { distance: None }, 4));
    let (a, b) = frames_of(&at);
    for k in 0..3 {
        assert!(same(a.axis(k), b.axis(k), 1e-11));
    }
    assert!(off_line(b.origin, a.origin, a.axis(2)) < 1e-10);
    let f = &arm.freedom;
    assert_eq!((f.dof, f.translations.len(), f.rotations.len()), (1, 1, 0));
    assert!(along(f.translations[0], a.axis(2)));
    let (at, arm, _) = solved(&mated(
        Kind::Slider {
            distance: Some(0.007),
        },
        5,
    ));
    let (a, b) = frames_of(&at);
    assert!((dot(sub(b.origin, a.origin), a.axis(2)) - 0.007).abs() < 1e-10);
    assert_eq!(arm.freedom.dof, 0);
}

#[test]
fn a_cylindrical_turns_about_and_slides_along_its_axis() {
    let kind = Kind::Cylindrical {
        angle: None,
        distance: None,
    };
    let (at, arm, _) = solved(&mated(kind, 6));
    let (a, b) = frames_of(&at);
    assert!(same(a.axis(2), b.axis(2), 1e-11));
    assert!(off_line(b.origin, a.origin, a.axis(2)) < 1e-10);
    let f = &arm.freedom;
    assert_eq!((f.dof, f.translations.len(), f.rotations.len()), (2, 1, 1));
    assert!(along(f.translations[0], a.axis(2)));
    assert!(along(f.rotations[0].direction, a.axis(2)));
    assert!(off_line(f.rotations[0].through, a.origin, a.axis(2)) < 1e-9);
    let kind = Kind::Cylindrical {
        angle: Some(-1.0),
        distance: Some(0.01),
    };
    let (_, arm, _) = solved(&mated(kind, 7));
    assert_eq!(arm.freedom.dof, 0);
}

#[test]
fn a_planar_slides_in_its_plane_and_turns_about_its_normal() {
    let kind = Kind::Planar {
        angle: None,
        distance: 0.002,
    };
    let (at, arm, _) = solved(&mated(kind, 8));
    let (a, b) = frames_of(&at);
    assert!(same(a.axis(2), b.axis(2), 1e-11));
    assert!((dot(sub(b.origin, a.origin), a.axis(2)) - 0.002).abs() < 1e-10);
    let f = &arm.freedom;
    assert_eq!((f.dof, f.translations.len(), f.rotations.len()), (3, 2, 1));
    for t in &f.translations {
        assert!(dot(*t, a.axis(2)).abs() < 1e-9);
    }
    assert!(along(f.rotations[0].direction, a.axis(2)));
}

#[test]
fn a_ball_turns_every_way_about_its_centre() {
    let (at, arm, _) = solved(&mated(Kind::Ball, 9));
    let (a, b) = frames_of(&at);
    assert!(same(a.origin, b.origin, 1e-10));
    let f = &arm.freedom;
    assert_eq!((f.dof, f.translations.len(), f.rotations.len()), (3, 0, 3));
    // The assembly's own axes, each through the ball's centre.
    for (k, r) in f.rotations.iter().enumerate() {
        let mut e = [0.0; 3];
        e[k] = 1.0;
        assert_eq!(r.direction, e);
        assert!(off_line(r.through, a.origin, e) < 1e-9);
    }
}

#[test]
fn an_unmated_body_is_free_every_way_and_a_fixed_one_not_at_all() {
    let mut asm = Assembly::default();
    asm.bodies.insert(1, fixed(Pose::IDENTITY));
    asm.bodies
        .insert(2, free(turned([0.0, 0.0, 1.0], 0.2, [0.1, 0.0, 0.0])));
    let s = solve(&asm).unwrap();
    assert!(s.report.converged && !s.report.changed);
    let f = &s.report.bodies[&2].freedom;
    assert_eq!((f.dof, f.translations.len(), f.rotations.len()), (6, 3, 3));
    assert_eq!(s.report.bodies[&1].freedom.dof, 0);
    assert_eq!(s.report.dof, 6);
    assert_eq!(s.poses[&2], asm.bodies[&2].pose);
}

#[test]
fn frames_far_apart_align_the_near_way() {
    // The arm a long way round from where it belongs, about every axis:
    // the solver still finds the frames agreeing, not pointing apart.
    let (a, b) = frames();
    let exact = a.then_after(&b.inverse());
    for (n, angle) in [
        ([1.0, 0.0, 0.0], 2.5),
        ([0.0, 1.0, 0.0], -2.8),
        ([0.0, 0.6, 0.8], 2.0),
        // Exactly opposite: no turn is nearer than another.
        ([0.0, 0.0, 1.0], std::f64::consts::PI),
        ([0.6, 0.0, 0.8], std::f64::consts::PI),
    ] {
        let start = turned(n, angle, [0.0; 3]).then_after(&exact);
        let s = solve(&pair(Kind::Fastened, a, b, start)).unwrap();
        assert!(s.report.converged, "{n:?} {angle}");
        let at = s.assembly(&pair(Kind::Fastened, a, b, start));
        let (wa, wb) = frames_of(&at);
        for k in 0..3 {
            assert!(same(wa.axis(k), wb.axis(k), 1e-11), "{n:?} {angle}");
        }
    }
}

#[test]
fn bodies_in_a_chain_follow_each_other() {
    // Base, link and tip: the link on a revolute, the tip fastened to the
    // link. The tip follows wherever the link goes and has its freedom.
    let (a, b) = frames();
    let mut asm = mated(Kind::Revolute { angle: None }, 11);
    asm.bodies
        .insert(3, free(turned([0.0, 0.0, 1.0], 0.1, [0.05, 0.0, 0.0])));
    asm.mates.insert(
        11,
        ryumate::Mate {
            kind: Kind::Fastened,
            a: on(2, a),
            b: on(3, b),
        },
    );
    let (at, _, r) = solved(&asm);
    assert_eq!(r.bodies[&3].freedom.dof, 1);
    assert_eq!(r.dof, 1);
    let tip = at.bodies[&3].pose;
    let link = at.bodies[&2].pose;
    let want = link.then_after(&a).then_after(&b.inverse());
    assert!(same(tip.origin, want.origin, 1e-10));
}
