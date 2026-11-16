//! The windings either side of every fragment, from one sweep.
//!
//! Fragments meet only at their ends, so along a sweep line the ones it
//! crosses keep one order from where they start to where they end. The
//! sweep visits fragments' ends in lexicographic order (as if the plane
//! were sheared by an infinitesimal amount, so none is vertical; every
//! comparison is an orientation, which a shear doesn't change), keeping
//! the fragments it crosses from bottom to top. The winding just below a
//! fragment is the winding just above the one under it where it starts,
//! and crossing it upwards adds what its edges add.

use ryu_board::geom::{Point, orient};

use super::snap::Frag;

/// The windings of each operand just below (right of, going from `a` to
/// `b`) and just above (left of) a fragment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Sides {
    pub below: [i32; 2],
    pub above: [i32; 2],
}

/// The windings either side of each fragment, by index.
pub(crate) fn windings(frags: &[Frag]) -> Vec<Sides> {
    let mut sides = vec![Sides::default(); frags.len()];
    // Events: (point, 0 for an end, 1 for a start, fragment).
    let mut events: Vec<(Point, u8, u32)> = Vec::with_capacity(2 * frags.len());
    for (k, f) in frags.iter().enumerate() {
        events.push((f.b, 0, k as u32));
        events.push((f.a, 1, k as u32));
    }
    events.sort_unstable();
    let mut status: Vec<u32> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let p = events[i].0;
        let mut j = i;
        while j < events.len() && events[j].0 == p {
            j += 1;
        }
        // Fragments crossing the sweep line below `p`: those with `p`
        // above them. Those ending at `p` follow, all together.
        let pos = status.partition_point(|&k| {
            let f = &frags[k as usize];
            orient(f.a, f.b, p) > 0
        });
        let ending = events[i..j].iter().filter(|e| e.1 == 0).count();
        status.drain(pos..pos + ending);
        let mut starting: Vec<u32> = events[i..j]
            .iter()
            .filter(|e| e.1 == 1)
            .map(|e| e.2)
            .collect();
        // Bottom to top: by direction, all within a half-turn.
        starting.sort_by(|&x, &y| {
            let (fx, fy) = (&frags[x as usize], &frags[y as usize]);
            0.cmp(&orient(p, fx.b, fy.b))
        });
        let mut w = match pos {
            0 => [0, 0],
            _ => sides[status[pos - 1] as usize].above,
        };
        for &k in &starting {
            let m = frags[k as usize].m;
            let above = [w[0] + m[0], w[1] + m[1]];
            sides[k as usize] = Sides { below: w, above };
            w = above;
        }
        status.splice(pos..pos, starting);
        i = j;
    }
    sides
}

#[cfg(test)]
mod tests {
    use super::super::snap::{fragments, ring_edges};
    use super::*;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn a_square_winds_once() {
        let mut e = Vec::new();
        ring_edges(&[p(0, 0), p(10, 0), p(10, 10), p(0, 10)], 0, &mut e);
        let f = fragments(e);
        let s = windings(&f);
        for (f, s) in f.iter().zip(&s) {
            // The bottom runs left to right with the inside above it; the
            // top right to left; the left side up (lexicographically) with
            // the inside on its right (below).
            let inside_above = s.above[0] == 1 && s.below[0] == 0;
            let inside_below = s.below[0] == 1 && s.above[0] == 0;
            assert!(inside_above || inside_below, "{f:?} {s:?}");
            let expect_above =
                (f.a, f.b) == (p(0, 0), p(10, 0)) || (f.a, f.b) == (p(10, 0), p(10, 10));
            assert_eq!(inside_above, expect_above, "{f:?}");
        }
    }

    #[test]
    fn nested_and_overlapping_rings_add() {
        let mut e = Vec::new();
        ring_edges(&[p(0, 0), p(30, 0), p(30, 30), p(0, 30)], 0, &mut e);
        ring_edges(&[p(10, 10), p(20, 10), p(20, 20), p(10, 20)], 0, &mut e);
        ring_edges(&[p(5, 5), p(25, 5), p(25, 25), p(5, 25)], 1, &mut e);
        let f = fragments(e);
        let s = windings(&f);
        let inner = f
            .iter()
            .position(|f| (f.a, f.b) == (p(10, 10), p(20, 10)))
            .unwrap();
        assert_eq!(
            s[inner],
            Sides {
                below: [1, 1],
                above: [2, 1]
            }
        );
    }
}
