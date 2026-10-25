//! Assemblies the tests build.

#![allow(dead_code)]

use ryukei::math;
use ryukei_testkit::Lcg;
use ryumate::{Assembly, Body, Connector, Kind, Mate, On, Pose};

/// The freedoms each kind leaves, undriven.
pub fn freedoms(kind: &Kind) -> usize {
    match *kind {
        Kind::Fastened => 0,
        Kind::Revolute { angle } => usize::from(angle.is_none()),
        Kind::Slider { distance } => usize::from(distance.is_none()),
        Kind::Cylindrical { angle, distance } => {
            usize::from(angle.is_none()) + usize::from(distance.is_none())
        }
        Kind::Planar { angle, .. } => 2 + usize::from(angle.is_none()),
        Kind::Ball => 3,
    }
}

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

/// A translation.
pub fn moved(by: [f64; 3]) -> Pose {
    Pose {
        origin: by,
        ..Pose::IDENTITY
    }
}

/// A kind drawn at random, its driven values (if any) within `size`, and
/// a motion it allows with those values: where `b`'s frame may be in
/// `a`'s.
pub fn kind(g: &mut Lcg, size: f64) -> (Kind, Pose) {
    let z = [0.0, 0.0, 1.0];
    let maybe = |g: &mut Lcg, v: f64| g.next_u64().is_multiple_of(2).then_some(v);
    let angle = g.next_f64(-3.0, 3.0);
    let d = g.next_f64(-size, size);
    match g.next_u64() % 6 {
        0 => (Kind::Fastened, Pose::IDENTITY),
        1 => {
            let a = maybe(g, angle);
            (Kind::Revolute { angle: a }, turned(z, angle, [0.0; 3]))
        }
        2 => {
            let s = maybe(g, d);
            (Kind::Slider { distance: s }, moved([0.0, 0.0, d]))
        }
        3 => {
            let (a, s) = (maybe(g, angle), maybe(g, d));
            (
                Kind::Cylindrical {
                    angle: a,
                    distance: s,
                },
                turned(z, angle, [0.0, 0.0, d]),
            )
        }
        4 => {
            let a = maybe(g, angle);
            let (x, y) = (g.next_f64(-size, size), g.next_f64(-size, size));
            (
                Kind::Planar {
                    angle: a,
                    distance: d,
                },
                turned(z, angle, [x, y, d]),
            )
        }
        _ => {
            let n = direction(g);
            (Kind::Ball, turned(n, angle, [0.0; 3]))
        }
    }
}

/// A random tree of `n` bodies within about `size` of the origin: body 0
/// fixed, each later one mated to an earlier one by a random kind between
/// random frames, placed where its mate holds. Mate `k` puts body `k` on
/// its parent. Its total freedom is the sum of what its mates leave.
pub fn mechanism(g: &mut Lcg, n: u32, size: f64) -> (Assembly, usize) {
    let mut asm = Assembly::default();
    asm.bodies.insert(0, fixed(pose(g, size)));
    let mut dof = 0;
    for k in 1..n {
        let parent = (g.next_u64() % u64::from(k)) as u32;
        let (a, b) = (pose(g, size / 4.0), pose(g, size / 4.0));
        let (kind, motion) = kind(g, size / 4.0);
        dof += freedoms(&kind);
        let place = asm.bodies[&parent]
            .pose
            .then_after(&a)
            .then_after(&motion)
            .then_after(&b.inverse());
        asm.bodies.insert(k, free(place));
        asm.mates.insert(
            k,
            Mate {
                kind,
                a: on(parent, a),
                b: on(k, b),
            },
        );
    }
    (asm, dof)
}

/// Every free body of `asm` nudged by up to `angle` and `by`.
pub fn jostle(asm: &mut Assembly, g: &mut Lcg, angle: f64, by: f64) {
    for b in asm.bodies.values_mut() {
        if !b.fixed {
            b.pose = nudged(g, &b.pose, angle, by);
        }
    }
}
