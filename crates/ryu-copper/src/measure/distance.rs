//! The distance between two bodies, rounded to the nanometre exactly:
//! the floating-point estimate says where to look, and the exact
//! comparisons either side of a half nanometre settle it.

use std::cmp::Ordering;

use ryu_board::geom::{Nm, Point};

use super::{Body, MAX_REACH, compare, estimate};

/// The distance between two bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Distance {
    /// The distance rounded to the nearest nanometre, ties to even.
    pub nm: Nm,
    /// Whether the distance is exactly `nm`.
    pub exact: bool,
    /// How far the true distance may lie from the measured one: 0 unless
    /// a body is bounded.
    pub bound: Nm,
    /// A nearest point on each body, each within a nanometre of the true
    /// nearest point (from the estimate that found it, rounded).
    pub a: Point,
    pub b: Point,
    /// The solids the nearest points lie on.
    pub a_solid: usize,
    pub b_solid: usize,
}

/// The distance between two bodies, or `None` when either is empty. The
/// nanometres are exact: found from floating-point estimates, then
/// settled by the exact comparisons either side of them.
pub fn distance(a: &Body, b: &Body) -> Option<Distance> {
    let est = estimate::estimate(a, b)?;
    let at = |n: i128| compare(a, b, n);
    let mut n = i128::from(est.nm.max(0));
    // Settle n - 1/2 <= distance <= n + 1/2.
    let mut steps = 0;
    loop {
        if n > 0 && at(2 * n - 1) == Ordering::Less {
            n -= 1;
        } else if at(2 * n + 1) == Ordering::Greater {
            n += 1;
        } else {
            break;
        }
        steps += 1;
        if steps == 8 {
            n = bisect(a, b);
            break;
        }
    }
    // A tie at n + 1/2 or n - 1/2 goes to the even one.
    if at(2 * n + 1) == Ordering::Equal && n % 2 == 1 {
        n += 1;
    } else if n > 0 && at(2 * n - 1) == Ordering::Equal && n % 2 == 1 {
        n -= 1;
    }
    let exact = at(2 * n) == Ordering::Equal;
    Some(Distance {
        nm: n as Nm,
        exact,
        bound: a.bound + b.bound,
        ..est
    })
}

/// The `n` with `n - 1/2 <= distance <= n + 1/2`, by bisection: for
/// estimates far off, which floating point never gives on a board.
fn bisect(a: &Body, b: &Body) -> i128 {
    let (mut lo, mut hi) = (0i128, 2 * i128::from(MAX_REACH) + 1);
    // Invariant: distance > lo / 2 unless lo == 0; distance <= hi / 2.
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if compare(a, b, mid) == Ordering::Greater {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // hi is the least doubled threshold the distance is within: the
    // distance lies in ((hi - 1) / 2, hi / 2], so it rounds to hi / 2
    // (a tie at an odd hi is the caller's).
    hi / 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::core::{self, Core};
    use crate::measure::{BodyOptions, Solid, Verdict, closer, contains, touches, within};
    use ryu_board::geom::{Arc, Dir};
    use ryu_board::model::Polarity;
    use ryu_board::shape::{Shape, circle, rect};
    use ryu_board::symbol::Corners;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn square(x0: i64, y0: i64, x1: i64, y1: i64) -> Body {
        let r = ryu_board::geom::Region::solid(rect(x0, y0, x1, y1, &Corners::SHARP).unwrap());
        Body::of_shape(
            &Shape::regions(Polarity::Positive, [r]),
            &BodyOptions::default(),
        )
    }

    fn disc(c: Point, r: i64) -> Body {
        let r = ryu_board::geom::Region::solid(circle(c, r).unwrap());
        Body::of_shape(
            &Shape::regions(Polarity::Positive, [r]),
            &BodyOptions::default(),
        )
    }

    #[test]
    fn regions_apart_touching_and_inside() {
        let a = square(0, 0, 100, 100);
        let d = distance(&a, &square(130, 40, 200, 60)).unwrap();
        assert_eq!((d.nm, d.exact), (30, true));
        // Any point of the facing sides between y = 40 and 60 is nearest.
        assert_eq!((d.a.x, d.b.x, d.a.y), (100, 130, d.b.y));
        assert!((40..=60).contains(&d.a.y));
        assert_eq!(touches(&a, &square(100, 100, 200, 200)), Verdict::Yes);
        assert_eq!(closer(&a, &square(100, 100, 200, 200), 1), Verdict::Yes);
        // One inside the other: no outline meets, yet they overlap.
        let inner = square(40, 40, 60, 60);
        assert_eq!(distance(&a, &inner).unwrap().nm, 0);
        assert_eq!(distance(&inner, &a).unwrap().nm, 0);
        assert_eq!(contains(&a, p(50, 50)), Verdict::Yes);
        assert_eq!(contains(&a, p(100, 50)), Verdict::Yes);
        assert_eq!(contains(&a, p(101, 50)), Verdict::No);
    }

    #[test]
    fn round_pens_and_discs() {
        // A trace 200 wide from (0, 0) to (1000, 0), and a disc of radius
        // 50 centred 300 above its middle: 300 - 100 - 50.
        let t = Body::stroke(p(0, 0), p(1000, 0), 200);
        let c = disc(p(500, 300), 50);
        let d = distance(&t, &c).unwrap();
        assert_eq!((d.nm, d.exact), (150, true));
        assert_eq!((d.a, d.b), (p(500, 100), p(500, 250)));
        assert_eq!(within(&t, &c, 150), Verdict::Yes);
        assert_eq!(closer(&t, &c, 150), Verdict::No);
        // An odd width: 101 wide reaches 50.5.
        let u = Body::stroke(p(0, 0), p(1000, 0), 101);
        let e = Body::point(p(500, 60));
        let d = distance(&u, &e).unwrap();
        // 9.5 rounds to the even 10.
        assert_eq!((d.nm, d.exact), (10, false));
        assert_eq!(within(&u, &e, 9), Verdict::No);
        assert_eq!(within(&u, &e, 10), Verdict::Yes);
        // Overlapping pens touch.
        assert_eq!(
            touches(&t, &Body::stroke(p(500, 150), p(500, 400), 120)),
            Verdict::Yes
        );
    }

    #[test]
    fn irrational_distances_round_to_the_nanometre() {
        // From (0, 0) to (1, 1): √2 = 1.414, rounds to 1, not exact.
        let d = distance(&Body::point(p(0, 0)), &Body::point(p(1, 1))).unwrap();
        assert_eq!((d.nm, d.exact), (1, false));
        // A point 1000 from a circle's centre of radius 300·√2 (r² =
        // 180000): 575.735 to the circle.
        let c = Body::new(
            vec![Solid::Thick {
                core: Core::Arc(core::ArcP::new(Arc {
                    start: p(300, 300),
                    end: p(300, 300),
                    centre: p(0, 0),
                    dir: Dir::Ccw,
                })),
                w: 0,
            }],
            0,
        );
        let d = distance(&c, &Body::point(p(1000, 0))).unwrap();
        assert_eq!((d.nm, d.exact), (576, false));
    }
}
