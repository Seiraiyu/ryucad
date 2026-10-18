//! Profiles: random regions that don't touch validate and nest as
//! sampling says, a crossing or touch is always caught, and validation is
//! deterministic and doesn't depend on where loops start or the order
//! they come in.

use core::f64::consts::{FRAC_1_SQRT_2, TAU};
use proptest::prelude::*;
use ryukei::geom::{
    Circle2, Curve2, Frame, Frame2, Line2, NurbsCurve2, Point2, Point3, Vector2, Vector3,
};
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidLoop, ValidProfile};
use ryukei::tol::LINEAR_RESOLUTION;
use ryukei::{Error, ProfileError, math};
use ryukei_testkit::Lcg;
use ryukei_testkit::nurbs::blob2;
use std::sync::Arc;

/// Fresh keys, shared by loops and segments.
struct Keys(u128);

impl Keys {
    fn next(&mut self) -> u128 {
        self.0 += 1;
        self.0
    }
}

fn line(keys: &mut Keys, a: Point2, b: Point2) -> Segment {
    Segment {
        key: SegmentKey(keys.next()),
        curve: Curve2::Line2(Line2::through(a, b).unwrap()),
        t0: 0.0,
        t1: a.distance(b),
    }
}

fn piece(keys: &mut Keys, curve: &Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(keys.next()),
        curve: curve.clone(),
        t0,
        t1,
    }
}

/// A turned rectangle inside the circle of radius `r` about `c`, holding
/// the circle of radius `0.56 r`.
fn rectangle(g: &mut Lcg, keys: &mut Keys, c: Point2, r: f64) -> Vec<Segment> {
    let a = r * FRAC_1_SQRT_2 * g.next_f64(0.8, 1.0);
    let b = r * FRAC_1_SQRT_2 * g.next_f64(0.8, 1.0);
    let (s, co) = math::sin_cos(g.next_f64(0.0, TAU));
    let corner = |x: f64, y: f64| Point2::new(c.x + x * co - y * s, c.y + x * s + y * co);
    let pts = [corner(-a, -b), corner(a, -b), corner(a, b), corner(-a, b)];
    (0..4)
        .map(|i| line(keys, pts[i], pts[(i + 1) % 4]))
        .collect()
}

/// A circle of radius `r` about `c`, whole or in two or three arcs.
fn round(g: &mut Lcg, keys: &mut Keys, c: Point2, r: f64) -> Vec<Segment> {
    let x = Vector2::new(g.next_f64(-1.0, 1.0), g.next_f64(0.1, 1.0));
    let f = Frame2::new(c, x, g.next_u64().is_multiple_of(2)).unwrap();
    let curve = Curve2::Circle2(Circle2::new(f, r).unwrap());
    match g.next_u64() % 3 {
        0 => vec![piece(keys, &curve, 0.0, TAU)],
        1 => {
            let a = g.next_f64(1.0, 5.0);
            vec![piece(keys, &curve, 0.0, a), piece(keys, &curve, a, TAU)]
        }
        _ => {
            let a = g.next_f64(0.5, 2.5);
            let b = g.next_f64(3.0, 5.5);
            vec![
                piece(keys, &curve, a - TAU, b - TAU),
                piece(keys, &curve, b - TAU, 0.0),
                piece(keys, &curve, 0.0, a),
            ]
        }
    }
}

/// A NURBS blob inside the circle of radius `r` about `c`, holding the
/// circle of radius `0.55 r`: whole or in two segments.
fn blob(g: &mut Lcg, keys: &mut Keys, c: Point2, r: f64) -> Vec<Segment> {
    let curve = Curve2::Nurbs2(Arc::new(blob2(g, c, r)));
    let [d0, d1] = curve.domain();
    if g.next_u64().is_multiple_of(2) {
        vec![piece(keys, &curve, d0, d1)]
    } else {
        let s = g.next_f64(d0 + 1.0, d1 - 1.0);
        vec![
            piece(keys, &curve, s, d1 + 0.5),
            piece(keys, &curve, d1 + 0.5, s + (d1 - d0)),
        ]
    }
}

/// A loop of a random shape inside the circle of radius `r` about `c`
/// that holds the circle of radius `0.55 r`, in a random direction and
/// starting at a random segment.
fn shape(g: &mut Lcg, keys: &mut Keys, c: Point2, r: f64) -> Loop {
    let key = LoopKey(keys.next());
    let mut segments = match g.next_u64() % 3 {
        0 => rectangle(g, keys, c, r),
        1 => round(g, keys, c, r),
        _ => blob(g, keys, c, r),
    };
    if g.next_u64().is_multiple_of(2) {
        segments = segments.iter().rev().map(reversed).collect();
    }
    let k = (g.next_u64() as usize) % segments.len();
    segments.rotate_left(k);
    Loop { key, segments }
}

fn reversed(s: &Segment) -> Segment {
    Segment {
        t0: s.t1,
        t1: s.t0,
        ..s.clone()
    }
}

/// A shape with up to two holes, each possibly with an island: holes fit
/// in the `0.55 r` circle every shape holds.
fn region(g: &mut Lcg, keys: &mut Keys, loops: &mut Vec<Loop>, c: Point2, r: f64, depth: u32) {
    loops.push(shape(g, keys, c, r));
    if depth == 2 {
        return;
    }
    match g.next_u64() % 3 {
        0 => {}
        1 => region(g, keys, loops, c, 0.25 * r, depth + 1),
        _ => {
            for side in [-1.0, 1.0] {
                let at = Point2::new(c.x + side * 0.3 * r, c.y);
                region(g, keys, loops, at, 0.2 * r, depth + 1);
            }
        }
    }
}

/// A random profile: regions in a three by three grid of cells, in a
/// shuffled order, in a random frame.
fn random_profile(seed: u64) -> Profile {
    let mut g = Lcg::new(seed);
    let mut keys = Keys(u128::from(g.next_u64()) << 32);
    let mut loops = Vec::new();
    for cell in 0..9 {
        if cell != 4 && g.next_u64().is_multiple_of(2) {
            continue;
        }
        let c = Point2::new(
            10.0 * f64::from(cell % 3) + g.next_f64(-0.5, 0.5),
            10.0 * f64::from(cell / 3) + g.next_f64(-0.5, 0.5),
        );
        let r = g.next_f64(2.0, 4.0);
        region(&mut g, &mut keys, &mut loops, c, r, 0);
    }
    shuffle(&mut g, &mut loops);
    let frame = Frame::new(
        Point3::new(g.next_f64(-100.0, 100.0), g.next_f64(-100.0, 100.0), 0.0),
        Vector3::new(g.next_f64(-1.0, 1.0), g.next_f64(-1.0, 1.0), 1.0),
        Vector3::new(1.0, g.next_f64(-1.0, 1.0), 0.0),
    )
    .unwrap();
    Profile { frame, loops }
}

fn shuffle<T>(g: &mut Lcg, items: &mut [T]) {
    for i in (1..items.len()).rev() {
        let j = (g.next_u64() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// A loop as a dense polygon.
fn sampled(l: &Loop) -> Vec<Point2> {
    l.segments
        .iter()
        .flat_map(|s| {
            (0..256).map(move |i| s.curve.point(s.t0 + (s.t1 - s.t0) * f64::from(i) / 256.0))
        })
        .collect()
}

/// Whether `p` is inside a polygon, by counting crossings of a ray.
fn inside(p: Point2, poly: &[Point2]) -> bool {
    let mut odd = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if x > p.x {
                odd = !odd;
            }
        }
    }
    odd
}

/// Each loop's depth and parent by sampling.
fn brute_force(p: &Profile) -> Vec<(usize, Option<usize>)> {
    let polys: Vec<_> = p.loops.iter().map(sampled).collect();
    let containers: Vec<Vec<usize>> = p
        .loops
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let q = l.segments[0].curve.point(l.segments[0].t0);
            (0..p.loops.len())
                .filter(|&j| j != i && inside(q, &polys[j]))
                .collect()
        })
        .collect();
    containers
        .iter()
        .map(|c| {
            let parent = c.iter().copied().max_by_key(|&j| containers[j].len());
            (c.len(), parent)
        })
        .collect()
}

/// The oriented loop is the input loop, maybe reversed, starting anywhere,
/// with consistent vertices.
fn check_loop(given: &Loop, got: &ValidLoop, ccw: bool) {
    assert_eq!(got.key, given.key);
    assert_eq!(got.area > 0.0, ccw);
    let n = given.segments.len();
    assert_eq!(
        (got.segments.len(), got.vertices.len(), got.tolerances.len()),
        (n, n, n)
    );
    let forward = got.segments.iter().all(|s| given.segments.contains(s));
    let back: Vec<_> = got.segments.iter().map(reversed).collect();
    assert!(forward || back.iter().all(|s| given.segments.contains(s)));
    for (i, s) in got.segments.iter().enumerate() {
        assert_eq!(got.vertices[i], s.curve.point(s.t0));
        assert_eq!(got.tolerances[i], LINEAR_RESOLUTION);
        let next = &got.segments[(i + 1) % n];
        assert!(s.curve.point(s.t1).distance(next.curve.point(next.t0)) <= LINEAR_RESOLUTION);
    }
}

/// The lumps match the brute-force nesting, in input order, with every
/// loop used once and turned to its role.
fn check_nesting(p: &Profile, v: &ValidProfile) {
    let brute = brute_force(p);
    let index = |k: LoopKey| p.loops.iter().position(|l| l.key == k).unwrap();
    let mut seen = vec![false; p.loops.len()];
    let mut last_outer = None;
    for lump in v.lumps() {
        let o = index(lump.outer.key);
        assert!(
            brute[o].0.is_multiple_of(2),
            "outer loop {o} at depth {}",
            brute[o].0
        );
        assert!(last_outer < Some(o), "lumps out of order");
        last_outer = Some(o);
        check_loop(&p.loops[o], &lump.outer, true);
        seen[o] = true;
        let mut last_hole = None;
        for hole in &lump.holes {
            let h = index(hole.key);
            assert_eq!(brute[h], (brute[o].0 + 1, Some(o)), "hole {h}");
            assert!(last_hole < Some(h), "holes out of order");
            last_hole = Some(h);
            check_loop(&p.loops[h], hole, false);
            seen[h] = true;
        }
    }
    assert!(seen.iter().all(|&s| s));
}

/// `c` moved by `d`.
fn moved(c: &Curve2, d: Vector2) -> Curve2 {
    match c {
        Curve2::Line2(l) => Curve2::Line2(Line2::new(l.origin() + d, l.dir()).unwrap()),
        Curve2::Circle2(c) => {
            let f = c.frame();
            let f = Frame2::new(f.origin() + d, f.x(), f.is_ccw()).unwrap();
            Curve2::Circle2(Circle2::new(f, c.radius()).unwrap())
        }
        Curve2::Nurbs2(n) => {
            let pts = n.control_points().map(|p| p + d).collect();
            let n = NurbsCurve2::new(n.knots().clone(), pts, Some(n.weights().to_vec())).unwrap();
            Curve2::Nurbs2(Arc::new(n))
        }
        other => panic!("not generated: {other:?}"),
    }
}

fn is_touching(r: Result<ValidProfile, Error>) -> bool {
    matches!(r, Err(Error::InvalidProfile(ProfileError::Touching { .. })))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn random_regions_validate_and_nest_as_sampling_says(seed in any::<u64>()) {
        let p = random_profile(seed);
        let v = ValidProfile::new(p.clone()).unwrap();
        check_nesting(&p, &v);
    }

    #[test]
    fn a_square_across_a_segment_is_caught(seed in any::<u64>(), at in 0.05f64..0.95) {
        let mut g = Lcg::new(seed ^ 0x5eed);
        let mut p = random_profile(seed);
        let l = (g.next_u64() as usize) % p.loops.len();
        let s = &p.loops[l].segments[(g.next_u64() as usize) % p.loops[l].segments.len()];
        let m = s.curve.point(s.t0 + (s.t1 - s.t0) * at);
        let mut keys = Keys(u128::MAX - 100);
        let (sin, cos) = math::sin_cos(g.next_f64(0.0, TAU));
        let h = 0.05;
        let corner = |x: f64, y: f64| Point2::new(m.x + x * cos - y * sin, m.y + x * sin + y * cos);
        let pts = [corner(-h, -h), corner(h, -h), corner(h, h), corner(-h, h)];
        let square = Loop {
            key: LoopKey(keys.next()),
            segments: (0..4).map(|i| line(&mut keys, pts[i], pts[(i + 1) % 4])).collect(),
        };
        let pos = (g.next_u64() as usize) % (p.loops.len() + 1);
        p.loops.insert(pos, square);
        prop_assert!(is_touching(ValidProfile::new(p)));
    }

    #[test]
    fn a_loop_moved_onto_another_is_caught(seed in any::<u64>(), at in 0.0f64..1.0) {
        let mut g = Lcg::new(seed ^ 0xface);
        let mut p = random_profile(seed);
        prop_assume!(p.loops.len() >= 2);
        let a = (g.next_u64() as usize) % p.loops.len();
        let b = (a + 1 + (g.next_u64() as usize) % (p.loops.len() - 1)) % p.loops.len();
        let target = &p.loops[b].segments[0];
        let onto = target.curve.point(target.t0 + (target.t1 - target.t0) * at);
        let from = &p.loops[a].segments[0];
        let d = onto - from.curve.point(from.t0);
        for s in &mut p.loops[a].segments {
            s.curve = moved(&s.curve, d);
        }
        prop_assert!(is_touching(ValidProfile::new(p)));
    }

    #[test]
    fn validation_is_deterministic_and_order_free(seed in any::<u64>()) {
        let mut g = Lcg::new(seed ^ 0x0de7);
        let p = random_profile(seed);
        let v = ValidProfile::new(p.clone()).unwrap();
        prop_assert_eq!(&v, &ValidProfile::new(p.clone()).unwrap());

        // Loops in another order: the same oriented loops, the lumps in
        // the new order.
        let mut shuffled = p.clone();
        shuffle(&mut g, &mut shuffled.loops);
        let w = ValidProfile::new(shuffled.clone()).unwrap();
        check_nesting(&shuffled, &w);
        let by_key = |v: &ValidProfile| {
            let mut lumps: Vec<_> = v
                .lumps()
                .iter()
                .map(|l| {
                    let mut holes = l.holes.clone();
                    holes.sort_by_key(|h| h.key);
                    (l.outer.clone(), holes)
                })
                .collect();
            lumps.sort_by_key(|l| l.0.key);
            lumps
        };
        prop_assert_eq!(by_key(&v), by_key(&w));

        // Loops starting at another segment, or run the other way: the
        // same loops up to where they start.
        let mut turned = p.clone();
        for l in &mut turned.loops {
            let k = (g.next_u64() as usize) % l.segments.len();
            l.segments.rotate_left(k);
            if g.next_u64().is_multiple_of(2) {
                l.segments = l.segments.iter().rev().map(reversed).collect();
            }
        }
        let t = ValidProfile::new(turned).unwrap();
        prop_assert_eq!(v.lumps().len(), t.lumps().len());
        for (a, b) in v.lumps().iter().zip(t.lumps()) {
            same_up_to_start(&a.outer, &b.outer);
            prop_assert_eq!(a.holes.len(), b.holes.len());
            for (x, y) in a.holes.iter().zip(&b.holes) {
                same_up_to_start(x, y);
            }
        }
    }
}

/// The same oriented loop, perhaps starting at another segment; areas
/// agree to rounding.
fn same_up_to_start(a: &ValidLoop, b: &ValidLoop) {
    assert_eq!(a.key, b.key);
    let k = b
        .segments
        .iter()
        .position(|s| s.key == a.segments[0].key)
        .unwrap();
    let mut segments = b.segments.clone();
    let mut vertices = b.vertices.clone();
    segments.rotate_left(k);
    vertices.rotate_left(k);
    assert_eq!(a.segments, segments);
    assert_eq!(a.vertices, vertices);
    assert!((a.area - b.area).abs() <= 1e-12 * a.area.abs());
}
