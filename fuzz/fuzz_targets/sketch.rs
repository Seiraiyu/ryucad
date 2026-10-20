//! Sketches from bytes: points, lines, arcs and circles on them, and
//! constraints of every kind between whatever the bytes pick, with any
//! values. Solving must never panic, never give a position that isn't
//! finite, and only name constraints that exist; a sketch that converged
//! must solve again to the same positions, within the tolerance.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei_testkit::euler_script::Bytes;
use ryusketch::{Constraint, Entity, Id, Sketch, solve};

fn sketch(b: &mut Bytes) -> Sketch {
    let mut s = Sketch::default();
    let points = 2 + b.pick(8);
    for id in 0..points as Id {
        let at = [b.range(-2.0, 2.0), b.range(-2.0, 2.0)];
        s.entities.insert(
            id,
            Entity::Point {
                at,
                fixed: b.byte() < 32,
            },
        );
    }
    let p = |b: &mut Bytes| b.pick(points) as Id;
    for id in 100..100 + b.pick(6) as Id {
        let construction = b.byte() < 64;
        let e = match b.pick(3) {
            0 => Entity::Line {
                a: p(b),
                b: p(b),
                construction,
            },
            1 => Entity::Arc {
                center: p(b),
                start: p(b),
                end: p(b),
                construction,
            },
            _ => Entity::Circle {
                center: p(b),
                r: b.range(0.0, 2.0),
                fixed: b.byte() < 32,
                construction,
            },
        };
        s.entities.insert(id, e);
    }
    let ids: Vec<Id> = s.entities.keys().copied().collect();
    let mut e = |b: &mut Bytes| b.one(&ids).unwrap_or(0);
    for id in 0..b.pick(12) as Id {
        let v = b.range(-0.5, 3.0);
        let c = match b.pick(19) {
            0 => Constraint::Coincident(e(b), e(b)),
            1 => Constraint::Horizontal(e(b)),
            2 => Constraint::Vertical(e(b)),
            3 => Constraint::Parallel(e(b), e(b)),
            4 => Constraint::Perpendicular(e(b), e(b)),
            5 => Constraint::Tangent(e(b), e(b)),
            6 => Constraint::Equal(e(b), e(b)),
            7 => Constraint::Symmetric(e(b), e(b), e(b)),
            8 => Constraint::Midpoint(e(b), e(b)),
            9 => Constraint::On(e(b), e(b)),
            10 => Constraint::Concentric(e(b), e(b)),
            11 => Constraint::Fix(e(b)),
            12 => Constraint::Distance(e(b), e(b), v),
            13 => Constraint::DistanceX(e(b), e(b), v),
            14 => Constraint::DistanceY(e(b), e(b), v),
            15 => Constraint::Length(e(b), v),
            16 => Constraint::Angle(e(b), e(b), v),
            17 => Constraint::Radius(e(b), v),
            _ => Constraint::Diameter(e(b), v),
        };
        s.constraints.insert(id, c);
    }
    s
}

fn positions(s: &Sketch) -> Vec<f64> {
    s.entities
        .values()
        .flat_map(|e| match *e {
            Entity::Point { at, .. } => at.to_vec(),
            Entity::Circle { r, .. } => vec![r],
            _ => vec![],
        })
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let s = sketch(&mut Bytes::new(data));
    let Ok(r) = solve(&s) else {
        return;
    };
    assert!(positions(&r.sketch).iter().all(|x| x.is_finite()));
    let named = r
        .report
        .redundant
        .iter()
        .chain(&r.report.conflicting)
        .flatten();
    for c in named {
        assert!(s.constraints.contains_key(c), "{c}");
    }
    if r.report.converged {
        let again = solve(&r.sketch).unwrap();
        assert!(again.report.converged, "{:?}", again.report);
        for (a, b) in positions(&r.sketch).iter().zip(positions(&again.sketch)) {
            assert!((a - b).abs() <= 1e-6, "{a} vs {b}");
        }
    }
});
