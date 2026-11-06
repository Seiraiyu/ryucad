//! Image pieces cut at their events, and the pieces that lie on each
//! other (design 326, phase 3).

use super::events::Overlap;
use super::{Image, ImageCurve, View};
use crate::geom::{Box2, Curve, Point2};

/// A stretch of an image piece that becomes one [`super::Piece`].
#[allow(dead_code)] // used from Task 11
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Span {
    /// The image piece it is cut from.
    pub(super) img: usize,
    /// Its stretch of the 3D curve's parameter.
    pub(super) range: [f64; 2],
    /// It lies in a stretch where a crossing couldn't be certified.
    pub(super) flagged: bool,
    /// The image of its middle, its image's box and its middle's depth.
    pub(super) mid: Point2,
    pub(super) bounds: Box2,
    pub(super) depth: f64,
}

#[allow(dead_code)] // used from Task 11
impl Span {
    pub(super) fn new(
        img: usize,
        (range, flagged): ([f64; 2], bool),
        image: &ImageCurve,
        curve: &Curve,
        view: &View,
    ) -> Self {
        let piece = ImageCurve {
            image: image.image.clone(),
            range,
        };
        let t = (range[0] + range[1]) / 2.0;
        Self {
            img,
            range,
            flagged,
            mid: piece.point(t),
            bounds: piece.bounds(),
            depth: view.depth(curve.point(t)),
        }
    }
}

/// `ic`'s stretches between its `cuts` (3D parameters, any order, those
/// outside it ignored), each flagged when its middle lies in one of
/// `unsure`. A cut within four tolerances in space of the last one kept,
/// or of the end, is dropped. A point image is one stretch.
#[allow(dead_code)] // used from Task 11
pub(super) fn cut(
    ic: &ImageCurve,
    curve: &Curve,
    cuts: &[f64],
    unsure: &[[f64; 2]],
    tol: f64,
) -> Vec<([f64; 2], bool)> {
    let [r0, r1] = ic.range;
    if matches!(ic.image, Image::Point(_)) {
        return vec![([r0, r1], false)];
    }
    let short = 4.0 * tol;
    let mut ts: Vec<f64> = cuts.iter().copied().filter(|&t| t > r0 && t < r1).collect();
    ts.sort_by(f64::total_cmp);
    ts.dedup();
    let end = curve.point(r1);
    let mut ends = vec![r0];
    for t in ts {
        let p = curve.point(t);
        let last = curve.point(*ends.last().expect("starts with r0"));
        if p.distance(last) > short && p.distance(end) > short {
            ends.push(t);
        }
    }
    ends.push(r1);
    ends.windows(2)
        .map(|w| {
            let mid = (w[0] + w[1]) / 2.0;
            let flagged = unsure
                .iter()
                .any(|u| mid > u[0].min(u[1]) && mid < u[0].max(u[1]));
            ([w[0], w[1]], flagged)
        })
        .collect()
}

/// For every span, the spans it lies on, nearest first: those of each
/// overlap's other image piece inside its stretch whose image holds this
/// one's middle, or whose middle this one's image holds. `first[i]..first[i
/// + 1]` are image piece `i`'s spans; `tols` the image pieces' tolerances.
#[allow(dead_code)] // used from Task 11
pub(super) fn link(
    spans: &[Span],
    first: &[usize],
    overlaps: &[Overlap],
    tols: &[f64],
) -> Vec<Vec<u32>> {
    let mut out: Vec<Vec<u32>> = vec![Vec::new(); spans.len()];
    let inside = |s: &Span, r: [f64; 2]| {
        let mid = (s.range[0] + s.range[1]) / 2.0;
        mid >= r[0] && mid <= r[1]
    };
    let holds = |b: Box2, p: Point2, g: f64| {
        p.x >= b.min().x - g && p.x <= b.max().x + g && p.y >= b.min().y - g && p.y <= b.max().y + g
    };
    for o in overlaps {
        let g = tols[o.a].max(tols[o.b]);
        for p in first[o.a]..first[o.a + 1] {
            if !inside(&spans[p], o.ra) {
                continue;
            }
            for q in first[o.b]..first[o.b + 1] {
                if !inside(&spans[q], o.rb) {
                    continue;
                }
                if holds(spans[q].bounds, spans[p].mid, g)
                    || holds(spans[p].bounds, spans[q].mid, g)
                {
                    out[p].push(q as u32);
                    out[q].push(p as u32);
                }
            }
        }
    }
    for list in &mut out {
        list.sort_by(|&x, &y| {
            spans[y as usize]
                .depth
                .total_cmp(&spans[x as usize].depth)
                .then(x.cmp(&y))
        });
        list.dedup();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Line, Point3, Vector3};

    #[test]
    fn cuts_too_close_merge_and_unsure_stretches_are_flagged() {
        let l = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)).unwrap());
        let v =
            crate::project::View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0))
                .unwrap();
        let ic = crate::project::project_curve(&l, [0.0, 1.0], &v)
            .unwrap()
            .remove(0);
        let got = cut(
            &ic,
            &l,
            &[0.7, 0.3, 0.3 + 1e-12, 1.5, 1.0 - 1e-12],
            &[[0.65, 0.9]],
            1e-8,
        );
        assert_eq!(
            got,
            vec![([0.0, 0.3], false), ([0.3, 0.7], false), ([0.7, 1.0], true)]
        );
    }

    #[test]
    fn overlapping_spans_name_each_other_nearest_first() {
        let span = |img: usize, depth: f64| Span {
            img,
            range: [0.0, 1.0],
            flagged: false,
            mid: Point2::new(0.5, 0.0),
            bounds: Box2::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)),
            depth,
        };
        let spans = vec![span(0, 0.0), span(1, -1.0), span(2, 2.0)];
        let o = |a, b| Overlap {
            a,
            ra: [0.0, 1.0],
            b,
            rb: [0.0, 1.0],
        };
        let links = link(
            &spans,
            &[0, 1, 2, 3],
            &[o(0, 1), o(0, 2), o(1, 2)],
            &[1e-8; 3],
        );
        assert_eq!(links, vec![vec![2, 1], vec![2, 0], vec![0, 1]]);
    }
}
