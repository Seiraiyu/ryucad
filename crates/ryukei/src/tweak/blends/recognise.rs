//! Blend recognition (design 399): fillets and chamfers read back from
//! any body's faces, certified.
//!
//! - A **fillet** of radius `r` is a face on a canal surface of that
//!   radius (a cylinder, a torus's tube, a pipe, or a NURBS certified
//!   within the tolerance of a cylinder or torus) whose *rails*, the
//!   edges that don't run across its spine, roll on exactly two faces,
//!   its supports: the stretch of spine under each rail lies on its
//!   support's offset by `r`, certified by `intersect::curve_surface`
//!   (the design's rule that the spine lies at distance `r` from both
//!   supports along their normals). It turns less than a half turn
//!   between them.
//! - A **chamfer** is a plane or cone face (a flat spline as its plane)
//!   whose edges to two faces are each set back a constant distance from
//!   where those faces' surfaces meet (a line, or a circle about a common
//!   axis), in closed form; of the pairs of faces that read so, the one
//!   setting back the least share of its supports' width, which must be
//!   under [`CHAMFER_SHARE`] (a mitred end's neighbours read so too, set
//!   back farther). A chamfer standing on another chamfer is left out,
//!   the narrower kept.
//! - A **corner** is a sphere face every edge of which runs into a fillet
//!   of its radius whose spine passes through its centre.
//!
//! Faces of one surface with the same supports and size are one blend;
//! blends of one size running on into each other (fillets across an
//! edge that is a section of both, chamfers across a tangent edge) are a
//! chain. A face that is nearly a blend but not within the tolerance is
//! left out.

// Taken out in Task 5, once every function here is used.
#![allow(dead_code, unused_imports)]

use super::{Corner, RecogniseOptions, Recognised, Recognition};
use crate::blend::{BlendKind, Chamfer};
use crate::geom::{Circle, Curve, Line, Point3, Surface, Vector3};
use crate::name::Name;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use crate::topo::{Body, EdgeId, FaceId, Sense};
use crate::work::Meter;
use crate::{Error, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// A chamfer sets back from its edge at most this share of a support's
/// width across the edge: a hexagonal prism's sides (a half each) are no
/// chamfers of a triangle's.
pub(crate) const CHAMFER_SHARE: f64 = 0.4;

/// A fillet turns at most a half turn less this between its supports: a
/// slot's round end, a half turn between parallel walls, is no fillet.
const FULL_ROUND: f64 = 0.01;

/// Samples taken along an edge to judge it.
const SAMPLES: usize = 5;
