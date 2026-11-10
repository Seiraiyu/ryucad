//! Where a part's designator and value go: the first free place of a
//! fixed list round its body (right, above, below, left, then each a step
//! further out), every line upright and on the grid.

use crate::font::{self, Anchor, SIZE};
use crate::geom::{GRID, Nm, Point, Rect};

/// From one line's baseline to the next.
pub const LINE: Nm = 2 * GRID;

fn up(v: Nm) -> Nm {
    v.div_euclid(GRID) * GRID + if v.rem_euclid(GRID) == 0 { 0 } else { GRID }
}

fn down(v: Nm) -> Nm {
    v.div_euclid(GRID) * GRID
}

/// The boxes of `lines` with the first's baseline starting at `at`, each
/// below the last.
pub fn boxes(lines: &[&str], at: Point) -> Vec<Rect> {
    lines
        .iter()
        .enumerate()
        .map(|(i, t)| font::text_box(t, SIZE, [at[0], at[1] - LINE * i as Nm], Anchor::Start))
        .collect()
}

/// One line of text from `at`: the first field there, each next one a
/// step past the last's end, on the grid.
pub fn row_line<const N: usize>(lines: &[&str; N], at: Point) -> [Point; N] {
    let mut out = [at; N];
    for (o, p) in out.iter_mut().zip(row_points(lines, at)) {
        *o = p;
    }
    out
}

/// `row_line` of any number of lines.
pub fn row_points(lines: &[&str], at: Point) -> Vec<Point> {
    let mut out = Vec::new();
    let mut x = at[0];
    for t in lines {
        out.push([x, at[1]]);
        x = up(x + font::width(t, SIZE)) + GRID;
    }
    out
}

/// How far `row_points` reaches from its start: its last line's end.
pub fn row_width(lines: &[&str]) -> Nm {
    let pts = row_points(lines, [0, 0]);
    match (pts.last(), lines.last()) {
        (Some(p), Some(t)) => p[0] + font::width(t, SIZE),
        _ => 0,
    }
}

/// Where a block of `lines` may start, best first, round `body`.
pub fn candidates(body: Rect, lines: &[&str]) -> Vec<Point> {
    let w = lines
        .iter()
        .map(|t| font::width(t, SIZE))
        .max()
        .unwrap_or(0);
    let n = lines.len() as Nm;
    let right = [up(body.max[0] + GRID), down(body.max[1]) - GRID];
    let above = [down(body.min[0]), up(body.max[1] + GRID) + (n - 1) * LINE];
    let below = [down(body.min[0]), down(body.min[1] - GRID - SIZE)];
    let left = [down(body.min[0] - GRID - w), right[1]];
    let mut out = Vec::new();
    for k in 0..=24 {
        let d = k * GRID;
        out.push([right[0] + d, right[1]]);
        out.push([above[0], above[1] + d]);
        out.push([below[0], below[1] - d]);
        out.push([left[0] - d, left[1]]);
        if k > 0 {
            out.push([right[0], right[1] - d]);
            out.push([right[0], right[1] + d]);
        }
    }
    out
}

/// Where a block of `lines` may start on one side of `body`, best first:
/// `candidates`' places on that side (and further out), sliding along it.
pub fn candidates_on(body: Rect, lines: &[&str], side: crate::geom::Side) -> Vec<Point> {
    use crate::geom::Side;
    let on = |at: &Point| {
        boxes(lines, *at).iter().all(|b| match side {
            Side::Left => b.max[0] <= body.min[0],
            Side::Right => b.min[0] >= body.max[0],
            Side::Up => b.min[1] >= body.max[1],
            Side::Down => b.max[1] <= body.min[1],
        })
    };
    let mut out: Vec<Point> = candidates(body, lines).into_iter().filter(on).collect();
    // Along the side too: the block slid a step at a time either way.
    let base = out.first().copied();
    if let Some(b0) = base {
        for k in 1..=12 {
            let d = k * GRID;
            for at in match side {
                Side::Left | Side::Right => [[b0[0], b0[1] - d], [b0[0], b0[1] + d]],
                Side::Up | Side::Down => [[b0[0] - d, b0[1]], [b0[0] + d, b0[1]]],
            } {
                if on(&at) && !out.contains(&at) {
                    out.push(at);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_right_of_its_body_first_on_the_grid() {
        let body = Rect::new([0, 0], [3 * GRID, 6 * GRID]);
        let c = candidates(body, &["R1", "10k"]);
        assert_eq!(c[0], [4 * GRID, 5 * GRID]);
        assert!(c.iter().all(|p| p[0] % GRID == 0 && p[1] % GRID == 0));
        let b = boxes(&["R1", "10k"], c[0]);
        assert!(b[1].max[1] < b[0].min[1]);
        assert!(!b[0].overlaps(&body));
    }

    /// A one-line text: the designator, then the value a step past it, both
    /// on the grid and on one baseline.
    #[test]
    fn a_row_line_puts_the_value_after_the_designator() {
        let at = [10 * GRID, -4 * GRID];
        let [r, v] = row_line(&["RM1A", "2.2k"], at);
        assert_eq!(r, at);
        assert_eq!(v[1], at[1]);
        assert!(v[0] % GRID == 0);
        let rb = boxes(&["RM1A"], r)[0];
        let vb = boxes(&["2.2k"], v)[0];
        assert!(rb.max[0] < vb.min[0], "apart: {rb:?} {vb:?}");
    }

    /// Candidates on one side lie wholly on that side of the body.
    #[test]
    fn candidates_on_a_side_stay_on_it() {
        let body = Rect::new([0, 0], [3 * GRID, 6 * GRID]);
        let lines = ["R1", "10k"];
        for side in crate::geom::Side::ALL {
            let c = candidates_on(body, &lines, side);
            assert!(!c.is_empty());
            for at in c {
                for b in boxes(&lines, at) {
                    let ok = match side {
                        crate::geom::Side::Left => b.max[0] <= body.min[0],
                        crate::geom::Side::Right => b.min[0] >= body.max[0],
                        crate::geom::Side::Up => b.min[1] >= body.max[1],
                        crate::geom::Side::Down => b.max[1] <= body.min[1],
                    };
                    assert!(ok, "{side:?}: {b:?}");
                }
            }
        }
    }
}
