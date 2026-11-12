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
    /// A render larger than the limit, or with no pixels, or whose
    /// geometry would pass its share of the session's memory budget (a
    /// quarter of `session.max_memory`, #400).
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
    /// A reference whose face or edge a later feature split or merged, in
    /// a role that can't take what it became (design 447): `reason`
    /// (`one_needed`, or `carriers_differ` where the pieces don't all lie
    /// on the old surface or curve), `candidates`, `was`, `removed_by`.
    ReferenceSplit,
    /// A warning: a reference whose face or edge a later feature split or
    /// merged, followed to what it became: `role`, `reference`, `pieces`
    /// and `by`, the feature that split or merged it.
    ReferenceFollowed,
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
    /// A part inserts, through inserts and instances at any depth, itself
    /// (design 521). The details give `part`, `child` and the `path`.
    InsertCycle,
    /// An insert's source shows no solid: nothing to insert.
    InsertSourceEmpty,
    /// A warning: an insert's source part failed to rebuild, so it
    /// inserts each solid as the source shows it (`failing`: the source's
    /// failed features).
    InsertSourceFailed,
    /// A warning: an insert pinned to a label an obsolete record names.
    InsertObsolete,
    /// A warning: a newer official release of an insert's source exists
    /// than the label it is pinned to (`insert`, `rev`, `newest`).
    InsertBehind,
    /// An insert at `working` in a part being released, or read at a
    /// release label, or at a label that isn't official or is obsolete.
    InsertNotReleased,
    /// A feature names a solid a `delete_body` before it deleted
    /// (`solid`, `by`).
    BodyDeleted,
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
    /// A mate's driven angle or distance outside its own limits: refused
    /// when written, and a `check` finding when a parameter moved it
    /// there. The mate, the value and the bound it passed are in the
    /// details.
    MateOutOfLimits,
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
    /// An assembly whose products' shapes are in other files that weren't
    /// found (design 448, plan 24.D): every document that didn't resolve
    /// is in the details (`missing`: file, products, referenced_by,
    /// searched, reason `not_found` or `format`). Nothing was written.
    ImportExternalMissing,
    /// An external file that doesn't say which of its roots a referencing
    /// product is: several roots and none alone has its id or name, or no
    /// root that makes anything. The file, the referencing file, the
    /// product and the roots are in the details.
    ImportExternalAmbiguous,
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
    /// `branch.switch` while project files differ from `HEAD`, staged or
    /// not (design 447): the files are in the details (`files`). ryucad
    /// never stages, commits or stashes; the person or agent does, in Git.
    CheckoutDirty,
    /// `branch.switch` to a branch another worktree has checked out: the
    /// `branch` and that worktree's project folder (`worktree`) are in
    /// the details.
    BranchInWorktree,
    /// `branch.switch` while Git is in a merge, cherry-pick, revert or
    /// rebase step: the `step`, as `merge.status` names it.
    MergeInProgress,
    /// A Git command ryucad ran to write the working tree failed (`git
    /// switch`): what it ran (`git`) and what Git said (`stderr`) are in
    /// the details.
    GitFailed,
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
    /// The project has no ID yet (no PDM write has given it one), so
    /// nothing can depend on it or be forked from it.
    ProjectIdMissing,
    /// A long request stopped at a boundary by `request.cancel` or the
    /// CLI's Ctrl+C, with `done` and `total`.
    Cancelled,
    /// `approvals` in `ryupdm.json` that breaks its rules: a rule without
    /// parts, a count past its reviewers, a login GitHub wouldn't take, a
    /// team (teams come later), or a key whose ID isn't its hash.
    ApprovalRuleInvalid,
    /// A part parameter named as a project parameter is: refused when
    /// written, and when a file has one anyway (a merge, an edit by hand)
    /// it and what reads it fail with this, and doctor reports it.
    ParameterShadows,
    /// A warning: parameters nothing reads (design 447), once for the
    /// project's (`scope: project`) and once per part (`part`), with
    /// `parameters: [{name, read_by, error?}]`: `read_by` the unused
    /// parameters that name it, `error` where it doesn't evaluate either.
    ParameterUnused,
    /// A datum its references can't define: collinear points, parallel
    /// planes for an axis, a point on the axis it would turn about, an
    /// axis not parallel to the plane it turns, an axis with no length.
    DatumDegenerate,
    /// A datum point where what it intersects doesn't meet: the two
    /// references are in the details.
    DatumNoIntersection,
    /// A datum point where what it intersects meets more than once: how
    /// often is `count` in the details.
    DatumAmbiguous,
    /// An imported body that would take more than `heal.budget` work
    /// units to heal: it fails alone (the import goes on with the
    /// others), with its STEP instances and the units spent in the
    /// details. Raise `heal.budget` to heal it.
    ImportBudgetExceeded,
    /// A path whose segments don't meet end to end: the two segments and
    /// the gap between their nearest ends are in the details.
    PathDisconnected,
    /// A path whose segments meet at an angle: the two segments and the
    /// angle are in the details (sweeps need tangent joints).
    PathNotTangent,
    /// A path naming one curve twice, directly or through a tangent
    /// chain.
    PathSegmentTwice,
    /// A path's propagated edge whose tangent chain forks.
    PathBranches,
    /// A path the kernel's path checks refuse otherwise: a segment too
    /// short, a whole closed curve among others.
    PathInvalid,
    /// A helix its numbers can't make: a radius that reaches zero, a
    /// pitch or height that isn't positive, too many turns; the field is
    /// in the details.
    HelixInvalid,
    /// ryueda: two sides of a merge changed one area of a block's layout
    /// differently; each conflict's sheets, areas and keys are in the
    /// details, and its repairs (ours, theirs, laid out again).
    LayoutConflict,
    /// ryueda lint: a supply port not pointing up, or ground not pointing
    /// down.
    SchPowerSideways,
    SchGroundSideways,
    /// ryueda lint: a net label or a port reading up or down the page.
    SchLabelVertical,
    /// ryueda lint: a wire stepping aside and going on the way it went.
    SchJog,
    /// ryueda lint: a bypass capacitor not wired on the line from the pin
    /// it serves.
    SchBypassApart,
    /// ryueda lint: a pin's name running past its part's body, or into
    /// another pin's name.
    SchPinNameOutside,
    /// ryueda lint: a port's flag whose wire turns on its way to the pin
    /// it meets.
    SchPortBent,
    /// ryueda lint: a part joined to nothing by a wire, its nets on its
    /// sheet all labels, though one of them has another pin there.
    SchFloating,
    /// ryueda lint: a resistor drawn in the style the project doesn't
    /// prefer (`prefer.resistor` in `lint.json`).
    SchResistorStyle,
    /// ryueda lint: a part in line with no other part on its sheet.
    SchOutOfLine,
    /// ryueda lint: like parts sharing their nets not in one row or column.
    SchBankNotInRow,
    /// ryueda lint: a recognised circuit (a buck converter, an op-amp
    /// stage) drawn out of its standard shape.
    SchTopology,
    /// ryueda lint: a two-pin part standing with its ground end up or its
    /// supply end down.
    SchPartFlipped,
    /// ryueda lint: a wire running round its own part's body to reach it.
    SchWireWraps,
    /// A sweep along a path the kernel refuses: `reason` (its refusal in
    /// snake case: `profile_along_path`, `path_too_curved`,
    /// `frame_undefined`, `not_closing`, `bad_scale`,
    /// `self_intersecting`, `uncertain`), and what it names in ryucad's
    /// terms (`path_segment`, `faces`, `entities`) are in the details.
    SweepFailed,
    /// A loft the kernel refuses, or one whose sections ryucad can't
    /// match: `reason` (`too_few_sections`, `loop_mismatch`,
    /// `segment_count_mismatch`, `bad_section`, `bad_tangent`,
    /// `bad_weights`, `self_intersecting`, `uncertain`), the `section` it
    /// names and what it names in ryucad's terms (`entities`, `faces`)
    /// are in the details.
    LoftFailed,
    /// The kernel refused a shell: a thickness it can't take, a face whose
    /// offset folds, an edge that would vanish, a corner it can't build
    /// yet, or the shell's boolean. Its `reason`, which call refused
    /// (`kernel`: `offset` or `boolean`) and the faces, edges and vertices
    /// it names (`entities`) are in the details.
    ShellFailed,
    /// The kernel refused an offset; its details as `shell_failed`'s.
    OffsetFailed,
    /// The kernel refused a mirror's copy: the reflection leaves the
    /// ±500 m size box. Its `reason` (`outside_size_box`) and the solid
    /// mirrored (`solid`) are in the details. A combined copy's boolean
    /// refuses as `boolean_failed`.
    MirrorFailed,
    /// The kernel refused a face tweak (`move_face`, `replace_face`, and
    /// from ryucad 17.B `draft`): its `reason` (the refusal's variant in
    /// snake case, an unsupported one's `case`), `kernel` (`tweak`), the
    /// kernel's text (`refusal`), the faces, edges and vertices it names
    /// (`entities`, each with its `role`) and, where there is a known way
    /// out, a `hint` are in the details.
    TweakFailed,
    /// A direct edit (ryucad 22: a `delete_face`, or a face tweak with
    /// `topology: change` or `blends: rebuild`) didn't finish within the
    /// `edit.budget` preference's work units: the `feature`, what it had
    /// `spent` and the `budget` are in the details, with a `hint`. Units
    /// are the same on every machine, so it is kept as any refusal is.
    EditBudgetExceeded,
    /// ryueda: an instruction naming a region (`region:<id>`) the layout
    /// no longer has, kept until released (a warning).
    RegionUnresolved,
    /// ryueda: paper fit grew a sheet one size to hold an edit; the sheet
    /// and both sizes are in the details (a warning).
    SheetResized,
    /// ryueda lint: a sheet on paper larger than what holds it, or too
    /// empty for its paper.
    SchPaperFit,
    /// ryueda lint: one function's parts (a recognised circuit, an IC's
    /// satellites, a connector's breakout) in more than one region.
    SchFunctionSplit,
    /// ryueda lint: a region with no title of its own (`Misc`).
    SchRegionUntitled,
    /// A feature was given a sheet where it needs a solid, or a solid
    /// where it needs a sheet (ryucad 18). The details name the
    /// `feature`, the `role` it was given in (`targets`, `tools`,
    /// `solid`, `target`), the body (`solid`, by founder), what it
    /// `wanted` and what it `got` (`solid`, `sheet` or `mixed`).
    WrongBodyKind,
    /// ryueda: a library component's parameter in the part vocabulary
    /// whose value doesn't read as its name's type (`capacitance` that
    /// isn't a capacitance): a warning on the component, with its pointer,
    /// and in ERC once per placed library component. In a search
    /// condition the value is `invalid_quantity` instead.
    ParameterInvalid,
    /// ryueda ERC: a placed component whose library component has no
    /// maker part number (a warning).
    PartUnidentified,
    /// ryueda ERC: a maker part number with no supplier offer, on a
    /// library component's first choice or an alternate (a warning).
    PartUnsourced,
    /// ryueda: a source of part data didn't answer (a connection refused,
    /// a call quiet for 60 s, a server error). Not the agent's fault.
    PartsSourceUnreachable,
    /// ryueda: a source answered what ryueda can't take: an answer shape
    /// it doesn't read (`field` names what), a format it doesn't know (a
    /// catalogue's `version`), or refused keys.
    PartsSourceRefused,
    /// ryueda: no source has the part asked for (`parts show`,
    /// `libcomp create --from`).
    PartsNotFound,
    /// ryueda: a catalogue this machine holds is older than
    /// `parts.max_age`, or missing (`reason`): a warning.
    PartsCacheStale,
    /// ryueda supply: a supplier's record of an offer names another maker
    /// part number than the library component (a warning).
    PartsMismatch,
    /// ryueda supply: fewer in stock than the build takes, at every
    /// offer's source with data (a warning).
    PartOutOfStock,
    /// ryueda supply: an assembly house's Extended part, with that house
    /// the project's assembly house (a warning).
    PartExtended,
    /// ryueda supply: no source holds a record of any of a placed part's
    /// offers (a warning).
    PartNoData,
    /// ryueda: a live source's rate limit is spent (`limit`: `minute` or
    /// `day` as ryueda counts them, `source` where the source said so);
    /// the details say when to try again (`retry_after_s`, `retry_at`).
    /// Not the agent's fault.
    PartsRateLimited,
    /// ryueda supply: a placed part a source says is not recommended for
    /// new designs, at last time buy, or obsolete (a warning).
    PartLifecycle,
    /// The kernel refused a sheet metal feature: a body that isn't sheet
    /// metal (recognition's refusal), or an edge flange it can't make.
    /// Its `reason` (the kernel's refusal in snake case), the kernel's
    /// text (`kernel`), the faces, edges, vertices and bends it names
    /// (`entities`) and, where an agent can fix it, a `hint` are in the
    /// details. A flange's boolean refuses as `boolean_failed`.
    SheetMetalFailed,
    /// The kernel refused a fill (ryucad 18): its `reason` (`too_few_sides`,
    /// `not_laminar`, `not_closed` with `gap_m`, `degenerate`,
    /// `angle_too_large` with `angle_deg`, `uncertain`), `kernel` (its
    /// text) and the side it names (`side`: its index in the file,
    /// `piece` within a curve of several, and `source`, as text).
    FillFailed,
    /// The kernel refused an extract: `reason` `no_faces` or
    /// `non_manifold_vertex` (with the vertex's reference).
    ExtractFailed,
    /// A sew left several pieces (`reason: pieces`, each piece's sheets and
    /// laminar edges), left a body open where a solid was asked for
    /// (`reason: open`, its laminar edges), or the kernel refused it
    /// (`reason` the healing refusal in snake case).
    SewFailed,
    /// The kernel refused a thicken: `reason` (the `OffsetError` variant
    /// in snake case, an unsupported one's `case`), `kernel`, `refusal`
    /// and `entities`, as `offset_failed` gives them.
    ThickenFailed,
    /// No annotation by that ID in `annotations/`; the details name the
    /// ID asked for.
    AnnotationNotFound,
    /// An annotation's proposed change no longer applies: a file it
    /// touches isn't the bytes it saw. The details list each file
    /// (`files`: `{path, base, now}`, BLAKE3s or null). Also a warning of
    /// any answer read from a stale `annotation:<id>` revision.
    AnnotationStale,
    /// An annotation someone else has claimed (or, for `edit` and
    /// `delete`, that its author can no longer change): the details name
    /// who (`claimed_by`) and the annotation.
    AnnotationClaimed,
    /// A human participant's request its mode doesn't allow: writing the
    /// model outside Direct edit, or annotating in View. The details give
    /// the participant, its `mode` and the mode the request `needs`.
    ParticipantMode,
    /// A feature that would change a reference import's body (a join,
    /// cut or intersect targeting it, a boolean, fillet, chamfer, shell,
    /// offset, split, hole, pattern or mirror acting on it): references
    /// are shown and read, never modelled on. The details carry `feature`,
    /// `solid`, `role` and `hint` (`ryucad import convert <part>/<feature>`).
    ReferenceBody,
    /// Mass of a reference whose shells don't close (laminar edges), or
    /// whose mesh doesn't (faces the mesher left out).
    ReferenceNotSolid,
    /// A warning: faces of a reference the mesher left out (they draw as
    /// holes), by entity reference and STEP instance.
    ReferenceFacesUnmeshed,
    /// A warning: faces of a file a reference couldn't lay out even alone,
    /// left out, by import name and STEP instance.
    ReferenceFacesDropped,
    /// A warning: faces of a piece the layout refused, each laid out as a
    /// lone sheet.
    ReferenceFacesSplit,
    /// A warning: faces whose loops or orientation couldn't be settled,
    /// kept as the file has them.
    ReferenceUnoriented,
    /// A warning: a USD export wrote a reference as meshes alone (a
    /// reference has no BrepArray).
    ExportReferenceMeshOnly,
    /// A warning: a parting-line draft's face whose certified angle from
    /// its pull lies more than a millidegree from the angle asked, or
    /// spreads wider than that (a parting line climbing along the pull
    /// holds the angle in the section square to it instead). The face,
    /// both angles and the angle asked are in the details.
    DraftAngleVaries,
    /// A warning: a USD export wrote a sheet as its mesh alone (ryucad
    /// 18.C writes no BrepArray of a sheet).
    ExportSheetBrepSkipped,
    /// No drawing of that ID in the part (design 417); the details name
    /// the part and the drawings it has (`drawings`).
    DrawingNotFound,
    /// A view the kernel couldn't project (`ViewError::Uncertain` at a
    /// face, an edge whose image couldn't be found): its `reason`, the
    /// kernel's text (`kernel`) and the faces it names as references
    /// (`faces`). The sheet still builds, the view's frame crossed out.
    DrawingViewFailed,
    /// A warning: pieces of a view whose visibility couldn't be certified,
    /// drawn visible; the details count them (`pieces`) and name their
    /// edges and faces (`sources`).
    DrawingViewUncertain,
    /// A drawing's reference whose entity is gone: `role`, `reference`,
    /// and `was`, `removed_by` and `candidates` as references give them.
    DrawingReferenceUnresolved,
    /// A drawing's reference whose entity is no longer in its view
    /// (hidden, or behind a section's cut).
    DrawingReferenceNotShown,
    /// A sheet's template that is neither built in nor in `templates/`;
    /// the details name it (`template`).
    TemplateNotFound,
    /// `packages.formats` in `ryupdm.json` naming a format a package can't
    /// hold (design 436): the name and its pointer are in the details.
    PackagesFormatUnknown,
    /// A release package that couldn't be built whole: each file that
    /// failed, with its own error, is in `failures`. Never written in part.
    PackageFailed,
    /// A package that doesn't verify: its `clause` (stable name) is in the
    /// details.
    PackageInvalid,
    /// A package of an obsolete release: obsolete releases have none.
    ReleaseObsolete,
    /// ryueda lint: an off-sheet connector on a net that stays on its
    /// sheet, or a region of a net that leaves with none (plan 2.C).
    SchOffSheetLocal,
    /// ryueda lint: four or more members of a bus between the same parts'
    /// sides drawn apart (plan 2.C).
    SchBusUnbundled,
    /// ryueda: a symbol standards file (`library/standards.json`) whose
    /// values its checks refuse; the file's `path` and a JSON `pointer`
    /// are in the details (plan 2.D).
    StandardsInvalid,
    /// ryueda symbol lint: a pin's connection point off the standards'
    /// grid.
    SymPinOffGrid,
    /// ryueda symbol lint: a pin on a side its class's standards don't
    /// put its role on (a power pin off the top, an output on the left).
    SymPinSide,
    /// ryueda symbol lint: a function's pins (I2C, SPI, GPIO…) split up,
    /// or a numbered group out of order.
    SymPinGroup,
    /// ryueda symbol lint: a pin longer or shorter than the standards'.
    SymPinLength,
    /// ryueda symbol lint: a resistor drawn to the other standard (the US
    /// zigzag or the IEC box) than its library's (plan 2.D; was
    /// `sch_resistor_style`).
    SymShape,
    /// ryueda symbol lint: a pin's name nearer its body's edge, or
    /// another name, than the standards' margin.
    SymBodyMargin,
    /// ryueda symbol lint: a unit with more pins than its class draws in
    /// one unit.
    SymUnits,
    /// ryueda: a style file (`style.json`, a project's or an
    /// organisation's) whose values its checks refuse; its `path` and a
    /// JSON `pointer` are in the details (plan 2.E).
    StyleInvalid,
    /// ryueda: a write, or a project's style or lint file, setting what
    /// the organisation's style locks; the `lock` and the organisation's
    /// `path` are in the details.
    StyleLocked,
    /// ryueda style lint: a colour under 3:1 against the paper, a body or
    /// a port's flag it is drawn on, in either theme.
    SchContrast,
    /// ryueda style lint: an override naming an object that is gone.
    SchOverrideUnused,
    /// ryueda: two components locked with one designator, which
    /// `annotate` can't number round; both are in the details (plan 2.F).
    AnnotateConflict,
    /// The project's `.gitattributes` lacks a product's lines (ryueda's
    /// doctor; ryucad's doctor says it by name).
    GitattributesMissing,
    /// Git's config lacks a product's driver entries, or follows renames.
    GitDriversMissing,
    /// A project file not written canonically (edited by hand?).
    NonCanonical,
    /// A file in a project's folders that no file kind holds.
    UnknownFile,
    /// A file under `library/models/` that no footprint names.
    ModelOrphaned,
    /// A project file Git left unmerged in the index.
    UnmergedPath,
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
            | Code::DependencyFetchFailed
            | Code::PartsSourceUnreachable
            | Code::PartsRateLimited
            | Code::Cancelled => 4,
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
    fn external_reference_codes() {
        for (c, name) in [
            (Code::ImportExternalMissing, "import_external_missing"),
            (Code::ImportExternalAmbiguous, "import_external_ambiguous"),
        ] {
            assert_eq!(json!(c), json!(name));
            assert_eq!(c.exit_code(), 1, "{name}");
        }
    }

    #[test]
    fn part_data_codes() {
        assert_eq!(json!(Code::ParameterInvalid), json!("parameter_invalid"));
        assert_eq!(json!(Code::PartUnidentified), json!("part_unidentified"));
        assert_eq!(json!(Code::PartUnsourced), json!("part_unsourced"));
        assert_eq!(Code::ParameterInvalid.exit_code(), 1);
        assert_eq!(Code::PartUnsourced.exit_code(), 1);
        for (code, name, exit) in [
            (Code::PartsSourceUnreachable, "parts_source_unreachable", 4),
            (Code::PartsSourceRefused, "parts_source_refused", 1),
            (Code::PartsNotFound, "parts_not_found", 1),
            (Code::PartsCacheStale, "parts_cache_stale", 1),
            (Code::PartsMismatch, "parts_mismatch", 1),
            (Code::PartOutOfStock, "part_out_of_stock", 1),
            (Code::PartExtended, "part_extended", 1),
            (Code::PartNoData, "part_no_data", 1),
            (Code::PartsRateLimited, "parts_rate_limited", 4),
            (Code::PartLifecycle, "part_lifecycle", 1),
        ] {
            assert_eq!(json!(code), json!(name));
            assert_eq!(code.exit_code(), exit, "{name}");
        }
    }

    #[test]
    fn annotation_codes_are_the_agents_to_fix() {
        assert_eq!(
            json!(Code::AnnotationNotFound),
            json!("annotation_not_found")
        );
        assert_eq!(json!(Code::AnnotationStale), json!("annotation_stale"));
        assert_eq!(json!(Code::AnnotationClaimed), json!("annotation_claimed"));
        assert_eq!(json!(Code::ParticipantMode), json!("participant_mode"));
        for c in [
            Code::AnnotationNotFound,
            Code::AnnotationStale,
            Code::AnnotationClaimed,
            Code::ParticipantMode,
        ] {
            assert_eq!(c.exit_code(), 1, "{c:?}");
        }
    }

    #[test]
    fn drawing_codes_are_the_agents_to_fix() {
        for (c, name) in [
            (Code::DrawingNotFound, "drawing_not_found"),
            (Code::DrawingViewFailed, "drawing_view_failed"),
            (Code::DrawingViewUncertain, "drawing_view_uncertain"),
            (
                Code::DrawingReferenceUnresolved,
                "drawing_reference_unresolved",
            ),
            (
                Code::DrawingReferenceNotShown,
                "drawing_reference_not_shown",
            ),
            (Code::TemplateNotFound, "template_not_found"),
        ] {
            assert_eq!(json!(c), json!(name));
            assert_eq!(c.exit_code(), 1, "{name}");
        }
    }

    #[test]
    fn package_codes_are_the_agents_to_fix() {
        for (c, name) in [
            (Code::PackagesFormatUnknown, "packages_format_unknown"),
            (Code::PackageFailed, "package_failed"),
            (Code::PackageInvalid, "package_invalid"),
            (Code::ReleaseObsolete, "release_obsolete"),
        ] {
            assert_eq!(json!(c), json!(name));
            assert_eq!(c.exit_code(), 1, "{name}");
        }
    }

    #[test]
    fn branch_codes_are_the_agents_to_fix() {
        for (c, name) in [
            (Code::CheckoutDirty, "checkout_dirty"),
            (Code::BranchInWorktree, "branch_in_worktree"),
            (Code::MergeInProgress, "merge_in_progress"),
            (Code::GitFailed, "git_failed"),
        ] {
            assert_eq!(json!(c), json!(name));
            assert_eq!(c.exit_code(), 1, "{name}");
        }
    }

    #[test]
    fn insert_codes() {
        for (c, name) in [
            (Code::InsertCycle, "insert_cycle"),
            (Code::InsertSourceEmpty, "insert_source_empty"),
            (Code::InsertSourceFailed, "insert_source_failed"),
            (Code::InsertObsolete, "insert_obsolete"),
            (Code::InsertBehind, "insert_behind"),
            (Code::InsertNotReleased, "insert_not_released"),
            (Code::BodyDeleted, "body_deleted"),
        ] {
            assert_eq!(json!(c), json!(name));
            assert_eq!(c.exit_code(), 1, "{name}");
        }
    }

    #[test]
    fn reference_codes() {
        for (c, name) in [
            (Code::ReferenceSplit, "reference_split"),
            (Code::ReferenceFollowed, "reference_followed"),
            (Code::ReferenceBody, "reference_body"),
            (Code::ReferenceNotSolid, "reference_not_solid"),
            (Code::ReferenceFacesUnmeshed, "reference_faces_unmeshed"),
            (Code::ReferenceFacesDropped, "reference_faces_dropped"),
            (Code::ReferenceFacesSplit, "reference_faces_split"),
            (Code::ReferenceUnoriented, "reference_unoriented"),
            (Code::ExportReferenceMeshOnly, "export_reference_mesh_only"),
            (Code::ExportSheetBrepSkipped, "export_sheet_brep_skipped"),
        ] {
            assert_eq!(json!(c), json!(name));
            assert_eq!(c.exit_code(), 1, "{name}");
        }
    }

    #[test]
    fn shell_and_offset_codes() {
        assert_eq!(json!(Code::ShellFailed), json!("shell_failed"));
        assert_eq!(json!(Code::OffsetFailed), json!("offset_failed"));
        assert_eq!(Code::ShellFailed.exit_code(), 1);
        assert_eq!(Code::OffsetFailed.exit_code(), 1);
    }

    #[test]
    fn draft_angle_varies_code() {
        assert_eq!(json!(Code::DraftAngleVaries), json!("draft_angle_varies"));
        assert_eq!(Code::DraftAngleVaries.exit_code(), 1);
    }

    #[test]
    fn edit_budget_code() {
        assert_eq!(
            json!(Code::EditBudgetExceeded),
            json!("edit_budget_exceeded")
        );
        assert_eq!(Code::EditBudgetExceeded.exit_code(), 1);
    }

    #[test]
    fn mirror_code() {
        assert_eq!(json!(Code::MirrorFailed), json!("mirror_failed"));
        assert_eq!(Code::MirrorFailed.exit_code(), 1);
        assert_eq!(json!(Code::TweakFailed), json!("tweak_failed"));
        assert_eq!(Code::TweakFailed.exit_code(), 1);
    }

    #[test]
    fn wrong_body_kind_code() {
        assert_eq!(json!(Code::WrongBodyKind), json!("wrong_body_kind"));
        assert_eq!(Code::WrongBodyKind.exit_code(), 1);
    }

    #[test]
    fn sheet_metal_code() {
        assert_eq!(json!(Code::SheetMetalFailed), json!("sheet_metal_failed"));
        assert_eq!(Code::SheetMetalFailed.exit_code(), 1);
    }

    #[test]
    fn surface_codes() {
        for (code, name) in [
            (Code::FillFailed, "fill_failed"),
            (Code::ExtractFailed, "extract_failed"),
            (Code::SewFailed, "sew_failed"),
            (Code::ThickenFailed, "thicken_failed"),
        ] {
            assert_eq!(json!(code), json!(name));
            assert_eq!(code.exit_code(), 1, "{name}");
        }
    }

    #[test]
    fn a_budget_exceeded_is_the_agents_to_fix() {
        assert_eq!(Code::ImportBudgetExceeded.exit_code(), 1);
        assert_eq!(
            serde_json::to_value(Code::ImportBudgetExceeded).unwrap(),
            "import_budget_exceeded"
        );
    }

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
        assert_eq!(json!(Code::RegionUnresolved), json!("region_unresolved"));
        assert_eq!(json!(Code::SheetResized), json!("sheet_resized"));
        assert_eq!(json!(Code::SchPaperFit), json!("sch_paper_fit"));
        assert_eq!(json!(Code::SchOffSheetLocal), json!("sch_off_sheet_local"));
        assert_eq!(json!(Code::SchBusUnbundled), json!("sch_bus_unbundled"));
        assert_eq!(json!(Code::StandardsInvalid), json!("standards_invalid"));
        assert_eq!(json!(Code::SymPinOffGrid), json!("sym_pin_off_grid"));
        assert_eq!(json!(Code::SymPinSide), json!("sym_pin_side"));
        assert_eq!(json!(Code::SymPinGroup), json!("sym_pin_group"));
        assert_eq!(json!(Code::SymPinLength), json!("sym_pin_length"));
        assert_eq!(json!(Code::SymShape), json!("sym_shape"));
        assert_eq!(json!(Code::SymBodyMargin), json!("sym_body_margin"));
        assert_eq!(json!(Code::SymUnits), json!("sym_units"));
        assert_eq!(json!(Code::StyleInvalid), json!("style_invalid"));
        assert_eq!(json!(Code::StyleLocked), json!("style_locked"));
        assert_eq!(json!(Code::SchContrast), json!("sch_contrast"));
        assert_eq!(json!(Code::SchOverrideUnused), json!("sch_override_unused"));
        assert_eq!(json!(Code::AnnotateConflict), json!("annotate_conflict"));
        assert_eq!(Code::AnnotateConflict.exit_code(), 1);
        for c in [Code::StyleInvalid, Code::StyleLocked, Code::SchContrast] {
            assert_eq!(c.exit_code(), 1);
        }
        for c in [Code::StandardsInvalid, Code::SymShape, Code::SymUnits] {
            assert_eq!(c.exit_code(), 1);
        }
        assert_eq!(json!(Code::SchFunctionSplit), json!("sch_function_split"));
        assert_eq!(json!(Code::SchRegionUntitled), json!("sch_region_untitled"));
        assert_eq!(json!(Code::LayoutConflict), json!("layout_conflict"));
        assert_eq!(json!(Code::SchBypassApart), json!("sch_bypass_apart"));
        assert_eq!(json!(Code::SchJog), json!("sch_jog"));
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
            json!("project_id_duplicate"),
        );
        assert_eq!(json!(Code::ProjectIdMissing), json!("project_id_missing"));
        assert_eq!(json!(Code::LockUnused), json!("lock_unused"));
        assert_eq!(
            json!(Code::ApprovalRuleInvalid),
            json!("approval_rule_invalid")
        );
        assert_eq!(Code::ApprovalRuleInvalid.exit_code(), 1);
        assert_eq!(json!(Code::ParameterShadows), json!("parameter_shadows"));
        assert_eq!(Code::ParameterShadows.exit_code(), 1);
        assert_eq!(json!(Code::ParameterUnused), json!("parameter_unused"));
        assert_eq!(Code::ParameterUnused.exit_code(), 1);
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
        assert_eq!(json!(Code::DatumDegenerate), json!("datum_degenerate"));
        assert_eq!(
            json!(Code::DatumNoIntersection),
            json!("datum_no_intersection")
        );
        assert_eq!(json!(Code::DatumAmbiguous), json!("datum_ambiguous"));
        assert_eq!(Code::DatumAmbiguous.exit_code(), 1);
        assert_eq!(json!(Code::PathDisconnected), json!("path_disconnected"));
        assert_eq!(json!(Code::PathNotTangent), json!("path_not_tangent"));
        assert_eq!(json!(Code::PathSegmentTwice), json!("path_segment_twice"));
        assert_eq!(json!(Code::PathBranches), json!("path_branches"));
        assert_eq!(json!(Code::PathInvalid), json!("path_invalid"));
        assert_eq!(json!(Code::HelixInvalid), json!("helix_invalid"));
        assert_eq!(Code::PathNotTangent.exit_code(), 1);
        assert_eq!(Code::HelixInvalid.exit_code(), 1);
        assert_eq!(json!(Code::SweepFailed), json!("sweep_failed"));
        assert_eq!(Code::SweepFailed.exit_code(), 1);
        assert_eq!(json!(Code::LoftFailed), json!("loft_failed"));
        assert_eq!(Code::LoftFailed.exit_code(), 1);
    }

    #[test]
    fn eda_doctor_codes() {
        for (c, s) in [
            (Code::GitattributesMissing, "gitattributes_missing"),
            (Code::GitDriversMissing, "git_drivers_missing"),
            (Code::NonCanonical, "non_canonical"),
            (Code::UnknownFile, "unknown_file"),
            (Code::ModelOrphaned, "model_orphaned"),
            (Code::UnmergedPath, "unmerged_path"),
        ] {
            assert_eq!(json!(c), json!(s));
            assert_eq!(c.exit_code(), 1);
        }
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
        assert_eq!(json!(Code::MateOutOfLimits), json!("mate_out_of_limits"));
        assert_eq!(Code::MateOutOfLimits.exit_code(), 1);
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
