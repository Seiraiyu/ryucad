//! Points, vectors, transforms and bounding boxes, all f64 in model meters,
//! plus the analytic curves and surfaces that sit in a [`Frame`].

mod bbox;
mod frame;
mod point;
mod transform;
mod vector;

pub use bbox::Box3;
pub use frame::Frame;
pub use point::{Point2, Point3};
pub use transform::{Affine, Rigid};
pub use vector::{UnitVector3, Vector2, Vector3};
