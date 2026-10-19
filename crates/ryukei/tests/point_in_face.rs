//! Point in face against the mesh: a point of a face's surface well away
//! from its boundary is inside exactly when it is on the face's mesh.

use core::f64::consts::TAU;
use ryukei::geom::{
    Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Rigid, Vector2, Vector3,
};
use ryukei::intersect::{Place, point_in_face};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use ryukei::topo::{Body, FaceId};
use ryukei_testkit::mesh::point_triangle;

const K: OpKey = OpKey(0x77);

fn seg(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(key),
        curve,
        t0,
        t1,
    }
}

fn polygon(key: u128, pts: &[(f64, f64)]) -> Loop {
    let n = pts.len();
    let segments = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let (p, q) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
            seg(
                key * 100 + i as u128,
                Curve2::Line2(Line2::through(p, q).unwrap()),
                0.0,
                p.distance(q),
            )
        })
        .collect();
    Loop {
        key: LoopKey(key),
        segments,
    }
}

fn circle(key: u128, c: (f64, f64), r: f64) -> Loop {
    let f = Frame2::new(Point2::new(c.0, c.1), Vector2::new(1.0, 0.0), true).unwrap();
    Loop {
        key: LoopKey(key),
        segments: vec![seg(
            key * 100,
            Curve2::Circle2(Circle2::new(f, r).unwrap()),
            0.0,
            TAU,
        )],
    }
}

fn profile(frame: Frame, loops: Vec<Loop>) -> ValidProfile {
    ValidProfile::new(Profile { frame, loops }).unwrap()
}

fn bodies() -> Vec<Body> {
    let tilted = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let mut out = vec![
        primitive::block(K, 2.0, 1.0, 1.5, &tilted).unwrap().0,
        primitive::cylinder(K, 1.0, 2.0, &tilted).unwrap().0,
        primitive::cone(K, 1.0, 1.5, &tilted).unwrap().0,
        primitive::frustum(K, 1.0, 0.4, 1.0, &tilted).unwrap().0,
        primitive::sphere(K, 1.0, &tilted).unwrap().0,
        primitive::torus(K, 2.0, 0.5, &tilted).unwrap().0,
    ];
    // A plate with two holes.
    let plate = profile(
        Frame::WORLD,
        vec![
            polygon(1, &[(0.0, 0.0), (4.0, 0.0), (4.0, 2.0), (0.0, 2.0)]),
            circle(2, (1.0, 1.0), 0.5),
            circle(3, (3.0, 1.0), 0.4),
        ],
    );
    out.push(
        extrude(K, &plate, Vector3::new(0.0, 0.0, 1.0), 0.0, 0.5)
            .unwrap()
            .0,
    );
    // A ring with a square section turned a full turn about y, and a
    // half-disk turned three quarters of a turn: sphere caps and planes
    // with arcs.
    let ring = profile(
        Frame::WORLD,
        vec![polygon(
            4,
            &[(1.0, -0.5), (2.0, -0.5), (2.0, 0.5), (1.0, 0.5)],
        )],
    );
    let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0)).unwrap();
    out.push(revolve(K, &ring, axis, 0.0, TAU).unwrap().0);
    let f = Frame2::new(Point2::ORIGIN, Vector2::new(0.0, -1.0), true).unwrap();
    let half = profile(
        Frame::WORLD,
        vec![Loop {
            key: LoopKey(5),
            segments: vec![
                seg(
                    500,
                    Curve2::Circle2(Circle2::new(f, 1.0).unwrap()),
                    0.0,
                    core::f64::consts::PI,
                ),
                seg(
                    501,
                    Curve2::Line2(
                        Line2::through(Point2::new(0.0, 1.0), Point2::new(0.0, -1.0)).unwrap(),
                    ),
                    0.0,
                    2.0,
                ),
            ],
        }],
    );
    out.push(
        revolve(K, &half, axis, 0.0, 1.5 * core::f64::consts::PI)
            .unwrap()
            .0,
    );
    out
}

/// The distance from `p` to the face's boundary edges.
fn to_boundary(b: &Body, f: FaceId, p: Point3) -> f64 {
    let mut d = f64::INFINITY;
    for l in b.face_loops(f) {
        if let Some(v) = b.loop_(l).unwrap().vertex {
            d = d.min(b.vertex(v).unwrap().point.distance(p));
        }
        for fin in b.loop_fins(l) {
            let e = b.edge(b.fin(fin).unwrap().edge).unwrap();
            let c = b.curve(e.curve).unwrap();
            for k in 0..=200 {
                let t = e.interval[0] + (e.interval[1] - e.interval[0]) * f64::from(k) / 200.0;
                d = d.min(c.point(t).distance(p));
            }
        }
    }
    d
}

#[test]
fn points_agree_with_the_mesh() {
    for b in bodies() {
        let m = tessellate(&b, &MeshParams::new(1e-3, 0.5)).unwrap();
        for fm in &m.faces {
            let f = fm.face;
            let s = b.surface(b.face(f).unwrap().surface).unwrap();
            let tris: Vec<[Point3; 3]> = m.triangles
                [fm.triangles.start as usize..fm.triangles.end as usize]
                .iter()
                .map(|t| t.map(|c| m.positions[m.corners[c as usize].position as usize]))
                .collect();
            let uvs: Vec<Point2> = (fm.corners.start..fm.corners.end)
                .map(|c| m.corners[c as usize].uv)
                .collect();
            let (mut lo, mut hi) = (uvs[0], uvs[0]);
            for q in &uvs {
                lo = Point2::new(lo.x.min(q.x), lo.y.min(q.y));
                hi = Point2::new(hi.x.max(q.x), hi.y.max(q.y));
            }
            let [[du0, du1], [dv0, dv1]] = s.domain();
            let grow = |a: f64, b: f64, d0: f64, d1: f64| {
                let w = (b - a).max(0.5);
                ((a - 0.3 * w).max(d0), (b + 0.3 * w).min(d1))
            };
            let (u0, u1) = grow(lo.x, hi.x, du0, du1);
            let (v0, v1) = grow(lo.y, hi.y, dv0, dv1);
            let n = 17;
            for i in 0..=n {
                for j in 0..=n {
                    let u = u0 + (u1 - u0) * f64::from(i) / f64::from(n);
                    let v = v0 + (v1 - v0) * f64::from(j) / f64::from(n);
                    let p = s.point(u, v);
                    if to_boundary(&b, f, p) < 0.05 {
                        continue;
                    }
                    let on_mesh = tris.iter().any(|&t| point_triangle(p, t) <= 2e-3);
                    let place = point_in_face(&b, f, Point2::new(u, v)).unwrap();
                    let want = if on_mesh {
                        Place::Inside
                    } else {
                        Place::Outside
                    };
                    assert_eq!(place, want, "face {f:?} at ({u}, {v})");
                }
            }
        }
    }
}
