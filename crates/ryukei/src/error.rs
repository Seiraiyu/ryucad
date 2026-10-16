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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideSizeBox => f.write_str("geometry lies outside the size box"),
            Self::Degenerate => f.write_str("degenerate geometry"),
            Self::NonFinite => f.write_str("non-finite coordinate"),
            Self::Topology(t) => write!(f, "topology: {t}"),
            Self::InvalidNurbs(n) => write!(f, "invalid NURBS: {n}"),
        }
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
        let e: Box<dyn std::error::Error> = Box::new(Error::Degenerate);
        assert_eq!(e.to_string(), "degenerate geometry");
        assert_eq!(
            Error::from(TopologyError::WouldDisconnect).to_string(),
            "topology: would split the shell"
        );
    }
}
