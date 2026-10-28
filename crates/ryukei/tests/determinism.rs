//! Bit-exact determinism check. Every CI platform must reproduce the hashes
//! in `tests/golden/determinism.txt`. After a deliberate change, regenerate
//! with `UPDATE_GOLDEN=1 cargo test -p ryukei --test determinism` and review
//! the diff like any other change.

use ryukei::geom::{
    Affine, Axis, Box3, Circle, Circle2, Cone, Curve, Curve2, Cylinder, Ellipse, Ellipse2, Frame,
    Frame2, Line, Line2, Plane, Point2, Point3, Rigid, Sphere, Surface, Torus, Vector2, Vector3,
};
use ryukei::interval::Interval;
use ryukei::math;
use ryukei::mesh::{BodyMesh, MeshParams, tessellate};
use ryukei::name::{Event, OpKey};
use ryukei::predicates::{self, Sign};
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use ryukei_testkit::euler_script::{Bytes, start, valid_step};
use ryukei_testkit::{Golden, Lcg, mesh, nurbs};

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

fn derivatives_section(golden: &mut Golden) {
    let mut s = golden.section("derivatives");
    let mut g = Lcg::new(0x6465_7276);
    for _ in 0..256 {
        let mut c = || g.next_f64(-10.0, 10.0);
        let (o, z, x) = (
            Point3::new(c(), c(), c()),
            Vector3::new(c(), c(), c()),
            Vector3::new(c(), c(), c()),
        );
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
            let e = cv.eval(u);
            let b = cv.bounds([u, u + r]);
            for p in [e.point, b.min(), b.max()] {
                s.f64(p.x);
                s.f64(p.y);
                s.f64(p.z);
            }
            for w in [e.d1, e.d2] {
                s.f64(w.x);
                s.f64(w.y);
                s.f64(w.z);
            }
            s.f64(cv.curvature(u).unwrap());
        }
        let surfaces = [
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, r).unwrap()),
            Surface::Cone(Cone::new(f, r, a).unwrap()),
            Surface::Sphere(Sphere::new(f, r).unwrap()),
            Surface::Torus(Torus::new(f, r, r * k).unwrap()),
        ];
        for sf in surfaces {
            let e = sf.eval(u, v);
            let b = sf.bounds();
            for p in [e.point, b.min(), b.max()] {
                s.f64(p.x);
                s.f64(p.y);
                s.f64(p.z);
            }
            for w in [e.du, e.dv, e.duu, e.duv, e.dvv] {
                s.f64(w.x);
                s.f64(w.y);
                s.f64(w.z);
            }
            match sf.principal_curvatures(u, v) {
                Some(k) => {
                    s.f64(k.k1);
                    s.f64(k.k2);
                    for w in [k.dir1.get(), k.dir2.get()] {
                        s.f64(w.x);
                        s.f64(w.y);
                        s.f64(w.z);
                    }
                }
                None => s.u64(0),
            }
        }
    }
}

fn ellipse_section(golden: &mut Golden) {
    let mut s = golden.section("ellipse");
    let mut g = Lcg::new(0x656c_6c70);
    for _ in 0..512 {
        let mut c = || g.next_f64(-10.0, 10.0);
        let (o, z, x) = (
            Point3::new(c(), c(), c()),
            Vector3::new(c(), c(), c()),
            Vector3::new(c(), c(), c()),
        );
        let a = c().abs() + 0.5;
        let k = (c().abs() + 0.1) / 10.1;
        let e = Curve::Ellipse(Ellipse::new(Frame::new(o, z, x).unwrap(), a, a * k).unwrap());
        // Query points near the centre, where the solver works hardest.
        let p = o + Vector3::new(c(), c(), c()) * (a / 10.0);
        let t = c();
        let ev = e.eval(t);
        let foot = e.project(p);
        let b = e.bounds([0.0, 1.0]);
        for q in [ev.point, foot.point, b.min(), b.max()] {
            s.f64(q.x);
            s.f64(q.y);
            s.f64(q.z);
        }
        for w in [ev.d1, ev.d2] {
            s.f64(w.x);
            s.f64(w.y);
            s.f64(w.z);
        }
        s.f64(foot.param);
        s.f64(foot.distance);
        s.f64(e.curvature(t).unwrap());
    }
}

fn conversion_section(golden: &mut Golden) {
    let mut s = golden.section("conversion");
    let mut g = Lcg::new(0x636f_6e76);
    for _ in 0..64 {
        let mut c = || g.next_f64(-10.0, 10.0);
        let (o, z, x) = (
            Point3::new(c(), c(), c()),
            Vector3::new(c(), c(), c()),
            Vector3::new(c(), c(), c()),
        );
        let (r, k, a) = (
            c().abs() + 0.5,
            (c().abs() + 1.0) / 12.0,
            (c().abs() + 1.0) / 8.0,
        );
        let (t, h) = (c(), c().abs() + 0.1);
        let f = Frame::new(o, z, x).unwrap();
        let curves = [
            Curve::Line(Line::new(o, z).unwrap()),
            Curve::Circle(Circle::new(f, r).unwrap()),
            Curve::Ellipse(Ellipse::new(f, r, r * k).unwrap()),
        ];
        for cv in curves {
            let (n, rp) = cv.to_nurbs([t, t + h]).unwrap();
            for p in n.control_points() {
                s.f64(p.x);
                s.f64(p.y);
                s.f64(p.z);
            }
            for &w in n.weights() {
                s.f64(w);
            }
            s.f64(rp.to_nurbs(t));
            s.f64(rp.from_nurbs(t));
        }
        let cone = Cone::new(f, r, a).unwrap();
        let surfaces = [
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, r).unwrap()),
            Surface::Cone(cone),
            Surface::Sphere(Sphere::new(f, r).unwrap()),
            Surface::Torus(Torus::new(f, r, r * k).unwrap()),
        ];
        let v0 = cone.apex_v().max(t);
        for sf in surfaces {
            let (n, _) = sf.to_nurbs([[t, t + h], [v0, v0 + h]]).unwrap();
            for p in n.control_points() {
                s.f64(p.x);
                s.f64(p.y);
                s.f64(p.z);
            }
            for &w in n.weights() {
                s.f64(w);
            }
        }
    }
}

fn projection_section(golden: &mut Golden) {
    let mut s = golden.section("projection");
    let mut g = Lcg::new(0x7072_6f6a);
    let point = |g: &mut Lcg| {
        Point3::new(
            g.next_f64(-8.0, 8.0),
            g.next_f64(-8.0, 8.0),
            g.next_f64(-8.0, 8.0),
        )
    };
    for _ in 0..64 {
        let c = nurbs::any_curve(&mut g);
        for _ in 0..4 {
            let p = point(&mut g);
            let foot = c.project(p);
            for x in [
                foot.param,
                foot.point.x,
                foot.point.y,
                foot.point.z,
                foot.distance,
            ] {
                s.f64(x);
            }
            s.u64(c.local_minima(p).len() as u64);
        }
    }
    for _ in 0..16 {
        let sf = nurbs::any_surface(&mut g);
        for _ in 0..2 {
            let p = point(&mut g);
            let foot = sf.project(p);
            let (u, v) = foot.param;
            for x in [
                u,
                v,
                foot.point.x,
                foot.point.y,
                foot.point.z,
                foot.distance,
            ] {
                s.f64(x);
            }
        }
    }
}

fn pcurves_section(golden: &mut Golden) {
    let mut s = golden.section("pcurves");
    let mut g = Lcg::new(0x7063_7276);
    for _ in 0..128 {
        let mut c = || g.next_f64(-10.0, 10.0);
        let (o, x) = (Point2::new(c(), c()), Vector2::new(c(), c()));
        let (r, k, t) = (c().abs() + 0.5, (c().abs() + 1.0) / 12.0, c());
        let p = Point2::new(c(), c());
        let f = Frame2::new(o, x, t > 0.0).unwrap();
        let curves = [
            Curve2::Line2(Line2::new(o, x).unwrap()),
            Curve2::Circle2(Circle2::new(f, r).unwrap()),
            Curve2::Ellipse2(Ellipse2::new(f, r, r * k).unwrap()),
        ];
        for cv in curves {
            let e = cv.eval(t);
            let foot = cv.project(p);
            for v in [e.point.x, e.point.y, e.d1.x, e.d1.y, e.d2.x, e.d2.y] {
                s.f64(v);
            }
            for v in [foot.param, foot.point.x, foot.point.y, foot.distance] {
                s.f64(v);
            }
        }
    }
    for _ in 0..64 {
        let n = nurbs::any_curve2(&mut g);
        let p = Point2::new(g.next_f64(-8.0, 8.0), g.next_f64(-8.0, 8.0));
        let foot = n.project(p);
        for v in [foot.param, foot.point.x, foot.point.y, foot.distance] {
            s.f64(v);
        }
    }
}

fn nurbs_bodies_section(golden: &mut Golden) {
    let mut s = golden.section("nurbs_bodies");
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    let at = Rigid::translation(Vector3::new(12.5, -3.25, 100.0))
        .then_after(&Rigid::rotation(axis, 2.5));
    let key = OpKey(0x6e75_7262);
    let bodies = [
        primitive::block(key, 1.5, 2.25, 0.75, &at),
        primitive::cylinder(key, 0.5, 3.0, &at),
        primitive::cone(key, 1.25, 2.0, &at),
        primitive::frustum(key, 2.0, 0.5, 1.0, &at),
    ];
    for result in bodies {
        let (mut body, _) = result.unwrap();
        nurbs::convert_body(&mut body).unwrap();
        s.str(&body.dump());
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

/// Every field of a mesh, bit for bit.
fn hash_mesh(s: &mut ryukei_testkit::Section<'_>, m: &BodyMesh) {
    s.u64(m.positions.len() as u64);
    for p in &m.positions {
        for x in p.to_array() {
            s.f64(x);
        }
    }
    for c in &m.corners {
        s.u64(u64::from(c.position));
        s.f64(c.uv.x);
        s.f64(c.uv.y);
        for x in c.normal.get().to_array() {
            s.f64(x);
        }
    }
    for t in &m.triangles {
        for &i in t {
            s.u64(u64::from(i));
        }
    }
    for f in &m.faces {
        s.u64(f.face.index() as u64);
        for x in [
            f.corners.start,
            f.corners.end,
            f.triangles.start,
            f.triangles.end,
        ] {
            s.u64(u64::from(x));
        }
    }
    for e in &m.edges {
        s.u64(e.edge.index() as u64);
        for (&i, &t) in e.positions.iter().zip(&e.params) {
            s.u64(u64::from(i));
            s.f64(t);
        }
    }
    s.u64(m.faults.len() as u64);
}

fn meshes_section(golden: &mut Golden) {
    let mut s = golden.section("meshes");
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    let at = Rigid::translation(Vector3::new(12.5, -3.25, 100.0))
        .then_after(&Rigid::rotation(axis, 2.5));
    let key = OpKey(0x6d65_7368);
    for place in [Rigid::IDENTITY, at] {
        let bodies = [
            primitive::block(key, 1.5, 2.25, 0.75, &place),
            primitive::cylinder(key, 0.5, 3.0, &place),
            primitive::cone(key, 1.25, 2.0, &place),
            primitive::frustum(key, 2.0, 0.5, 1.0, &place),
            primitive::sphere(key, 4.0, &place),
            primitive::torus(key, 3.0, 0.75, &place),
        ];
        for result in bodies {
            let (body, _) = result.unwrap();
            for p in [MeshParams::new(2e-3, 0.3), MeshParams::new(2e-2, 0.8)] {
                hash_mesh(&mut s, &tessellate(&body, &p).unwrap());
            }
        }
    }
    let mut bodies: Vec<_> = [
        primitive::block(key, 1.5, 2.25, 0.75, &at),
        primitive::cylinder(key, 0.5, 3.0, &at),
        primitive::cone(key, 1.25, 2.0, &at),
        primitive::frustum(key, 2.0, 0.5, 1.0, &at),
    ]
    .into_iter()
    .map(|r| {
        let mut b = r.unwrap().0;
        nurbs::convert_body(&mut b).unwrap();
        b
    })
    .collect();
    bodies.push(mesh::bump_block(key, [1.5, 2.25, 0.75], 0.5, &at).unwrap());
    for body in &bodies {
        hash_mesh(
            &mut s,
            &tessellate(body, &MeshParams::new(2e-2, 0.5)).unwrap(),
        );
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
                let (tag, names) = match e {
                    Event::Created(n) => (0, vec![*n]),
                    Event::Deleted(n) => (1, vec![*n]),
                    Event::Modified(n) => (2, vec![*n]),
                    Event::Split { parent, children } => (
                        3,
                        [*parent]
                            .into_iter()
                            .chain(children.iter().copied())
                            .collect(),
                    ),
                    Event::Merged { parents, child } => {
                        (4, parents.iter().copied().chain([*child]).collect())
                    }
                    Event::Placed { from, to, .. } => (5, vec![*from, *to]),
                    Event::Replaced { old, new } => {
                        (6, [*old].into_iter().chain(new.iter().copied()).collect())
                    }
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

/// A profile in a tilted frame: loops of lines and arcs, `(x, y)` pairs
/// in order for a polygon or `(cx, cy, r)` for a whole circle.
fn sweep_profile(loops: &[&[f64]], circle: &[bool]) -> ValidProfile {
    let frame = Frame::new(
        Point3::new(2.5, -1.0, 0.75),
        Vector3::new(0.25, -0.5, 1.0),
        Vector3::new(1.0, 0.5, 0.0),
    )
    .unwrap();
    let mut key = 0u128;
    let loops = loops
        .iter()
        .zip(circle)
        .enumerate()
        .map(|(l, (pts, &round))| {
            let segments = if round {
                key += 1;
                let f = Frame2::new(Point2::new(pts[0], pts[1]), Vector2::new(1.0, 0.0), true);
                vec![Segment {
                    key: SegmentKey(key),
                    curve: Curve2::Circle2(Circle2::new(f.unwrap(), pts[2]).unwrap()),
                    t0: 0.0,
                    t1: core::f64::consts::TAU,
                }]
            } else {
                let n = pts.len() / 2;
                (0..n)
                    .map(|i| {
                        key += 1;
                        let j = (i + 1) % n;
                        let a = Point2::new(pts[2 * i], pts[2 * i + 1]);
                        let b = Point2::new(pts[2 * j], pts[2 * j + 1]);
                        Segment {
                            key: SegmentKey(key),
                            curve: Curve2::Line2(Line2::through(a, b).unwrap()),
                            t0: 0.0,
                            t1: a.distance(b),
                        }
                    })
                    .collect()
            };
            Loop {
                key: LoopKey(l as u128 + 1),
                segments,
            }
        })
        .collect();
    ValidProfile::new(Profile { frame, loops }).unwrap()
}

fn sweeps_section(golden: &mut Golden) {
    let mut s = golden.section("sweeps");
    let key = OpKey(0x7377_6570);
    let square = [0.0, 0.0, 2.0, 0.0, 2.0, 1.5, 0.0, 1.5];
    let off_axis = [1.0, 0.0, 2.0, 0.0, 2.0, 1.5, 1.0, 1.5];
    let against = [0.0, 0.0, 1.0, 0.0, 1.0, 1.5, 0.0, 1.5];
    let holed = sweep_profile(&[&square, &[1.0, 0.75, 0.4]], &[false, true]);
    let lumps = sweep_profile(&[&square, &[4.0, 0.75, 0.5]], &[false, true]);
    let ring = sweep_profile(&[&off_axis, &[1.5, 0.75, 0.25]], &[false, true]);
    let torus = sweep_profile(&[&[2.0, 0.0, 0.5]], &[true]);
    let solid = sweep_profile(&[&against], &[false]);
    // A NURBS blob off the axis: an extrusion and a revolution.
    let curve = nurbs::blob2(&mut Lcg::new(0x626c_6f62), Point2::new(2.0, 0.5), 0.75);
    let [t0, t1] = curve.domain();
    let blob = ValidProfile::new(Profile {
        frame: *solid.frame(),
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![Segment {
                key: SegmentKey(1),
                curve: Curve2::Nurbs2(std::sync::Arc::new(curve)),
                t0,
                t1,
            }],
        }],
    })
    .unwrap();
    let frame = holed.frame();
    let (n, x, y) = (frame.z().get(), frame.x().get(), frame.y().get());
    let axis = Axis::new(frame.origin(), y).unwrap();
    let bodies = [
        extrude(key, &holed, n, 0.0, 1.0),
        extrude(key, &holed, n + x * 0.5, -0.25, 0.75),
        extrude(key, &lumps, -n, 0.0, 0.5),
        revolve(key, &ring, axis, 0.25, 2.0),
        // A cavity: the ring turned whole.
        revolve(key, &ring, axis, 0.0, core::f64::consts::TAU),
        revolve(key, &torus, axis, 0.0, core::f64::consts::TAU),
        revolve(key, &solid, axis, 0.0, core::f64::consts::TAU),
        revolve(key, &solid, axis, -1.0, 1.0),
        extrude(key, &blob, n, 0.0, 1.0),
        revolve(key, &blob, axis, 0.0, 1.5),
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
        hash_mesh(
            &mut s,
            &tessellate(&body, &MeshParams::new(2e-2, 0.5)).unwrap(),
        );
    }
}

/// Intersections: curve–surface hits, surface–surface branches with their
/// pcurves (closed forms and traced), face–face stretches and the
/// self-intersection check.
fn intersections_section(golden: &mut Golden) {
    use ryukei::intersect::{
        CurveHit, FaceMeet, Side, SurfaceHit, curve_surface, face_face, surface_surface,
    };
    let mut s = golden.section("intersections");
    let side = |x: Side| match x {
        Side::Negative => 0u64,
        Side::Positive => 1,
        Side::Beyond => 2,
    };
    let f = |o: (f64, f64, f64), z: (f64, f64, f64)| {
        Frame::new(
            Point3::new(o.0, o.1, o.2),
            Vector3::new(z.0, z.1, z.2),
            Vector3::new(1.0, 0.2, 0.1),
        )
        .unwrap()
    };
    let z = (0.0, 0.0, 1.0);
    let surfaces = [
        Surface::Plane(Plane::new(f((0.0, 0.0, 0.3), (0.2, 0.1, 1.0)))),
        Surface::Cylinder(Cylinder::new(f((0.0, 0.0, 0.0), z), 1.0).unwrap()),
        Surface::Cone(Cone::new(f((0.1, 0.0, 0.0), z), 1.0, 0.5).unwrap()),
        Surface::Sphere(Sphere::new(f((1.2, 0.0, 0.0), z), 1.0).unwrap()),
        Surface::Torus(Torus::new(f((0.0, 0.0, 0.0), z), 3.0, 1.0).unwrap()),
        Surface::Cylinder(Cylinder::new(f((0.0, 0.3, 0.0), (1.0, 0.0, 0.0)), 0.6).unwrap()),
        Surface::Sphere(Sphere::new(f((0.0, 0.0, 1.5), z), 2.0).unwrap()),
    ];
    let mut g = Lcg::new(0x696e_7473);
    for surface in &surfaces {
        for _ in 0..16 {
            let o = Point3::new(
                g.next_f64(-3.0, 3.0),
                g.next_f64(-3.0, 3.0),
                g.next_f64(-3.0, 3.0),
            );
            let d = Vector3::new(
                g.next_f64(-1.0, 1.0),
                g.next_f64(-1.0, 1.0),
                g.next_f64(-1.0, 1.0),
            );
            let curve = if g.next_f64(0.0, 1.0) < 0.5 {
                Line::new(o, d).map(Curve::Line).ok()
            } else {
                Frame::new(o, d, Vector3::new(1.0, 0.0, 0.0))
                    .ok()
                    .and_then(|fr| Circle::new(fr, g.next_f64(0.5, 3.0)).ok())
                    .map(Curve::Circle)
            };
            let Some(curve) = curve else { continue };
            let range = match curve {
                Curve::Line(_) => [-6.0, 6.0],
                _ => [0.0, core::f64::consts::TAU],
            };
            match curve_surface(&curve, range, surface, 1e-8) {
                Err(_) => s.u64(99),
                Ok(hits) => {
                    for h in hits {
                        match h {
                            CurveHit::Point {
                                t,
                                point,
                                before,
                                after,
                            } => {
                                s.f64(t);
                                s.f64(point.x);
                                s.u64(side(before) * 3 + side(after));
                            }
                            CurveHit::On { t } => {
                                s.f64(t[0]);
                                s.f64(t[1]);
                            }
                        }
                    }
                }
            }
        }
    }
    let region = Box3::new(Point3::new(-6.0, -6.0, -6.0), Point3::new(6.0, 6.0, 6.0));
    for (i, a) in surfaces.iter().enumerate() {
        for b in &surfaces[i + 1..] {
            match surface_surface(a, b, &region) {
                Err(_) => s.u64(99),
                Ok(SurfaceHit::Coincident { same }) => s.u64(u64::from(same)),
                Ok(SurfaceHit::Meet { branches, points }) => {
                    for br in branches {
                        s.f64(br.tolerance);
                        s.u64(u64::from(br.tangent));
                        for k in 0..=8 {
                            let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 8.0;
                            let p = br.curve.point(t);
                            for x in [p.x, p.y, p.z] {
                                s.f64(x);
                            }
                            for (c2, map) in &br.pcurves {
                                let q = c2.point(map[0] + map[1] * t);
                                s.f64(q.x);
                                s.f64(q.y);
                            }
                        }
                    }
                    for p in points {
                        s.f64(p.x);
                    }
                }
            }
        }
    }
    let (block, _) = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY).unwrap();
    let (rod, _) = primitive::cylinder(
        OpKey(2),
        0.5,
        3.0,
        &Rigid::translation(Vector3::new(1.0, 1.0, -0.5)),
    )
    .unwrap();
    for fa in block.faces() {
        for fb in rod.faces() {
            match face_face((&block, fa), (&rod, fb)) {
                Ok(FaceMeet::Meet {
                    stretches,
                    contacts,
                }) => {
                    s.u64(stretches.len() as u64 * 16 + contacts.len() as u64);
                    for st in stretches {
                        s.f64(st.range[0]);
                        s.f64(st.range[1]);
                    }
                }
                Ok(FaceMeet::Coincident { rims, .. }) => s.u64(1000 + rims.len() as u64),
                Err(_) => s.u64(99),
            }
        }
    }
    let torus = primitive::torus(OpKey(3), 2.0, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for body in [&block, &rod, &torus] {
        s.u64(ryukei::check::check_intersections(body).len() as u64);
    }
}

/// Intersections where a surface has no implicit form: curves against
/// splines and swept surfaces, splines against analytic surfaces and each
/// other, coincidence, and the self-intersection check of a swept body.
fn general_section(golden: &mut Golden) {
    use ryukei::geom::{Extrusion, Revolution};
    use ryukei::intersect::{CurveHit, SurfaceHit, curve_surface, surface_surface};
    use std::sync::Arc;
    let mut s = golden.section("general");
    let f = |o: (f64, f64, f64), z: (f64, f64, f64)| {
        Frame::new(
            Point3::new(o.0, o.1, o.2),
            Vector3::new(z.0, z.1, z.2),
            Vector3::new(1.0, 0.2, 0.1),
        )
        .unwrap()
    };
    let tau = core::f64::consts::TAU;
    let z = (0.0, 0.0, 1.0);
    let spline =
        |s: Surface, range: [[f64; 2]; 2]| Surface::Nurbs(Arc::new(s.to_nurbs(range).unwrap().0));
    let circle = Circle::new(f((0.2, 0.1, 0.0), (0.1, 0.0, 1.0)), 0.8).unwrap();
    let meridian = Frame::new(
        Point3::new(1.5, 0.0, 0.0),
        Vector3::new(0.0, -1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let surfaces = [
        spline(
            Surface::Plane(Plane::new(f((0.0, 0.0, 0.3), (0.2, 0.1, 1.0)))),
            [[-2.0, 2.0], [-2.0, 2.0]],
        ),
        spline(
            Surface::Cylinder(Cylinder::new(f((0.0, 0.0, 0.0), z), 1.0).unwrap()),
            [[0.0, tau], [-2.0, 2.0]],
        ),
        spline(
            Surface::Sphere(Sphere::new(f((1.2, 0.0, 0.0), z), 1.0).unwrap()),
            [[0.0, tau], [-1.4, 1.4]],
        ),
        Surface::Extrusion(Arc::new(
            Extrusion::new(
                Curve::Nurbs(Arc::new(circle.to_nurbs().unwrap().0)),
                Vector3::new(0.1, 0.2, 1.0),
            )
            .unwrap(),
        )),
        Surface::Revolution(Arc::new(
            Revolution::new(
                Frame::WORLD,
                Curve::Circle(Circle::new(meridian, 0.5).unwrap()),
            )
            .unwrap(),
        )),
    ];
    let mut g = Lcg::new(0x6765_6e65);
    for surface in &surfaces {
        for _ in 0..6 {
            let o = Point3::new(
                g.next_f64(-2.0, 2.0),
                g.next_f64(-2.0, 2.0),
                g.next_f64(-2.0, 2.0),
            );
            let d = Vector3::new(
                g.next_f64(-1.0, 1.0),
                g.next_f64(-1.0, 1.0),
                g.next_f64(-1.0, 1.0),
            );
            let Ok(line) = Line::new(o, d) else { continue };
            match curve_surface(&Curve::Line(line), [-5.0, 5.0], surface, 1e-8) {
                Err(_) => s.u64(99),
                Ok(hits) => {
                    for h in hits {
                        match h {
                            CurveHit::Point { t, point, .. } => {
                                s.f64(t);
                                s.f64(point.x);
                            }
                            CurveHit::On { t } => {
                                s.f64(t[0]);
                                s.f64(t[1]);
                            }
                        }
                    }
                }
            }
        }
    }
    let analytic = [
        Surface::Plane(Plane::new(f((0.0, 0.2, 0.0), (1.0, 0.3, 0.2)))),
        Surface::Cylinder(Cylinder::new(f((0.0, 0.3, 0.0), (1.0, 0.0, 0.0)), 0.6).unwrap()),
        Surface::Sphere(Sphere::new(f((0.0, 0.0, 1.5), z), 2.0).unwrap()),
    ];
    let region = Box3::new(Point3::new(-6.0, -6.0, -6.0), Point3::new(6.0, 6.0, 6.0));
    let mut pairs: Vec<(&Surface, &Surface)> = Vec::new();
    for a in &surfaces {
        pairs.push((a, &analytic[0]));
    }
    pairs.push((&surfaces[1], &analytic[1]));
    pairs.push((&surfaces[2], &analytic[2]));
    pairs.push((&surfaces[0], &surfaces[1]));
    pairs.push((&surfaces[1], &surfaces[2]));
    pairs.push((&surfaces[3], &surfaces[0]));
    pairs.push((&surfaces[4], &surfaces[0]));
    for (a, b) in pairs {
        match surface_surface(a, b, &region) {
            Err(_) => s.u64(99),
            Ok(SurfaceHit::Coincident { same }) => s.u64(u64::from(same)),
            Ok(SurfaceHit::Meet { branches, points }) => {
                for br in branches {
                    s.f64(br.tolerance);
                    s.u64(u64::from(br.tangent));
                    for k in 0..=8 {
                        let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 8.0;
                        let p = br.curve.point(t);
                        for x in [p.x, p.y, p.z] {
                            s.f64(x);
                        }
                        for (c2, map) in &br.pcurves {
                            let q = c2.point(map[0] + map[1] * t);
                            s.f64(q.x);
                            s.f64(q.y);
                        }
                    }
                }
                for p in points {
                    s.f64(p.x);
                }
            }
        }
    }
    // Coincident: a spline and its analytic surface, a part of it.
    let cyl = Surface::Cylinder(Cylinder::new(f((0.0, 0.0, 0.0), z), 1.0).unwrap());
    let part = spline(cyl.clone(), [[0.0, tau], [-0.5, 0.7]]);
    for (a, b) in [(&surfaces[1], &cyl), (&part, &surfaces[1])] {
        match surface_surface(a, b, &region) {
            Ok(SurfaceHit::Coincident { same }) => s.u64(u64::from(same)),
            _ => s.u64(99),
        }
    }
    // The self-intersection check of a periodic cubic blob extruded, its
    // side an extrusion meeting its planar caps.
    let n = 8;
    let pts: Vec<Point2> = (0..n)
        .map(|i| {
            let (sn, cs) = ryukei::math::sin_cos(tau * f64::from(i) / f64::from(n));
            let k = [1.0, 0.8, 1.1, 0.9][i as usize % 4];
            Point2::new(k * cs, k * sn)
        })
        .collect();
    let knots: Vec<f64> = (0..=n).map(f64::from).collect();
    let kv = ryukei::geom::KnotVector::periodic(3, &knots, &vec![1; n as usize + 1]).unwrap();
    let blob = Curve2::Nurbs2(Arc::new(
        ryukei::geom::NurbsCurve2::new(kv, pts, None).unwrap(),
    ));
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![Segment {
                key: SegmentKey(1),
                curve: blob,
                t0: 0.0,
                t1: f64::from(n),
            }],
        }],
    })
    .unwrap();
    let (body, _) =
        ryukei::sweep::extrude(OpKey(4), &profile, Vector3::new(0.0, 0.2, 1.0), 0.0, 1.0).unwrap();
    s.u64(ryukei::check::check_intersections(&body).len() as u64);
}

fn booleans_section(golden: &mut Golden) {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let mut s = golden.section("booleans");
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    let far = Rigid::translation(Vector3::new(12.5, -3.25, 100.0))
        .then_after(&Rigid::rotation(axis, 2.5));
    let key = OpKey(0x626f_6f6c);
    let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
    let y_axis = Vector3::new(0.0, 1.0, 0.0).normalize().unwrap();
    // A rod turned onto the x axis, through the upright one's middle.
    let across =
        at(-1.5, 0.0, 1.5).then_after(&Rigid::rotation(y_axis, core::f64::consts::FRAC_PI_2));
    let block = primitive::block(key, 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(key, 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let thin = primitive::cylinder(key, 0.35, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let ball = primitive::sphere(key, 0.8, &Rigid::IDENTITY).unwrap().0;
    let torus = primitive::torus(key, 1.0, 0.3, &Rigid::IDENTITY).unwrap().0;
    let slab = primitive::block(key, 3.0, 3.0, 0.4, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let cone = primitive::cone(key, 0.8, 1.5, &Rigid::IDENTITY).unwrap().0;
    // Each case: the target, then its tools in place.
    let cases: [(&ryukei::topo::Body, Vec<(&ryukei::topo::Body, Rigid)>); 5] = [
        (&block, vec![(&rod, at(1.0, 1.0, -0.5))]),
        (
            &block,
            vec![
                (&rod, at(0.6, 0.6, -0.5)),
                (&rod, at(1.4, 1.4, -0.5)),
                (&ball, at(2.0, 2.0, 2.0)),
            ],
        ),
        (&rod, vec![(&thin, across)]),
        (&torus, vec![(&slab, at(-1.5, -1.5, -0.1))]),
        (&cone, vec![(&ball, at(0.2, 0.0, 1.0))]),
    ];
    for place in [Rigid::IDENTITY, far] {
        for (target, tools) in &cases {
            let target = target.transformed(&place).unwrap();
            let tools: Vec<Tool> = tools
                .iter()
                .enumerate()
                .map(|(i, (body, p))| Tool {
                    key: ToolKey(i as u128 + 1),
                    body,
                    place: place.then_after(p),
                })
                .collect();
            for op in [BooleanOp::Unite, BooleanOp::Intersect, BooleanOp::Subtract] {
                match boolean(key, op, &target, &tools) {
                    Ok((body, journal)) => {
                        s.str(&body.dump());
                        s.u64(journal.events().len() as u64);
                        if body.counts().faces > 0 {
                            hash_mesh(
                                &mut s,
                                &tessellate(&body, &MeshParams::new(2e-2, 0.5)).unwrap(),
                            );
                        }
                    }
                    Err(e) => s.str(&format!("{e:?}")),
                }
            }
        }
    }
}

fn pipes_section(golden: &mut Golden) {
    use ryukei::geom::{KnotVector, NurbsCurve, Pipe};
    use ryukei::intersect::{CurveHit, SurfaceHit, curve_pipe, pipe_surface};
    use std::sync::Arc;
    let mut s = golden.section("pipes");
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    let far = Rigid::translation(Vector3::new(12.5, -3.25, 100.0))
        .then_after(&Rigid::rotation(axis, 2.5));
    // A wavy spine rolling on the plane z = 0, and an ellipse rolling
    // round a cylinder.
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let r = 0.25;
    let pts = [(0.0, 0.0), (1.0, 1.0), (2.0, -0.5), (3.0, 0.5), (4.0, 0.0)]
        .iter()
        .map(|&(x, y)| Point3::new(x, y, r))
        .collect();
    let wavy = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    let on_plane = Surface::Pipe(Arc::new(
        Pipe::new(wavy, r, Surface::Plane(Plane::new(Frame::WORLD))).unwrap(),
    ));
    let (sn, cs) = (0.5, 0.75f64.sqrt());
    let tilted = Frame::new(
        Point3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, -sn, cs),
        Vector3::new(0.0, cs, sn),
    )
    .unwrap();
    let ellipse = Curve::Ellipse(Ellipse::new(tilted, 3.25 / cs, 3.25).unwrap());
    let round = Surface::Pipe(Arc::new(
        Pipe::new(
            ellipse,
            r,
            Surface::Cylinder(Cylinder::new(Frame::WORLD, 3.0).unwrap()),
        )
        .unwrap(),
    ));
    let mut g = Lcg::new(0x7069_7065);
    for pipe in [&on_plane, &round] {
        for place in [Rigid::IDENTITY, far] {
            let p = pipe.transformed(&place);
            let [[s0, s1], _] = p.domain();
            for _ in 0..24 {
                let (u, v) = (g.next_f64(s0, s1), g.next_f64(-3.0, 3.0));
                let e = p.eval(u, v);
                for x in [e.point.to_vector(), e.du, e.dv, e.duu, e.duv, e.dvv] {
                    for c in x.to_array() {
                        s.f64(c);
                    }
                }
                let q = e.point + Vector3::new(0.01, -0.02, 0.015);
                let f = p.project(q);
                s.f64(f.param.0);
                s.f64(f.param.1);
                s.f64(f.distance);
                let b = p.derivative_bound([[u, u + 0.01], [v, v + 0.1]]);
                for x in [b.duu, b.duv, b.dvv, b.turn_u, b.turn_v] {
                    s.f64(x);
                }
            }
        }
    }
    // A closed tube on its body, meshed.
    let mut b = ryukei::topo::Body::new();
    let surface = b.add_surface(round.clone());
    let (m, _) = ryukei::euler::mvfs(&mut b, OpKey(1), round.point(0.0, 0.0), surface).unwrap();
    ryukei::euler::kvr(&mut b, m.loop_).unwrap();
    s.str(&b.dump());
    hash_mesh(
        &mut s,
        &tessellate(&b, &MeshParams::new(2e-2, 0.5)).unwrap(),
    );
    // Where the wavy tube meets a plane across it and a sphere.
    let c = match &on_plane {
        Surface::Pipe(p) => p.spine().point(0.8),
        _ => unreachable!(),
    };
    let across = Frame::new(
        Point3::new(1.5, 0.0, 0.0),
        Vector3::new(1.0, 0.1, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let ball = Frame::new(c, Vector3::new(0.0, 0.0, 1.0), Vector3::new(1.0, 0.0, 0.0)).unwrap();
    let region = Box3::new(Point3::new(-9.0, -9.0, -9.0), Point3::new(9.0, 9.0, 9.0));
    for other in [
        Surface::Plane(Plane::new(across)),
        Surface::Sphere(Sphere::new(ball, 0.4).unwrap()),
    ] {
        match pipe_surface(&on_plane, &other, &region) {
            Ok(SurfaceHit::Meet { branches, points }) => {
                s.u64(points.len() as u64);
                for br in &branches {
                    s.f64(br.tolerance);
                    let [r0, r1] = br.range;
                    for i in 0..=8 {
                        for x in br
                            .curve
                            .point(r0 + (r1 - r0) * f64::from(i) / 8.0)
                            .to_array()
                        {
                            s.f64(x);
                        }
                    }
                }
            }
            other => s.str(&format!("{other:?}")),
        }
    }
    for _ in 0..8 {
        let o = Point3::new(
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
            g.next_f64(0.0, 1.0),
        );
        let d = Vector3::new(
            g.next_f64(0.5, 1.0),
            g.next_f64(-0.3, 0.3),
            g.next_f64(-0.2, 0.2),
        );
        let line = Curve::Line(Line::new(o, d).unwrap());
        match curve_pipe(&line, [0.0, 5.0], &on_plane, 1e-8) {
            Ok(hits) => {
                for h in hits {
                    match h {
                        CurveHit::Point { t, .. } => s.f64(t),
                        CurveHit::On { t } => {
                            s.f64(t[0]);
                            s.f64(t[1]);
                        }
                    }
                }
            }
            Err(e) => s.str(&format!("{e:?}")),
        }
    }
}

fn blends_section(golden: &mut Golden) {
    use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
    use ryukei::name::{FaceRole, Name, Role};
    let mut s = golden.section("blends");
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    let far = Rigid::translation(Vector3::new(12.5, -3.25, 100.0))
        .then_after(&Rigid::rotation(axis, 2.5));
    let key = OpKey(0x626c_656e);
    let face = |r: FaceRole| Name::derive(key, Role::Face(r), &[]);
    let edge = |a: FaceRole, b: FaceRole| {
        let mut f = [face(a), face(b)];
        f.sort();
        Name::derive(key, Role::Edge, &f)
    };
    let fillet = |radius: f64| BlendKind::Fillet { radius };
    let chamfer = BlendKind::Chamfer;
    let box_edge = edge(FaceRole::PlusX, FaceRole::PlusZ);
    let kinds = [
        fillet(0.25),
        chamfer(Chamfer::Equal { distance: 0.2 }),
        chamfer(Chamfer::Distances {
            first: 0.1,
            second: 0.3,
            first_face: face(FaceRole::PlusX),
        }),
        chamfer(Chamfer::DistanceAngle {
            distance: 0.2,
            angle: 0.5,
            face: face(FaceRole::PlusZ),
        }),
    ];
    for place in [Rigid::IDENTITY, far] {
        let mut cases: Vec<(ryukei::topo::Body, Vec<EdgeBlend>)> = Vec::new();
        let b = primitive::block(key, 1.0, 2.0, 3.0, &place).unwrap().0;
        for kind in kinds {
            cases.push((
                b.clone(),
                vec![EdgeBlend {
                    edge: box_edge,
                    kind,
                }],
            ));
        }
        let c = primitive::cylinder(key, 1.0, 1.5, &place).unwrap().0;
        let rims: Vec<Name> = c.edges().map(|e| c.edge(e).unwrap().name).collect();
        for kind in [fillet(0.2), chamfer(Chamfer::Equal { distance: 0.15 })] {
            let all = rims.iter().map(|&edge| EdgeBlend { edge, kind }).collect();
            cases.push((c.clone(), all));
        }
        for (body, blends) in &cases {
            match blend(key, body, blends) {
                Ok((r, journal)) => {
                    s.str(&r.dump());
                    s.u64(journal.events().len() as u64);
                    hash_mesh(
                        &mut s,
                        &tessellate(&r, &MeshParams::new(2e-2, 0.5)).unwrap(),
                    );
                }
                Err(e) => s.str(&format!("{e:?}")),
            }
        }
    }
}

fn corners_section(golden: &mut Golden) {
    use FaceRole::*;
    use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
    use ryukei::name::{FaceRole, Name, Role};
    let mut s = golden.section("corners");
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    let far = Rigid::translation(Vector3::new(12.5, -3.25, 100.0))
        .then_after(&Rigid::rotation(axis, 2.5));
    let key = OpKey(0x636f_726e);
    let face = |r: FaceRole| Name::derive(key, Role::Face(r), &[]);
    let edge = |a: FaceRole, b: FaceRole| {
        let mut f = [face(a), face(b)];
        f.sort();
        Name::derive(key, Role::Edge, &f)
    };
    let fillet = |e: Name, radius: f64| EdgeBlend {
        edge: e,
        kind: BlendKind::Fillet { radius },
    };
    let chamfer = |e: Name, distance: f64| EdgeBlend {
        edge: e,
        kind: BlendKind::Chamfer(Chamfer::Equal { distance }),
    };
    let [e1, e2, e3] = [edge(PlusX, PlusZ), edge(PlusY, PlusZ), edge(PlusX, PlusY)];
    let all: Vec<Name> = [
        (PlusX, PlusY),
        (PlusX, MinusY),
        (MinusX, PlusY),
        (MinusX, MinusY),
        (PlusX, PlusZ),
        (PlusX, MinusZ),
        (MinusX, PlusZ),
        (MinusX, MinusZ),
        (PlusY, PlusZ),
        (PlusY, MinusZ),
        (MinusY, PlusZ),
        (MinusY, MinusZ),
    ]
    .iter()
    .map(|&(a, b)| edge(a, b))
    .collect();
    let cases: Vec<Vec<EdgeBlend>> = vec![
        // A ball corner, every edge, trims, a mitre, a roll.
        vec![fillet(e1, 0.2), fillet(e2, 0.2), fillet(e3, 0.2)],
        all.iter().map(|&e| fillet(e, 0.25)).collect(),
        vec![fillet(e2, 0.2), fillet(e3, 0.2)],
        vec![fillet(e2, 0.15), chamfer(e3, 0.3)],
        vec![chamfer(e1, 0.2), chamfer(e2, 0.2), chamfer(e3, 0.2)],
        vec![fillet(e1, 0.3), fillet(e2, 0.1), fillet(e3, 0.1)],
    ];
    for place in [Rigid::IDENTITY, far] {
        let b = primitive::block(key, 1.0, 2.0, 3.0, &place).unwrap().0;
        for blends in &cases {
            match blend(key, &b, blends) {
                Ok((r, journal)) => {
                    s.str(&r.dump());
                    s.u64(journal.events().len() as u64);
                    hash_mesh(
                        &mut s,
                        &tessellate(&r, &MeshParams::new(2e-2, 0.5)).unwrap(),
                    );
                }
                Err(e) => s.str(&format!("{e:?}")),
            }
        }
    }
}

fn blend_booleans_section(golden: &mut Golden) {
    use ryukei::blend::{BlendKind, EdgeBlend, blend};
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::check::check_intersections;
    use ryukei::geom::Pipe;
    use ryukei::intersect::beside;
    use std::sync::Arc;
    let mut s = golden.section("blend_booleans");
    let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
    let far = Rigid::translation(Vector3::new(12.5, -3.25, 100.0))
        .then_after(&Rigid::rotation(axis, 2.5));
    let key = OpKey(0x626f_6f6c);
    // A cylinder's rounded rim halved by a block and drilled by a rod.
    for place in [Rigid::IDENTITY, far] {
        let c = primitive::cylinder(key, 1.0, 1.0, &place).unwrap().0;
        let rim = c.edges().next().unwrap();
        let blends = [EdgeBlend {
            edge: c.edge(rim).unwrap().name,
            kind: BlendKind::Fillet { radius: 0.2 },
        }];
        let (r, _) = blend(key, &c, &blends).unwrap();
        let half = Rigid::translation(Vector3::new(0.0, -1.5, -1.0)).then_after(&place);
        let block = primitive::block(OpKey(2), 3.0, 3.0, 3.0, &half).unwrap().0;
        let down = Rigid::translation(Vector3::new(0.9, 0.0, -1.0)).then_after(&place);
        let rod = primitive::cylinder(OpKey(3), 0.05, 3.0, &down).unwrap().0;
        for tool in [&block, &rod] {
            let t = Tool {
                key: ToolKey(1),
                body: tool,
                place: Rigid::IDENTITY,
            };
            match boolean(key, BooleanOp::Subtract, &r, &[t]) {
                Ok((b, journal)) => {
                    s.str(&b.dump());
                    s.u64(journal.events().len() as u64);
                    s.u64(check_intersections(&b).len() as u64);
                    hash_mesh(
                        &mut s,
                        &tessellate(&b, &MeshParams::new(2e-2, 0.5)).unwrap(),
                    );
                }
                Err(e) => s.str(&format!("{e:?}")),
            }
        }
    }
    // An ellipse rolling round a cylinder: its NURBS form, and its side
    // of the cylinder and of a plane across it.
    let (sn, cs) = (0.5, 0.75f64.sqrt());
    let tilted = Frame::new(
        Point3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, -sn, cs),
        Vector3::new(0.0, cs, sn),
    )
    .unwrap();
    let ellipse = Curve::Ellipse(Ellipse::new(tilted, 3.25 / cs, 3.25).unwrap());
    let anchor = Surface::Cylinder(Cylinder::new(Frame::WORLD, 3.0).unwrap());
    let pipe = Pipe::new(ellipse, 0.25, anchor.clone()).unwrap();
    let (n, _, bound) = pipe.to_nurbs_within([0.5, 2.5], 1e-6).unwrap();
    s.f64(bound);
    for p in n.control_points() {
        for c in p.to_vector().to_array() {
            s.f64(c);
        }
    }
    let surface = Surface::Pipe(Arc::new(pipe));
    let rail = Curve2::Line2(Line2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.0)).unwrap());
    let across = Surface::Plane(Plane::new(
        Frame::new(
            Point3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    ));
    for other in [&anchor, &across] {
        let cell = [[0.5, 2.5], [-0.5, 0.5]];
        s.str(&format!(
            "{:?}",
            beside(&surface, other, cell, &[(&rail, [0.0, 3.0])], 1e-3)
        ));
    }
}

/// Bodies written as a file must describe them and healed again: the
/// seamed shapes and the bodies healed from them, for the primitives and
/// for seeded sweeps, booleans and blends.
fn heal_section(golden: &mut Golden) {
    use ryukei::blend::blend;
    use ryukei::boolean::boolean;
    use ryukei::heal::{HealOptions, heal, seamed};
    use ryukei_testkit::blend_bytes::blend_case;
    use ryukei_testkit::boolean_bytes::boolean_case;
    use ryukei_testkit::sweep_bytes::sweep;
    let mut s = golden.section("heal");
    let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let k = OpKey(1);
    let mut bodies = vec![
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
        primitive::sphere(k, 1.0, &t).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
    ];
    let bytes = |seed: u64| {
        let mut g = Lcg::new(seed);
        (0..96)
            .map(|_| (g.next_u64() >> 56) as u8)
            .collect::<Vec<u8>>()
    };
    for seed in 0..12 {
        if let Some((b, _)) = sweep(&bytes(seed)) {
            bodies.push(b);
        }
        if let Some(case) = boolean_case(&bytes(seed))
            && let Ok((b, _)) = boolean(OpKey(9), case.op, &case.target, &case.tools())
        {
            bodies.push(b);
        }
        if let Some(case) = blend_case(&bytes(seed))
            && let Ok((b, _)) = blend(OpKey(9), &case.body, &case.blends)
        {
            bodies.push(b);
        }
    }
    for b in &bodies {
        let shape = seamed(b, 1e-7).unwrap();
        s.str(&format!("{shape:?}"));
        let (h, journal, report) = heal(OpKey(3), &shape, &HealOptions::default()).unwrap();
        s.str(&h.dump());
        s.str(&format!("{journal:?} {report:?}"));
    }
}

/// Mirror images: the primitives and seeded sweeps, booleans and blends
/// reflected in an oblique plane, with their meshes.
fn mirror_section(golden: &mut Golden) {
    use ryukei::blend::blend;
    use ryukei::boolean::boolean;
    use ryukei_testkit::blend_bytes::blend_case;
    use ryukei_testkit::boolean_bytes::boolean_case;
    use ryukei_testkit::sweep_bytes::sweep;
    let mut s = golden.section("mirror");
    let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let k = OpKey(1);
    let mut bodies = vec![
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
        primitive::sphere(k, 1.0, &t).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
    ];
    let bytes = |seed: u64| {
        let mut g = Lcg::new(seed);
        (0..96)
            .map(|_| (g.next_u64() >> 56) as u8)
            .collect::<Vec<u8>>()
    };
    for seed in 0..12 {
        if let Some((b, _)) = sweep(&bytes(seed)) {
            bodies.push(b);
        }
        if let Some(case) = boolean_case(&bytes(seed))
            && let Ok((b, _)) = boolean(OpKey(9), case.op, &case.target, &case.tools())
        {
            bodies.push(b);
        }
        if let Some(case) = blend_case(&bytes(seed))
            && let Ok((b, _)) = blend(OpKey(9), &case.body, &case.blends)
        {
            bodies.push(b);
        }
    }
    let plane = Plane::new(
        Frame::new(
            Point3::new(0.3, -0.2, 0.1),
            Vector3::new(1.0, 0.4, -0.7),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    );
    for b in &bodies {
        match b.mirrored(&plane) {
            Ok(m) => {
                s.str(&m.dump());
                if m.counts().faces > 0 {
                    hash_mesh(
                        &mut s,
                        &tessellate(&m, &MeshParams::new(2e-2, 0.5)).unwrap(),
                    );
                }
            }
            Err(e) => s.str(&format!("{e:?}")),
        }
    }
}

/// The body codec: the encodings of the primitives and of seeded sweeps,
/// booleans and blends, of the journals that made them, and of the
/// primitives' meshes.
fn codec_section(golden: &mut Golden) {
    use ryukei::blend::blend;
    use ryukei::boolean::boolean;
    use ryukei::codec::{encode_body, encode_journal, encode_mesh};
    use ryukei_testkit::blend_bytes::blend_case;
    use ryukei_testkit::boolean_bytes::boolean_case;
    use ryukei_testkit::sweep_bytes::sweep;
    let mut s = golden.section("codec");
    let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let k = OpKey(1);
    let mut made = vec![
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap(),
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap(),
        primitive::cone(k, 1.0, 2.0, &t).unwrap(),
        primitive::sphere(k, 1.0, &t).unwrap(),
        primitive::torus(k, 2.0, 0.5, &t).unwrap(),
    ];
    let bytes = |seed: u64| {
        let mut g = Lcg::new(seed);
        (0..96)
            .map(|_| (g.next_u64() >> 56) as u8)
            .collect::<Vec<u8>>()
    };
    for seed in 0..12 {
        if let Some((b, _)) = sweep(&bytes(seed)) {
            made.push((b, ryukei::name::Journal::new()));
        }
        if let Some(case) = boolean_case(&bytes(seed))
            && let Ok(r) = boolean(OpKey(9), case.op, &case.target, &case.tools())
        {
            made.push(r);
        }
        if let Some(case) = blend_case(&bytes(seed))
            && let Ok(r) = blend(OpKey(9), &case.body, &case.blends)
        {
            made.push(r);
        }
    }
    for (b, j) in &made {
        s.bytes(&encode_body(b));
        s.bytes(&encode_journal(j));
    }
    for (b, _) in &made[..5] {
        let m = tessellate(b, &MeshParams::new(2e-2, 0.5)).unwrap();
        s.bytes(&encode_mesh(&m));
    }
}

/// A loop of keyed lines through `pts` (`x, y` pairs) and a circle.
fn keyed_loops(pts: &[(f64, f64)], circle: Option<(f64, f64, f64)>) -> Vec<Loop> {
    let n = pts.len();
    let mut out = vec![Loop {
        key: LoopKey(1),
        segments: (0..n)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % n]);
                let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
                Segment {
                    key: SegmentKey(10 + i as u128),
                    curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                    t0: 0.0,
                    t1: p.distance(q),
                }
            })
            .collect(),
    }];
    if let Some((x, y, r)) = circle {
        let f = Frame2::new(Point2::new(x, y), Vector2::new(1.0, 0.0), true).unwrap();
        out.push(Loop {
            key: LoopKey(2),
            segments: vec![Segment {
                key: SegmentKey(50),
                curve: Curve2::Circle2(Circle2::new(f, r).unwrap()),
                t0: 0.0,
                t1: core::f64::consts::TAU,
            }],
        });
    }
    out
}

fn square_pts(h: f64) -> Vec<(f64, f64)> {
    vec![(-h, -h), (h, -h), (h, h), (-h, h)]
}

/// Hashes a body, the names its journal creates and its mesh.
fn hash_body(
    s: &mut ryukei_testkit::Section<'_>,
    body: &ryukei::topo::Body,
    journal: &ryukei::name::Journal,
) {
    s.str(&body.dump());
    for e in journal.events() {
        if let Event::Created(n) = e {
            s.u64(n.to_u128() as u64);
            s.u64((n.to_u128() >> 64) as u64);
        }
    }
    hash_mesh(s, &tessellate(body, &MeshParams::new(2e-2, 0.5)).unwrap());
}

/// Sweeps along paths: the sweep surface's points, derivatives, frames
/// and NURBS forms, and bodies along open and closed paths of lines,
/// arcs and splines.
fn sweeps_along_section(golden: &mut Golden) {
    use ryukei::geom::{FrameLaw, KnotVector, NurbsCurve, Sweep, UnitVector3};
    use ryukei::sweep::{AlongOptions, Path, PathSegment, along};
    use std::sync::Arc;
    let mut s = golden.section("sweeps_along");
    let key = OpKey(0x616c_6f6e);
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let wavy = Curve::Nurbs(Arc::new(
        NurbsCurve::new(
            k,
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(2.0, 1.0, 0.5),
                Point3::new(3.0, 0.5, 1.5),
                Point3::new(4.0, 0.0, 2.0),
            ],
            None,
        )
        .unwrap(),
    ));
    let across = Frame::new(
        Point3::ORIGIN,
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let z = UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap();
    // The surface alone, by each law.
    for law in [FrameLaw::Minimal, FrameLaw::Binormal(z)] {
        let arc = Curve::Circle(Circle::new(across, 0.2).unwrap());
        let w = Sweep::new(
            (arc, [0.3, 2.5]),
            (wavy.clone(), [0.0, 2.0]),
            across,
            law,
            0.7,
            1.5,
        )
        .unwrap();
        for i in 0..=6 {
            for j in 0..=8 {
                let (u, t) = (0.3 + 2.2 * f64::from(i) / 6.0, 2.0 * f64::from(j) / 8.0);
                let e = Surface::Sweep(Arc::new(w.clone())).eval(u, t);
                for v in [e.point.to_vector(), e.du, e.dv, e.duu, e.duv, e.dvv] {
                    s.f64(v.x);
                    s.f64(v.y);
                    s.f64(v.z);
                }
            }
        }
        let foot = Surface::Sweep(Arc::new(w.clone())).project(Point3::new(1.5, 0.7, 0.9));
        s.f64(foot.param.0);
        s.f64(foot.param.1);
        let (n, _, bound) = w.to_nurbs_within([0.0, 2.0], 1e-6).unwrap();
        s.f64(bound);
        for p in n.control_points() {
            s.f64(p.x);
            s.f64(p.y);
            s.f64(p.z);
        }
    }
    // Bodies: a bar round a bend of lines and an arc; a twisted, growing
    // tube along the spline, and one upright; a tube round a closed
    // rounded rectangle.
    let line = |from: Point3, dir: Vector3| Curve::Line(Line::new(from, dir).unwrap());
    let part = |key: u128, curve: Curve, range: [f64; 2]| PathSegment {
        key: SegmentKey(key),
        curve,
        range,
    };
    let quarter = |key: u128, c: (f64, f64), r: f64, a: f64| {
        let f = Frame::new(
            Point3::new(c.0, c.1, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        part(
            key,
            Curve::Circle(Circle::new(f, r).unwrap()),
            [a, a + core::f64::consts::FRAC_PI_2],
        )
    };
    let x = Vector3::new(1.0, 0.0, 0.0);
    let y = Vector3::new(0.0, 1.0, 0.0);
    let bend = Path {
        segments: vec![
            part(1, line(Point3::ORIGIN, x), [0.0, 2.0]),
            quarter(2, (2.0, 1.0), 1.0, -core::f64::consts::FRAC_PI_2),
            part(3, line(Point3::new(3.0, 1.0, 0.0), y), [0.0, 2.0]),
        ],
    };
    let rectangle = Path {
        segments: vec![
            part(1, line(Point3::ORIGIN, x), [0.0, 2.0]),
            quarter(2, (2.0, 0.5), 0.5, -core::f64::consts::FRAC_PI_2),
            part(3, line(Point3::new(2.5, 0.5, 0.0), y), [0.0, 3.0]),
            quarter(4, (2.0, 3.5), 0.5, 0.0),
            part(5, line(Point3::new(2.0, 4.0, 0.0), -x), [0.0, 2.0]),
            quarter(6, (0.0, 3.5), 0.5, core::f64::consts::FRAC_PI_2),
            part(7, line(Point3::new(-0.5, 3.5, 0.0), -y), [0.0, 3.0]),
            quarter(8, (0.0, 0.5), 0.5, core::f64::consts::PI),
        ],
    };
    let spline = Path {
        segments: vec![part(1, wavy, [0.0, 2.0])],
    };
    let profile = |loops: Vec<Loop>| {
        ValidProfile::new(Profile {
            frame: across,
            loops,
        })
        .unwrap()
    };
    let bar = profile(keyed_loops(&square_pts(0.2), None));
    let tube = profile(vec![keyed_loops(&[], Some((0.0, 0.0, 0.12))).remove(1)]);
    let plain = AlongOptions::default();
    let twisted = AlongOptions {
        twist: 0.5,
        scale: 1.3,
        ..plain
    };
    let upright = AlongOptions {
        law: FrameLaw::Binormal(z),
        ..plain
    };
    for (p, path, opts) in [
        (&bar, &bend, plain),
        (&tube, &spline, twisted),
        (&tube, &spline, upright),
        (&tube, &rectangle, plain),
    ] {
        let (body, journal) = along(key, p, path, &opts).unwrap();
        hash_body(&mut s, &body, &journal);
    }
}

/// Lofts: a frustum, a frustum with a hole and end tangents, and a ring.
fn lofts_section(golden: &mut Golden) {
    use ryukei::geom::UnitVector3;
    use ryukei::sweep::{LoftOptions, Tangent, loft};
    let mut s = golden.section("lofts");
    let key = OpKey(0x6c6f_6674);
    let at = |frame: Frame, loops: Vec<Loop>| ValidProfile::new(Profile { frame, loops }).unwrap();
    let level = |h: f64| {
        Frame::new(
            Point3::new(0.0, 0.0, h),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap()
    };
    let z = UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let frustum = [
        at(level(0.0), keyed_loops(&square_pts(1.0), None)),
        at(level(1.0), keyed_loops(&square_pts(0.5), None)),
    ];
    let holed = [
        at(
            level(0.0),
            keyed_loops(&square_pts(1.0), Some((0.0, 0.0, 0.3))),
        ),
        at(
            level(0.6),
            keyed_loops(&square_pts(0.8), Some((0.1, 0.0, 0.25))),
        ),
        at(
            level(1.5),
            keyed_loops(&square_pts(0.6), Some((0.0, 0.0, 0.2))),
        ),
    ];
    let ring: Vec<ValidProfile> = (0..4)
        .map(|k| {
            let (sa, ca) = math::sin_cos(core::f64::consts::TAU * f64::from(k) / 4.0);
            let f = Frame::new(
                Point3::new(3.0 * ca, 3.0 * sa, 0.0),
                Vector3::new(-sa, ca, 0.0),
                Vector3::new(ca, sa, 0.0),
            )
            .unwrap();
            at(f, keyed_loops(&square_pts(0.5), None))
        })
        .collect();
    let tangent = Some(Tangent {
        direction: z,
        weight: 1.0,
    });
    let ends = LoftOptions {
        closed: false,
        start: tangent,
        end: tangent,
    };
    let closed = LoftOptions {
        closed: true,
        ..LoftOptions::default()
    };
    for (sections, opts) in [
        (&frustum[..], LoftOptions::default()),
        (&holed[..], ends),
        (&ring[..], closed),
    ] {
        let (body, journal) = loft(key, sections, &opts).unwrap();
        hash_body(&mut s, &body, &journal);
    }
}

/// Booleans and healing on every surface: spline forms of primitives
/// drilled, halved through poles, cut along knot lines and flush with
/// each other; lofts drilled, standing on a block and closed round on
/// themselves; a spline rim tightened.
fn general_booleans_section(golden: &mut Golden) {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::heal::{HealOptions, heal, seamed};
    use ryukei::sweep::{LoftOptions, loft};
    let mut s = golden.section("general_booleans");
    let spline = |mut b: ryukei::topo::Body| {
        nurbs::convert_body(&mut b).unwrap();
        b
    };
    let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
    let run = |op, target: &ryukei::topo::Body, tool: &ryukei::topo::Body, place: Rigid| {
        let tools = [Tool {
            key: ToolKey(1),
            body: tool,
            place,
        }];
        boolean(OpKey(0x6765_6e62), op, target, &tools).unwrap()
    };
    let square = |h: f64, first: u128, side: f64, turn: f64| {
        let (sn, cs) = math::sin_cos(turn);
        let d = side / 2.0;
        let pts = [(-d, -d), (d, -d), (d, d), (-d, d)];
        let segments = (0..4)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % 4]);
                let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
                Segment {
                    key: SegmentKey(first + i as u128),
                    curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                    t0: 0.0,
                    t1: p.distance(q),
                }
            })
            .collect();
        ValidProfile::new(Profile {
            frame: Frame::new(
                Point3::new(0.0, 0.0, h),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(cs, sn, 0.0),
            )
            .unwrap(),
            loops: vec![Loop {
                key: LoopKey(1),
                segments,
            }],
        })
        .unwrap()
    };
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let ball = spline(
        primitive::sphere(OpKey(3), 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    let block = primitive::block(OpKey(7), 2.0, 3.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (frustum, _) = loft(
        OpKey(5),
        &[square(0.0, 10, 1.0, 0.0), square(1.0, 20, 0.5, 0.3)],
        &LoftOptions::default(),
    )
    .unwrap();
    let base = primitive::block(OpKey(8), 2.0, 2.0, 1.0, &at(-1.0, -1.0, -1.0))
        .unwrap()
        .0;
    let unit = |k| {
        primitive::block(OpKey(k), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0
    };
    let cases = [
        (
            BooleanOp::Subtract,
            spline(plate.clone()),
            spline(rod.clone()),
            at(2.0, 2.0, -1.0),
        ),
        (
            BooleanOp::Intersect,
            ball.clone(),
            block.clone(),
            at(0.0, -1.5, -1.5),
        ),
        (
            BooleanOp::Subtract,
            ball.clone(),
            block.clone(),
            Rigid::IDENTITY,
        ),
        (BooleanOp::Unite, base, frustum, Rigid::IDENTITY),
        (
            BooleanOp::Unite,
            spline(unit(1)),
            spline(unit(2)),
            at(1.0, 0.5, 0.0),
        ),
    ];
    for (op, target, tool, place) in &cases {
        let (body, journal) = run(*op, target, tool, *place);
        hash_body(&mut s, &body, &journal);
    }
    // A closed loft of circles: one face with no loops.
    let rings: Vec<ValidProfile> = (0..4)
        .map(|k| {
            let a = core::f64::consts::TAU * f64::from(k) / 4.0;
            let (sn, cs) = math::sin_cos(a);
            ValidProfile::new(Profile {
                frame: Frame::new(
                    Point3::new(3.0 * cs, 3.0 * sn, 0.0),
                    Vector3::new(-sn, cs, 0.0),
                    Vector3::new(cs, sn, 0.0),
                )
                .unwrap(),
                loops: keyed_loops(&[], Some((0.0, 0.0, 0.5))).split_off(1),
            })
            .unwrap()
        })
        .collect();
    let opts = LoftOptions {
        closed: true,
        ..LoftOptions::default()
    };
    let (tube, journal) = loft(OpKey(9), &rings, &opts).unwrap();
    hash_body(&mut s, &tube, &journal);
    // A spline plate's rim moved 2 µm off its faces and tightened back.
    let (holed, _) = run(BooleanOp::Subtract, &plate, &rod, at(2.0, 2.0, -1.0));
    let mut shape = seamed(&spline(holed), 1e-6).unwrap();
    for e in 0..shape.edges.len() {
        let c = &shape.curves[shape.edges[e].curve];
        let [t0, t1] = shape.edges[e].interval;
        let p = c.point((t0 + t1) / 2.0);
        if (math::hypot(p.x - 2.0, p.y - 2.0) - 0.5).abs() < 1e-6 && (p.z - 1.0).abs() < 1e-6 {
            let moved = c.transformed(&at(2e-6, 1e-6, 0.0));
            shape.curves.push(moved);
            shape.edges[e].curve = shape.curves.len() - 1;
        }
    }
    let (healed, journal, _) = heal(OpKey(10), &shape, &HealOptions::default()).unwrap();
    hash_body(&mut s, &healed, &journal);
}

/// Measurement: entity properties, distances, angles and mass
/// properties, value and bound bits, on one thread and four.
fn measure_section(golden: &mut Golden) {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::measure::{
        DistanceOptions, Entity, MassOptions, Properties, Value, angle, distance, mass, properties,
    };
    let mut s = golden.section("measure");
    let value = |s: &mut ryukei_testkit::Section<'_>, v: Value| {
        s.f64(v.value());
        s.f64(v.bound());
    };
    let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let holed = boolean(
        OpKey(3),
        BooleanOp::Subtract,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &rod,
            place: at(2.0, 2.0, -1.0),
        }],
    )
    .unwrap()
    .0;
    let ball = primitive::sphere(OpKey(4), 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let work = || {
        let mut out: Vec<Value> = Vec::new();
        let mut points: Vec<Point3> = Vec::new();
        for b in [&plate, &rod, &holed, &ball] {
            let m = mass(b, 7850.0, &MassOptions::default()).unwrap();
            out.extend([m.volume, m.area, m.mass]);
            points.push(m.centre);
            out.push(Value::Bounded {
                value: m.centre_bound,
                bound: m.inertia_bound,
            });
            for row in m.inertia {
                out.extend(row.map(Value::Exact));
            }
            out.extend(m.principal_moments.map(Value::Exact));
        }
        for e in holed.edges() {
            if let Properties::Edge { length, .. } = properties(&holed, Entity::Edge(e)).unwrap() {
                out.push(length);
            }
        }
        for f in rod.faces() {
            if let Properties::Face { area, .. } = properties(&rod, Entity::Face(f)).unwrap() {
                out.push(area);
            }
        }
        for place in [at(1.3, 0.2, 0.4), at(0.6, 0.6, 1.4)] {
            let d = distance(
                (&ball, Entity::Body, &Rigid::IDENTITY),
                (&holed, Entity::Body, &place),
                &DistanceOptions::default(),
            )
            .unwrap();
            out.push(d.distance);
            points.extend(d.points);
        }
        let f = holed.faces().next().unwrap();
        let g = holed.faces().nth(1).unwrap();
        let turn = Rigid::rotation(Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(), 0.7);
        if let Ok(a) = angle(
            (&holed, Entity::Face(f), &Rigid::IDENTITY),
            (&holed, Entity::Face(g), &turn),
        ) {
            out.extend([a.angle, a.supplement]);
        }
        (out, points)
    };
    let pool = |n| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
    };
    let one = pool(1).install(work);
    assert_eq!(one, pool(4).install(work));
    for v in one.0 {
        value(&mut s, v);
    }
    for p in one.1 {
        for x in p.to_array() {
            s.f64(x);
        }
    }
}

/// Offset surfaces of a spline, an extrusion and a revolution: points and
/// derivatives at a grid, feet of points near them and their derivative
/// bounds; and a pipe on a rail.
fn offsets_section(golden: &mut Golden) {
    use ryukei::geom::{Extrusion, KnotVector, NurbsCurve, NurbsSurface, Offset, Pipe, Revolution};
    use std::sync::Arc;
    let mut s = golden.section("offsets");
    let ku = KnotVector::clamped(3, &[0.0, 0.4, 1.0], &[4, 1, 4]).unwrap();
    let kv = KnotVector::clamped(3, &[0.0, 0.3, 0.7, 1.0], &[4, 1, 1, 4]).unwrap();
    let mut pts = Vec::new();
    let mut w = Vec::new();
    for i in 0..5 {
        for j in 0..6 {
            let (x, y) = (f64::from(i) * 0.5, f64::from(j) * 0.4);
            pts.push(Point3::new(
                x,
                y,
                0.3 * math::sin(x * 1.3) * math::cos(y + 0.2),
            ));
            w.push(1.0 + 0.3 * f64::from((i + 2 * j) % 3));
        }
    }
    let wavy = Surface::Nurbs(Arc::new(NurbsSurface::new([ku, kv], pts, Some(w)).unwrap()));
    let tilted = Frame::new(
        Point3::new(0.3, -0.2, 0.5),
        Vector3::new(1.0, 0.4, 0.2),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let (circle, _) = Circle::new(tilted, 0.8).unwrap().to_nurbs().unwrap();
    let extrusion = Surface::Extrusion(Arc::new(
        Extrusion::new(Curve::Nurbs(Arc::new(circle)), Vector3::new(1.0, 0.5, 0.3)).unwrap(),
    ));
    let meridian = Frame::new(
        Point3::new(2.0, 0.0, 0.3),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let ring = Circle::new(meridian, 0.5).unwrap();
    let revolution = Surface::Revolution(Arc::new(
        Revolution::new(Frame::WORLD, Curve::Circle(ring)).unwrap(),
    ));
    let cases = [
        (wavy, [[0.0, 1.0], [0.0, 1.0]], 0.2),
        (extrusion, [[0.5, 4.0], [-1.0, 1.0]], -0.5),
        (
            revolution,
            [[0.0, core::f64::consts::TAU], [0.0, core::f64::consts::TAU]],
            0.2,
        ),
    ];
    let mut g = Lcg::new(0x6f66_6673);
    for (base, cell, d) in &cases {
        let o = Surface::Offset(Arc::new(Offset::new(base.clone(), *d, *cell).unwrap()));
        for _ in 0..16 {
            let u = g.next_f64(cell[0][0], cell[0][1]);
            let v = g.next_f64(cell[1][0], cell[1][1]);
            let e = o.eval(u, v);
            for x in [e.point.to_vector(), e.du, e.dv, e.duu, e.duv, e.dvv] {
                for c in x.to_array() {
                    s.f64(c);
                }
            }
            let n = o.normal(u, v).unwrap().get();
            let f = o.project(e.point + n * 0.01);
            s.f64(f.param.0);
            s.f64(f.param.1);
            s.f64(f.distance);
        }
        let b = o.derivative_bound(*cell);
        for x in [b.duu, b.duv, b.dvv, b.turn_u, b.turn_v] {
            s.f64(x);
        }
    }
    // A pipe on a rail: the plane as a spline, the rail the spine's shadow.
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let r = 0.25;
    let xy = [(0.0, 0.0), (1.0, 1.0), (2.0, -0.5), (3.0, 0.5), (4.0, 0.0)];
    let spine = NurbsCurve::new(
        k.clone(),
        xy.iter().map(|&(x, y)| Point3::new(x, y, r)).collect(),
        None,
    )
    .unwrap();
    let shadow = ryukei::geom::NurbsCurve2::new(
        k,
        xy.iter().map(|&(x, y)| Point2::new(x, y)).collect(),
        None,
    )
    .unwrap();
    let (flat, _) = Surface::Plane(Plane::new(Frame::WORLD))
        .to_nurbs([[-1.0, 5.0], [-2.0, 2.0]])
        .unwrap();
    let pipe = Pipe::on_rail(
        Curve::Nurbs(Arc::new(spine)),
        r,
        Surface::Nurbs(Arc::new(flat)),
        Curve2::Nurbs2(Arc::new(shadow)),
        [0.0, 1.0],
    )
    .unwrap();
    for _ in 0..32 {
        let (u, v) = (g.next_f64(0.0, 2.0), g.next_f64(-3.0, 3.0));
        let e = Surface::Pipe(Arc::new(pipe.clone())).eval(u, v);
        for x in [e.point.to_vector(), e.du, e.dv, e.duu] {
            for c in x.to_array() {
                s.f64(c);
            }
        }
    }
}

/// Blends on spline faces: a box's edge, every edge of a box (ball
/// corners), its top rim filleted and chamfered, a cylinder's rim, an
/// extruded spline circle's rim, and a twisted loft's laterals filleted
/// and chamfered.
fn spline_blends_section(golden: &mut Golden) {
    use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
    use ryukei::name::{FaceRole, Name, Role};
    use ryukei::sweep::{LoftOptions, loft};
    let mut s = golden.section("spline_blends");
    let key = OpKey(0x7370_626c);
    let spline = |mut b: ryukei::topo::Body| {
        nurbs::convert_body(&mut b).unwrap();
        b
    };
    let edge = |k: OpKey, a: FaceRole, b: FaceRole| {
        let mut f = [
            Name::derive(k, Role::Face(a), &[]),
            Name::derive(k, Role::Face(b), &[]),
        ];
        f.sort();
        Name::derive(k, Role::Edge, &f)
    };
    let fillet = |e: Name, radius: f64| EdgeBlend {
        edge: e,
        kind: BlendKind::Fillet { radius },
    };
    let chamfer = |e: Name, distance: f64| EdgeBlend {
        edge: e,
        kind: BlendKind::Chamfer(Chamfer::Equal { distance }),
    };
    let k = OpKey(1);
    let bx = spline(
        primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    use FaceRole::*;
    let all: Vec<Name> = [
        (PlusX, PlusY),
        (PlusX, MinusY),
        (MinusX, PlusY),
        (MinusX, MinusY),
        (PlusX, PlusZ),
        (PlusX, MinusZ),
        (MinusX, PlusZ),
        (MinusX, MinusZ),
        (PlusY, PlusZ),
        (PlusY, MinusZ),
        (MinusY, PlusZ),
        (MinusY, MinusZ),
    ]
    .iter()
    .map(|&(a, b)| edge(k, a, b))
    .collect();
    let top: Vec<Name> = [PlusX, MinusX, PlusY, MinusY]
        .iter()
        .map(|&f| edge(k, f, PlusZ))
        .collect();
    let cyl = spline(
        primitive::cylinder(OpKey(2), 1.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    let rim = cyl.edge(cyl.edges().next().unwrap()).unwrap().name;
    let mut cases: Vec<(ryukei::topo::Body, Vec<EdgeBlend>)> = vec![
        (bx.clone(), vec![fillet(edge(k, PlusX, PlusZ), 0.25)]),
        (bx.clone(), all.iter().map(|&e| fillet(e, 0.25)).collect()),
        (bx.clone(), top.iter().map(|&e| fillet(e, 0.15)).collect()),
        (bx.clone(), top.iter().map(|&e| chamfer(e, 0.15)).collect()),
        (cyl.clone(), vec![fillet(rim, 0.2)]),
        (cyl, vec![chamfer(rim, 0.2)]),
    ];
    // A twisted loft: its sides are curved splines.
    let square = |h: f64, turn: f64| {
        let (sn, cs) = math::sin_cos(turn);
        let pts = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
        let segments = (0..4)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % 4]);
                let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
                Segment {
                    key: SegmentKey(10 + i as u128),
                    curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                    t0: 0.0,
                    t1: p.distance(q),
                }
            })
            .collect();
        ValidProfile::new(Profile {
            frame: Frame::new(
                Point3::new(0.0, 0.0, h),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(cs, sn, 0.0),
            )
            .unwrap(),
            loops: vec![Loop {
                key: LoopKey(1),
                segments,
            }],
        })
        .unwrap()
    };
    let (twisted, _) = loft(
        OpKey(5),
        &[square(0.0, 0.0), square(1.0, core::f64::consts::PI / 6.0)],
        &LoftOptions::default(),
    )
    .unwrap();
    let laterals: Vec<Name> = twisted
        .edges()
        .filter(|&e| {
            let ed = twisted.edge(e).unwrap();
            let z = [ed.start, ed.end].map(|v| twisted.vertex(v).unwrap().point.z);
            (z[0] - z[1]).abs() > 0.5
        })
        .map(|e| twisted.edge(e).unwrap().name)
        .collect();
    cases.push((
        twisted.clone(),
        laterals.iter().map(|&e| fillet(e, 0.05)).collect(),
    ));
    cases.push((
        twisted,
        laterals.iter().map(|&e| chamfer(e, 0.05)).collect(),
    ));
    for (body, blends) in &cases {
        let (b, journal) = blend(key, body, blends).unwrap();
        hash_body(&mut s, &b, &journal);
    }
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
    derivatives_section(&mut golden);
    ellipse_section(&mut golden);
    conversion_section(&mut golden);
    projection_section(&mut golden);
    pcurves_section(&mut golden);
    nurbs_bodies_section(&mut golden);
    primitives_section(&mut golden);
    euler_section(&mut golden);
    meshes_section(&mut golden);
    sweeps_section(&mut golden);
    sweeps_along_section(&mut golden);
    lofts_section(&mut golden);
    intersections_section(&mut golden);
    general_section(&mut golden);
    booleans_section(&mut golden);
    pipes_section(&mut golden);
    blends_section(&mut golden);
    corners_section(&mut golden);
    blend_booleans_section(&mut golden);
    heal_section(&mut golden);
    mirror_section(&mut golden);
    codec_section(&mut golden);
    general_booleans_section(&mut golden);
    measure_section(&mut golden);
    offsets_section(&mut golden);
    spline_blends_section(&mut golden);
    golden.finish();
}
