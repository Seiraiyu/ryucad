//! Where image pieces meet (design 326, phase 3): every pair whose boxes
//! overlap is met by certified plane-curve crossings, and each piece is
//! cut at every crossing, touch and overlap end. A pair whose crossings
//! can't be certified is halved; what stays uncertain is cut at its ends
//! and flagged.

use super::{Image, ImageCurve};
use crate::geom::intersect2::{Meet2, crossings_in};
use crate::geom::{Box2, Box3, Point3};
use crate::intersect::broad::BoxTree;
use crate::tol::LINEAR_RESOLUTION;
use crate::work::Meter;

/// How many times a pair whose crossings can't be certified is halved
/// each way before its stretches are cut at their ends and flagged.
const MEET_DEPTH: u32 = 3;

/// Two image pieces whose images run together, each over a stretch of
/// its 3D curve's parameter (increasing).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Overlap {
    pub(super) a: usize,
    pub(super) ra: [f64; 2],
    pub(super) b: usize,
    pub(super) rb: [f64; 2],
}

/// What meeting every pair found, by image piece.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Events {
    /// 3D parameters to cut at.
    pub(super) cuts: Vec<Vec<f64>>,
    /// 3D stretches where a crossing couldn't be certified.
    pub(super) unsure: Vec<Vec<[f64; 2]>>,
    pub(super) overlaps: Vec<Overlap>,
}

/// What one pair found.
#[derive(Default)]
struct Met {
    a: Vec<f64>,
    b: Vec<f64>,
    a_unsure: Vec<[f64; 2]>,
    b_unsure: Vec<[f64; 2]>,
    overlaps: Vec<([f64; 2], [f64; 2])>,
}

/// An image-plane box as a flat box, grown by `grow`, for the box tree.
pub(super) fn flat(b: Box2, grow: f64) -> Box3 {
    let (lo, hi) = (b.min(), b.max());
    Box3::new(
        Point3::new(lo.x - grow, lo.y - grow, 0.0),
        Point3::new(hi.x + grow, hi.y + grow, 0.0),
    )
}

/// The events of every image piece in `imgs` (each with its curve's
/// tolerance) against every other.
pub(super) fn events(imgs: &[(&ImageCurve, f64)], m: &Meter) -> Events {
    let boxes: Vec<(Box3, usize)> = imgs
        .iter()
        .enumerate()
        .filter(|(_, (ic, _))| !matches!(ic.image, Image::Point(_)))
        .map(|(i, (ic, tol))| (flat(ic.bounds(), *tol), i))
        .collect();
    let pairs = BoxTree::new(boxes).self_pairs();
    let met = m.par_map(&pairs, |m, &(a, b)| {
        let tol = imgs[a].1.max(imgs[b].1).max(LINEAR_RESOLUTION);
        let mut out = Met::default();
        let (ia, ib) = (imgs[a].0, imgs[b].0);
        meet(ia, ia.range, ib, ib.range, tol, 0, m, &mut out);
        out
    });
    let n = imgs.len();
    let mut ev = Events {
        cuts: vec![Vec::new(); n],
        unsure: vec![Vec::new(); n],
        overlaps: Vec::new(),
    };
    for (&(a, b), mt) in pairs.iter().zip(met) {
        ev.cuts[a].extend(mt.a);
        ev.cuts[b].extend(mt.b);
        ev.unsure[a].extend(mt.a_unsure);
        ev.unsure[b].extend(mt.b_unsure);
        for (ra, rb) in mt.overlaps {
            ev.overlaps.push(Overlap { a, ra, b, rb });
        }
    }
    ev
}

fn sorted([a, b]: [f64; 2]) -> [f64; 2] {
    if a <= b { [a, b] } else { [b, a] }
}

fn halves([r0, r1]: [f64; 2]) -> [[f64; 2]; 2] {
    let mid = (r0 + r1) / 2.0;
    [[r0, mid], [mid, r1]]
}

/// Meets `a` over its 3D stretch `ra` with `b` over `rb`.
#[allow(clippy::too_many_arguments)]
fn meet(
    a: &ImageCurve,
    ra: [f64; 2],
    b: &ImageCurve,
    rb: [f64; 2],
    tol: f64,
    depth: u32,
    m: &Meter,
    out: &mut Met,
) {
    // A pair of stretches met. Halted, `work::run` reports it.
    if m.charge(1).is_err() {
        return;
    }
    let (Image::Curve { curve: ca, map: ma }, Image::Curve { curve: cb, map: mb }) =
        (&a.image, &b.image)
    else {
        return;
    };
    let sub_a = ImageCurve {
        image: a.image.clone(),
        range: ra,
    };
    let sub_b = ImageCurve {
        image: b.image.clone(),
        range: rb,
    };
    let (sa, sb) = (ra.map(|t| ma.apply(t)), rb.map(|t| mb.apply(t)));
    let __k = {
        use crate::geom::Curve2 as C;
        let conic = |c: &C| matches!(c, C::Circle2(_) | C::Ellipse2(_));
        match (ca, cb) {
            (C::Nurbs2(_), _) | (_, C::Nurbs2(_)) => 10,
            (C::Line2(_), C::Line2(_)) => 7,
            (C::Line2(_), y) | (y, C::Line2(_)) if conic(y) => 8,
            (x, y) if conic(x) && conic(y) => 9,
            _ => 10,
        }
    };
    let __t = std::time::Instant::now();
    let __found = crossings_in((ca, sa), (cb, sb), tol, m);
    super::lap(__k, __t);
    match __found {
        Some(found) => {
            for meeting in found {
                match meeting {
                    Meet2::Point { ta, tb, .. } => {
                        out.a.push(sub_a.param_of(ta));
                        out.b.push(sub_b.param_of(tb));
                    }
                    Meet2::Overlap { ta, tb } => {
                        let (pa, pb) = (
                            sorted(ta.map(|s| sub_a.param_of(s))),
                            sorted(tb.map(|s| sub_b.param_of(s))),
                        );
                        out.a.extend(pa);
                        out.b.extend(pb);
                        out.overlaps.push((pa, pb));
                    }
                }
            }
        }
        None if m.halted() => {}
        None if depth < MEET_DEPTH => {
            for ha in halves(ra) {
                for hb in halves(rb) {
                    meet(a, ha, b, hb, tol, depth + 1, m, out);
                }
            }
        }
        None => {
            out.a.extend(ra);
            out.b.extend(rb);
            out.a_unsure.push(ra);
            out.b_unsure.push(rb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Curve, Frame, Line, Point3, Vector3};
    use crate::project::{View, project_curve};
    use core::f64::consts::TAU;

    fn front() -> View {
        View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap()
    }

    #[test]
    fn two_crossing_segments_cut_each_other_once() {
        let a = Curve::Line(
            Line::new(Point3::new(0.0, 0.0, 0.5), Vector3::new(1.0, 0.0, 0.0)).unwrap(),
        );
        let b = Curve::Line(
            Line::new(Point3::new(0.5, 0.3, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap(),
        );
        let ia = project_curve(&a, [0.0, 1.0], &front()).unwrap().remove(0);
        let ib = project_curve(&b, [0.0, 1.0], &front()).unwrap().remove(0);
        // unmetered: a unit test
        let ev = events(&[(&ia, 1e-8), (&ib, 1e-8)], &Meter::free());
        for cuts in &ev.cuts {
            assert_eq!(cuts.len(), 1, "{ev:?}");
            assert!((cuts[0] - 0.5).abs() < 1e-12);
        }
        assert!(ev.overlaps.is_empty());
    }

    #[test]
    fn an_edge_on_circles_halves_lie_on_each_other() {
        let f = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.6, 0.8, 0.0),
        )
        .unwrap();
        let c = Curve::Circle(Circle::new(f, 0.5).unwrap());
        let pieces = project_curve(&c, [0.0, TAU], &front()).unwrap();
        assert_eq!(pieces.len(), 3);
        let imgs: Vec<(&ImageCurve, f64)> = pieces.iter().map(|p| (p, 1e-8)).collect();
        // unmetered: a unit test
        let ev = events(&imgs, &Meter::free());
        let mut pairs: Vec<(usize, usize)> = ev.overlaps.iter().map(|o| (o.a, o.b)).collect();
        pairs.sort();
        pairs.dedup();
        assert_eq!(pairs, vec![(0, 1), (1, 2)]);
        assert!(ev.unsure.iter().all(Vec::is_empty));
    }
}
