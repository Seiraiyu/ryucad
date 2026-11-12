//! Joint limits: stops on the angle and the distance a mate leaves free,
//! and reading those values at the bodies' poses. A limit is an
//! inequality, never a row of the system: [`crate::solve`] keeps every
//! value within its stops by pinning the ones it finds past a bound at
//! that bound (the driven row the kind already has) and solving again.

use core::f64::consts::{PI, TAU};

/// A range a mate's free value stays within, radians or meters. Either
/// bound may be missing (a one-sided stop), never both.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Range {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// A mate's stops: on the turn about `a`'s z (revolute, cylindrical,
/// planar) and the slide along it (slider, cylindrical).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub angle: Option<Range>,
    pub distance: Option<Range>,
}

impl Limits {
    pub const NONE: Limits = Limits {
        angle: None,
        distance: None,
    };

    pub fn is_none(&self) -> bool {
        self.angle.is_none() && self.distance.is_none()
    }
}

/// Which bound a value rests on or lies past.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stop {
    Min,
    Max,
}

impl Range {
    /// Bound `s`, if it has one.
    pub fn bound(&self, s: Stop) -> Option<f64> {
        match s {
            Stop::Min => self.min,
            Stop::Max => self.max,
        }
    }

    /// The bound `v` lies beyond by more than `tol`, if any.
    pub fn past(&self, v: f64, tol: f64) -> Option<Stop> {
        if self.min.is_some_and(|m| v < m - tol) {
            return Some(Stop::Min);
        }
        if self.max.is_some_and(|m| v > m + tol) {
            return Some(Stop::Max);
        }
        None
    }

    /// The bound `v` rests on, within `tol`, if any: the nearer one if
    /// both are within it.
    pub fn at(&self, v: f64, tol: f64) -> Option<Stop> {
        let off = |b: Option<f64>| b.map(|b| (v - b).abs()).filter(|d| *d <= tol);
        match (off(self.min), off(self.max)) {
            (Some(a), Some(b)) if b < a => Some(Stop::Max),
            (Some(_), _) => Some(Stop::Min),
            (None, Some(_)) => Some(Stop::Max),
            (None, None) => None,
        }
    }

    /// Where an angle range is read from: the middle of a two-sided
    /// range, or a one-sided range's bound, so an angle is unwrapped into
    /// the half turn either side of it.
    fn centre(&self) -> f64 {
        match (self.min, self.max) {
            (Some(lo), Some(hi)) => 0.5 * (lo + hi),
            (Some(b), None) | (None, Some(b)) => b,
            (None, None) => 0.0,
        }
    }
}

/// Angle `a` (radians) turned by whole turns into `[c - π, c + π)`,
/// `c` the range's [`centre`](Range::centre), so a range across the cut
/// (170° to 190°) reads as one interval, and a one-sided stop allows a
/// half turn on its side.
pub fn unwrap_angle(a: f64, range: &Range) -> f64 {
    let lo = range.centre() - PI;
    let k = ((a - lo) / TAU).floor();
    let mut out = a - k * TAU;
    // Rounding may leave it a hair outside.
    if out < lo {
        out += TAU;
    }
    if out >= lo + TAU {
        out -= TAU;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_unwrap_about_the_range() {
        let r = Range {
            min: Some(170f64.to_radians()),
            max: Some(190f64.to_radians()),
        };
        let a = unwrap_angle(-175f64.to_radians(), &r);
        assert!((a - 185f64.to_radians()).abs() < 1e-12, "{a}");
        assert_eq!(r.past(a, 1e-12), None);
        let one = Range {
            min: Some(-0.1),
            max: None,
        };
        // A one-sided stop allows a half turn on its side.
        assert!((unwrap_angle(3.0, &one) - 3.0).abs() < 1e-15);
        assert!(unwrap_angle(3.1, &one) < -0.1);
        assert_eq!(one.past(unwrap_angle(3.1, &one), 1e-12), Some(Stop::Min));
    }

    #[test]
    fn stops_are_found_within_their_tolerance() {
        let r = Range {
            min: Some(-1.0),
            max: Some(1.0),
        };
        assert_eq!(r.past(1.0 + 1e-13, 1e-12), None);
        assert_eq!(r.past(1.0 + 1e-11, 1e-12), Some(Stop::Max));
        assert_eq!(r.at(1.0 - 1e-13, 1e-12), Some(Stop::Max));
        assert_eq!(r.at(0.5, 1e-12), None);
        assert_eq!(r.bound(Stop::Min), Some(-1.0));
    }
}
