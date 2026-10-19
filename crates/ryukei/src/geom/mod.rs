//! Points, vectors, transforms and bounding boxes, all f64 in model meters,
//! plus the analytic curves and surfaces that sit in a [`Frame`], NURBS,
//! and swept surfaces that keep their generating curve.

mod axis;
// Used by the intersections, from the next commit on.
mod bbox;
#[allow(dead_code)]
pub(crate) mod bernstein;
mod bound2;
pub(crate) mod chain2;
pub(crate) mod chain3;
mod curve;
mod curve2;
mod ellipse;
mod eval;
mod frame;
pub(crate) mod intersect2;
mod nurbs;
mod point;
mod project;
mod surface;
mod swept;
mod transform;
mod vector;
pub(crate) mod winding2;

pub use axis::Axis;
pub use bbox::{Box2, Box3};
pub use bound2::{CurveBound, SurfaceBound};
pub use curve::{Circle, Curve, Line};
pub use curve2::{Circle2, Curve2, Ellipse2, Frame2, Line2};
pub use ellipse::Ellipse;
pub use eval::{CurveEval, CurveEval2, PrincipalCurvatures, SurfaceEval};
pub use frame::Frame;
pub(crate) use frame::polar_angle;
pub use nurbs::{
    Basis, Direction, KnotVector, MAX_DEGREE, NurbsCurve, NurbsCurve2, NurbsSurface, Reparam,
};
pub(crate) use nurbs::{BezierCurve, Hom};
pub use point::{Point2, Point3};
pub use project::Projection;
pub use surface::{Cone, Cylinder, Plane, Sphere, Surface, Torus};
pub use swept::{Extrusion, Revolution};
pub use transform::{Affine, Rigid};
pub use vector::{UnitVector3, Vector2, Vector3};
