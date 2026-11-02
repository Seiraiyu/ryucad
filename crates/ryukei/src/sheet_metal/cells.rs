//! Certified tests over a face's parameter box, cell by cell.
//!
//! A quantity of the surface's point and derivatives is enclosed over a
//! cell by the mean value form: its value at the cell's middle (over a
//! box of no width, so rounding is enclosed) plus its gradient's
//! enclosure over the cell times the cell's half widths. Over a cell a
//! form wraps loosely (a tilted plane's axis-aligned box), the gradient's
//! enclosure stays tight, so a quantity that vanishes on a surface is
//! certified small on cells far wider than its box would allow. A cell no
//! judgement settles is halved both ways, up to [`MAX_CELLS`].

use crate::geom::jet::JVec;
use crate::geom::sheet::Sheet;
use crate::geom::{Surface, Vector3};
use crate::interval::Interval;
use std::collections::VecDeque;

/// The most cells a test judges before it is uncertain.
pub(super) const MAX_CELLS: usize = 4096;

/// How a test came out over a part or the whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Yes,
    No,
    Unsure,
}

pub(super) type IVec = [Interval; 3];

pub(super) fn iv(v: Vector3) -> IVec {
    v.to_array().map(Interval::point)
}

pub(super) fn dot(a: IVec, b: IVec) -> Interval {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(super) fn cross(a: IVec, b: IVec) -> IVec {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(super) fn sub(a: IVec, b: IVec) -> IVec {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(super) fn add(a: IVec, b: IVec) -> IVec {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(super) fn scale(a: IVec, k: Interval) -> IVec {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// The point and its derivatives over a box, by name.
#[derive(Clone, Copy, Debug)]
pub(super) struct Parts {
    pub(super) p: IVec,
    pub(super) u: IVec,
    pub(super) v: IVec,
    pub(super) uu: IVec,
    pub(super) uv: IVec,
    pub(super) vv: IVec,
}

fn parts(j: &JVec<Interval>) -> Parts {
    Parts {
        p: [j[0].v, j[1].v, j[2].v],
        u: [j[0].d[0], j[1].d[0], j[2].d[0]],
        v: [j[0].d[1], j[1].d[1], j[2].d[1]],
        uu: [j[0].h[0], j[1].h[0], j[2].h[0]],
        uv: [j[0].h[1], j[1].h[1], j[2].h[1]],
        vv: [j[0].h[2], j[1].h[2], j[2].h[2]],
    }
}

/// A quantity of the surface enclosed over a cell: `value` from the
/// enclosures at its middle, `gradient` from those over it.
pub(super) struct Form<'a> {
    pub(super) value: &'a dyn Fn(&Parts) -> Interval,
    pub(super) gradient: &'a dyn Fn(&Parts) -> [Interval; 2],
}

impl Form<'_> {
    /// The quantity's enclosure over `cell` by the mean value form, and
    /// the parts over the whole cell; `None` where an enclosure fails.
    pub(super) fn over(&self, s: &Sheet<'_>, cell: [[f64; 2]; 2]) -> Option<(Interval, Parts)> {
        let mid = [
            0.5 * (cell[0][0] + cell[0][1]),
            0.5 * (cell[1][0] + cell[1][1]),
        ];
        let at = parts(&s.jet_box([[mid[0], mid[0]], [mid[1], mid[1]]])?);
        let over = parts(&s.jet_box(cell)?);
        let [gu, gv] = (self.gradient)(&over);
        let [hu, hv] = steps(cell, mid)?;
        Some(((self.value)(&at) + gu * hu + gv * hv, over))
    }
}

/// The steps from `mid` to every point of `cell`, rounded outward: `mid`
/// needn't be the cell's exact middle, so each side is its own.
fn steps(cell: [[f64; 2]; 2], mid: [f64; 2]) -> Option<[Interval; 2]> {
    let step = |k: usize| Some(Interval::new(cell[k][0], cell[k][1])? - Interval::point(mid[k]));
    Some([step(0)?, step(1)?])
}

/// Judges `rect` of `surface` cell by cell: `Yes` when every cell is,
/// `No` as soon as one is, `Unsure` past [`MAX_CELLS`] cells or where an
/// enclosure fails. Cells are taken breadth first from a 4 × 4 grid, so
/// the result doesn't depend on anything but the inputs.
pub(super) fn judge(
    surface: &Surface,
    rect: [[f64; 2]; 2],
    test: impl Fn(&Sheet<'_>, [[f64; 2]; 2]) -> Verdict,
) -> Verdict {
    let sheet = Sheet::of(surface);
    let mut queue: VecDeque<[[f64; 2]; 2]> = VecDeque::new();
    let cut = |r: [f64; 2], i: usize, n: usize| {
        let w = r[1] - r[0];
        let at = |k: usize| {
            if k == n {
                r[1]
            } else {
                r[0] + w * k as f64 / n as f64
            }
        };
        [at(i), at(i + 1)]
    };
    for i in 0..4 {
        for j in 0..4 {
            queue.push_back([cut(rect[0], i, 4), cut(rect[1], j, 4)]);
        }
    }
    let mut seen = 0;
    while let Some(cell) = queue.pop_front() {
        seen += 1;
        if seen > MAX_CELLS {
            return Verdict::Unsure;
        }
        match test(&sheet, cell) {
            Verdict::Yes => {}
            Verdict::No => return Verdict::No,
            Verdict::Unsure => {
                for i in 0..2 {
                    for j in 0..2 {
                        queue.push_back([cut(cell[0], i, 2), cut(cell[1], j, 2)]);
                    }
                }
            }
        }
    }
    Verdict::Yes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Frame, Plane, Point3};

    /// A cell whose middle rounds up: its low end is still in the step,
    /// though `mid - hi` would miss it by an ulp.
    #[test]
    fn steps_reach_both_ends_of_a_cell_whose_middle_rounds() {
        let (lo, hi) = (1.0, 1.0 + 3.0 * f64::EPSILON);
        let mid = 0.5 * (lo + hi);
        assert!(lo - mid < mid - hi, "the middle rounds up");
        let [h, _] = steps([[lo, hi], [lo, hi]], [mid, mid]).unwrap();
        assert!(h.lo() <= lo - mid && h.hi() >= hi - mid);
        assert!(h.lo() + mid <= lo && h.hi() + mid >= hi);
    }

    /// A plane tilted across every axis: its height along its own normal
    /// is certified zero on cells its boxes are far too loose for.
    #[test]
    fn the_mean_value_form_is_tight_on_a_tilted_plane() {
        let frame = Frame::new(
            Point3::new(0.1, 0.2, 0.3),
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let n = iv(frame.z().get());
        let o = iv(frame.origin().to_vector());
        let s = Surface::Plane(Plane::new(frame));
        let value = |q: &Parts| dot(sub(q.p, o), n);
        let gradient = |q: &Parts| [dot(q.u, n), dot(q.v, n)];
        let form = Form {
            value: &value,
            gradient: &gradient,
        };
        let verdict = judge(&s, [[-1.0, 1.0], [-1.0, 1.0]], |sheet, cell| {
            let (h, _) = form.over(sheet, cell).unwrap();
            if h.mag() <= 1e-12 {
                Verdict::Yes
            } else {
                Verdict::Unsure
            }
        });
        assert_eq!(verdict, Verdict::Yes);
    }
}
