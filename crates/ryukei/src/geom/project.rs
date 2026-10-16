//! The result of a closest-point projection.

/// The nearest point of a curve or surface to a query point: its parameter
/// (`f64` for a curve, `(u, v)` for a surface), the point itself and its
/// distance from the query point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection<T, P> {
    pub param: T,
    pub point: P,
    pub distance: f64,
}
