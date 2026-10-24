//! The one error type. Every error crosses the protocol as
//! `{code, message, details}`, so it is built in that shape from the start.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;

/// A stable error code. Agents branch on these, so a code is never renamed
/// or reused; new failures get new codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// A file or argument is not valid JSON.
    InvalidJson,
    /// JSON of the wrong shape: a missing or unexpected field, a wrong type.
    InvalidDocument,
    /// A quantity string that doesn't parse, or a non-positive length.
    InvalidQuantity,
    /// A part or feature ID that breaks the naming rules.
    InvalidId,
    /// Axes with no direction, or parallel.
    InvalidPlacement,
    /// A feature `type` this version doesn't know.
    UnknownFeatureType,
    /// A parameter that is missing, unknown, or out of range.
    InvalidParam,
    /// The ID is already taken.
    DuplicateId,
    /// No such part, feature or file.
    NotFound,
    /// The part changed since the agent last saw it.
    Conflict,
    /// The kernel refused a feature.
    RebuildFailed,
    /// `init` in a folder that already holds a project.
    ProjectExists,
    /// No `ryucad.json` here or above.
    ProjectNotFound,
    /// A file written by a newer ryucad.
    FormatTooNew,
    /// Bad command-line arguments.
    Usage,
    /// Reading or writing a file failed.
    Io,
    /// The daemon couldn't be reached or answered badly.
    Daemon,
    /// A bug: a panic or a broken invariant.
    Internal,
    /// A request that needs the daemon's live session, run without one.
    NoLiveSession,
    /// "The human" was meant, but several are attached.
    AmbiguousParticipant,
    /// Nothing of the caller's is left to undo or redo.
    NothingToUndo,
    /// The files an undo would restore have changed since.
    UndoConflict,
    /// A setting no schema knows.
    UnknownSetting,
    /// A setting's value, or scope, is wrong.
    InvalidSetting,
    /// The project isn't in a Git repository (a warning, not a failure).
    NotAGitRepo,
    /// The kernel couldn't mesh a body for display.
    MeshFailed,
    /// A render larger than the limit, or with no pixels.
    RenderTooLarge,
    /// A folder that looks like a project but can't be opened (no
    /// manifest): `ryucad doctor` can repair it.
    ProjectDamaged,
    /// A repair planned against files that have changed since.
    RepairConflict,
    /// `ryucad open` found no desktop app to start (a CLI-only install).
    AppNotInstalled,
    /// An expression names a parameter the part doesn't have.
    UnknownParameter,
    /// An expression mixes what can't be mixed (a length plus an angle),
    /// or gives the wrong kind (an angle where a length goes).
    UnitMismatch,
    /// Parameters that read each other in a circle.
    ParameterCycle,
    /// Refused: other features or parameters read what would go.
    HasDependents,
    /// A sketch whose constraints can't all be made to hold.
    SketchUnsolved,
    /// A sketch edit that would leave constraints redundant or in conflict.
    SketchOverconstrained,
    /// A sketch's loops don't make a profile to sweep: they cross, don't
    /// close, or there are none.
    ProfileInvalid,
    /// An extrude's length or a revolve's angle out of range.
    InvalidExtent,
    /// A feature that refers to one after it in the part, or to itself.
    ReferenceForward,
    /// A reference to the wrong kind of thing: a box as a sketch, a circle
    /// as an axis.
    ReferenceWrongKind,
    /// A feature whose input, another feature, failed to rebuild.
    DependencyFailed,
    /// A reference to a face, edge or vertex whose name the referenced
    /// feature's body no longer has.
    ReferenceUnresolved,
    /// An edge a sketch can't project: neither a line nor a circle parallel
    /// to the sketch, or one that projects to nothing.
    ProjectionUnsupported,
    /// A Git revision (`main`, `HEAD~2`, a hash) the repository doesn't
    /// have.
    RevNotFound,
    /// `ryucad check` found errors; its report is in the details.
    CheckFailed,
    /// A file a merge left with conflicts (a top-level `conflicts` key):
    /// `ryucad merge status` lists them, `ryucad merge resolve` settles them.
    MergeConflict,
    /// A merge request when Git is in no merge, cherry-pick, revert or
    /// rebase step.
    NotMerging,
    /// A merge ryucad can't report on: an octopus merge, of more than two
    /// heads.
    MergeUnsupported,
    /// `merge.resolve` named a conflict the merge doesn't have (now).
    ConflictNotFound,
    /// A part property that is neither one of ryupdm's core properties nor
    /// declared in `ryupdm.json`.
    PropertyUnknown,
    /// A property value of the wrong type, outside its choices or of the
    /// wrong dimension; or a declaration of a core property's name.
    PropertyInvalid,
    /// A part number that doesn't match the project's pattern.
    PartNumberInvalid,
    /// Two parts of one project with the same number.
    PartNumberDuplicate,
    /// A part ID that a release record names, so it can't be used again.
    PartIdRetired,
    /// The revision scheme can't change once any part has been released.
    RevisionSchemeLocked,
    /// A join, cut or intersect that changes nothing of its target: the
    /// tool misses it, or a join lies inside it.
    OperationNoEffect,
    /// A cut or intersect that leaves nothing of its target.
    BodyEmptied,
    /// The kernel refused to join, cut or intersect: faces it can't
    /// intersect yet, an uncertain intersection, or a result touching
    /// itself. The faces are in the details.
    BooleanFailed,
    /// A feature acts on, or refers to, a solid that a `boolean` feature
    /// before it used as a tool: it is part of the boolean's target now.
    BodyConsumed,
    /// A part that can't be released as it is: a load, rebuild or property
    /// fault, a missing number or required property (each in `problems`).
    ReleaseNotReady,
    /// The part is already its latest release (or that revision is already
    /// obsolete).
    NothingToRelease,
    /// The part has a proposed record that isn't official yet.
    ReleasePending,
    /// A fillet or chamfer whose edges belong to more than one solid: one
    /// feature rounds or bevels one solid.
    TargetsMixed,
    /// An edge a fillet or chamfer rounds or bevels in two of its groups,
    /// listed there or reached along a tangent chain.
    EdgeListedTwice,
    /// The kernel refused a fillet or chamfer: a radius too large, a blend
    /// that runs off a face, a corner or surface it can't build yet. Its
    /// reason and the edges and faces involved are in the details.
    BlendFailed,
}

impl Code {
    /// The CLI's exit code: 1 the agent can fix it, 2 usage, 3 not a
    /// usable project, 4 not the agent's fault.
    pub fn exit_code(self) -> u8 {
        match self {
            Code::Usage => 2,
            Code::ProjectNotFound | Code::FormatTooNew | Code::ProjectDamaged => 3,
            Code::Io | Code::Daemon | Code::Internal | Code::AppNotInstalled => 4,
            _ => 1,
        }
    }
}

/// An error with a stable code, a message for people, and structured
/// details for agents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub code: Code,
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

impl Error {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: Value::Null,
        }
    }

    /// Adds `key: value` to the details object.
    pub fn with(mut self, key: &str, value: impl Serialize) -> Self {
        if !self.details.is_object() {
            self.details = json!({});
        }
        self.details[key] = serde_json::to_value(value).unwrap_or(Value::Null);
        self
    }

    pub fn io(path: &std::path::Path, e: &std::io::Error) -> Self {
        Self::new(Code::Io, format!("{}: {e}", path.display())).with("path", path)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T, E = Error> = core::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialize_as_snake_case() {
        assert_eq!(json!(Code::FormatTooNew), json!("format_too_new"));
        assert_eq!(json!(Code::InvalidJson), json!("invalid_json"));
    }

    #[test]
    fn exit_codes() {
        assert_eq!(Code::InvalidParam.exit_code(), 1);
        assert_eq!(Code::Conflict.exit_code(), 1);
        assert_eq!(Code::Usage.exit_code(), 2);
        assert_eq!(Code::ProjectNotFound.exit_code(), 3);
        assert_eq!(Code::FormatTooNew.exit_code(), 3);
        assert_eq!(Code::Internal.exit_code(), 4);
        assert_eq!(Code::NoLiveSession.exit_code(), 1);
        assert_eq!(Code::AmbiguousParticipant.exit_code(), 1);
        assert_eq!(Code::AppNotInstalled.exit_code(), 4);
        assert_eq!(Code::ProjectDamaged.exit_code(), 3);
        assert_eq!(Code::UnknownParameter.exit_code(), 1);
        assert_eq!(Code::UnitMismatch.exit_code(), 1);
        assert_eq!(Code::ParameterCycle.exit_code(), 1);
        assert_eq!(Code::HasDependents.exit_code(), 1);
        assert_eq!(Code::MergeConflict.exit_code(), 1);
        assert_eq!(Code::NotMerging.exit_code(), 1);
        assert_eq!(Code::MergeUnsupported.exit_code(), 1);
        assert_eq!(Code::ConflictNotFound.exit_code(), 1);
        assert_eq!(Code::PropertyUnknown.exit_code(), 1);
        assert_eq!(Code::PartNumberDuplicate.exit_code(), 1);
        assert_eq!(Code::PartIdRetired.exit_code(), 1);
        assert_eq!(Code::RevisionSchemeLocked.exit_code(), 1);
        assert_eq!(Code::ReleaseNotReady.exit_code(), 1);
        assert_eq!(Code::NothingToRelease.exit_code(), 1);
        assert_eq!(Code::ReleasePending.exit_code(), 1);
        assert_eq!(Code::TargetsMixed.exit_code(), 1);
        assert_eq!(Code::EdgeListedTwice.exit_code(), 1);
        assert_eq!(Code::BlendFailed.exit_code(), 1);
    }

    #[test]
    fn details_accumulate() {
        let e = Error::new(Code::NotFound, "no part")
            .with("part", "a")
            .with("n", 2);
        assert_eq!(e.details, json!({"part": "a", "n": 2}));
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"code": "not_found", "message": "no part", "details": {"part": "a", "n": 2}})
        );
        let bare = serde_json::to_value(Error::new(Code::Internal, "x")).unwrap();
        assert_eq!(bare, json!({"code": "internal", "message": "x"}));
    }
}
