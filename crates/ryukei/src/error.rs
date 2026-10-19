use crate::sweep::{LoopKey, SegmentKey};
use crate::topo::{EdgeId, FaceId};
use core::fmt;

/// Errors from fallible kernel constructors and operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Geometry outside the size box (see [`crate::tol::SIZE_BOX_HALF`]).
    OutsideSizeBox,
    /// A zero-length direction, a singular transform, or a tolerance below
    /// resolution.
    Degenerate,
    /// A NaN or infinity reached a constructor.
    NonFinite,
    /// A topological operator's precondition does not hold. The body is
    /// unchanged.
    Topology(TopologyError),
    /// A knot vector, control net or weight a NURBS cannot be built from.
    InvalidNurbs(NurbsError),
    /// Mesh parameters out of range: see [`crate::mesh::MeshParams`].
    InvalidMeshParams,
    /// A mesh would have more triangles than its budget allows: the
    /// estimate, or the count when meshing stopped.
    MeshTooLarge { triangles: u64 },
    /// A sweep profile that is not a valid planar region: see
    /// [`crate::sweep::ValidProfile::new`].
    InvalidProfile(ProfileError),
    /// Extrude or revolve parameters that don't fit the profile.
    InvalidSweep(SweepError),
    /// An edge with no exact pcurve on a face it bounds: see
    /// [`crate::pcurve::exact`].
    NoPcurve { edge: EdgeId, face: FaceId },
}

/// Why a sweep profile was refused. Each names the caller's keys, so the
/// caller can point at its sketch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// The profile has no loops.
    Empty,
    /// A loop has no segments.
    EmptyLoop { loop_: LoopKey },
    /// A segment key or a loop key is used twice. Segment keys are unique
    /// across the whole profile.
    DuplicateKey,
    /// `t0` or `t1` is not finite, `t0 == t1`, the range leaves a clamped
    /// curve's domain, or it is longer than a period.
    BadRange { segment: SegmentKey },
    /// The segment is shorter than [`crate::tol::LINEAR_RESOLUTION`]: nine
    /// evenly spaced parameters all give points within resolution of its
    /// start.
    ShortSegment { segment: SegmentKey },
    /// A segment spanning a whole period shares its loop with others.
    ClosedSegmentNotAlone { segment: SegmentKey },
    /// A loop's only segment is not a whole period of a periodic curve:
    /// its ends would meet at a seam. Split it into two segments.
    SeamLoop { segment: SegmentKey },
    /// Segment `after` ends farther than resolution from the start of the
    /// next segment in its loop (cyclically).
    NotClosed { loop_: LoopKey, after: SegmentKey },
    /// Two segments cross, overlap, come closer than resolution, or could
    /// not be certified apart; `a == b` for a segment that touches itself.
    Touching { a: SegmentKey, b: SegmentKey },
    /// A loop's orientation or nesting could not be certified: its area is
    /// too small to sign.
    Degenerate { loop_: LoopKey },
    /// The profile's image in model space leaves the size box.
    OutsideSizeBox,
}

/// Why a NURBS constructor refused its input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NurbsError {
    /// A degree outside `1..=MAX_DEGREE`.
    Degree,
    /// Knots that do not strictly increase.
    KnotOrder,
    /// Multiplicities that don't fit the degree and kind (clamped ends at
    /// `degree + 1`, everything else at most `degree`, periodic ends
    /// equal), or a count that doesn't match the knots.
    KnotMultiplicity,
    /// The wrong number of control points or weights for the knots.
    ControlCount,
    /// A weight that is not positive.
    Weight,
    /// A domain shorter than [`crate::tol::LINEAR_RESOLUTION`].
    ShortDomain,
}

/// Why a topological operator refused to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopologyError {
    /// An ID that is not a live entity of this body.
    DeadEntity,
    /// A vertex or fin is not where the operator needs it.
    NotInLoop,
    /// The entities are in different loops where the operator needs one
    /// loop, or in one loop where it needs two.
    WrongLoop,
    /// The same for faces.
    WrongFace,
    /// The same for shells.
    WrongShell,
    /// The loop is not a lone-vertex loop.
    NotLone,
    /// The edge does not have exactly two fins.
    NotManifold,
    /// The entity is not in the shape the operator removes, such as a
    /// vertex with more edges than the operator kills.
    NotRemovable,
    /// The operation would split a shell in two.
    WouldDisconnect,
    /// An operation built a body that fails `check_topology`: a kernel
    /// bug, reported rather than handed on.
    Inconsistent,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideSizeBox => f.write_str("geometry lies outside the size box"),
            Self::Degenerate => f.write_str("degenerate geometry"),
            Self::NonFinite => f.write_str("non-finite coordinate"),
            Self::Topology(t) => write!(f, "topology: {t}"),
            Self::InvalidNurbs(n) => write!(f, "invalid NURBS: {n}"),
            Self::InvalidMeshParams => f.write_str("mesh parameters out of range"),
            Self::MeshTooLarge { triangles } => {
                write!(f, "mesh too large: {triangles} triangles over budget")
            }
            Self::InvalidProfile(p) => write!(f, "invalid profile: {p}"),
            Self::InvalidSweep(e) => write!(f, "invalid sweep: {e}"),
            Self::NoPcurve { edge, face } => write!(
                f,
                "edge {} has no exact pcurve on face {}",
                edge.index(),
                face.index()
            ),
        }
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("no loops"),
            Self::EmptyLoop { loop_ } => write!(f, "loop {:#x} has no segments", loop_.0),
            Self::DuplicateKey => f.write_str("a segment or loop key is used twice"),
            Self::BadRange { segment } => {
                write!(f, "segment {:#x} has a bad parameter range", segment.0)
            }
            Self::ShortSegment { segment } => {
                write!(f, "segment {:#x} is shorter than resolution", segment.0)
            }
            Self::ClosedSegmentNotAlone { segment } => write!(
                f,
                "closed segment {:#x} is not alone in its loop",
                segment.0
            ),
            Self::SeamLoop { segment } => write!(
                f,
                "segment {:#x} closes its loop at a seam; split it in two",
                segment.0
            ),
            Self::NotClosed { loop_, after } => write!(
                f,
                "loop {:#x} is open after segment {:#x}",
                loop_.0, after.0
            ),
            Self::Touching { a, b } if a == b => write!(f, "segment {:#x} touches itself", a.0),
            Self::Touching { a, b } => {
                write!(f, "segments {:#x} and {:#x} touch", a.0, b.0)
            }
            Self::Degenerate { loop_ } => {
                write!(f, "loop {:#x} is too small to orient", loop_.0)
            }
            Self::OutsideSizeBox => f.write_str("profile lies outside the size box"),
        }
    }
}

/// Why an extrude or a revolve refused its parameters. Segments and loops
/// are named by the caller's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SweepError {
    /// An extrude direction within [`crate::tol::ANGULAR_RESOLUTION`] of
    /// the profile's plane.
    DirectionInPlane,
    /// An extrude thinner than `10 × LINEAR_RESOLUTION` along the
    /// profile's normal, or a revolve whose farthest point moves less.
    ShortSpan,
    /// A revolve axis that does not lie in the profile's plane.
    AxisNotInPlane,
    /// Revolve angles whose span is not in `(0, 2π]`.
    BadAngle,
    /// A segment that crosses a revolve's axis.
    CrossesAxis { segment: SegmentKey },
    /// A segment that touches a revolve's axis other than at its ends.
    TouchesAxisInside { segment: SegmentKey },
    /// A full revolve of a profile that touches the axis at a lone vertex,
    /// the start of `segment`: the solid would meet itself in a point.
    PinchedOnAxis { segment: SegmentKey },
    /// A full revolve that would enclose a cavity: a hole, or a loop that
    /// touches the axis in more than one stretch.
    EnclosesVoid { loop_: LoopKey },
    /// The swept solid would leave the size box.
    OutsideSizeBox,
}

impl fmt::Display for SweepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectionInPlane => f.write_str("extrude direction lies in the profile's plane"),
            Self::ShortSpan => f.write_str("sweep shorter than resolution"),
            Self::AxisNotInPlane => f.write_str("revolve axis is not in the profile's plane"),
            Self::BadAngle => f.write_str("revolve angle not in (0, 2π]"),
            Self::CrossesAxis { segment } => write!(f, "segment {:#x} crosses the axis", segment.0),
            Self::TouchesAxisInside { segment } => {
                write!(
                    f,
                    "segment {:#x} touches the axis between its ends",
                    segment.0
                )
            }
            Self::PinchedOnAxis { segment } => {
                write!(
                    f,
                    "the profile touches the axis at the start of segment {:#x}",
                    segment.0
                )
            }
            Self::EnclosesVoid { loop_ } => {
                write!(
                    f,
                    "a full turn of loop {:#x} would enclose a cavity",
                    loop_.0
                )
            }
            Self::OutsideSizeBox => f.write_str("the sweep leaves the size box"),
        }
    }
}

impl From<ProfileError> for Error {
    fn from(p: ProfileError) -> Self {
        Self::InvalidProfile(p)
    }
}

impl From<SweepError> for Error {
    fn from(e: SweepError) -> Self {
        Self::InvalidSweep(e)
    }
}

impl fmt::Display for NurbsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Degree => "degree outside 1 to 25",
            Self::KnotOrder => "knots are not strictly increasing",
            Self::KnotMultiplicity => "knot multiplicities do not fit the degree",
            Self::ControlCount => "wrong number of control points or weights",
            Self::Weight => "weights must be positive",
            Self::ShortDomain => "domain shorter than resolution",
        })
    }
}

impl From<NurbsError> for Error {
    fn from(n: NurbsError) -> Self {
        Self::InvalidNurbs(n)
    }
}

impl fmt::Display for TopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DeadEntity => "not a live entity of this body",
            Self::NotInLoop => "not in the required loop",
            Self::WrongLoop => "wrong loop",
            Self::WrongFace => "wrong face",
            Self::WrongShell => "wrong shell",
            Self::NotLone => "not a lone-vertex loop",
            Self::NotManifold => "edge does not have exactly two fins",
            Self::NotRemovable => "not in the shape this operator removes",
            Self::WouldDisconnect => "would split the shell",
            Self::Inconsistent => "the result fails the topology check",
        })
    }
}

impl From<TopologyError> for Error {
    fn from(t: TopologyError) -> Self {
        Self::Topology(t)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nurbs_errors_display() {
        assert_eq!(
            Error::from(NurbsError::KnotOrder).to_string(),
            "invalid NURBS: knots are not strictly increasing"
        );
    }

    #[test]
    fn displays() {
        assert_eq!(Error::NonFinite.to_string(), "non-finite coordinate");
        assert_eq!(
            Error::MeshTooLarge { triangles: 7 }.to_string(),
            "mesh too large: 7 triangles over budget"
        );
        let e: Box<dyn std::error::Error> = Box::new(Error::Degenerate);
        assert_eq!(e.to_string(), "degenerate geometry");
        assert_eq!(
            Error::from(TopologyError::WouldDisconnect).to_string(),
            "topology: would split the shell"
        );
        assert_eq!(
            Error::from(ProfileError::Touching {
                a: SegmentKey(1),
                b: SegmentKey(0x2a),
            })
            .to_string(),
            "invalid profile: segments 0x1 and 0x2a touch"
        );
        assert_eq!(
            ProfileError::Touching {
                a: SegmentKey(3),
                b: SegmentKey(3),
            }
            .to_string(),
            "segment 0x3 touches itself"
        );
        assert_eq!(
            Error::from(SweepError::CrossesAxis {
                segment: SegmentKey(0x2a)
            })
            .to_string(),
            "invalid sweep: segment 0x2a crosses the axis"
        );
        assert_eq!(
            SweepError::EnclosesVoid { loop_: LoopKey(2) }.to_string(),
            "a full turn of loop 0x2 would enclose a cavity"
        );
        assert_eq!(
            Error::from(TopologyError::Inconsistent).to_string(),
            "topology: the result fails the topology check"
        );
    }
}
