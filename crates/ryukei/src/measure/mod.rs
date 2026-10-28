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

mod gauss;
mod length;
mod mass;
mod props;

pub use mass::{MassOptions, MassProperties, mass};
pub use props::{DEFAULT_TOLERANCE, EdgeShape, FaceShape, Properties, properties};

use crate::topo::{EdgeId, FaceId, VertexId};

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
