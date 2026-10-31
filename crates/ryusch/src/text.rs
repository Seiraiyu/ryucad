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
}
