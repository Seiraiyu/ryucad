//! Measurement: what an entity is (lengths, areas, centres, axes, radii),
//! how far apart two entities are, the angle between two, and a body's
//! mass properties.
//!
//! Every number comes back as a [`Value`]: exact up to rounding where a
//! closed form gives it, else with a stated bound on its error that the
//! certified mesh guarantee (see [`crate::mesh`]) or a certified bound on
//! the geometry's derivatives makes rigorous. A measure that can't be
//! certified is refused with [`MeasureError::Uncertain`], never guessed.
//!
//! Work over faces runs on the current rayon pool and is gathered in ID
//! order, so results are the same bits on one thread or many.

mod angle;
mod distance;
mod gauss;
mod length;
mod mass;
mod prims;
mod props;
mod tree;

pub use angle::{Angle, angle};
pub use distance::{Distance, DistanceOptions, distance};
pub use mass::{MassOptions, MassProperties, mass, principal};
pub use props::{DEFAULT_TOLERANCE, EdgeShape, FaceShape, Properties, properties};

use crate::geom::Curve;
use crate::topo::{EdgeId, FaceId, VertexId};

/// The length of `curve` over `range`: exact for a line or a circle, else
/// certified, its bound about `rel` times the length or less. `None` for
/// a range that doesn't increase or a `rel` that isn't positive, and where
/// the curve's derivative bounds can't certify one. An edge's length in
/// [`properties`] is this at [`DEFAULT_TOLERANCE`].
pub fn curve_length(curve: &Curve, range: [f64; 2], rel: f64) -> Option<Value> {
    let [t0, t1] = range;
    if !t0.is_finite() || !t1.is_finite() || t0 >= t1 || rel.is_nan() || rel <= 0.0 {
        return None;
    }
    length::length(curve, range, rel)
}

/// What to measure in a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Entity {
    Vertex(VertexId),
    Edge(EdgeId),
    Face(FaceId),
    /// The whole body: its solid regions, for distances and mass.
    Body,
}

/// A measured number: exact up to rounding, or within `bound` of the true
/// value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Exact(f64),
    Bounded { value: f64, bound: f64 },
}

impl Value {
    pub fn value(self) -> f64 {
        match self {
            Self::Exact(v) | Self::Bounded { value: v, .. } => v,
        }
    }

    /// The most the true value may differ from [`Value::value`]: 0 for an
    /// exact one.
    pub fn bound(self) -> f64 {
        match self {
            Self::Exact(_) => 0.0,
            Self::Bounded { bound, .. } => bound,
        }
    }

    /// Whether `x` lies within the bound of the value, plus `slack`.
    pub fn holds(self, x: f64, slack: f64) -> bool {
        (x - self.value()).abs() <= self.bound() + slack
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_lengths_are_the_edge_lengths_and_refuse_bad_ranges() {
        use crate::geom::{Ellipse, Frame, Line, Point3, Vector3};
        use core::f64::consts::TAU;
        let l = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(0.0, 3.0, 4.0)).unwrap());
        assert_eq!(curve_length(&l, [1.0, 2.5], 1e-9), Some(Value::Exact(1.5)));
        let f = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let e = Curve::Ellipse(Ellipse::new(f, 2.0, 1.0).unwrap());
        assert_eq!(
            curve_length(&e, [0.0, TAU], DEFAULT_TOLERANCE),
            length::length(&e, [0.0, TAU], DEFAULT_TOLERANCE)
        );
        assert!(
            curve_length(&e, [0.0, TAU], 1e-9)
                .unwrap()
                .holds(9.688_448_220_547_675, 0.0)
        );
        for (range, rel) in [
            ([1.0, 1.0], 1e-9),
            ([2.0, 1.0], 1e-9),
            ([f64::NAN, 1.0], 1e-9),
            ([0.0, f64::INFINITY], 1e-9),
            ([0.0, 1.0], 0.0),
            ([0.0, 1.0], f64::NAN),
        ] {
            assert_eq!(curve_length(&e, range, rel), None, "{range:?} {rel}");
        }
    }

    #[test]
    fn values_hold_within_their_bounds() {
        assert_eq!(Value::Exact(2.0).bound(), 0.0);
        assert!(Value::Exact(2.0).holds(2.0 + 1e-12, 1e-11));
        let b = Value::Bounded {
            value: 1.0,
            bound: 0.1,
        };
        assert_eq!((b.value(), b.bound()), (1.0, 0.1));
        assert!(b.holds(1.09, 0.0));
        assert!(!b.holds(1.2, 0.0));
    }
}
