//! The bodies of design 94's NURBS race: a block whose top is a lofted
//! bicubic NURBS face, and a pin lofted through wavy sections, placed on
//! a grid of 40 × 25 so the block loses 1,000 holes. Each pin pokes out of
//! both faces, or, in the flush variant, stands on the block's bottom
//! plane (issue #77).

use core::f64::consts::TAU;
use ryukei::geom::{
    Curve2, Frame, KnotVector, Line2, NurbsCurve2, Point2, Point3, Rigid, Vector3,
};
use ryukei::name::OpKey;
use ryukei::sweep::{LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, loft};
use ryukei::topo::Body;
use std::sync::Arc;

/// Columns and rows of pins.
pub const COLS: usize = 40;
pub const ROWS: usize = 25;
/// The block's thickness at its edges.
pub const THICKNESS: f64 = 0.5;
/// How far the top rises and falls about `THICKNESS`.
pub const WAVE: f64 = 0.04;
/// The pin's mean radius.
pub const RADIUS: f64 = 0.3;

fn seg(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(key),
        curve,
        t0,
        t1,
    }
}

fn line(key: u128, p: Point2, q: Point2) -> Segment {
    seg(
        key,
        Curve2::Line2(Line2::through(p, q).unwrap()),
        0.0,
        p.distance(q),
    )
}

/// The top's height at `y` in the section at `x`, at the control points
/// of its cubic: `THICKNESS` at both ends, waves between.
fn top(x: f64, y: f64) -> f64 {
    let w = ryukei::math::sin(TAU * y / ROWS as f64 * 2.0) * ryukei::math::cos(TAU * x / COLS as f64);
    THICKNESS + WAVE * w
}

/// The section of the block in the plane `x`: the bottom from `y = 0` to
/// `ROWS`, the sides up to `THICKNESS`, and a cubic across the top back.
fn block_section(x: f64) -> ValidProfile {
    let n = 8;
    let w = ROWS as f64;
    let mut pts = Vec::new();
    for i in 0..n {
        let y = w * (1.0 - i as f64 / (n - 1) as f64);
        let z = if i == 0 || i == n - 1 { THICKNESS } else { top(x, y) };
        pts.push(Point2::new(y, z));
    }
    let knots: Vec<f64> = (0..=n - 3).map(|i| i as f64).collect();
    let mut mults = vec![1; knots.len()];
    mults[0] = 4;
    *mults.last_mut().unwrap() = 4;
    let curve = NurbsCurve2::new(KnotVector::clamped(3, &knots, &mults).unwrap(), pts, None).unwrap();
    let t1 = (n - 3) as f64;
    let (a, b, c, d) = (
        Point2::new(0.0, 0.0),
        Point2::new(w, 0.0),
        Point2::new(w, THICKNESS),
        Point2::new(0.0, THICKNESS),
    );
    let l = Loop {
        key: LoopKey(1),
        segments: vec![
            line(1, a, b),
            line(2, b, c),
            seg(3, Curve2::Nurbs2(Arc::new(curve)), 0.0, t1),
            line(4, d, a),
        ],
    };
    ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::new(x, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
        loops: vec![l],
    })
    .unwrap()
}

/// The block: `[0, COLS] × [0, ROWS]`, its top lofted through four
/// sections across `x`.
pub fn block() -> Body {
    let c = COLS as f64;
    let sections: Vec<ValidProfile> = [0.0, c / 3.0, 2.0 * c / 3.0, c]
        .into_iter()
        .map(block_section)
        .collect();
    loft(OpKey(1), &sections, &LoftOptions::default()).unwrap().0
}

/// A wavy closed cubic about the origin: the circle of `RADIUS` pushed out
/// and in three times round, turned by `phase`.
fn wavy(phase: f64) -> Curve2 {
    let n = 12;
    let pts = (0..n)
        .map(|i| {
            let t = TAU * i as f64 / n as f64;
            let r = RADIUS * (1.0 + 0.06 * ryukei::math::cos(3.0 * t + phase));
            let (s, c) = ryukei::math::sin_cos(t);
            Point2::new(r * c, r * s)
        })
        .collect();
    let knots: Vec<f64> = (0..=n).map(|i| i as f64).collect();
    let mults = vec![1; knots.len()];
    let k = KnotVector::periodic(3, &knots, &mults).unwrap();
    Curve2::Nurbs2(Arc::new(NurbsCurve2::new(k, pts, None).unwrap()))
}

/// The pin: four wavy sections from `z0` to `z1`, each turned further.
pub fn pin(z0: f64, z1: f64) -> Body {
    let sections: Vec<ValidProfile> = (0..4)
        .map(|k| {
            let z = z0 + (z1 - z0) * k as f64 / 3.0;
            ValidProfile::new(Profile {
                frame: Frame::new(
                    Point3::new(0.0, 0.0, z),
                    Vector3::new(0.0, 0.0, 1.0),
                    Vector3::new(1.0, 0.0, 0.0),
                )
                .unwrap(),
                loops: vec![Loop {
                    key: LoopKey(1),
                    segments: vec![seg(1, wavy(0.4 * k as f64), 0.0, 12.0)],
                }],
            })
            .unwrap()
        })
        .collect();
    loft(OpKey(2), &sections, &LoftOptions::default()).unwrap().0
}

/// The pin poking out of both faces.
pub fn clear_pin() -> Body {
    pin(-0.25, THICKNESS + 0.25)
}

/// The pin standing on the block's bottom plane, poking out of its top.
pub fn flush_pin() -> Body {
    pin(0.0, THICKNESS + 0.25)
}

/// Where the pins go: the middle of each unit cell, `count` of them in
/// row order.
pub fn places(count: usize) -> Vec<Rigid> {
    (0..ROWS)
        .flat_map(|j| (0..COLS).map(move |i| (i, j)))
        .take(count)
        .map(|(i, j)| Rigid::translation(Vector3::new(i as f64 + 0.5, j as f64 + 0.5, 0.0)))
        .collect()
}
