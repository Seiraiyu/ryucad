//! Fills of seeded boundaries against dense sampling (design 295, plan
//! 12.G).
//!
//! - **Free boundaries.** Three to eight sides round a wobbling loop,
//!   each a line, a quadratic or cubic Bézier curve or a rational
//!   quadratic, some given backwards: every fill is sound (checks,
//!   crosses nothing, meshes), every side lies within its reported
//!   deviation of the fill's edges at 17 points, and across every inner
//!   edge the two faces' normals are within the reported inner angle at
//!   17 points.
//! - **Tangent sides.** A seeded quarter cylinder sheet's side and three
//!   sides leaving its corners in its tangent plane, the far one bulging
//!   off it: the fill's normal is
//!   within the side's reported angle of the cylinder's at 17 points.
//!
//! A typed refusal is counted, never a failure. Release only.

use ryukei::Error;
use ryukei::check::{check, check_intersections};
use ryukei::fill::{FillBoundary, FillOptions, FillReport, FillSide, fill};
use ryukei::geom::{Curve, Cylinder, Frame, KnotVector, NurbsCurve, Point3, Surface, Vector3};
use ryukei::math;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Name, OpKey, Role, SheetRole};
use ryukei::topo::{Body, EdgeId, FinId, Sense};
use ryukei_testkit::Lcg;
use std::sync::Arc;

/// Seeds tried for each test.
const SEEDS: u64 = 48;

const OPTIONS: FillOptions = FillOptions {
    tolerance: 1e-7,
    angle: 0.01,
};

fn sound(b: &Body) -> bool {
    check(b).is_empty()
        && check_intersections(b).is_empty()
        && tessellate(b, &MeshParams::new(1e-3, 0.5))
            .is_ok_and(|m| m.faults.is_empty() && check_mesh(b, &m).is_ok())
}

fn nurbs(points: Vec<Point3>, weights: Option<Vec<f64>>) -> NurbsCurve {
    let d = points.len() - 1;
    let k = KnotVector::clamped(d, &[0.0, 1.0], &[d + 1, d + 1]).unwrap();
    NurbsCurve::new(k, points, weights).unwrap()
}

/// A seeded loop's sides as curves, each from one corner to the next.
fn free_boundary(g: &mut Lcg) -> Vec<NurbsCurve> {
    let n = 3 + (g.next_u64() % 6) as usize;
    let corners: Vec<Point3> = (0..n)
        .map(|k| {
            let a = core::f64::consts::TAU * (k as f64 + g.next_f64(-0.2, 0.2)) / n as f64;
            let r = g.next_f64(0.8, 1.2);
            Point3::new(r * math::cos(a), r * math::sin(a), g.next_f64(-0.3, 0.3))
        })
        .collect();
    (0..n)
        .map(|k| {
            let (a, b) = (corners[k], corners[(k + 1) % n]);
            let mid = Point3::ORIGIN + (a.to_vector() + b.to_vector()) * 0.5;
            // Outwards from the loop's middle, and up or down.
            let out = Vector3::new(mid.x, mid.y, 0.0) * g.next_f64(-0.2, 0.3)
                + Vector3::new(0.0, 0.0, g.next_f64(-0.2, 0.2));
            match g.next_u64() % 4 {
                0 => nurbs(vec![a, b], None),
                1 => nurbs(vec![a, mid + out, b], None),
                2 => {
                    let w = g.next_f64(0.5, 1.5);
                    nurbs(vec![a, mid + out, b], Some(vec![1.0, w, 1.0]))
                }
                _ => {
                    let lean = Vector3::new(0.0, 0.0, g.next_f64(-0.1, 0.1));
                    let p1 = a + (b - a) / 3.0 + out + lean;
                    let p2 = a + (b - a) * (2.0 / 3.0) + out - lean;
                    nurbs(vec![a, p1, p2, b], None)
                }
            }
        })
        .collect()
}

/// The least distance from `p` to the curve of any laminar edge.
fn to_boundary(b: &Body, p: Point3) -> f64 {
    b.edges()
        .filter(|&e| b.is_laminar(e))
        .map(|e| {
            let edge = b.edge(e).unwrap();
            let c = b.curve(edge.curve).unwrap();
            let [t0, t1] = edge.interval;
            (0..=64)
                .map(|i| c.point(t0 + (t1 - t0) * i as f64 / 64.0))
                .fold(f64::INFINITY, |m, q| m.min(q.distance(p)))
                .min(c.project(p).point.distance(p))
        })
        .fold(f64::INFINITY, f64::min)
}

/// A face's normal (its outward one) through fin `f` at the edge's `t`.
fn normal(b: &Body, f: FinId, t: f64) -> Vector3 {
    let fin = b.fin(f).unwrap();
    let face = b.face(b.loop_(fin.loop_).unwrap().face).unwrap();
    let uv = b.fin_uv(f, t).unwrap();
    let n = b
        .surface(face.surface)
        .unwrap()
        .normal(uv.x, uv.y)
        .unwrap()
        .get();
    if face.sense == Sense::Forward { n } else { -n }
}

/// The angle between two vectors' lines.
fn angle(a: Vector3, b: Vector3) -> f64 {
    math::atan2(a.cross(b).length(), a.dot(b).abs())
}

/// The largest angle between the two faces of an inner edge at 17 points.
fn across(b: &Body, e: EdgeId) -> f64 {
    let fins: Vec<FinId> = b.edge_fins(e).collect();
    let [t0, t1] = b.edge(e).unwrap().interval;
    (0..=16)
        .map(|i| {
            let t = t0 + (t1 - t0) * i as f64 / 16.0;
            angle(normal(b, fins[0], t), normal(b, fins[1], t))
        })
        .fold(0.0, f64::max)
}

fn judge_free(sides: &[NurbsCurve], b: &Body, report: &FillReport) {
    assert!(sound(b), "not sound");
    for (k, c) in sides.iter().enumerate() {
        for i in 0..=16 {
            let p = c.point(i as f64 / 16.0);
            let d = to_boundary(b, p);
            assert!(d <= report.deviations[k] + 1e-9, "side {k}: {d}");
        }
    }
    let worst = b
        .edges()
        .filter(|&e| !b.is_laminar(e))
        .map(|e| across(b, e))
        .fold(0.0, f64::max);
    assert!(
        worst <= report.inner.unwrap_or(0.0) + 1e-9,
        "inner {worst} {report:?}"
    );
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn free_boundaries_fill_within_their_reports() {
    let mut g = Lcg::new(0x6669_6c6c);
    let (mut judged, mut refused) = (0, 0);
    for seed in 0..SEEDS {
        let curves = free_boundary(&mut g);
        // Every third seed gives its odd sides backwards.
        let sides: Vec<FillSide<'_>> = curves
            .iter()
            .enumerate()
            .map(|(k, c)| {
                let c = if seed % 3 == 0 && k % 2 == 1 {
                    c.reversed()
                } else {
                    c.clone()
                };
                FillSide::Curve {
                    curve: Curve::Nurbs(Arc::new(c)),
                    range: [0.0, 1.0],
                }
            })
            .collect();
        match fill(OpKey(seed as u128), &FillBoundary { sides }, &OPTIONS) {
            Ok((b, _, report)) => {
                judge_free(&curves, &b, &report);
                judged += 1;
            }
            Err(Error::InvalidFill(e)) => {
                eprintln!("seed {seed}: refused {e:?}");
                refused += 1;
            }
            Err(e) => panic!("seed {seed}: {e:?}"),
        }
    }
    eprintln!("{judged} judged, {refused} refused, of {SEEDS}");
    assert!(judged >= SEEDS * 3 / 4, "too many refused");
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn tangent_sides_fill_within_their_angles() {
    let mut g = Lcg::new(0x7461_6e67);
    let (mut judged, mut refused) = (0, 0);
    for seed in 0..SEEDS / 3 {
        let r = g.next_f64(0.5, 2.0);
        let u1 = g.next_f64(0.5, 2.5);
        let h = g.next_f64(0.5, 1.5);
        let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, r).unwrap());
        let key = OpKey(0x100 + seed as u128);
        let (sheet, _) = ryukei::sheet::from_surface(key, &cyl, [[0.0, u1], [0.0, h]]).unwrap();
        let face = Name::derive(key, Role::Sheet(SheetRole::Face), &[]);
        let side = Name::derive(key, Role::Sheet(SheetRole::SideU1), &[face]);
        // The side at u1 runs up z; the cylinder's tangent there, across
        // it, is t, and its normal n.
        let (cu, su) = (math::cos(u1), math::sin(u1));
        let (t, n) = (Vector3::new(-su, cu, 0.0), Vector3::new(cu, su, 0.0));
        let top = Point3::new(r * cu, r * su, h);
        let bottom = Point3::new(r * cu, r * su, 0.0);
        let (a, drop) = (g.next_f64(0.5, 1.5), g.next_f64(-0.6, 0.6));
        let bulge = g.next_f64(-0.4, 0.4);
        let far_top = top + t * a + n * drop;
        let far_bottom = bottom + t * a + n * drop + Vector3::new(0.0, 0.0, g.next_f64(-0.1, 0.1));
        let curves = [
            nurbs(vec![top, top + t * (0.5 * a), far_top], None),
            // Bulging off the cylinder's tangent plane, so the patch's
            // cross derivative along the side leaves it between the corners.
            nurbs(
                vec![
                    far_top,
                    Point3::ORIGIN
                        + (far_top.to_vector() + far_bottom.to_vector()) * 0.5
                        + n * bulge
                        + t * (0.2 * bulge),
                    far_bottom,
                ],
                None,
            ),
            nurbs(vec![far_bottom, bottom + t * (0.5 * a), bottom], None),
        ];
        let mut sides = vec![FillSide::Edge {
            body: &sheet,
            edge: side,
            tangent: true,
        }];
        sides.extend(curves.iter().map(|c| FillSide::Curve {
            curve: Curve::Nurbs(Arc::new(c.clone())),
            range: [0.0, 1.0],
        }));
        match fill(OpKey(seed as u128), &FillBoundary { sides }, &OPTIONS) {
            Ok((b, _, report)) => {
                assert!(sound(&b), "seed {seed}: not sound");
                let bound = report.angles[0].unwrap();
                // The fill's edge on the cylinder's side: its points' radius
                // is r and the cylinder's normal is radial.
                let e = b
                    .edges()
                    .find(|&e| {
                        let edge = b.edge(e).unwrap();
                        let c = b.curve(edge.curve).unwrap();
                        let [t0, t1] = edge.interval;
                        let m = c.point(0.5 * (t0 + t1));
                        (m - Point3::new(r * cu, r * su, m.z)).length() < 1e-9
                    })
                    .expect("the tangent side's edge");
                let fin = b.edge_fins(e).next().unwrap();
                let [t0, t1] = b.edge(e).unwrap().interval;
                for i in 0..=16 {
                    let tt = t0 + (t1 - t0) * i as f64 / 16.0;
                    let p = b.curve(b.edge(e).unwrap().curve).unwrap().point(tt);
                    let radial = Vector3::new(p.x, p.y, 0.0);
                    let a = angle(normal(&b, fin, tt), radial);
                    assert!(a <= bound + 1e-9, "seed {seed}: {a} > {bound}");
                }
                judged += 1;
            }
            Err(Error::InvalidFill(e)) => {
                eprintln!("seed {seed}: refused {e:?}");
                refused += 1;
            }
            Err(e) => panic!("seed {seed}: {e:?}"),
        }
    }
    eprintln!("{judged} judged, {refused} refused, of {}", SEEDS / 3);
    assert!(judged >= SEEDS / 3 * 3 / 4, "too many refused");
}
