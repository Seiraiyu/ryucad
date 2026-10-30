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
    /// `ryucad review` has no GitHub token, or the service refused the one
    /// it has.
    ReviewUnauthenticated,
    /// The clone has no `origin` remote on GitHub, so `ryucad review`
    /// doesn't know which repository to ask about.
    ReviewNoRemote,
    /// The review service can't be reached (network, DNS, TLS, timeout).
    ReviewUnreachable,
    /// The App isn't installed on the repository, or the repository isn't
    /// in the beta.
    ReviewNotInstalled,
    /// The service has no such pull request, thread or artifact.
    ReviewNotFound,
    /// The token can't read the repository, or can't push to the branch a
    /// merge-in would update.
    ReviewForbidden,
    /// The service hasn't finished the work asked about; try again later.
    ReviewPending,
    /// The PR's head moved since the merge was validated.
    ReviewHeadMoved,
    /// The PR's base moved since the merge was validated; it is being
    /// validated again.
    ReviewBaseMoved,
    /// A fork's PR whose head repository the App isn't installed on.
    ReviewForkNotInstalled,
    /// A job passed one of the service's limits (time, memory, disk, size).
    ReviewLimitExceeded,
    /// A comment named an entity reference the service can't read.
    ReviewReferenceInvalid,
    /// The service failed, or answered with a code this build doesn't know
    /// (in the details as `service_code`).
    ReviewFailed,
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
    /// A feature of a kind its part doesn't hold: an instance in a part,
    /// or a solid, sketch or blend in an assembly.
    WrongPartKind,
    /// An assembly that places itself, through instances at any depth.
    /// The path is in the details.
    AssemblyCycle,
    /// An instance names a release label its part has no record of.
    LabelNotFound,
    /// An instance names a release that isn't official yet, and whose
    /// proposed record doesn't describe the part's files as they are.
    LabelNotOfficial,
    /// An instance a release can't hold: at `working` inside a part read
    /// at a release label, or (releasing an assembly) at `working`, at a
    /// label that isn't official, or at an obsolete one.
    InstanceNotReleased,
    /// A move of an instance that is fixed where it is.
    InstanceFixed,
    /// A connector on an entity no frame is defined from: a NURBS or
    /// swept face, an ellipse or a spline edge.
    ConnectorUnsupported,
    /// An assembly whose mates can't all hold. The conflicting mates are
    /// in the details.
    AssemblyUnsolved,
    /// A write that would leave an assembly's mates unable to all hold.
    /// The conflicting mates are in the details.
    AssemblyOverconstrained,
    /// An import feature names a file under `imports/` that isn't there.
    ImportFileMissing,
    /// An import feature's file isn't the bytes it was written against:
    /// its BLAKE3 differs from the one the feature holds.
    ImportFileChanged,
    /// A file ryustep refuses as a whole: not STEP, a syntax error, a
    /// schema it doesn't read. ryustep's reason is in the details.
    StepUnreadable,
    /// An import feature names a product its file doesn't have. The
    /// file's products are in the details.
    ImportProductMissing,
    /// An import feature names a body its product doesn't have.
    ImportBodyMissing,
    /// A body of an imported file that can't be read or healed. Its
    /// reason and the STEP instances involved are in the details.
    ImportFailed,
    /// An export of a part or assembly some feature of which fails, at
    /// any depth. The failing features are in the details.
    ExportFailed,
    /// An export of a body STEP can't describe: a face that can't be cut
    /// open along seams. The face is in the details.
    ExportUnrepresentable,
    /// The review service's beta doesn't take this account or repository.
    ReviewNotInBeta,
    /// The review service refused a request as malformed.
    ReviewInvalidRequest,
    /// The review service can't merge the pull request's base in: it is
    /// closed, holds no project, failed, conflicts (settle it locally) or
    /// is up to date. Its reason is in the details (`service.reason`).
    ReviewMergeUnavailable,
    /// The review service couldn't reach GitHub.
    ReviewGithubUnreachable,
    /// An assembly places a part that has no item number.
    BomItemMissing,
    /// Two parts of one assembly share an item number.
    BomItemDuplicate,
    /// An assembly gives an item number to a part it no longer places.
    BomItemUnused,
    /// A measure ryukei refuses: an entity it doesn't apply to, geometry
    /// it doesn't cover, or a result it can't certify. The reason
    /// (`not_an_entity`, `unsupported`, `uncertain`) and the entities are
    /// in the details.
    MeasureFailed,
    /// A part or assembly has no named view by that ID (`view.delete`,
    /// `view.rename`, `camera.set` and `render` with `view`); the details
    /// name the part and the views it has.
    ViewNotFound,
    /// An appearance entry naming a solid, face or occurrence the model
    /// no longer has (a doctor warning, and a warning of `appearance.get`):
    /// kept, never moved to another, with `removed_by` where the session
    /// knows what took a face away.
    AppearanceUnresolved,
    /// An assembly has no exploded view by that ID (`explode.delete`,
    /// `explode.show`, `render` with `explode`); the details name the
    /// assembly and the explodes it has.
    ExplodeNotFound,
    /// An explode's step names an instance path the assembly no longer
    /// places (a warning: the step moves the rest).
    ExplodeUnresolved,
    /// A ryueda library alias whose path holds no library project: no
    /// project there, not a library, or outside the repository.
    LibraryMissing,
    /// A ryueda library project that names libraries of its own, or one of
    /// its files that names another library: references stay one step.
    LibraryNested,
    /// A symbol's units: a count under 1, a pin or graphic in a unit the
    /// symbol doesn't have, or a unit with no pins.
    LibraryUnitInvalid,
    /// A symbol pin that can't be: a number that can't name it, a negative
    /// length, an alternate repeated.
    LibraryPinInvalid,
    /// A symbol or footprint graphic that can't be drawn: an arc through
    /// three collinear points, a radius or size that isn't positive, too
    /// few points.
    LibraryGraphicInvalid,
    /// A coordinate or size beyond a metre from a library object's origin.
    LibraryOutOfRange,
    /// A footprint pad that can't be made: a size that isn't positive, a
    /// drill outside its pad, a custom shape that crosses itself.
    LibraryPadInvalid,
    /// A footprint layer ryueda doesn't know.
    LibraryLayerUnknown,
    /// A footprint with no courtyard (a warning).
    LibraryCourtyardMissing,
    /// A footprint names a 3D model under `library/models/` that isn't
    /// there.
    ModelFileMissing,
    /// A model file isn't the bytes the footprint was written against: its
    /// BLAKE3 differs.
    ModelFileChanged,
    /// A model file that isn't a STEP exchange structure. The reason is in
    /// the details.
    ModelUnreadable,
    /// A symbol has no pin of that number.
    PinNotFound,
    /// A library component maps a symbol pin to no pad of a footprint.
    PinWithoutPad,
    /// A numbered pad of a library component's footprint that no pin
    /// maps (a warning).
    PadWithoutPin,
    /// A pad that two pins of a library component map.
    PadMappedTwice,
    /// A ryueda block that contains itself, through instances at some
    /// depth. The chain is in the details.
    BlockCycle,
    /// A block has no port of that ID or name.
    PortNotFound,
    /// A block has no net of that ID or name.
    NetNotFound,
    /// Two nets, or a net and a port, of one block share a name, or a
    /// block's net takes a rail's name.
    NetNameDuplicate,
    /// A net that pins or instances still name can't be deleted.
    NetHasPins,
    /// ERC: two pins on one net whose types conflict by the pin-type
    /// matrix (an output with an output, a power output with an output).
    PinConflict,
    /// ERC: a net with power inputs and nothing to drive them: no power
    /// output, no rail, no power port.
    PowerUndriven,
    /// ERC: a component pin on no net and not marked not connected.
    PinUnconnected,
    /// ERC: a pin of type `no_connect` that is on a net.
    NcConnected,
    /// ERC: a net with one pin.
    NetSinglePin,
    /// ERC: a net of input pins only.
    InputUndriven,
    /// ERC: two component occurrences with one reference designator.
    RefdesDuplicate,
    /// ERC: a component with no reference designator.
    RefdesMissing,
    /// ERC: a component with no footprint to take: its library component
    /// has none, or not the one it names.
    FootprintMissing,
    /// ERC: a differential pair whose nets don't name each other, or
    /// whose polarities match.
    PairUnmatched,
    /// ERC: nets joined through a port or a rail with different voltages.
    IntentConflict,
    /// ERC: a library object a design places that fails its own checks.
    LibraryFault,
    /// ERC: a waiver in `erc.json` whose finding is gone (a warning).
    WaiverUnused,
    /// A KiCad file, or an item in one, that ryueda can't read: an
    /// s-expression that doesn't parse (line and column in the details),
    /// or a symbol or footprint that can't be what ryueda holds.
    KicadUnreadable,
    /// A KiCad file in a legacy format (`.lib`, `.dcm`, a KiCad 5
    /// `module`): open and save it in KiCad 6 or later first.
    KicadLegacyUnsupported,
    /// Import: fields of a KiCad file ryueda has no place for, named once
    /// per file (a warning).
    ImportIgnored,
    /// Import: numbers finer than a nanometre or a millidegree, rounded
    /// half to even (a warning naming them as written).
    ImportRounded,
    /// Import: a KiCad power symbol, skipped: a rail is a net with intent,
    /// not a component (a warning).
    ImportPowerSkipped,
    /// Import: a symbol's De Morgan body style, dropped (a warning).
    ImportDemorganDropped,
    /// Import: a KiCad layer with no ryueda equivalent; what is on it is
    /// left out (a warning).
    ImportLayerUnmapped,
    /// Import: a 3D model that isn't STEP and has no STEP file beside it,
    /// left out (a warning).
    ImportModelSkipped,
    /// Import: a footprint a symbol names, or a model a footprint names,
    /// that isn't where the import was told to look (a warning).
    ImportUnresolved,
    /// Import: a KiCad file in a format newer than KiCad 9's, read as far
    /// as ryueda knows it (a warning).
    ImportVersionNewer,
    /// A warning: a daemon can't watch its project folder for file events
    /// (a filesystem without them, a watch limit reached), so it checks
    /// every file on every request. Why is in the details.
    WatchUnavailable,
    /// A warning: what a daemon must hold in memory (the bodies its
    /// assemblies show, and display meshes) is more than
    /// `session.max_memory`, so it holds more than its budget.
    MemoryBudgetTooSmall,
    /// An export to a mesh format of a body with faces ryukei can't mesh
    /// to the export's chord and angle. The faces, by reference, and
    /// ryukei's fault for each are in the details.
    ExportMeshFaults,
    /// An export to a mesh format of a body whose mesh would pass
    /// ryukei's triangle budget at the export's chord and angle. The
    /// body and the estimate are in the details.
    ExportTooLarge,
    /// ryudfm: not an ODB++ job: no `matrix/matrix` at the root or under
    /// one top folder, or not a folder, tar, gzipped tar or zip.
    NotABoard,
    /// ryudfm: the job's matrix, `misc/info` or board step couldn't be
    /// read (the file inside the job, line and column in the details).
    OdbUnreadable,
    /// ryudfm: `misc/info` names an ODB++ version other than 7.0, 8.0 or
    /// 8.1.
    OdbVersionUnsupported,
    /// ryudfm: more than one step, or none, could be the board: name one
    /// with `--step` (the candidates in `steps`).
    OdbStepAmbiguous,
    /// ryudfm: the step asked for isn't in the job (its steps in
    /// `steps`).
    OdbStepNotFound,
    /// ryudfm: an archive entry with an absolute path, `..`, a link or a
    /// device, or an archive past a million entries or 8 GiB unpacked.
    ArchiveUnsafe,
    /// ryudfm: a board whose input isn't in the project.
    BoardInputMissing,
    /// ryudfm: a board whose input's BLAKE3 isn't the one `board.json`
    /// names (`expected` and `found`).
    BoardInputChanged,
    /// ryudfm fault: a layer's features file that doesn't parse; the layer
    /// is read with no features.
    OdbLayerUnreadable,
    /// ryudfm fault: a symbol name neither standard nor a user symbol;
    /// the features using it have no shape.
    OdbSymbolUnknown,
    /// ryudfm fault: user symbols that use themselves.
    OdbSymbolCycle,
    /// ryudfm fault: a components file, or a component in it, that
    /// doesn't read.
    OdbComponentInvalid,
    /// ryudfm fault: an `eda/data` file, or a record in it, that doesn't
    /// read.
    OdbEdaInvalid,
    /// ryudfm fault: any other file of a job that doesn't read (a step
    /// header, profile, attribute list, tools file, netlist, font, stackup
    /// or user symbol).
    OdbFileInvalid,
    /// ryudfm: a record kind, file or attribute the reader doesn't take,
    /// left out (a warning).
    OdbIgnored,
    /// ryudfm: numbers finer than a nanometre, rounded half to even (a
    /// warning with the count and the largest change).
    OdbRounded,
    /// ryudfm: an arc whose written end is more than 1 µm off its circle
    /// (a warning).
    OdbArcInconsistent,
    /// ryudfm: characters a text's font lacks, left as gaps (a warning).
    OdbFontMissingChar,
    /// ryudfm: dynamic text (`$$DATE`), kept as written (a warning).
    OdbTextDynamic,
    /// ryudfm: barcodes, kept and left out of shapes (a warning).
    OdbBarcodeSkipped,
    /// ryudfm: no `misc/info`, or one lacking fields: read as ODB++ 7.0 (a
    /// warning).
    OdbInfoMissing,
    /// The remote cache didn't answer, or answered with a server error: a
    /// warning (the local cache serves alone, and the remote is tried again
    /// after a while), or the error of `cache push`.
    CacheRemoteUnreachable,
    /// The remote cache refused the token (401), or a read token may not
    /// write (403): a warning, or the error of `cache push`.
    CacheRemoteRefused,
    /// A note: an entry from the remote cache failed its header, hash or
    /// decoding and was dropped, never used.
    CacheEntryBad,
    /// A path that isn't a worktree of the project's repository holding
    /// the project.
    WorktreeNotFound,
    /// `worktree remove` of a worktree with changes Git would lose,
    /// without `--force`.
    WorktreeDirty,
    /// A dependency in `ryucad.json` with a bad name, URL or path.
    DependencyInvalid,
    /// An instance naming a project that is neither a dependency nor one
    /// the lock reaches through one.
    DependencyMissing,
    /// What a dependency's label needs isn't in the machine's dependency
    /// cache: run `ryucad deps fetch` (the dependency and its URL in the
    /// details).
    DependencyNotFetched,
    /// Fetching a dependency failed (its URL and Git's reason in the
    /// details; credentials are never shown).
    DependencyFetchFailed,
    /// A dependency's folder holds no project, or one with another ID, or
    /// a record there names another project.
    DependencyProjectMismatch,
    /// A dependency's release branch no longer makes a locked label
    /// official at its locked commit: its history was rewritten.
    DependencyRewritten,
    /// A foreign label the working files reach that the lock lacks.
    DependencyUnlocked,
    /// An instance of another project's part at `working`: only official
    /// labels may be placed from other projects.
    InstanceForeignWorking,
    /// A warning: an instance at a label older than its part's latest
    /// official, non-obsolete one.
    InstanceOutdated,
    /// `ryucad.lock` that doesn't read.
    LockInvalid,
    /// A warning: a lock entry nothing the working files place reaches.
    LockUnused,
    /// A dependency with the project's own ID, or one project ID listed
    /// under two names.
    ProjectIdDuplicate,
    /// ryueda: a block's layout no longer matches its connectivity (a
    /// file edited by hand, a checkout): laid out again in memory, written
    /// only by `layout` (a warning).
    LayoutStale,
    /// ryueda: a layout file that doesn't read, or breaks a hard rule:
    /// laid out again in memory, never trusted.
    LayoutInvalid,
    /// ryueda: an instruction whose result would break a hard rule of
    /// readability; the rule is in the details.
    LayoutRule,
    /// ryueda: a sheet with no room for what was asked of it; the item
    /// goes on a new sheet meanwhile, and the details list fixes.
    SheetFull,
    /// ryueda: instructions that can't hold together, naming each other.
    InstructionConflict,
    /// ryueda: an instruction whose subject is gone, kept until released
    /// (a warning).
    InstructionUnresolved,
    /// ryueda: a reference designator format that doesn't parse, or does
    /// arithmetic on a property that isn't an integer.
    RefdesFormatInvalid,
    /// ryueda's schematic lint (warnings): wires that cross.
    SchCrossings,
    /// Wires with more bends than they need.
    SchBends,
    /// A wire longer than a sheet's lint threshold.
    SchLongWire,
    /// A pair of labels a short wire would replace.
    SchLabelWhereWire,
    /// A wire long or crowded enough that labels would read better.
    SchWireWhereLabel,
    /// A sheet holding more ink than its area reads well with.
    SchDensity,
    /// A signal running right to left with no instruction saying so.
    SchFlowReversed,
    /// A symbol whose pins aren't on the 50 mil grid relative to each
    /// other.
    SchPinOffGrid,
    /// An idiom drawn apart from its canonical shape.
    SchIdiomMissed,
}

impl Code {
    /// The CLI's exit code: 1 the agent can fix it, 2 usage, 3 not a
    /// usable project, 4 not the agent's fault.
    pub fn exit_code(self) -> u8 {
        match self {
            Code::Usage => 2,
            Code::ProjectNotFound | Code::FormatTooNew | Code::ProjectDamaged => 3,
            Code::Io
            | Code::Daemon
            | Code::Internal
            | Code::AppNotInstalled
            | Code::ReviewUnreachable
            | Code::ReviewGithubUnreachable
            | Code::ReviewFailed
            | Code::CacheRemoteUnreachable
            | Code::DependencyFetchFailed => 4,
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
        assert_eq!(json!(Code::ExportFailed), json!("export_failed"));
        assert_eq!(
            json!(Code::ExportUnrepresentable),
            json!("export_unrepresentable")
        );
        assert_eq!(json!(Code::BomItemMissing), json!("bom_item_missing"));
        assert_eq!(json!(Code::BomItemDuplicate), json!("bom_item_duplicate"));
        assert_eq!(json!(Code::BomItemUnused), json!("bom_item_unused"));
        assert_eq!(json!(Code::MeasureFailed), json!("measure_failed"));
        assert_eq!(json!(Code::ViewNotFound), json!("view_not_found"));
        assert_eq!(
            json!(Code::AppearanceUnresolved),
            json!("appearance_unresolved")
        );
        assert_eq!(json!(Code::ExplodeNotFound), json!("explode_not_found"));
        assert_eq!(json!(Code::ExplodeUnresolved), json!("explode_unresolved"));
        assert_eq!(json!(Code::LibraryMissing), json!("library_missing"));
        assert_eq!(
            json!(Code::LibraryCourtyardMissing),
            json!("library_courtyard_missing")
        );
        assert_eq!(json!(Code::ModelFileChanged), json!("model_file_changed"));
        assert_eq!(json!(Code::PadMappedTwice), json!("pad_mapped_twice"));
        assert_eq!(json!(Code::BlockCycle), json!("block_cycle"));
        assert_eq!(json!(Code::NetNameDuplicate), json!("net_name_duplicate"));
        assert_eq!(json!(Code::NcConnected), json!("nc_connected"));
        assert_eq!(json!(Code::WaiverUnused), json!("waiver_unused"));
        assert_eq!(json!(Code::KicadUnreadable), json!("kicad_unreadable"));
        assert_eq!(
            json!(Code::KicadLegacyUnsupported),
            json!("kicad_legacy_unsupported")
        );
        assert_eq!(
            json!(Code::ImportDemorganDropped),
            json!("import_demorgan_dropped")
        );
        assert_eq!(json!(Code::ImportUnresolved), json!("import_unresolved"));
        assert_eq!(Code::KicadUnreadable.exit_code(), 1);
        assert_eq!(Code::PinConflict.exit_code(), 1);
        assert_eq!(Code::LibraryNested.exit_code(), 1);
        assert_eq!(Code::PinWithoutPad.exit_code(), 1);
        assert_eq!(json!(Code::WatchUnavailable), json!("watch_unavailable"));
        assert_eq!(
            json!(Code::MemoryBudgetTooSmall),
            json!("memory_budget_too_small")
        );
        assert_eq!(json!(Code::ExportMeshFaults), json!("export_mesh_faults"));
        assert_eq!(json!(Code::ExportTooLarge), json!("export_too_large"));
        assert_eq!(json!(Code::NotABoard), json!("not_a_board"));
        assert_eq!(json!(Code::OdbUnreadable), json!("odb_unreadable"));
        assert_eq!(json!(Code::BoardInputChanged), json!("board_input_changed"));
        assert_eq!(
            json!(Code::OdbFontMissingChar),
            json!("odb_font_missing_char")
        );
        assert_eq!(json!(Code::OdbInfoMissing), json!("odb_info_missing"));
        assert_eq!(json!(Code::LayoutStale), json!("layout_stale"));
        assert_eq!(
            json!(Code::RefdesFormatInvalid),
            json!("refdes_format_invalid")
        );
        assert_eq!(
            json!(Code::SchLabelWhereWire),
            json!("sch_label_where_wire")
        );
        assert_eq!(Code::InstructionConflict.exit_code(), 1);
        assert_eq!(Code::ArchiveUnsafe.exit_code(), 1);
        assert_eq!(
            json!(Code::DependencyNotFetched),
            json!("dependency_not_fetched")
        );
        assert_eq!(
            json!(Code::InstanceForeignWorking),
            json!("instance_foreign_working")
        );
        assert_eq!(
            json!(Code::ProjectIdDuplicate),
            json!("project_id_duplicate")
        );
        assert_eq!(json!(Code::LockUnused), json!("lock_unused"));
        assert_eq!(Code::DependencyFetchFailed.exit_code(), 4);
        assert_eq!(Code::DependencyNotFetched.exit_code(), 1);
        assert_eq!(Code::DependencyRewritten.exit_code(), 1);
        assert_eq!(Code::BoardInputMissing.exit_code(), 1);
        assert_eq!(
            json!(Code::CacheRemoteUnreachable),
            json!("cache_remote_unreachable")
        );
        assert_eq!(
            json!(Code::CacheRemoteRefused),
            json!("cache_remote_refused")
        );
        assert_eq!(json!(Code::CacheEntryBad), json!("cache_entry_bad"));
        assert_eq!(Code::CacheRemoteUnreachable.exit_code(), 4);
        assert_eq!(Code::CacheRemoteRefused.exit_code(), 1);
        assert_eq!(json!(Code::WorktreeNotFound), json!("worktree_not_found"));
        assert_eq!(json!(Code::WorktreeDirty), json!("worktree_dirty"));
        assert_eq!(Code::WorktreeNotFound.exit_code(), 1);
        assert_eq!(Code::WorktreeDirty.exit_code(), 1);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(Code::InvalidParam.exit_code(), 1);
        assert_eq!(Code::ExportFailed.exit_code(), 1);
        assert_eq!(Code::ExportUnrepresentable.exit_code(), 1);
        assert_eq!(Code::MeasureFailed.exit_code(), 1);
        assert_eq!(Code::ViewNotFound.exit_code(), 1);
        assert_eq!(Code::AppearanceUnresolved.exit_code(), 1);
        assert_eq!(Code::ExplodeNotFound.exit_code(), 1);
        assert_eq!(Code::ExplodeUnresolved.exit_code(), 1);
        assert_eq!(Code::ExportMeshFaults.exit_code(), 1);
        assert_eq!(Code::ExportTooLarge.exit_code(), 1);
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
        assert_eq!(Code::ReviewUnauthenticated.exit_code(), 1);
        assert_eq!(Code::ReviewNoRemote.exit_code(), 1);
        assert_eq!(Code::ReviewHeadMoved.exit_code(), 1);
        assert_eq!(Code::ReviewUnreachable.exit_code(), 4);
        assert_eq!(Code::ReviewFailed.exit_code(), 4);
        assert_eq!(Code::ReviewGithubUnreachable.exit_code(), 4);
        assert_eq!(Code::ReviewMergeUnavailable.exit_code(), 1);
        assert_eq!(Code::ReviewNotInBeta.exit_code(), 1);
        assert_eq!(Code::ReviewInvalidRequest.exit_code(), 1);
        assert_eq!(
            json!(Code::ReviewGithubUnreachable),
            json!("review_github_unreachable")
        );
        assert_eq!(
            json!(Code::ReviewForkNotInstalled),
            json!("review_fork_not_installed")
        );
        assert_eq!(Code::TargetsMixed.exit_code(), 1);
        assert_eq!(Code::EdgeListedTwice.exit_code(), 1);
        assert_eq!(Code::BlendFailed.exit_code(), 1);
        assert_eq!(Code::WrongPartKind.exit_code(), 1);
        assert_eq!(json!(Code::AssemblyCycle), json!("assembly_cycle"));
        assert_eq!(Code::LabelNotOfficial.exit_code(), 1);
        assert_eq!(
            json!(Code::InstanceNotReleased),
            json!("instance_not_released")
        );
        assert_eq!(
            json!(Code::ConnectorUnsupported),
            json!("connector_unsupported")
        );
        assert_eq!(Code::AssemblyUnsolved.exit_code(), 1);
        assert_eq!(
            json!(Code::AssemblyOverconstrained),
            json!("assembly_overconstrained")
        );
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
