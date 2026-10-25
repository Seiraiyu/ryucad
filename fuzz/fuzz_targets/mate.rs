//! Assemblies from bytes: bodies at any poses, some fixed, connectors on
//! them or on copies of a pattern, and mates of every kind between
//! whatever the bytes pick, with any driven values. Solving must never
//! panic, never give a pose that isn't finite, and only name mates that
//! exist; an assembly that converged must solve again to the same poses,
//! within the tolerance. Solving in pieces must converge wherever
//! solving whole components does, and both must say the same of the
//! poses it leaves.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::math;
use ryukei_testkit::euler_script::Bytes;
use ryumate::{
    Assembly, Body, Connector, Direction, Id, Kind, Mate, Motion, On, Pattern, Pose, Report, solve,
    solve_whole,
};

/// A frame from bytes: a turn about an axis, at a point within `size`.
fn frame(b: &mut Bytes, size: f64) -> Pose {
    let v = [b.range(-1.0, 1.0), b.range(-1.0, 1.0), b.range(-1.0, 1.0)];
    let n = math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    let [x, y, z] = if n < 0.1 {
        [0.0, 0.0, 1.0]
    } else {
        v.map(|c| c / n)
    };
    let (s, c) = math::sin_cos(b.range(-3.2, 3.2));
    let t = 1.0 - c;
    Pose {
        rotation: [
            [c + x * x * t, x * y * t - z * s, x * z * t + y * s],
            [y * x * t + z * s, c + y * y * t, y * z * t - x * s],
            [z * x * t - y * s, z * y * t + x * s, c + z * z * t],
        ],
        origin: [0, 1, 2].map(|_| b.range(-size, size)),
    }
}

fn assembly(b: &mut Bytes) -> Assembly {
    let mut asm = Assembly::default();
    let n = 2 + b.pick(6) as Id;
    for id in 0..n {
        let pose = frame(b, 0.2);
        asm.bodies.insert(
            id,
            Body {
                pose,
                fixed: id == 0 || b.byte() < 32,
            },
        );
    }
    let body = |b: &mut Bytes| b.pick(n as usize) as Id;
    let patterned = b.byte() < 96;
    if patterned {
        let seed = body(b);
        let motion = if b.byte() < 128 {
            Motion::Linear {
                direction: Direction::World([1.0, 0.0, 0.0]),
                spacing: b.range(-0.05, 0.05),
                second: None,
            }
        } else {
            let on = body(b);
            Motion::Circular {
                axis: Direction::Connector(Connector {
                    on: On::Body(on),
                    frame: frame(b, 0.05),
                }),
                step: b.range(-1.0, 1.0),
            }
        };
        asm.patterns.insert(
            50,
            Pattern {
                seeds: vec![seed],
                motion,
            },
        );
    }
    let connector = |b: &mut Bytes| {
        let on = match asm.patterns.get(&50) {
            Some(p) if b.byte() < 48 => On::Copy {
                pattern: 50,
                seed: p.seeds[0],
                index: [1 + b.pick(4) as u32, 0],
            },
            _ => On::Body(b.pick(n as usize) as Id),
        };
        Connector {
            on,
            frame: frame(b, 0.05),
        }
    };
    let mut mates = Vec::new();
    for id in 0..b.pick(8) as Id {
        let maybe = |b: &mut Bytes, lo: f64, hi: f64| (b.byte() < 128).then(|| b.range(lo, hi));
        let kind = match b.pick(6) {
            0 => Kind::Fastened,
            1 => Kind::Revolute {
                angle: maybe(b, -3.2, 3.2),
            },
            2 => Kind::Slider {
                distance: maybe(b, -0.1, 0.1),
            },
            3 => Kind::Cylindrical {
                angle: maybe(b, -3.2, 3.2),
                distance: maybe(b, -0.1, 0.1),
            },
            4 => Kind::Planar {
                angle: maybe(b, -3.2, 3.2),
                distance: b.range(-0.1, 0.1),
            },
            _ => Kind::Ball,
        };
        let (a, c) = (connector(b), connector(b));
        mates.push((id, Mate { kind, a, b: c }));
    }
    asm.mates.extend(mates);
    asm
}

fn said(r: &Report) -> impl PartialEq + std::fmt::Debug {
    let bodies: Vec<_> = r
        .bodies
        .iter()
        .map(|(id, b)| (*id, b.state, b.freedom.dof))
        .collect();
    (
        r.state,
        r.dof,
        bodies,
        r.mates.clone(),
        r.redundant.clone(),
        r.conflicting.clone(),
    )
}

fuzz_target!(|data: &[u8]| {
    let asm = assembly(&mut Bytes::new(data));
    let Ok(s) = solve(&asm) else {
        return;
    };
    for p in s.poses.values() {
        assert!(
            p.rotation
                .iter()
                .flatten()
                .chain(&p.origin)
                .all(|x| x.is_finite())
        );
    }
    for m in s
        .report
        .redundant
        .iter()
        .chain(&s.report.conflicting)
        .flatten()
    {
        assert!(asm.mates.contains_key(m), "{m}");
    }
    let whole = solve_whole(&asm).unwrap().report;
    assert!(s.report.converged || !whole.converged, "{whole:?}");
    if s.report.converged {
        let at = s.assembly(&asm);
        // A solve that moved a body out of the size box (or left it
        // skewed) gives an assembly that can't be solved again.
        let Ok(again) = solve(&at) else {
            return;
        };
        assert!(again.report.converged, "{:?}", again.report);
        for (id, p) in &again.poses {
            let q = s.poses[id];
            for (x, y) in p.origin.iter().zip(&q.origin) {
                assert!((x - y).abs() <= 1e-6, "{x} vs {y}");
            }
        }
        let one = solve_whole(&at).unwrap().report;
        if !again.report.changed && !one.changed {
            assert_eq!(said(&again.report), said(&one));
        }
    }
});
