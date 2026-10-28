use crate::geom::Point3;
use crate::name::{Name, ToolKey};
use crate::sweep::{LoopKey, SegmentKey};
use crate::topo::{EdgeId, FaceId};
use core::fmt;

/// Errors from fallible kernel constructors and operations.
#[derive(Clone, Debug, PartialEq)]
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
    /// Extrude or revolve parameters that don't fit the profile, or a path
    /// a profile can't be swept along.
    InvalidSweep(SweepError),
    /// Sections that can't be lofted: see [`crate::sweep::loft`].
    InvalidLoft(LoftError),
    /// An edge with no exact pcurve on a face it bounds: see
    /// [`crate::pcurve::exact`].
    NoPcurve { edge: EdgeId, face: FaceId },
    /// A boolean that can't be done: see [`crate::boolean::boolean`].
    InvalidBoolean(BooleanError),
    /// A blend that can't be done: see `crate::blend::blend`.
    InvalidBlend(BlendError),
    /// A shape that can't be healed into a body: see
    /// [`crate::heal::heal`].
    InvalidHeal(HealError),
    /// Bytes [`crate::codec`] can't decode: another format or version,
    /// cut short, corrupted, or describing a body that breaks the
    /// structural checks. `what` says which.
    InvalidEncoding { what: &'static str },
    /// A measure that can't be taken: see [`crate::measure`].
    InvalidMeasure(MeasureError),
}

/// Why a measure was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureError {
    /// The entity isn't a live vertex, edge or face of the body, or the
    /// measure doesn't apply to it (an angle of a point, an axis of a
    /// plane).
    NotAnEntity,
    /// Geometry no measure covers: a face that doesn't mesh, a pair with
    /// no closed form and no mesh.
    Unsupported,
    /// An intersection or a bound that stayed uncertain: refused rather
    /// than guessed.
    Uncertain,
}

/// Why a shape couldn't be healed into a body. Entities are named by
/// their import names (see [`crate::heal`]); nothing is built when one
/// is refused.
#[derive(Clone, Debug, PartialEq)]
pub enum HealError {
    /// The shape refers to an entity it doesn't have, or breaks its own
    /// rules in a way no file can mean: `what` says how.
    Malformed { what: &'static str },
    /// A shell's faces leave these edges used once: it doesn't close.
    OpenShell { free_edges: Vec<Name> },
    /// An edge used by more than two faces, or a vertex where faces meet
    /// in more than one fan: `at` is the edge or vertex.
    NonManifold { at: Name },
    /// The two uses of an edge run the same way along it, or a face or
    /// shell faces inwards: `at` is the edge or face.
    Misoriented { at: Name },
    /// Two shells that cross each other, each partly inside the other,
    /// so neither encloses the other: named by a face of each, sorted.
    ShellsCross { a: Name, b: Name },
    /// Two faces on one surface that overlap in an area, sorted.
    FacesOverlap { a: Name, b: Name },
    /// An edge one face uses twice that isn't a seam: a whole parameter
    /// line where the face's surface wraps.
    BadSeam { face: Name },
    /// A face whose loops don't bound a region of its surface ryukei can
    /// hold: no loops on a surface that doesn't close up, or loops that
    /// wind the wrong way.
    BadDomain { face: Name },
    /// An edge that gets no pcurve on a face it bounds: none exact, the
    /// file's off the edge, and none could be fitted.
    NoPcurve { edge: Name, face: Name },
    /// A face whose loops run the wrong way where they cross each other,
    /// or one crosses itself, so no direction for them can be read.
    SelfCrossingLoop { face: Name },
    /// A face whose one loop is two edges between the same vertices,
    /// narrower than the cap but wider than their tolerances: collapsing
    /// it would move the shape by more than a tolerance.
    SliverTooWide { face: Name },
    /// An edge, vertex or face farther from the geometry it should lie
    /// on than its tolerance allows: `gap` is the distance found.
    GapTooLarge { entity: Name, gap: f64 },
    /// A decision rounding left uncertain: refused rather than guessed.
    Uncertain { at: Name },
}

/// Why a boolean was refused. Entities are named by their stable names,
/// a tool's after it is placed (see [`crate::boolean`]).
#[derive(Clone, Debug, PartialEq)]
pub enum BooleanError {
    /// Two tools share a key.
    DuplicateToolKey { key: ToolKey },
    /// A placed tool leaves the size box.
    OutsideSizeBox { tool: ToolKey },
    /// Two faces that may meet on surfaces no intersector takes.
    Unsupported { a: Name, b: Name },
    /// The result would have an edge with more than two faces, or a
    /// vertex where faces meet in more than one fan: bodies touching along
    /// an edge or at a point.
    NonManifold { at: Point3, faces: Vec<Name> },
    /// An intersection, an ordering or a classification that stayed
    /// uncertain: refused rather than guessed.
    Uncertain { a: Name, b: Name },
}

/// Why a blend was refused. Entities are named by their stable names in
/// the body given to the blend; nothing is changed when it is refused.
#[derive(Clone, Debug, PartialEq)]
pub enum BlendError {
    /// No edge of the body has this name.
    NoSuchEdge { edge: Name },
    /// The edge is named twice.
    DuplicateEdge { edge: Name },
    /// A size that is not finite or under `10 × LINEAR_RESOLUTION`, or a
    /// chamfer angle outside `(0, π/2]`.
    BadSize { edge: Name },
    /// A chamfer names a face the edge does not bound.
    NotAFace { edge: Name, face: Name },
    /// The edge's faces meet tangentially somewhere along it: there is
    /// nothing to round.
    SmoothEdge { edge: Name },
    /// The edge is convex in one place and concave in another.
    ConvexityChanges { edge: Name },
    /// A blended edge meets an edge that is not blended tangentially:
    /// the blend would have to stop where nothing ends it. Blend the
    /// whole chain (see `crate::blend::tangent_chain`).
    OpenTangentChain { blended: Name, open: Name },
    /// Two edges that meet tangentially are blended with different kinds
    /// or sizes.
    SizeChangeAlongChain { a: Name, b: Name },
    /// A blend would run off `face`, a face it lies on, or cross another
    /// blend there.
    Overflow { edge: Name, face: Name },
    /// The radius or distance is too large for the curvature of the
    /// faces or the edge.
    RadiusTooLarge { edge: Name },
    /// A face, a vertex or an edge the blend can't handle yet: a surface
    /// it can't offset or intersect, or a kind of vertex it doesn't build.
    Unsupported { at: Name },
    /// A decision rounding left uncertain: refused rather than guessed.
    Uncertain { at: Name },
    /// Blends that would meet only at an edge or a point.
    NonManifold { at: Name },
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
    /// The region is the void around the body, where the operator needs a
    /// solid one.
    WrongRegion,
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
            Self::InvalidLoft(e) => write!(f, "invalid loft: {e}"),
            Self::NoPcurve { edge, face } => write!(
                f,
                "edge {} has no exact pcurve on face {}",
                edge.index(),
                face.index()
            ),
            Self::InvalidBoolean(b) => write!(f, "invalid boolean: {b}"),
            Self::InvalidBlend(b) => write!(f, "invalid blend: {b}"),
            Self::InvalidHeal(h) => write!(f, "can't heal: {h}"),
            Self::InvalidEncoding { what } => write!(f, "invalid encoding: {what}"),
            Self::InvalidMeasure(m) => write!(f, "can't measure: {m}"),
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

/// Why an extrude, a revolve or a sweep along a path refused its
/// parameters. Segments and loops are named by the caller's keys (a
/// path's segments by theirs), faces by their names.
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
    /// The swept solid would leave the size box.
    OutsideSizeBox,
    /// A sweep along a path with no segments.
    EmptyPath,
    /// A path segment that can't be swept along: its range is empty or
    /// runs backwards, leaves a clamped curve's domain or is longer than
    /// a period; it is shorter than resolution; its key is the key of an
    /// earlier one; it is a whole period in a path of more than one
    /// segment; or it is a closed path's one clamped curve, which can't
    /// be made periodic.
    BadPathSegment { segment: SegmentKey },
    /// The joint before `at` (or, at the first segment, a closed path's
    /// last) where segments don't meet within resolution, or meet at an
    /// angle more than [`crate::tol::ANGULAR_RESOLUTION`].
    PathNotG1 { at: SegmentKey },
    /// A profile whose plane is within
    /// [`crate::tol::ANGULAR_RESOLUTION`] of the path's start tangent.
    ProfileAlongPath,
    /// A frame law undefined somewhere along `segment`: a binormal
    /// direction the path's tangent runs along.
    FrameUndefined { segment: SegmentKey },
    /// The profile can't be swept along `segment` without folding: it
    /// reaches past the path's centre of curvature, or the sweep can't be
    /// certified regular.
    PathTooCurved { segment: SegmentKey },
    /// A scale that isn't a positive number.
    BadScale,
    /// A twist that isn't a whole number of turns, or a scale other than
    /// 1, on a closed path: the profile wouldn't come back to itself.
    NotClosing,
    /// Two faces of the swept body that meet away from their shared
    /// edges: the path brings the body back into itself.
    SelfIntersecting { faces: [Name; 2] },
    /// Two faces of the swept body whose meeting can't be certified, so
    /// the body can't be checked: refused rather than guessed.
    Uncertain { faces: [Name; 2] },
}

/// Why sections couldn't be lofted. Sections are numbered from 0, loops
/// and segments named by the caller's keys (a segment by the first
/// section's), faces by their names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoftError {
    /// Fewer than two sections, or than three round a closed loft.
    TooFewSections,
    /// A section whose lumps and loops don't match the first's: a loop
    /// the first has that it lacks, or one in another lump or role.
    LoopMismatch { section: usize, loop_: LoopKey },
    /// A section whose loop has another number of segments than the
    /// first's.
    SegmentCountMismatch { section: usize, loop_: LoopKey },
    /// A section in the plane of the one before it, or facing the other
    /// way from it.
    BadSection { section: usize },
    /// An end tangent that isn't finite and positive, points along its
    /// section's plane, or is given for a closed loft.
    BadTangent,
    /// The skin's weights would fall to zero or below between sections.
    BadWeights { loop_: LoopKey, segment: SegmentKey },
    /// Two faces of the lofted body that meet away from their shared
    /// edges.
    SelfIntersecting { faces: [Name; 2] },
    /// Two faces of the lofted body whose meeting can't be certified.
    Uncertain { faces: [Name; 2] },
}

impl fmt::Display for LoftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewSections => f.write_str("too few sections"),
            Self::LoopMismatch { section, loop_ } => {
                write!(f, "section {section} doesn't match loop {:#x}", loop_.0)
            }
            Self::SegmentCountMismatch { section, loop_ } => write!(
                f,
                "section {section} has another number of segments in loop {:#x}",
                loop_.0
            ),
            Self::BadSection { section } => {
                write!(
                    f,
                    "section {section} lies in the plane before or faces back"
                )
            }
            Self::BadTangent => f.write_str("an end tangent can't be used"),
            Self::BadWeights { loop_, segment } => write!(
                f,
                "the skin's weights fall to zero along segment {:#x} of loop {:#x}",
                segment.0, loop_.0
            ),
            Self::SelfIntersecting { faces: [a, b] } => {
                write!(f, "faces {a} and {b} of the loft meet")
            }
            Self::Uncertain { faces: [a, b] } => {
                write!(
                    f,
                    "where faces {a} and {b} of the loft meet could not be certified"
                )
            }
        }
    }
}

impl From<LoftError> for Error {
    fn from(e: LoftError) -> Self {
        Self::InvalidLoft(e)
    }
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
            Self::OutsideSizeBox => f.write_str("the sweep leaves the size box"),
            Self::EmptyPath => f.write_str("the path has no segments"),
            Self::BadPathSegment { segment } => {
                write!(f, "path segment {:#x} can't be swept along", segment.0)
            }
            Self::PathNotG1 { at } => {
                write!(f, "the path isn't smooth at the joint before {:#x}", at.0)
            }
            Self::ProfileAlongPath => f.write_str("the profile's plane runs along the path"),
            Self::FrameUndefined { segment } => {
                write!(f, "the frame law is undefined along {:#x}", segment.0)
            }
            Self::PathTooCurved { segment } => {
                write!(f, "the profile would fold along {:#x}", segment.0)
            }
            Self::BadScale => f.write_str("the scale isn't positive"),
            Self::NotClosing => {
                f.write_str("the twist or scale doesn't bring the profile round a closed path")
            }
            Self::SelfIntersecting { faces: [a, b] } => {
                write!(f, "faces {a} and {b} of the sweep meet")
            }
            Self::Uncertain { faces: [a, b] } => {
                write!(
                    f,
                    "where faces {a} and {b} of the sweep meet could not be certified"
                )
            }
        }
    }
}

impl fmt::Display for BooleanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateToolKey { key } => write!(f, "tool key {:#x} is used twice", key.0),
            Self::OutsideSizeBox { tool } => {
                write!(f, "tool {:#x} leaves the size box", tool.0)
            }
            Self::Unsupported { a, b } => write!(
                f,
                "faces {a} and {b} may meet on surfaces no intersector takes"
            ),
            Self::NonManifold { at, faces } => write!(
                f,
                "the result would be non-manifold at ({}, {}, {}), between {} faces",
                at.x,
                at.y,
                at.z,
                faces.len()
            ),
            Self::Uncertain { a, b } => {
                write!(f, "where faces {a} and {b} meet could not be certified")
            }
        }
    }
}

impl fmt::Display for BlendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchEdge { edge } => write!(f, "no edge is named {edge}"),
            Self::DuplicateEdge { edge } => write!(f, "edge {edge} is named twice"),
            Self::BadSize { edge } => write!(f, "edge {edge} has a bad size"),
            Self::NotAFace { edge, face } => write!(f, "edge {edge} does not bound face {face}"),
            Self::SmoothEdge { edge } => write!(f, "the faces of edge {edge} are tangent"),
            Self::ConvexityChanges { edge } => {
                write!(f, "edge {edge} is convex in places and concave in others")
            }
            Self::OpenTangentChain { blended, open } => write!(
                f,
                "blended edge {blended} meets edge {open} tangentially and it is not blended"
            ),
            Self::SizeChangeAlongChain { a, b } => {
                write!(f, "tangent edges {a} and {b} are blended differently")
            }
            Self::Overflow { edge, face } => {
                write!(f, "the blend of edge {edge} runs off face {face}")
            }
            Self::RadiusTooLarge { edge } => write!(f, "the blend of edge {edge} is too large"),
            Self::Unsupported { at } => write!(f, "blending at {at} is not supported"),
            Self::Uncertain { at } => write!(f, "blending at {at} could not be certified"),
            Self::NonManifold { at } => {
                write!(f, "blends would meet in an edge or a point at {at}")
            }
        }
    }
}

impl fmt::Display for HealError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { what } => write!(f, "malformed shape: {what}"),
            Self::OpenShell { free_edges } => {
                write!(f, "a shell is open at {} edges", free_edges.len())?;
                if let Some(e) = free_edges.first() {
                    write!(f, ", the first {e}")?;
                }
                Ok(())
            }
            Self::NonManifold { at } => write!(f, "faces meet in more than one fan at {at}"),
            Self::Misoriented { at } => write!(f, "{at} is oriented against its neighbours"),
            Self::ShellsCross { a, b } => write!(f, "the shells of {a} and {b} cross"),
            Self::FacesOverlap { a, b } => write!(f, "faces {a} and {b} overlap"),
            Self::BadSeam { face } => write!(f, "face {face} uses an edge twice off a seam"),
            Self::BadDomain { face } => write!(f, "the loops of face {face} bound no region"),
            Self::NoPcurve { edge, face } => write!(f, "edge {edge} has no pcurve on face {face}"),
            Self::SelfCrossingLoop { face } => write!(f, "the loops of face {face} cross"),
            Self::SliverTooWide { face } => {
                write!(f, "face {face} is a sliver wider than its tolerance")
            }
            Self::GapTooLarge { entity, gap } => {
                write!(f, "{entity} is {gap:e} m from where it should be")
            }
            Self::Uncertain { at } => write!(f, "healing at {at} could not be certified"),
        }
    }
}

impl fmt::Display for MeasureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAnEntity => "not an entity this measure applies to",
            Self::Unsupported => "geometry no measure covers",
            Self::Uncertain => "the result could not be certified",
        })
    }
}

impl From<MeasureError> for Error {
    fn from(m: MeasureError) -> Self {
        Self::InvalidMeasure(m)
    }
}

impl From<HealError> for Error {
    fn from(h: HealError) -> Self {
        Self::InvalidHeal(h)
    }
}

impl From<BlendError> for Error {
    fn from(b: BlendError) -> Self {
        Self::InvalidBlend(b)
    }
}

impl From<BooleanError> for Error {
    fn from(b: BooleanError) -> Self {
        Self::InvalidBoolean(b)
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
            Self::WrongRegion => "not a solid region",
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
            Error::NoPcurve {
                edge: crate::topo::Id::from_index(3),
                face: crate::topo::Id::from_index(1)
            }
            .to_string(),
            "edge 3 has no exact pcurve on face 1"
        );
        assert_eq!(
            Error::from(TopologyError::WrongRegion).to_string(),
            "topology: not a solid region"
        );
        assert_eq!(
            Error::from(TopologyError::Inconsistent).to_string(),
            "topology: the result fails the topology check"
        );
        let n = Name::from_u128(0x2a);
        assert_eq!(
            Error::from(BlendError::Overflow { edge: n, face: n }).to_string(),
            "invalid blend: the blend of edge 0000000000000000000000000000002a runs off face \
             0000000000000000000000000000002a"
        );
    }
}
