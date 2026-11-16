//! Rings cut to a box, so a question about a small place costs the rings
//! near it, not a boolean of a whole plane.

use ryu_board::geom::{Point, Rect, div_round};

use super::Rings;

/// Rings cut to `r`: each ring whose box meets `r` walked round the box's
/// four sides (Sutherland and Hodgman), crossings rounded to the
/// nanometre; rings whose box misses it left out. Every point inside `r`
/// farther than a nanometre from the rings' edges winds as it did, so the
/// bound grows by a nanometre.
pub fn clip(rings: &Rings, r: Rect) -> Rings {
    let out = rings
        .rings
        .iter()
        .filter(|ring| {
            let b = super::ring_bounds(ring);
            b.min.x <= r.max.x && r.min.x <= b.max.x && b.min.y <= r.max.y && r.min.y <= b.max.y
        })
        .map(|ring| clip_ring(ring, &r))
        .filter(|c| c.len() >= 3)
        .collect();
    Rings::new(out, rings.bound + 1)
}

/// Where `a`-`b` crosses the line of side `side` (0 left, 1 right, 2
/// bottom, 3 top), rounded to the nanometre; `a` and `b` lie either side.
fn cross(a: Point, b: Point, r: &Rect, side: usize) -> Point {
    let (ax, ay, bx, by) = (
        i128::from(a.x),
        i128::from(a.y),
        i128::from(b.x),
        i128::from(b.y),
    );
    if side < 2 {
        let x = i128::from(if side == 0 { r.min.x } else { r.max.x });
        let y = ay + div_round((by - ay) * (x - ax), bx - ax);
        Point::new(x as i64, y as i64)
    } else {
        let y = i128::from(if side == 2 { r.min.y } else { r.max.y });
        let x = ax + div_round((bx - ax) * (y - ay), by - ay);
        Point::new(x as i64, y as i64)
    }
}

fn clip_ring(ring: &[Point], r: &Rect) -> Vec<Point> {
    let mut out: Vec<Point> = ring.to_vec();
    for side in 0..4 {
        if out.is_empty() {
            break;
        }
        let inside = |p: Point| match side {
            0 => p.x >= r.min.x,
            1 => p.x <= r.max.x,
            2 => p.y >= r.min.y,
            _ => p.y <= r.max.y,
        };
        let input = std::mem::take(&mut out);
        let n = input.len();
        for k in 0..n {
            let (prev, cur) = (input[(k + n - 1) % n], input[k]);
            match (inside(prev), inside(cur)) {
                (true, true) => out.push(cur),
                (true, false) => out.push(cross(prev, cur, r, side)),
                (false, true) => {
                    out.push(cross(prev, cur, r, side));
                    out.push(cur);
                }
                (false, false) => {}
            }
        }
        out.dedup();
        while out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poly::winding;
    use ryu_board::testing::Lcg;

    #[test]
    fn windings_inside_the_box_are_kept() {
        let mut g = Lcg::new(0x636c_6970);
        for _ in 0..200 {
            let ring: Vec<Point> = (0..g.range(3, 12))
                .map(|_| Point::new(g.range(0, 1_000), g.range(0, 1_000)))
                .collect();
            let r = Rect::new(
                Point::new(g.range(0, 500), g.range(0, 500)),
                Point::new(g.range(500, 1_000), g.range(500, 1_000)),
            );
            let cut = clip(&Rings::new(vec![ring.clone()], 0), r);
            assert_eq!(cut.bound, 1);
            for _ in 0..50 {
                let q = Point::new(
                    g.range(r.min.x + 2, r.max.x - 2),
                    g.range(r.min.y + 2, r.max.y - 2),
                );
                // Clear of every edge of the ring: windings are only kept
                // farther than a nanometre from them.
                let n = ring.len();
                let near = (0..n).any(|i| {
                    crate::measure::within(
                        &crate::measure::Body::stroke(ring[i], ring[(i + 1) % n], 0),
                        &crate::measure::Body::point(q),
                        2,
                    )
                    .is_yes()
                });
                if near {
                    continue;
                }
                let got: i32 = cut.rings.iter().map(|c| winding(c, q)).sum();
                assert_eq!(got, winding(&ring, q), "{ring:?} in {r:?} at {q:?}");
            }
        }
        // A ring wholly outside is dropped.
        let far = vec![
            Point::new(5_000, 5_000),
            Point::new(6_000, 5_000),
            Point::new(6_000, 6_000),
        ];
        let r = Rect::new(Point::new(0, 0), Point::new(10, 10));
        assert!(clip(&Rings::new(vec![far], 0), r).rings.is_empty());
    }
}
