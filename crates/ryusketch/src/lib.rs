//! ryusketch: the 2D constraint solver behind ryucad's sketches.
//!
//! A [`Sketch`] is points, lines, arcs and circles, and the constraints
//! between them, each under an ID the caller chooses. [`solve`] moves the
//! geometry as little as it can until every constraint holds, starting
//! from the positions it is given, and reports what is still free, which
//! constraints are redundant or in conflict, and the closed loops a profile
//! can be made of.
//!
//! It knows nothing of files, units, parts or bodies: positions are meters,
//! angles radians, and IDs are opaque. Arithmetic is f64 in a fixed order,
//! with `ryukei::math` for every function, so a sketch solves to the same
//! bits on every platform.

mod equation;
mod linalg;
mod loops;
mod report;
mod sketch;
mod solve;
mod system;

pub use loops::{Chain, Loop, Segment};
pub use report::{Free, State};
pub use sketch::{Constraint, Entity, Id, MAX_LENGTH, Sketch, SketchError};
pub use solve::{Report, Solved, solve};
pub use system::{ANGLE_TOL, Coord, SOLVE_TOL};
