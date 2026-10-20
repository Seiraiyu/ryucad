//! Pipes against seeded random analytic surfaces and lines, checked
//! against sampling: every branch `pipe_surface` returns lies within its
//! tolerance of both surfaces, its pcurves too, and every place a grid of
//! the pipe's points changes side of the other surface is near a branch;
//! every hit `curve_pipe` returns is on the pipe, and every change of
//! side along the line is near a hit. Refusals (`Uncertain`) are allowed
//! but must stay rare. Fitting pipe branches is slow in a debug build, so
//! this runs in release only (`cargo test -p ryukei --release --test
//! pipe_props`).

use ryukei::geom::{
    Box3, Cone, Curve, Cylinder, Frame, KnotVector, Line, NurbsCurve, Pipe, Plane, Point3, Sphere,
    Surface, Torus, Vector3,
};
use ryukei::intersect::{CurveHit, Failure, SurfaceHit, curve_pipe, pipe_surface};
use ryukei_testkit::Lcg;
use std::f64::consts::{PI, TAU};
use std::sync::Arc;

/// A random pipe rolling on the plane z = 0: a cubic spine wandering
/// above it, and a radius well inside its curvature. `None` where the
/// pipe can't be certified regular.
fn pipe(g: &mut Lcg) -> Option<Surface> {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = (0..5)
        .map(|i| {
            Point3::new(
                f64::from(i) + g.next_f64(-0.2, 0.2),
                g.next_f64(-0.8, 0.8),
                g.next_f64(0.4, 0.9),
            )
        })
        .collect();
    let spine = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    let r = g.next_f64(0.08, 0.3);
    Pipe::new(spine, r, Surface::Plane(Plane::new(Frame::WORLD)))
        .ok()
        .map(|p| Surface::Pipe(Arc::new(p)))
}

/// A random unit direction.
fn direction(g: &mut Lcg) -> Vector3 {
    loop {
        let v = Vector3::new(
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
        );
        if let Some(u) = v.normalize()
            && v.length() <= 1.0
        {
            return u.get();
        }
    }
}

/// A random analytic surface placed where it may meet the pipe.
fn other(g: &mut Lcg) -> Surface {
    let at = Point3::new(
        g.next_f64(0.5, 3.5),
        g.next_f64(-0.6, 0.6),
        g.next_f64(0.2, 1.0),
    );
    let z = direction(g);
    let x = if z.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let f = Frame::new(at, z, x).unwrap();
    match g.next_u64() % 5 {
        0 => Surface::Plane(Plane::new(f)),
        1 => Surface::Sphere(Sphere::new(f, g.next_f64(0.2, 0.9)).unwrap()),
        2 => Surface::Cylinder(Cylinder::new(f, g.next_f64(0.1, 0.6)).unwrap()),
        3 => Surface::Cone(Cone::new(f, g.next_f64(0.1, 0.5), g.next_f64(0.2, 1.0)).unwrap()),
        _ => {
            let minor = g.next_f64(0.1, 0.3);
            Surface::Torus(Torus::new(f, minor + g.next_f64(0.2, 0.6), minor).unwrap())
        }
    }
}

/// Which side of a surface a point is on, by its foot's natural normal.
fn side(s: &Surface, p: Point3) -> f64 {
    let (u, v) = s.project(p).param;
    let foot = s.point(u, v);
    (p - foot).dot(s.normal(u, v).unwrap().get())
}

fn region() -> Box3 {
    Box3::new(Point3::new(-5.0, -5.0, -5.0), Point3::new(9.0, 5.0, 5.0))
}

/// Checks one pipe and surface; true when the intersection was refused.
fn surfaces_agree(pipe: &Surface, other: &Surface) -> bool {
    let hit = match pipe_surface(pipe, other, &region()) {
        Ok(h) => h,
        Err(Failure::Uncertain) => return true,
        Err(e) => panic!("{e:?} for {other:?}"),
    };
    let SurfaceHit::Meet { branches, points } = hit else {
        panic!("a pipe is never coincident");
    };
    for b in &branches {
        assert!(b.tolerance <= 1e-7, "{}", b.tolerance);
        let [r0, r1] = b.range;
        for i in 0..=32 {
            let t = r0 + (r1 - r0) * f64::from(i) / 32.0;
            let p = b.curve.point(t);
            assert!(pipe.distance(p) <= b.tolerance, "off the pipe: {other:?}");
            assert!(other.distance(p) <= b.tolerance, "off the other: {other:?}");
            for (k, s) in [pipe, other].into_iter().enumerate() {
                let (c, map) = &b.pcurves[k];
                let uv = c.point(map[0] + map[1] * t);
                assert!(s.point(uv.x, uv.y).distance(p) <= b.tolerance);
            }
        }
    }
    // Every change of side across a grid of the pipe, near the other
    // surface (away from where side flips across its medial axis), is
    // near a branch or a point.
    let Surface::Pipe(pp) = pipe else {
        unreachable!()
    };
    let [d0, d1] = pp.spine().domain();
    let (n, m) = (80, 48);
    let at = |i: u32, j: u32| {
        pipe.point(
            d0 + (d1 - d0) * f64::from(i) / f64::from(n),
            -PI + TAU * f64::from(j) / f64::from(m),
        )
    };
    for i in 0..n {
        for j in 0..m {
            let q = at(i, j);
            for r in [at(i + 1, j), at(i, j + 1)] {
                let (a, b) = (side(other, q), side(other, r));
                let near = other.distance(q) < 0.1 && other.distance(r) < 0.1;
                if near && (a > 0.0) != (b > 0.0) {
                    let step = q.distance(r) + 1e-3;
                    let found = branches.iter().any(|br| {
                        let [r0, r1] = br.range;
                        (0..=400).any(|k| {
                            br.curve
                                .point(r0 + (r1 - r0) * f64::from(k) / 400.0)
                                .distance(q)
                                <= step + (r1 - r0).abs() / 400.0 * 10.0
                        })
                    }) || points.iter().any(|p| p.distance(q) <= step);
                    assert!(found, "a change of side at {q:?} missed: {other:?}");
                }
            }
        }
    }
    false
}

/// Checks one pipe and line; true when the intersection was refused.
fn lines_agree(pipe: &Surface, line: &Curve) -> bool {
    let hits = match curve_pipe(line, [0.0, 6.0], pipe, 1e-8) {
        Ok(h) => h,
        Err(Failure::Uncertain) => return true,
        Err(e) => panic!("{e:?}"),
    };
    for h in &hits {
        match h {
            CurveHit::Point { point, .. } => assert!(pipe.distance(*point) <= 1e-8),
            CurveHit::On { t } => {
                for k in 0..=8 {
                    let x = t[0] + (t[1] - t[0]) * f64::from(k) / 8.0;
                    assert!(pipe.distance(line.point(x)) <= 1e-8);
                }
            }
        }
    }
    // Every change of side along the line, near the pipe and away from
    // its open ends (where the side flips with no surface between), is
    // near a hit.
    let Surface::Pipe(pp) = pipe else {
        unreachable!()
    };
    let [d0, d1] = pp.spine().domain();
    let inside = |p: Point3| {
        let f = pipe.project(p);
        f.distance < 0.05 && f.param.0 > d0 + 1e-3 && f.param.0 < d1 - 1e-3
    };
    let n = 3000;
    for i in 0..n {
        let (x, y) = (
            6.0 * f64::from(i) / f64::from(n),
            6.0 * f64::from(i + 1) / f64::from(n),
        );
        let (p, q) = (line.point(x), line.point(y));
        let near = inside(p) && inside(q);
        if near && (side(pipe, p) > 0.0) != (side(pipe, q) > 0.0) {
            let found = hits.iter().any(|h| match h {
                CurveHit::Point { t, .. } => *t >= x - 1e-6 && *t <= y + 1e-6,
                CurveHit::On { t } => t[0] <= y + 1e-6 && t[1] >= x - 1e-6,
            });
            assert!(found, "a crossing in [{x}, {y}] missed: {hits:?}");
        }
    }
    false
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn random_surfaces() {
    let mut g = Lcg::new(0x51_7e_6a);
    let (mut tried, mut refused) = (0, 0);
    while tried < 48 {
        let Some(p) = pipe(&mut g) else {
            continue;
        };
        let o = other(&mut g);
        tried += 1;
        if surfaces_agree(&p, &o) {
            refused += 1;
        }
    }
    assert!(refused * 8 <= tried, "{refused} of {tried} refused");
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn random_lines() {
    let mut g = Lcg::new(0x11_9e_33);
    let (mut tried, mut refused) = (0, 0);
    while tried < 48 {
        let Some(p) = pipe(&mut g) else {
            continue;
        };
        let start = Point3::new(
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.5, 1.5),
            g.next_f64(0.0, 1.2),
        );
        let towards = Point3::new(
            g.next_f64(3.0, 5.0),
            g.next_f64(-1.5, 1.5),
            g.next_f64(0.0, 1.2),
        );
        let line = Curve::Line(Line::through(start, towards).unwrap());
        tried += 1;
        if lines_agree(&p, &line) {
            refused += 1;
        }
    }
    assert!(refused * 8 <= tried, "{refused} of {tried} refused");
}
