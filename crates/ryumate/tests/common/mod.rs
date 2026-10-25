//! Assemblies the tests build.

#![allow(dead_code)]

use ryukei::math;
use ryukei_testkit::Lcg;
use ryumate::{Assembly, Body, Connector, Kind, Mate, On, Pose};

/// The rotation by `angle` about the unit axis `n`, as a pose at `origin`.
pub fn turned(n: [f64; 3], angle: f64, origin: [f64; 3]) -> Pose {
    let (s, c) = math::sin_cos(angle);
    let t = 1.0 - c;
    let [x, y, z] = n;
    Pose {
        rotation: [
            [c + x * x * t, x * y * t - z * s, x * z * t + y * s],
            [y * x * t + z * s, c + y * y * t, y * z * t - x * s],
            [z * x * t - y * s, z * y * t + x * s, c + z * z * t],
        ],
        origin,
    }
}

/// A unit vector from three draws.
pub fn direction(g: &mut Lcg) -> [f64; 3] {
    loop {
        let v = [
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
        ];
        let n = math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
        if n > 0.1 {
            return v.map(|c| c / n);
        }
    }
}

/// A random pose within `size` of the origin.
pub fn pose(g: &mut Lcg, size: f64) -> Pose {
    let n = direction(g);
    let a = g.next_f64(-3.0, 3.0);
    turned(n, a, [0, 1, 2].map(|_| g.next_f64(-size, size)))
}

/// `p` nudged: turned by up to `angle` and moved by up to `by`.
pub fn nudged(g: &mut Lcg, p: &Pose, angle: f64, by: f64) -> Pose {
    let n = direction(g);
    let r = turned(n, g.next_f64(-angle, angle), [0.0; 3]);
    let mut out = r.then_after(p);
    for k in 0..3 {
        out.origin[k] = p.origin[k] + g.next_f64(-by, by);
    }
    out
}

pub fn free(pose: Pose) -> Body {
    Body { pose, fixed: false }
}

pub fn fixed(pose: Pose) -> Body {
    Body { pose, fixed: true }
}

pub fn on(body: u32, frame: Pose) -> Connector {
    Connector {
        on: On::Body(body),
        frame,
    }
}

/// Body 1 fixed at the origin, body 2 free at `at`, and mate 10 of `kind`
/// between a frame on each.
pub fn pair(kind: Kind, a: Pose, b: Pose, at: Pose) -> Assembly {
    let mut asm = Assembly::default();
    asm.bodies.insert(1, fixed(Pose::IDENTITY));
    asm.bodies.insert(2, free(at));
    asm.mates.insert(
        10,
        Mate {
            kind,
            a: on(1, a),
            b: on(2, b),
        },
    );
    asm
}

/// Connector `c`'s frame in the assembly, with body poses from `asm`.
pub fn world(asm: &Assembly, c: &Connector) -> Pose {
    match c.on {
        On::Body(b) => asm.bodies[&b].pose.then_after(&c.frame),
        On::Copy {
            pattern,
            seed,
            index,
        } => ryumate::copy_poses(asm, &[(pattern, seed, index)])[0].then_after(&c.frame),
    }
}

pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Whether two directions agree within `tol`.
pub fn same(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= tol)
}
