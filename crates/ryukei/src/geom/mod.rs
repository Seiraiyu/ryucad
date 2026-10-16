//! Points, vectors, transforms and bounding boxes, all f64 in model meters,
//! plus the analytic curves and surfaces that sit in a [`Frame`].

mod bbox;
mod curve;
mod ellipse;
mod eval;
mod frame;
mod nurbs;
mod point;
mod project;
mod surface;
mod transform;
mod vector;

pub use bbox::Box3;
pub use curve::{Circle, Curve, Line};
pub use ellipse::Ellipse;
pub use eval::{CurveEval, PrincipalCurvatures, SurfaceEval};
pub use frame::Frame;
pub(crate) use frame::polar_angle;
pub use nurbs::{Basis, KnotVector, MAX_DEGREE, NurbsCurve, NurbsSurface};
pub use point::{Point2, Point3};
pub use project::Projection;
pub use surface::{Cone, Cylinder, Plane, Sphere, Surface, Torus};
pub use transform::{Affine, Rigid};
pub use vector::{UnitVector3, Vector2, Vector3};
