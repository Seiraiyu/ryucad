//! ryumate: the rigid-body mate solver behind ryucad's assemblies.
//!
//! An [`Assembly`] is rigid bodies at stored poses, fixed or free,
//! connectors (frames) on them, mates between connectors, and patterns
//! that place copies of bodies, each under an ID the caller chooses.
//! [`solve`] moves the free bodies as little as it can until every mate
//! holds, starting from their stored poses, and reports each body's
//! degrees of freedom and the motions left to it, each mate's state, and
//! the mates that are redundant or in conflict.
//!
//! Each free body has six variables, a translation and a rotation vector
//! applied to its stored pose through the exponential map, so a solve
//! starts at zero and an assembly that already holds comes back with the
//! very same bits. Mates become residuals written once over dual numbers
//! with exactly one zero near where they hold (frames pointing opposite
//! ways never read as holding). The steps, the Dulmage-Mendelsohn
//! decomposition and the diagnosis are `ryusketch::core`'s, so an
//! assembly is solved as a sketch is: each connected component in its
//! pieces, falling back to the whole component.
//!
//! It knows nothing of files, parts, instances or bodies' geometry:
//! poses are meters, angles radians, and IDs are opaque. Arithmetic is
//! f64 in a fixed order, with `ryukei::math` for every function, so an
//! assembly solves to the same bits on every platform.

mod assembly;
mod pose;
mod report;
mod rot;
mod solve;
mod system;

pub use assembly::{
    AXIS_TOL, Assembly, Body, Connector, Direction, Id, Kind, MAX_LENGTH, Mate, MateError, Motion,
    On, Pattern,
};
pub use pose::Pose;
pub use report::{Axis, BodyReport, BodyState, Freedom, MateState, Report, State};
pub use solve::{Solved, solve, solve_whole};
pub use system::copy_poses;
