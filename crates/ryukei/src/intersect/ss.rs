//! Surface–surface intersection: the entry point and its results.

use super::Failure;
use super::coincide::coincident;
use super::implicit::Implicit;
use super::ss_exact;
use crate::geom::{Box3, Curve, Point3, Surface};

/// One branch of the curve where two surfaces meet.
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub curve: Curve,
    /// The stretch of the curve's parameter the branch covers: a whole
    /// period for a closed curve.
    pub range: [f64; 2],
    /// The surfaces touch along the branch without crossing.
    pub tangent: bool,
    /// The branch is within this of both surfaces (at least
    /// [`crate::tol::LINEAR_RESOLUTION`]).
    pub tolerance: f64,
}

/// How two surfaces meet.
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceHit {
    /// The same surface by the coincidence rule; `same` when their natural
    /// normals agree.
    Coincident { same: bool },
    /// Where they meet in the region: curves, and points where they touch
    /// or where a curve degenerates (a cone's apex, a pole).
    Meet {
        branches: Vec<Branch>,
        points: Vec<Point3>,
    },
}

impl SurfaceHit {
    /// No contact at all.
    pub fn none() -> Self {
        Self::Meet {
            branches: Vec::new(),
            points: Vec::new(),
        }
    }
}

/// Where surfaces `a` and `b` meet inside `region`. Curves are clipped to
/// the region where they are unbounded, and closed curves that meet it
/// are returned whole.
///
/// Fails with `Unsupported` unless both surfaces are analytic, and for the
/// pairs that have no closed form yet.
pub fn surface_surface(a: &Surface, b: &Surface, region: &Box3) -> Result<SurfaceHit, Failure> {
    if Implicit::of(a).is_none() || Implicit::of(b).is_none() {
        return Err(Failure::Unsupported);
    }
    if let Some(same) = coincident(a, b) {
        return Ok(SurfaceHit::Coincident { same });
    }
    ss_exact::exact(a, b, region).ok_or(Failure::Unsupported)
}
