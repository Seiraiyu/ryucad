//! Bit-exact determinism check. Every CI platform must reproduce the hashes
//! in `tests/golden/determinism.txt`. After a deliberate change, regenerate
//! with `UPDATE_GOLDEN=1 cargo test -p ryukei --test determinism` and review
//! the diff like any other change.

use ryukei::geom::{
    Affine, Box3, Circle, Cone, Curve, Cylinder, Frame, Line, Plane, Point2, Point3, Rigid, Sphere,
    Surface, Torus, Vector3,
};
use ryukei::interval::Interval;
use ryukei::math;
use ryukei::name::{Event, OpKey};
use ryukei::predicates::{self, Sign};
use ryukei::primitive;
use ryukei_testkit::euler_script::{Bytes, start, valid_step};
use ryukei_testkit::{Golden, Lcg};

fn math_section(golden: &mut Golden) {
    let mut s = golden.section("math");
    let mut g = Lcg::new(0x6d61_7468);
    for _ in 0..4096 {
        let x = g.next_f64(-1.0e3, 1.0e3);
        let y = g.next_f64(-1.0e3, 1.0e3);
        let p = g.next_f64(1.0e-6, 1.0e3);
        let u = g.next_f64(-1.0, 1.0);
        for v in [
            math::sqrt(p),
            math::fma(x, y, p),
            math::sin(x),
            math::cos(x),
            math::sin_cos(x).0,
            math::sin_cos(x).1,
            math::tan(x),
            math::asin(u),
            math::acos(u),
            math::atan2(y, x),
            math::exp(u * 700.0),
            math::ln(p),
            math::pow(p, u * 10.0),
            math::hypot(x, y),
            math::cbrt(x),
        ] {
            s.f64(v);
        }
    }
}

fn geom_section(golden: &mut Golden) {
    let mut s = golden.section("geom");
    let mut g = Lcg::new(0x6765_6f6d);
    let mut chain = Rigid::IDENTITY;
    for _ in 0..1024 {
        let mut p = || {
            Point3::new(
                g.next_f64(-500.0, 500.0),
                g.next_f64(-500.0, 500.0),
                g.next_f64(-500.0, 500.0),
            )
        };
        let (a, b, c) = (p(), p(), p());
        let angle = g.next_f64(-10.0, 10.0);
        let step = match (b - a).normalize() {
            Some(axis) => {
                Rigid::translation(c.to_vector() * 1e-3).then_after(&Rigid::rotation(axis, angle))
            }
            None => Rigid::IDENTITY,
        };
        chain = step.then_after(&chain);
        let q = chain.apply_point(a);
        let v = (b - a).cross(c - a);
        let n = v
            .normalize()
            .map_or(Vector3::ZERO, |u| chain.apply_unit(u).get());
        let aff = Affine::scale(Vector3::new(angle, 2.0, -0.5)).then_after(&Affine::from(chain));
        let r = aff.inverse().map_or(Point3::ORIGIN, |i| i.apply_point(q));
        let bx = Box3::new(a, b).transform_rigid(&chain);
        for x in [
            q.x,
            q.y,
            q.z,
            v.x,
            v.y,
            v.z,
            n.x,
            n.y,
            n.z,
            r.x,
            r.y,
            r.z,
            bx.min().x,
            bx.max().z,
            a.distance(c),
            aff.determinant(),
        ] {
            s.f64(x);
        }
    }
    chain = chain.reorthonormalize();
    for row in chain.rotation_matrix() {
        for x in row {
            s.f64(x);
        }
    }
}

fn sign_bits(s: Sign) -> u64 {
    match s {
        Sign::Negative => 0,
        Sign::Zero => 1,
        Sign::Positive => 2,
    }
}

fn predicates_section(golden: &mut Golden) {
    let mut s = golden.section("predicates");
    let mut g = Lcg::new(0x7072_6564);
    for _ in 0..1024 {
        let mut c = || g.next_f64(-500.0, 500.0);
        let (a, b, d) = (
            Point3::new(c(), c(), c()),
            Point3::new(c(), c(), c()),
            Point3::new(c(), c(), c()),
        );
        let (t, u) = (c() / 250.0, c() / 250.0);
        // Near-degenerate: on the plane/line through a, b, d up to rounding.
        let on_plane = a + (b - a) * t + (d - a) * u;
        let on_line = Point2::new(a.x, a.y).lerp(Point2::new(b.x, b.y), t);
        let e = Point3::new(c(), c(), c());
        for sign in [
            predicates::orient2d(Point2::new(a.x, a.y), Point2::new(b.x, b.y), on_line),
            predicates::orient3d(a, b, d, on_plane),
            predicates::orient3d(a, b, d, e),
            predicates::incircle(
                Point2::new(a.x, a.y),
                Point2::new(b.x, b.y),
                Point2::new(d.x, d.y),
                on_line,
            ),
            predicates::insphere(a, b, d, e, on_plane),
        ] {
            s.u64(sign_bits(sign));
        }
    }
}

fn interval_section(golden: &mut Golden) {
    let mut s = golden.section("interval");
    let mut g = Lcg::new(0x6976_6c73);
    for _ in 0..1024 {
        let mut iv = || {
            let (a, b) = (g.next_f64(-1.0e3, 1.0e3), g.next_f64(-1.0e3, 1.0e3));
            Interval::new(a.min(b), a.max(b)).unwrap()
        };
        let (a, b, c) = (iv(), iv(), iv());
        let r = (a + b) * c - a;
        let q = r.checked_div(b).unwrap_or(r);
        let root = (a * a + b * b).sqrt().unwrap();
        for x in [r.lo(), r.hi(), q.lo(), q.hi(), root.lo(), root.hi()] {
            s.f64(x);
        }
    }
}

fn analytic_section(golden: &mut Golden) {
    let mut s = golden.section("analytic");
    let mut g = Lcg::new(0x616e_6c79);
    for _ in 0..256 {
        let mut c = || g.next_f64(-10.0, 10.0);
        let (o, z, x) = (
            Point3::new(c(), c(), c()),
            Vector3::new(c(), c(), c()),
            Vector3::new(c(), c(), c()),
        );
        let p = Point3::new(c(), c(), c());
        let (u, v) = (c(), c() / 10.0);
        let (r, k, a) = (
            c().abs() + 0.5,
            (c().abs() + 1.0) / 12.0,
            (c().abs() + 1.0) / 8.0,
        );
        let f = Frame::new(o, z, x).unwrap();
        let curves = [
            Curve::Line(Line::new(o, z).unwrap()),
            Curve::Circle(Circle::new(f, r).unwrap()),
        ];
        for cv in curves {
            let q = cv.point(u);
            let t = cv.tangent(u).unwrap().get();
            for x in [
                q.x,
                q.y,
                q.z,
                t.x,
                t.y,
                t.z,
                cv.distance(p),
                cv.project(p).param,
            ] {
                s.f64(x);
            }
        }
        let surfaces = [
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, r).unwrap()),
            Surface::Cone(Cone::new(f, r, a).unwrap()),
            Surface::Sphere(Sphere::new(f, r).unwrap()),
            Surface::Torus(Torus::new(f, r, r * k).unwrap()),
        ];
        for sf in surfaces {
            let q = sf.point(u, v);
            let n = sf.normal(u, v).unwrap().get();
            let (pu, pv) = sf.project(p).param;
            for x in [q.x, q.y, q.z, n.x, n.y, n.z, sf.distance(p), pu, pv] {
                s.f64(x);
            }
        }
    }
}

fn primitives_section(golden: &mut Golden) {
    let mut s = golden.section("primitives");
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    let placements = [
        Rigid::IDENTITY,
        Rigid::translation(Vector3::new(12.5, -3.25, 100.0))
            .then_after(&Rigid::rotation(axis, 2.5)),
    ];
    for (i, at) in placements.iter().enumerate() {
        let key = OpKey(0x7072_696d + i as u128);
        let bodies = [
            primitive::block(key, 1.5, 2.25, 0.75, at),
            primitive::cylinder(key, 0.5, 3.0, at),
            primitive::cone(key, 1.25, 2.0, at),
            primitive::frustum(key, 2.0, 0.5, 1.0, at),
            primitive::sphere(key, 4.0, at),
            primitive::torus(key, 3.0, 0.75, at),
        ];
        for result in bodies {
            let (body, journal) = result.unwrap();
            s.str(&body.dump());
            for e in journal.events() {
                if let Event::Created(n) = e {
                    s.u64(n.to_u128() as u64);
                    s.u64((n.to_u128() >> 64) as u64);
                }
            }
        }
    }
}

fn euler_section(golden: &mut Golden) {
    let mut s = golden.section("euler");
    let mut g = Lcg::new(0x6575_6c72);
    let data: Vec<u8> = (0..4000).map(|_| (g.next_u64() >> 56) as u8).collect();
    let mut body = start();
    let mut bytes = Bytes::new(&data);
    let mut n = 0u128;
    while !bytes.is_empty() {
        n += 1;
        if let Some(step) = valid_step(&mut body, OpKey(1000 + n), &mut bytes) {
            s.str(step.op);
            for e in step.journal.events() {
                let (tag, names) = match *e {
                    Event::Created(n) => (0, vec![n]),
                    Event::Deleted(n) => (1, vec![n]),
                    Event::Modified(n) => (2, vec![n]),
                    Event::Split { parent, children } => {
                        (3, vec![parent, children[0], children[1]])
                    }
                    Event::Merged { parents, child } => (4, vec![parents[0], parents[1], child]),
                };
                s.u64(tag);
                for n in names {
                    s.u64(n.to_u128() as u64);
                    s.u64((n.to_u128() >> 64) as u64);
                }
            }
        }
    }
    s.str(&body.dump());
}

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/determinism.txt"
    ));
    math_section(&mut golden);
    geom_section(&mut golden);
    predicates_section(&mut golden);
    interval_section(&mut golden);
    analytic_section(&mut golden);
    primitives_section(&mut golden);
    euler_section(&mut golden);
    golden.finish();
}
