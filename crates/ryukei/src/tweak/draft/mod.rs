//! Draft (design 384): faces tilted to an angle from a pull direction
//! about a neutral plane or a parting line, so a part releases from a
//! mould. A drafted face's surface is ruled from its hinge: a plane
//! through a line, a cone through a circle about an axis along the pull,
//! else a sweep of a straight profile along the hinge in the binormal
//! frame of the pull.

pub(crate) mod hinge;
pub(crate) mod report;
pub(crate) mod surface;

/// A drafted face, as the engine reads it: its hinge, what the hinge
/// lies on, its pull and signed angle.
#[derive(Clone, Debug)]
pub(crate) struct Drafted {
    pub(crate) hinge: hinge::HingeCurve,
    pub(crate) on: HingeOn,
    pub(crate) pull: crate::geom::UnitVector3,
    pub(crate) angle: f64,
}

/// What a hinge lies on.
#[derive(Clone, Debug)]
pub(crate) enum HingeOn {
    /// The neutral plane.
    Plane(crate::geom::Surface),
    /// The face's parting-line edges.
    Edges(Vec<crate::topo::EdgeId>),
}

/// What a draft made of each face: its least and greatest angle from
/// its pull (radians, positive along the pull's side the draft tilts
/// to), certified. Exactly the angle asked for wherever the hinge is
/// square to the pull (every neutral plane); a parting line climbing
/// along the pull holds the angle in the section square to the hinge
/// instead, and this says what the face's angle is.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftReport {
    pub faces: Vec<FaceAngle>,
}

/// One drafted face's angles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceAngle {
    pub face: crate::name::Name,
    pub least: f64,
    pub greatest: f64,
}
