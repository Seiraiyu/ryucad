//! Paths a profile is swept along: chains of curves meeting end to end and
//! tangentially.

use super::SegmentKey;
use crate::geom::{Curve, NurbsCurve, Point3, Vector3};
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use crate::{Error, SweepError, math};
use std::collections::BTreeSet;
use std::sync::Arc;

/// A chain of curves (see [`super::along`]): each segment ends where the
/// next starts, along its tangent. A chain whose last segment ends where
/// the first starts, tangentially, is closed; so is one segment that is a
/// whole period of a periodic curve.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub segments: Vec<PathSegment>,
}

/// A curve over an increasing range of its parameter, with a caller key
/// (unique in the path) that names what it sweeps.
#[derive(Clone, Debug, PartialEq)]
pub struct PathSegment {
    pub key: SegmentKey,
    pub curve: Curve,
    pub range: [f64; 2],
}

/// A path checked: its segments in order (a closed path's one clamped
/// curve made periodic), and whether it closes.
#[derive(Clone, Debug)]
pub(crate) struct ValidPath {
    pub(crate) segments: Vec<PathSegment>,
    pub(crate) closed: bool,
}

fn bad(segment: SegmentKey) -> Error {
    SweepError::BadPathSegment { segment }.into()
}

/// The unit tangent at `t`, or `None` where the curve stops.
fn tangent(c: &Curve, t: f64) -> Option<Vector3> {
    c.eval(t).d1.normalize().map(|u| u.get())
}

/// Whether two unit tangents are within [`ANGULAR_RESOLUTION`].
fn smooth(a: Vector3, b: Vector3) -> bool {
    math::atan2(a.cross(b).length(), a.dot(b)) <= ANGULAR_RESOLUTION
}

/// A segment's checks on its own: finite, increasing, inside a clamped
/// domain or no longer than a period, longer than resolution, defined
/// tangents at its ends.
fn check_segment(s: &PathSegment) -> Result<(), Error> {
    let [t0, t1] = s.range;
    if !(t0.is_finite() && t1.is_finite()) {
        return Err(Error::NonFinite);
    }
    let ok = match s.curve.period() {
        Some(p) => t1 - t0 <= p,
        None => {
            let [a, b] = s.curve.domain();
            a <= t0 && t1 <= b
        }
    };
    let start = s.curve.point(t0);
    let short = (1..=8).all(|i| {
        let t = t0 + (t1 - t0) * f64::from(i) / 8.0;
        s.curve.point(t).distance(start) <= LINEAR_RESOLUTION
    });
    let ends = tangent(&s.curve, t0).is_some() && tangent(&s.curve, t1).is_some();
    if t0 < t1 && ok && !short && ends {
        Ok(())
    } else {
        Err(bad(s.key))
    }
}

/// A clamped NURBS over `range` whose ends meet, as the periodic curve of
/// the same pieces over the same domain: cut to the range, in Bézier form
/// (every interior knot raised to the degree), its last control point
/// dropped for its first. `None` where the ends' weights differ.
fn periodic(n: &NurbsCurve, range: [f64; 2]) -> Option<NurbsCurve> {
    let part = n.part(range).ok()?;
    let kv = part.knots();
    let p = kv.degree();
    let (knots, mults) = (kv.knots(), kv.mults());
    let mut extra = Vec::new();
    for (k, &m) in knots.iter().zip(mults).take(knots.len() - 1).skip(1) {
        extra.extend(core::iter::repeat_n(*k, p - m));
    }
    let bezier = if extra.is_empty() {
        part
    } else {
        part.refine(&extra).ok()?
    };
    bezier.bezier_periodic()
}

impl ValidPath {
    /// Checks a path, reporting the first fault in this order: no
    /// segments (`EmptyPath`); each segment in order (`NonFinite`, or
    /// `BadPathSegment` for its range, length, ends, a repeated key, or a
    /// whole period among others); each joint in order (`PathNotG1` naming
    /// the segment after it); then a closing joint at an angle
    /// (`PathNotG1` naming the first segment), or a closed path's one
    /// clamped curve that can't be made periodic (`BadPathSegment`).
    pub(crate) fn new(path: &Path) -> Result<Self, Error> {
        let segs = &path.segments;
        if segs.is_empty() {
            return Err(SweepError::EmptyPath.into());
        }
        let mut keys = BTreeSet::new();
        for s in segs {
            check_segment(s)?;
            let whole = s.curve.period() == Some(s.range[1] - s.range[0]);
            if !keys.insert(s.key) || (whole && segs.len() > 1) {
                return Err(bad(s.key));
            }
        }
        let end = |s: &PathSegment| {
            let t = s.range[1];
            (s.curve.point(t), tangent(&s.curve, t).expect("checked"))
        };
        let start = |s: &PathSegment| {
            let t = s.range[0];
            (s.curve.point(t), tangent(&s.curve, t).expect("checked"))
        };
        let meets = |(p, a): (Point3, Vector3), (q, b): (Point3, Vector3)| {
            (p.distance(q) <= LINEAR_RESOLUTION, smooth(a, b))
        };
        for w in segs.windows(2) {
            if meets(end(&w[0]), start(&w[1])) != (true, true) {
                return Err(SweepError::PathNotG1 { at: w[1].key }.into());
            }
        }
        let first = &segs[0];
        let whole =
            segs.len() == 1 && first.curve.period() == Some(first.range[1] - first.range[0]);
        let closed = whole || {
            match meets(end(&segs[segs.len() - 1]), start(first)) {
                (true, true) => true,
                (true, false) => return Err(SweepError::PathNotG1 { at: first.key }.into()),
                _ => false,
            }
        };
        let mut segments = segs.clone();
        if closed && !whole {
            // One clamped curve closing on itself sweeps a periodic face.
            if let [s] = segments.as_mut_slice() {
                let made = match &s.curve {
                    Curve::Nurbs(n) => periodic(n, s.range),
                    _ => None,
                };
                let n = made.ok_or_else(|| bad(s.key))?;
                s.range = n.domain();
                s.curve = Curve::Nurbs(Arc::new(n));
            }
        }
        Ok(Self { segments, closed })
    }

    /// Each segment's length, by 16-point Gauss–Legendre quadrature of
    /// its speed over each of its pieces: the share of a path's twist and
    /// scale each segment takes.
    pub(crate) fn lengths(&self) -> Vec<f64> {
        self.segments
            .iter()
            .map(|s| {
                let mut cuts = vec![s.range[0]];
                if let Curve::Nurbs(n) = &s.curve {
                    for p in n.pieces() {
                        if p.range[0] > s.range[0] && p.range[0] < s.range[1] {
                            cuts.push(p.range[0]);
                        }
                    }
                }
                cuts.push(s.range[1]);
                cuts.windows(2)
                    .map(|w| {
                        let (m, h) = ((w[0] + w[1]) / 2.0, (w[1] - w[0]) / 2.0);
                        GAUSS
                            .iter()
                            .map(|&(x, wt)| {
                                let d = |t: f64| s.curve.eval(t).d1.length();
                                wt * (d(m - h * x) + d(m + h * x))
                            })
                            .sum::<f64>()
                            * h
                    })
                    .sum()
            })
            .collect()
    }
}

/// The nodes (positive half) and weights of 16-point Gauss–Legendre
/// quadrature on `[-1, 1]`.
const GAUSS: [(f64, f64); 8] = [
    (0.095_012_509_837_637_44, 0.189_450_610_455_068_5),
    (0.281_603_550_779_258_9, 0.182_603_415_044_923_6),
    (0.458_016_777_657_227_4, 0.169_156_519_395_002_5),
    (0.617_876_244_402_643_7, 0.149_595_988_816_576_7),
    (0.755_404_408_355_003, 0.124_628_971_255_533_9),
    (0.865_631_202_387_831_8, 0.095_158_511_682_492_78),
    (0.944_575_023_073_232_6, 0.062_253_523_938_647_89),
    (0.989_400_934_991_649_9, 0.027_152_459_411_754_09),
];

#[cfg(test)]
mod tests;
