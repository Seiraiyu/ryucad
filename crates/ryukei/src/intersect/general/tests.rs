use super::*;
use crate::geom::{Curve, Cylinder, Frame, Plane, Sphere, Vector3};
use crate::intersect::{SurfaceHit, surface_surface};
use core::f64::consts::TAU;
use std::sync::Arc;

fn region() -> Box3 {
    Box3::new(Point3::new(-5.0, -5.0, -5.0), Point3::new(5.0, 5.0, 5.0))
}

/// An analytic surface's exact NURBS form over a box of its parameters.
fn spline(s: &Surface, range: [[f64; 2]; 2]) -> Surface {
    let (n, _) = s.to_nurbs(range).unwrap();
    Surface::Nurbs(Arc::new(n))
}

fn frame(o: (f64, f64, f64), z: (f64, f64, f64)) -> Frame {
    Frame::new(
        Point3::new(o.0, o.1, o.2),
        Vector3::new(z.0, z.1, z.2),
        Vector3::new(1.0, 0.2, 0.1),
    )
    .unwrap()
}

/// Every traced point on both surfaces.
fn on_both(a: &Surface, b: &Surface, topo: &Topology) {
    for br in &topo.branches {
        for &p in &br.points {
            assert!(a.distance(p) <= 1e-9 && b.distance(p) <= 1e-9, "{p:?}");
        }
    }
    for &p in &topo.points {
        assert!(a.distance(p) <= 1e-8 && b.distance(p) <= 1e-8, "{p:?}");
    }
}

#[test]
fn two_spline_planes_meet_in_a_line() {
    let a = spline(
        &Surface::Plane(Plane::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)))),
        [[-2.0, 2.0], [-2.0, 2.0]],
    );
    let b = spline(
        &Surface::Plane(Plane::new(frame((0.3, 0.0, 0.0), (1.0, 0.1, 0.4)))),
        [[-2.0, 2.0], [-2.0, 2.0]],
    );
    let topo = trace(&a, &b, &region(), &Meter::free()).unwrap(); // unmetered: a test
    assert_eq!(topo.branches.len(), 1, "{topo:?}");
    assert!(!topo.branches[0].closed);
    on_both(&a, &b, &topo);
}

#[test]
fn a_spline_cylinder_and_a_spline_plane_meet_in_a_loop() {
    let cyl =
        Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 0.7).unwrap());
    let a = spline(&cyl, [[0.0, TAU], [-2.0, 2.0]]);
    let b = spline(
        &Surface::Plane(Plane::new(frame((0.0, 0.0, 0.2), (0.3, 0.1, 1.0)))),
        [[-2.0, 2.0], [-2.0, 2.0]],
    );
    let topo = trace(&a, &b, &region(), &Meter::free()).unwrap(); // unmetered: a test
    assert_eq!(topo.branches.len(), 1, "{topo:?}");
    assert!(topo.branches[0].closed, "{topo:?}");
    on_both(&a, &b, &topo);
    // Fitted, the loop is within its tolerance of both.
    let SurfaceHit::Meet { branches, .. } = surface_surface(&a, &b, &region()).unwrap() else {
        panic!("coincident")
    };
    assert_eq!(branches.len(), 1);
    let br = &branches[0];
    for k in 0..=64 {
        let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 64.0;
        let p = br.curve.point(t);
        assert!(a.distance(p) <= br.tolerance && b.distance(p) <= br.tolerance);
        assert!(cyl.distance(p) <= br.tolerance + 1e-12);
    }
}

#[test]
fn two_spline_spheres() {
    let s1 = Surface::Sphere(Sphere::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.0).unwrap());
    let s2 = Surface::Sphere(Sphere::new(frame((0.8, 0.3, 0.2), (0.2, 0.0, 1.0)), 0.9).unwrap());
    let a = spline(&s1, [[0.0, TAU], [-1.5, 1.5]]);
    let b = spline(&s2, [[0.0, TAU], [-1.5, 1.5]]);
    let topo = trace(&a, &b, &region(), &Meter::free()).unwrap(); // unmetered: a test
    assert_eq!(topo.branches.len(), 1, "{topo:?}");
    on_both(&a, &b, &topo);
    // Touching from outside, at one point.
    let s3 = Surface::Sphere(Sphere::new(frame((1.5, 0.0, 0.0), (0.2, 0.0, 1.0)), 0.5).unwrap());
    let c = spline(&s3, [[0.0, TAU], [-1.5, 1.5]]);
    let topo = trace(&a, &c, &region(), &Meter::free()).unwrap(); // unmetered: a test
    assert!(topo.branches.is_empty(), "{topo:?}");
    assert_eq!(topo.points.len(), 1, "{topo:?}");
    on_both(&a, &c, &topo);
    // Apart.
    let s4 = Surface::Sphere(Sphere::new(frame((3.0, 0.0, 0.1), (0.2, 0.0, 1.0)), 0.5).unwrap());
    let d = spline(&s4, [[0.0, TAU], [-1.5, 1.5]]);
    let topo = trace(&a, &d, &region(), &Meter::free()).unwrap(); // unmetered: a test
    assert!(topo.branches.is_empty() && topo.points.is_empty());
}

#[test]
fn small_systems() {
    let m = [
        [2.0, 1.0, 0.0, 0.0],
        [0.0, 3.0, 1.0, 0.0],
        [1.0, 0.0, 4.0, 1.0],
        [0.0, 0.0, 1.0, 5.0],
    ];
    let x = solve4(m, [3.0, 4.0, 6.0, 6.0]).unwrap();
    for (row, r) in m.iter().zip([3.0, 4.0, 6.0, 6.0]) {
        let s: f64 = row.iter().zip(x).map(|(a, b)| a * b).sum();
        assert!((s - r).abs() < 1e-12);
    }
    let jac = m.map(|r| r.map(Iv::point));
    let f = |x: [f64; 4]| -> [Iv; 4] {
        core::array::from_fn(|i| Iv::point((0..4).map(|j| m[i][j] * x[j]).sum::<f64>()))
    };
    let around = [[-0.1, 0.1]; 4];
    assert_eq!(krawczyk4(f([0.0; 4]), jac, around), Some(true));
    let away = [[1.0, 1.2]; 4];
    assert_eq!(krawczyk4(f([1.1; 4]), jac, away), Some(false));
}

#[test]
fn coincident_surfaces_are_said_to_be() {
    let cyl =
        Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 0.7).unwrap());
    let a = spline(&cyl, [[0.0, TAU], [-2.0, 2.0]]);
    let b = spline(&cyl, [[0.5, 2.0], [-1.0, 1.0]]);
    for (x, y) in [(&a, &cyl), (&cyl, &a), (&a, &b), (&b, &a)] {
        assert_eq!(
            surface_surface(x, y, &region()),
            Ok(SurfaceHit::Coincident { same: true }),
            "{x:?} {y:?}"
        );
    }
}

#[test]
fn an_extrusion_taller_than_the_region_is_traced_to_its_edge() {
    // An extruded ellipse (unbounded along its direction, so its first
    // cells end where the region does) against a steep spline plane that
    // reaches past the region: both branches leave through the cells'
    // cut-off ends, which lie on the region's faces.
    let ellipse =
        crate::geom::Ellipse::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 0.8, 0.5).unwrap();
    let a = Surface::Extrusion(Arc::new(
        crate::geom::Extrusion::new(Curve::Ellipse(ellipse), Vector3::new(0.0, 0.0, 1.0)).unwrap(),
    ));
    let b = spline(
        &Surface::Plane(Plane::new(frame((0.3, 0.0, 0.0), (1.0, 0.1, 0.2)))),
        [[-4.0, 4.0], [-4.0, 4.0]],
    );
    let region = Box3::new(Point3::new(-2.0, -2.0, -1.0), Point3::new(2.0, 2.0, 1.0));
    let topo = trace(&a, &b, &region, &Meter::free()).unwrap(); // unmetered: a test
    assert_eq!(topo.branches.len(), 2, "{topo:?}");
    for br in &topo.branches {
        assert!(!br.closed);
        // Each runs from the region's floor to its ceiling.
        let z = |p: &Point3| p.z;
        let (lo, hi) = br
            .points
            .iter()
            .map(z)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| {
                (l.min(x), h.max(x))
            });
        assert!(lo <= -1.0 + 1e-6 && hi >= 1.0 - 1e-6, "{lo} {hi}");
    }
    on_both(&a, &b, &topo);
}

/// The spline cylinder and spline plane of the loop test.
fn crossing_splines() -> (Surface, Surface, Box3) {
    let cyl =
        Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 0.7).unwrap());
    let a = spline(&cyl, [[0.0, TAU], [-2.0, 2.0]]);
    let b = spline(
        &Surface::Plane(Plane::new(frame((0.0, 0.0, 0.2), (0.3, 0.1, 1.0)))),
        [[-2.0, 2.0], [-2.0, 2.0]],
    );
    (a, b, region())
}

#[test]
fn a_trace_halts_at_its_budget_and_charges_alike_on_any_thread_count() {
    use crate::work::Work;
    let (a, b, region) = crossing_splines();
    let free = Work::unlimited();
    let m = Meter::new(&free);
    assert!(super::trace(&a, &b, &region, &m).is_ok());
    let spent = m.spent();
    assert!(spent > 0);
    for threads in [1, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let got = pool.install(|| {
            let w = Work::unlimited();
            let m = Meter::new(&w);
            super::trace(&a, &b, &region, &m).unwrap();
            m.spent()
        });
        assert_eq!(got, spent);
    }
    let w = Work::unlimited().with_budget(spent / 2);
    let m = Meter::new(&w);
    assert_eq!(
        super::trace(&a, &b, &region, &m).err(),
        Some(Failure::Uncertain)
    );
    assert!(m.halted());
    // A cap is Uncertain without halting.
    let w = Work::unlimited();
    let m = Meter::new(&w);
    let r = m.capped_by(spent / 2, || super::trace(&a, &b, &region, &m));
    assert_eq!(r.err(), Some(Failure::Uncertain));
    assert!(!m.halted());
}

#[test]
fn apart_within_gives_up_when_halted() {
    use crate::work::Work;
    let (a, b, _) = crossing_splines();
    let w = Work::unlimited().with_budget(0);
    let m = Meter::new(&w);
    let ra = a.domain();
    assert_eq!(
        super::apart_within(&a, ra, &b, None, &|_| false, &|_| false, 1 << 12, &m),
        None
    );
}

#[test]
fn the_second_surface_is_split_neither_as_the_first_nor_its_mirror() {
    // On the attempt off the surfaces' pieces; the first two split both
    // surfaces alike, as they always have.
    for (attempt, &f) in super::SPLITS.iter().enumerate() {
        let g = super::second_fraction(attempt);
        let (s, t) = (super::CELL_SHIFTS[attempt], super::second_shift(attempt));
        if s > 0.0 {
            assert!(
                (g - f).abs() > 1e-3 && (g - (1.0 - f)).abs() > 1e-3,
                "{attempt}: {f} {g}"
            );
            assert!(
                (t - s).abs() > 1e-3 && (t - (1.0 - s)).abs() > 1e-3,
                "{attempt}: {s} {t}"
            );
        } else {
            assert_eq!((g, t), (f, 0.0), "{attempt}");
        }
    }
}

/// A face of the footed corner pair (issue #177) and two boxes of it
/// the general tracer's search met side by side, too small for Krawczyk's
/// test to contract (`E`'s rounding at a point, times the inverse
/// Jacobian, is wider than they are). The crossing lies in the second.
const CORNER_AT: f64 = -3.117_287_711_041_021_8;
const CORNER_X: [[[f64; 2]; 3]; 2] = [
    [
        [0.167_789_468_676_068_42, 0.167_789_468_681_042_77],
        [1.056_348_278_084_568_3, 1.056_348_278_094_517],
        [-3.117_287_711_222_461_5, -3.117_287_711_041_021_8],
    ],
    [
        [0.167_789_468_681_042_77, 0.167_789_468_686_017_16],
        [1.056_348_278_084_568_3, 1.056_348_278_094_517],
        [-3.117_287_711_222_461_5, -3.117_287_711_041_021_8],
    ],
];

fn corner_face<'a>(a: &'a Surface, b: &'a Surface, m: &'a Meter<'a>) -> Face<'a> {
    Face {
        sheets: [Sheet::of(a), Sheet::of(b)],
        meter: m,
        side: 0,
        param: 1,
        at: CORNER_AT,
        reach: [[f64::NEG_INFINITY, f64::INFINITY]; 3],
    }
}

#[test]
fn a_face_crossing_rounding_hides_is_certified_by_inflation() {
    let (a, b) = crate::testing::footed_corner_pair();
    let m = Meter::free(); // unmetered: a test
    let face = corner_face(&a, &b, &m);
    let x = CORNER_X[0];
    let (em, jac) = face.enclose(x).unwrap();
    let (at, _) = face
        .enclose(x.map(|[lo, hi]| {
            let c = (lo + hi) / 2.0;
            [c, c]
        }))
        .unwrap();
    assert_eq!(krawczyk3(at, jac, x), None, "{em:?}");
    assert!(outgrown(&at, &jac, x));
    let (r, b) = face.inflated(x).expect("certified");
    // The box holds no crossing: the one its certified box holds lies
    // beyond it.
    assert!((0..3).all(|k| b[k][0] <= x[k][0] && x[k][1] <= b[k][1]));
    assert!((0..3).all(|k| b[k][0] <= r[k] && r[k] <= b[k][1]));
    assert!(!(0..3).all(|k| x[k][0] <= r[k] && r[k] < x[k][1]), "{r:?}");
    let (at, _) = face
        .enclose(b.map(|[lo, hi]| {
            let c = (lo + hi) / 2.0;
            [c, c]
        }))
        .unwrap();
    let (_, jac) = face.enclose(b).unwrap();
    assert_eq!(krawczyk3(at, jac, b), Some(true));
}

#[test]
fn inflation_stays_within_the_faces_reach() {
    let (a, b) = crate::testing::footed_corner_pair();
    let m = Meter::free(); // unmetered: a test
    let mut face = corner_face(&a, &b, &m);
    let x = CORNER_X[0];
    let (_, b) = face.inflated(x).expect("certified");
    // The certified box reaches past `x`: held to `x`, inflation gives up
    // (and splitting decides), as it does where an offset's loose
    // enclosures, not rounding, make a box outgrown.
    assert!((0..3).any(|k| b[k][0] < x[k][0] || x[k][1] < b[k][1]));
    face.reach = x;
    assert_eq!(face.inflated(x), None);
    face.reach = b;
    assert!(face.inflated(x).is_some());
}

#[test]
fn neighbouring_boxes_claim_an_inflated_crossing_once() {
    let (a, b) = crate::testing::footed_corner_pair();
    let m = Meter::free(); // unmetered: a test
    let face = corner_face(&a, &b, &m);
    for order in [[0, 1], [1, 0]] {
        let shared = Inflated::default();
        let claims = order.map(|i| face.claim(CORNER_X[i], &shared).expect("certified"));
        let [first, second] = claims;
        // Both name the same root, and exactly one box (the second)
        // holds it.
        let roots: Vec<[f64; 3]> = shared.borrow().iter().map(|&(r, _)| r).collect();
        assert!(roots.windows(2).all(|w| w[0] == w[1]), "{roots:?}");
        let held = |c: &Claim, i: usize| (c.0.is_some(), i);
        let mut by_box = [held(&first, order[0]), held(&second, order[1])];
        by_box.sort_by_key(|&(_, i)| i);
        assert_eq!(by_box, [(false, 0), (true, 1)], "{order:?}");
    }
}

#[test]
fn a_periodic_direction_cut_off_its_start_folds_its_end_onto_its_start() {
    // A cylinder's first cells moved on by the third attempt's shifts end a
    // period past where they start, beyond the domain's end: a crossing on
    // the face there is the one on the face at their start (issue #177's
    // footed corners found each such crossing twice and refused it).
    let cyl =
        Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 0.7).unwrap());
    let m = Meter::free(); // unmetered: a test
    for side in 0..2 {
        let mut t = Tracer::new([&cyl, &cyl], &region(), SPLITS[2], &m);
        t.shift = CELL_SHIFTS[2];
        t.attempt = 2;
        let firsts = [
            cells_off(&cyl, &t.region, t.shift),
            cells_off(&cyl, &t.region, second_shift(2)),
        ];
        t.note_wraps(&firsts);
        let [lo, hi] = t.wraps[side][0].expect("a whole period");
        let [d0, _] = cyl.domain()[0];
        assert!(lo > d0, "{side}: {lo}");
        assert!((hi - lo - TAU).abs() < 1e-12, "{side}: {lo} {hi}");
        assert_eq!(t.plane(side, 0, hi), t.plane(side, 0, lo), "{side}");
        assert_eq!(t.wraps[side][1], None, "{side}");
    }
}

#[test]
fn a_footed_corners_tangencies_are_saddles_with_four_ends() {
    // Two fillets of a ball corner's cap edges (issue #177) touch where the
    // corner triangle's vertex is and again where their tubes' tops meet:
    // both are saddles, two branches crossing at each.
    let (a, b) = crate::testing::footed_corner_pair();
    let region = crate::testing::footed_corner_region();
    let m = Meter::free(); // unmetered: a test
    let topo = trace(&a, &b, &region, &m).expect("traced");
    assert!(topo.points.is_empty(), "{:?}", topo.points);
    assert!(topo.tangents.is_empty());
    on_both(&a, &b, &topo);
    // The branches' ends, gathered where they coincide.
    let mut ends: Vec<(Point3, usize)> = Vec::new();
    for br in &topo.branches {
        assert!(!br.closed);
        for p in [br.points[0], *br.points.last().unwrap()] {
            match ends.iter_mut().find(|(q, _)| q.distance(p) <= 1e-9) {
                Some((_, n)) => *n += 1,
                None => ends.push((p, 1)),
            }
        }
    }
    let meeting: Vec<_> = ends.iter().filter(|&&(_, n)| n > 1).collect();
    assert_eq!(meeting.len(), 2, "{ends:?}");
    for &&(p, n) in &meeting {
        assert_eq!(n, 4, "{p:?}");
        // Tangent there: the normals are parallel.
        let fa = a.project(p);
        let fb = b.project(p);
        let (na, nb) = (
            a.normal(fa.param.0, fa.param.1).unwrap(),
            b.normal(fb.param.0, fb.param.1).unwrap(),
        );
        assert!(na.get().cross(nb.get()).length() < 1e-6, "{p:?}");
    }
}
