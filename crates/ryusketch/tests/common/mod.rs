//! Random sketches for the tests: polygons measured from a random shape,
//! then jostled, so the constraints agree and the solver has work to do.

#![allow(dead_code)]

use ryukei::math;
use ryukei_testkit::Lcg;
use ryusketch::{Constraint, Entity, Id, Sketch};

pub fn at(s: &Sketch, id: Id) -> [f64; 2] {
    match s.entities[&id] {
        Entity::Point { at, .. } => at,
        _ => panic!("{id} is not a point"),
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    math::sqrt(dx * dx + dy * dy)
}

/// The angle from `b - a` to `d - c`.
fn angle(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let (u, w) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
    math::atan2(u[0] * w[1] - u[1] * w[0], u[0] * w[0] + u[1] * w[1])
}

/// The first side's ID: side `k` (from point `k + 1` to the next) is
/// `SIDES + k`.
pub const SIDES: Id = 100_000;
/// The fixed construction line along x, and its points.
pub const AXIS: [Id; 3] = [200_000, 200_001, 200_002];

/// A convex polygon of `n` points (IDs 1..=n) and sides (from [`SIDES`]),
/// dimensioned exactly: its first point fixed, every side's length, the
/// first side's angle to a fixed construction line along x ([`AXIS`]), and
/// the angles between sides 1..n-3 and the next. Constraint IDs count from
/// 1.
pub fn polygon(g: &mut Lcg, n: usize, size: f64) -> Sketch {
    let mut s = Sketch::default();
    let step = 2.0 * core::f64::consts::PI / n as f64;
    let pts: Vec<[f64; 2]> = (0..n)
        .map(|k| {
            let t = step * (k as f64 + g.next_f64(-0.3, 0.3));
            let r = size * g.next_f64(0.6, 1.0);
            [r * math::cos(t), r * math::sin(t)]
        })
        .collect();
    for (k, p) in pts.iter().enumerate() {
        s.entities.insert(
            k as Id + 1,
            Entity::Point {
                at: *p,
                fixed: false,
            },
        );
    }
    let side = |k: usize| SIDES + k as Id;
    for k in 0..n {
        s.entities.insert(
            side(k),
            Entity::Line {
                a: k as Id + 1,
                b: ((k + 1) % n) as Id + 1,
                construction: false,
            },
        );
    }
    let [axis, from, to] = AXIS;
    for (id, x) in [(from, 0.0), (to, 1.0)] {
        s.entities.insert(
            id,
            Entity::Point {
                at: [x, 0.0],
                fixed: true,
            },
        );
    }
    s.entities.insert(
        axis,
        Entity::Line {
            a: from,
            b: to,
            construction: true,
        },
    );
    let mut c = 1;
    let mut add = |s: &mut Sketch, k: Constraint| {
        s.constraints.insert(c, k);
        c += 1;
    };
    add(&mut s, Constraint::Fix(1));
    for k in 0..n {
        add(
            &mut s,
            Constraint::Length(side(k), dist(pts[k], pts[(k + 1) % n])),
        );
    }
    add(
        &mut s,
        Constraint::Angle(axis, side(0), angle([0.0, 0.0], [1.0, 0.0], pts[0], pts[1])),
    );
    for k in 0..n.saturating_sub(3) {
        let v = angle(pts[k], pts[k + 1], pts[k + 1], pts[k + 2]);
        add(&mut s, Constraint::Angle(side(k), side(k + 1), v));
    }
    s
}

/// A slot of length `l` and width `w` with its ends rounded: two lines and
/// two arcs, tangent at their shared ends, the arcs of equal radius.
pub fn slot(l: f64, w: f64) -> Sketch {
    let mut s = Sketch::default();
    let h = w / 2.0;
    for (id, x, y) in [
        (1, 0.0, -h),
        (2, l, -h),
        (3, l, h),
        (4, 0.0, h),
        (5, l, 0.0),
        (6, 0.0, 0.0),
    ] {
        s.entities.insert(
            id,
            Entity::Point {
                at: [x, y],
                fixed: false,
            },
        );
    }
    let line = |a, b| Entity::Line {
        a,
        b,
        construction: false,
    };
    s.entities.insert(10, line(1, 2));
    s.entities.insert(
        11,
        Entity::Arc {
            center: 5,
            start: 2,
            end: 3,
            construction: false,
        },
    );
    s.entities.insert(12, line(3, 4));
    s.entities.insert(
        13,
        Entity::Arc {
            center: 6,
            start: 4,
            end: 1,
            construction: false,
        },
    );
    for (id, c) in [
        (1, Constraint::Fix(6)),
        (2, Constraint::Horizontal(10)),
        (3, Constraint::Tangent(10, 11)),
        (4, Constraint::Tangent(11, 12)),
        (5, Constraint::Tangent(12, 13)),
        (6, Constraint::Tangent(13, 10)),
        (7, Constraint::Equal(11, 13)),
        (8, Constraint::Length(10, l)),
        (9, Constraint::Radius(11, h)),
    ] {
        s.constraints.insert(id, c);
    }
    s
}

/// Moves every free point by up to `amount` each way.
pub fn jostle(s: &mut Sketch, g: &mut Lcg, amount: f64) {
    for e in s.entities.values_mut() {
        if let Entity::Point { at, fixed: false } = e {
            at[0] += g.next_f64(-amount, amount);
            at[1] += g.next_f64(-amount, amount);
        }
    }
}
