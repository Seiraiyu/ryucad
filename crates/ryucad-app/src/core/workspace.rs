//! An open project: the link to its daemon and what the window knows, kept
//! up to date from answers and notifications.

use crate::core::activity::{Activity, LOADED};
use crate::core::bom::BomTab;
use crate::core::branches::Branches;
use crate::core::compare::Compare;
use crate::core::deps::{Deps, Picker, Update};
use crate::core::edit::{DimForm, Form};
use crate::core::equations::Equations;
use crate::core::interference::Interference;
use crate::core::link::{Event, Link, Wake};
use crate::core::measure::{Clearance, Mass, Tool};
use crate::core::merge::Merge;
use crate::core::model::{BodyKey, Listed, Model, Part};
use crate::core::params::Params;
use crate::core::pdm::States;
use crate::core::people::{People, Person};
use crate::core::progress::Progress;
use crate::core::props::PartProps;
use crate::core::releases::ReleaseTab;
use crate::core::settings::Settings;
use crate::core::toast::Toasts;
use ryucad::client::{Ask, Daemon};
use ryucad::error::{Code, Error, Result};
use ryucad::live::SelectMode;
use ryucad::proto::{Answer, Request, Transfer};
use ryucad::view::{Camera, CameraArg, Projection, ProjectionArg};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Projects of at most this many parts load whole when the window opens;
/// a larger one loads what the window shows, and each part as its row in
/// the tree opens or something in it is chosen.
pub const LOAD_WHOLE: usize = 200;

/// How many of an assembly's bodies one `mesh.get` asks for.
const MESH_BATCH: usize = 256;

/// What a request was for, so its answer lands in the right place.
#[derive(Clone, Debug, PartialEq)]
enum Pending {
    Info,
    /// This window's own `participant.mode`; the mode it was in before.
    Mode(crate::core::modes::Mode),
    /// A model edit sent as a proposal in Annotate: what its own answer
    /// was for.
    Proposal(Box<Pending>, bool),
    /// `annotation.list`.
    Annotations,
    /// An annotation made, answered or closed from the window.
    AnnotationWrite,
    Parts,
    /// A part's report, asked for at this revision (the draft, or the
    /// working files).
    Rebuild(String, Option<String>),
    /// A body's mesh, asked for at this hash.
    Mesh(BodyKey, String),
    /// Several bodies of one assembly's occurrences, each at its hash.
    Meshes(Vec<(BodyKey, String)>),
    /// A body's mesh at another level of detail.
    Level(String, ryucad::mesh::Quality),
    /// A sketch's drawing, asked for at this part revision.
    Sketch(BodyKey, String),
    /// A path's or helix's drawing, asked for at this part revision.
    Path(BodyKey, String),
    /// The chosen sweep's or loft's drawing, asked for at this part
    /// revision.
    Sweep(BodyKey, String),
    People,
    /// This window's own `visibility.set`.
    Hidden,
    /// A feature read for the form: its part and ID; after a conflict,
    /// with the form as it was when the save failed.
    Form(String, String, Option<Box<Form>>),
    Save,
    /// A sketch read for a dimension's form: its part, sketch and
    /// constraint; after a conflict, with the form as it was.
    Dim(String, String, String, Option<Box<DimForm>>),
    DimSave,
    /// A part's parameters for the panel; after a conflict, with the
    /// panel as it was.
    Params(String, Option<Box<Params>>),
    ParamSave,
    /// The Equations tile's table (`equations.list`), and whether a
    /// write conflicted. Read again over the table as it is when the
    /// answer comes, never as it was when it was asked for: what was
    /// typed meanwhile stays.
    Equations(bool),
    /// A row of the Equations tile written.
    EquationSave,
    /// The Drawings tile's list of this part's drawings.
    DrawingList(String),
    /// Every part's drawings, for the tree.
    DrawingsAll,
    /// The Drawings tile's sheet: part, drawing and sheet asked for.
    DrawingSheet(String, String, String),
    /// A drag or form of the Drawings tile written.
    DrawingWrite,
    Activity,
    History,
    /// The Branches tab's worktrees (`worktree.list`).
    Branches,
    /// A merge preview with the worktree at this path, made from this key.
    Preview(String, String),
    /// `deps.list` and `deps.outdated`, for the tab and the marks.
    DepsList,
    DepsOutdated,
    /// A fetch, an add, an update or a removal from the tab.
    DepsWrite,
    /// An update's dry run, for the names and part it was asked for.
    DepsDry(Vec<String>, Option<String>),
    /// The picker's `deps.parts`, for this dependency.
    DepsParts(String),
    /// The picker's `feature.add`.
    DepsPlace,
    /// A foreign instance's form's labels (`deps.parts`), for this
    /// assembly and instance.
    FormLabels(String, String),
    Config,
    /// The diff of a compare, the `n`th asked for.
    Compare(u64),
    /// A ghost's mesh, asked for at this hash.
    Ghost(BodyKey, String),
    /// The model's solids, read for the faces a feature made, at this
    /// part revision.
    Made(BodyKey, String),
    /// Every release record and part's release state (`release.list`).
    States,
    /// The release checks' findings, for the Releases tab.
    ReleaseCheck,
    /// A proposal's dry run, for `part`'s Propose dialog.
    ProposeDry(String),
    /// A proposal, an obsolete record or a tag sync, from the Releases tab.
    ReleaseWrite,
    /// ryupdm's settings, and whether the project has an ID yet.
    PdmConfig,
    /// The BOM tab's indented BOM, or (`flat`) its flat one.
    Bom {
        flat: bool,
    },
    /// An item number set from the BOM tab (`bom.renumber`).
    BomRenumber(crate::core::bom::ItemEdit),
    /// The BOM's CSV, to write at this path.
    BomExport(PathBuf),
    /// Where a part is used, for the Part section.
    WhereUsed(String),
    /// The BOM tab's `bom.diff` from the compared revision.
    BomDiff,
    /// A part's properties for the Part section; after a conflict, with
    /// the form as it was.
    Props(String, Option<Box<PartProps>>),
    PropsSave,
    /// The merge Git is in, if any.
    Merge,
    Resolve,
    Doctor,
    /// An assembly's interference (`interference`).
    Interference(String),
    /// The Measure tool's picks (`measure`).
    Measure(Vec<String>),
    /// What the Mass properties panel weighs (`mass`).
    Mass(String),
    /// An assembly's clearances (`clearance`).
    Clearance(String),
    /// What the disk cache holds (`cache.stats` or `cache.clear`).
    Cache,
    /// The shared cache's token stored or removed (`cache.login`,
    /// `cache.logout`).
    CacheToken,
    /// An import's dry run, for the file at this path, as solids or not.
    ImportPlan((String, bool)),
    /// An import or update of the file at this path, written.
    Import((String, bool)),
    /// An export, which the daemon writes where the dialog says.
    Export,
    /// A drag's preview solve (`assembly.solve`), for this assembly.
    DragSolve(String),
    /// A drag's move, written (`assembly.move`).
    DragMove(String),
    /// The Move tool's preview of an assembly, and its write.
    MoveSolve(String),
    MoveWrite(String),
    /// The Mate dialog's preview of an assembly, and its add.
    MatePreview(String, u64),
    MateAdd(String),
    /// The Markup bar's Post (`annotation.create` or `annotation.edit`).
    MarkupPost,
    /// A part's named views (`view.list`).
    Views(String),
    /// A named view of a part, saved (`view.save`).
    ViewSaved(String),
    /// A camera normal to a face (`camera.set` with `normal_to`).
    NormalTo,
    /// How the shown bodies look (`appearance.get`), the reading's number.
    Appearance(u64),
    /// An appearance written (`appearance.set` or `appearance.clear`).
    AppearanceWritten,
    /// An import's mode set (`import.convert`): its part and feature.
    ImportConvert(String, String),
    /// This window's own `section.set`.
    Sections,
    /// An assembly's explodes (`explode.list`).
    Explodes(String),
    /// This window's own `explode.show`.
    ExplodeShown,
    /// An assembly's explode written or deleted, to show `explode` next
    /// if it is still there.
    ExplodeSaved(String, Option<String>),
    /// A sheet metal solid's table (`sheet.get`): its part and the solid
    /// asked for, at a part revision.
    SheetTable(BodyKey, String),
    /// A sheet metal solid's flat bend lines (`flat.get`), likewise.
    FlatLines(BodyKey, String),
    /// The flat view's body (`mesh.get` with `flat`), likewise.
    FlatMesh(BodyKey, String),
    /// Only a failure matters.
    Quiet,
}

/// How often a drag asks the daemon to solve at most.
pub const DRAG_EVERY: Duration = Duration::from_millis(50);

/// A freehand drag's suggestion says this until the person says more.
pub const ROUGH_COMMENT: &str = "Move about here";

/// What a request proposed in Annotate says beside its change.
#[derive(Clone, Debug, Default)]
struct Proposal {
    comment: Option<String>,
    rough: bool,
}

/// An instance being dragged: its assembly, how far it has been moved
/// from where it was, when the last preview was asked for and whether
/// its answer is awaited.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub part: String,
    pub instance: String,
    pub translate: [f64; 3],
    asked: Option<(Instant, [f64; 3])>,
    waiting: bool,
}

/// How often the window's camera is shared at most while it moves: often
/// enough for a follower, rarely enough not to flood watching agents.
pub const CAMERA_EVERY: Duration = Duration::from_millis(150);
/// A long request of the window's that ran this long and finished while
/// the window wasn't focused gets a toast.
pub const DONE_TOAST_AFTER: Duration = Duration::from_secs(2);

/// How long the status line shows a failed request.
pub const ERROR_SHOWN: Duration = Duration::from_secs(8);

/// How long a window that lost its daemon waits before trying again, at
/// first and at most: each failure doubles the wait.
pub const RETRY_FIRST: Duration = Duration::from_millis(250);
pub const RETRY_MAX: Duration = Duration::from_secs(5);

/// How often the History tab reads the Git history again while it is
/// shown: commits made outside ryucad change no project file, so no
/// notification says so.
pub const HISTORY_EVERY: Duration = Duration::from_secs(5);

/// A lost window's next attempt to reconnect.
struct Retry {
    at: Instant,
    delay: Duration,
    /// An attempt in progress, on a thread: connecting may start a daemon.
    trying: Option<Receiver<Result<Link>>>,
}

/// The faces a feature made, as the model shows them: by solid, by name
/// (hex), at a part revision; and the edges where they meet faces it
/// didn't make (a shell's opening, a fillet's rails), by solid. A fill's
/// sides and a sew's sewn edges (`lines`), and a thicken's walls
/// (`walls`), by their origins (ryucad 18).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Made {
    pub feature: BodyKey,
    pub revision: String,
    pub faces: std::collections::BTreeMap<BodyKey, std::collections::BTreeSet<String>>,
    pub rims: std::collections::BTreeMap<BodyKey, std::collections::BTreeSet<String>>,
    /// Edges whose origin is the feature's `side` (a fill's) or `sewn` (a
    /// sew's): drawn dashed when it is chosen.
    pub lines: std::collections::BTreeMap<BodyKey, std::collections::BTreeSet<String>>,
    /// Faces whose origin is the feature's `wall_of` (a thicken's walls):
    /// painted in place of all it made.
    pub walls: std::collections::BTreeMap<BodyKey, std::collections::BTreeSet<String>>,
}

impl Made {
    /// The faces of an `inspect` of the model whose origin is `feature`;
    /// of a tweak's own `inspect`, the faces it modified.
    pub fn from_inspect(feature: &BodyKey, revision: &str, v: &Value) -> Made {
        if let Some(kind) = crate::core::tweaks::KINDS
            .into_iter()
            .find(|k| v[*k].is_object())
        {
            return Made {
                feature: feature.clone(),
                revision: revision.to_owned(),
                faces: crate::core::tweaks::modified(v, kind),
                rims: std::collections::BTreeMap::new(),
                lines: std::collections::BTreeMap::new(),
                walls: std::collections::BTreeMap::new(),
            };
        }
        let mut faces = std::collections::BTreeMap::new();
        let mut rims = std::collections::BTreeMap::new();
        let mut lines = std::collections::BTreeMap::new();
        let mut walls = std::collections::BTreeMap::new();
        // The names in `list` whose origin is this feature's and, given
        // `keys`, holds one of them.
        let named = |list: &Value, keys: &[&str]| -> std::collections::BTreeSet<String> {
            list.as_array()
                .into_iter()
                .flatten()
                .filter(|e| e["origin"]["feature"] == feature.1.as_str())
                .filter(|e| keys.is_empty() || keys.iter().any(|k| e["origin"].get(*k).is_some()))
                .filter_map(|e| e["name"].as_str().map(str::to_owned))
                .collect()
        };
        for b in v["bodies"].as_array().into_iter().flatten() {
            let Some(solid) = b["solid"].as_str() else {
                continue;
            };
            let key = (feature.0.clone(), solid.to_owned());
            // A sew makes no face of its own: its sheets' faces keep theirs.
            let marked = named(&b["edges"], &["side", "sewn"]);
            if !marked.is_empty() {
                lines.insert(key.clone(), marked);
            }
            let names = named(&b["faces"], &[]);
            if names.is_empty() {
                continue;
            }
            let wall = named(&b["faces"], &["wall_of"]);
            if !wall.is_empty() {
                walls.insert(key.clone(), wall);
            }
            // An edge between a face it made and one it didn't.
            let rim: std::collections::BTreeSet<String> = b["edges"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|e| {
                    let on: Vec<bool> = e["faces"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|f| names.contains(f))
                        .collect();
                    on.contains(&true) && on.contains(&false)
                })
                .filter_map(|e| e["name"].as_str().map(str::to_owned))
                .collect();
            if !rim.is_empty() {
                rims.insert(key.clone(), rim);
            }
            faces.insert(key, names);
        }
        Made {
            feature: feature.clone(),
            revision: revision.to_owned(),
            faces,
            rims,
            lines,
            walls,
        }
    }
}

/// A translation as a request takes it: quantities in meters.
fn meters(t: [f64; 3]) -> [Value; 3] {
    t.map(|x| json!(format!("{x} m")))
}

/// The findings of a check report.
fn findings(report: &Value) -> Vec<Value> {
    report["findings"].as_array().cloned().unwrap_or_default()
}

/// A camera as a request gives it: points as quantities in meters.
pub fn camera_arg(c: &Camera) -> CameraArg {
    let q = |x: f64| json!(format!("{x} m"));
    CameraArg {
        eye: c.eye.map(q),
        target: c.target.map(q),
        up: c.up,
        projection: match c.projection {
            Projection::Perspective { fov_deg } => ProjectionArg::Perspective { fov_deg },
            Projection::Orthographic { height } => {
                ProjectionArg::Orthographic { height: q(height) }
            }
        },
    }
}

pub struct Workspace {
    pub root: PathBuf,
    pub link: Link,
    /// The window's mode (design 374): View when it opens and whenever it
    /// connects again; Annotate and Direct edit by the person's choice.
    pub mode: crate::core::modes::Mode,
    /// The project's annotations (design 374), read when the window opens
    /// and followed from `annotation.changed`.
    pub annotations: crate::core::annotations::Annotations,
    /// The annotation chosen in the tile or by its marker.
    pub chosen_annotation: Option<String>,
    /// Annotate's Show agent's model: the working files drawn though the
    /// person has a draft.
    pub show_agent: bool,
    /// What the model was last read at: its revision and the draft's
    /// suggestions laid.
    drawn: (Option<String>, Vec<String>),
    /// The Annotations tile's filter and what is typed in it.
    pub annotation_filter: crate::core::annotations::Filter,
    pub annotation_ui: crate::core::annotations::Typed,
    pub model: Model,
    /// Parts whose reports are asked for and not yet answered.
    pub loading: BTreeSet<String>,
    /// The revision each part in `loading` was last asked for at.
    asked_at: BTreeMap<String, Option<String>>,
    /// Parts the tree, as it draws, found it wants loaded (its rows
    /// opened): loaded once it is drawn ([`Workspace::load_wanted`]).
    pub wanted: std::cell::RefCell<BTreeSet<String>>,
    pub people: People,
    /// The participant whose camera this window follows.
    pub following: Option<String>,
    /// The property panel's form, for the feature chosen in the tree.
    pub form: Option<Form>,
    /// The faces the feature chosen in the tree made, by the solid that
    /// shows them; and the feature, once asked for.
    pub made: Option<Made>,
    made_for: Option<BodyKey>,
    /// The sweep or loft chosen in the tree as `sweep.get` drew it: its
    /// profile and path, or its sections with their starts.
    pub swept: Option<crate::core::sweeps::SweepDrawing>,
    /// The part revision the chosen feature was last looked at for a
    /// sweep's or loft's drawing.
    swept_at: Option<String>,
    /// Sheet metal solids' tables and flats, as read (design 346, The
    /// app): the Part tile's sheet metal section and what a chosen unfold
    /// or refold paints.
    pub sheets: crate::core::sheet::Cache,
    /// The flat view (View > Flat pattern), while on.
    pub flat_view: Option<crate::core::flat::FlatView>,
    /// The faces of the bend under the pointer in the Part tile's bend
    /// table, painted in the hover colour.
    pub sheet_hover: Vec<String>,
    /// The property panel's form for a dimension chosen by its label;
    /// shown instead of `form`.
    pub dimension: Option<DimForm>,
    /// What the property panel was last asked to show, a feature or a
    /// dimension: only its answer opens a form.
    form_for: Option<(String, String)>,
    dimension_for: Option<(String, String, String)>,
    /// The parameter panel, for the part it shows (see `show_params`).
    pub params: Option<Params>,
    params_part: Option<String>,
    /// The Equations tile's table, once it is open (see `show_equations`).
    pub equations: Option<Equations>,
    equations_open: bool,
    /// The Drawings tile (see `show_drawings`).
    pub drawings: crate::core::drawings::Tile,
    /// Every part's drawings, for the tree: ID, name, and whether its
    /// file doesn't read (`drawing.list` of the project).
    pub drawing_rows: BTreeMap<String, Vec<(String, String, bool)>>,
    pub activity: Activity,
    /// The project's Git history (`history.log`), once asked for.
    pub history: Option<Value>,
    /// When it was last asked for, whether an answer is on its way, and
    /// whether it was wanted again meanwhile.
    history_at: Option<Instant>,
    history_waiting: bool,
    history_again: bool,
    /// The Branches tab: the worktrees and their merge previews, once
    /// asked for; when they were last read and whether a reading is on its
    /// way.
    pub branches: Branches,
    branches_at: Option<Instant>,
    branches_waiting: bool,
    /// The project's dependencies, the lock's labels and what is
    /// outdated: the Dependencies tab's, and the marks' in the tree.
    pub deps: Deps,
    deps_waiting: bool,
    deps_again: bool,
    /// Place from dependency…, while open.
    pub picker: Option<Picker>,
    /// The review service's organization-wide where-used on its way, for
    /// this part; and one that came before `bom.where_used` answered.
    org: Option<(String, std::sync::mpsc::Receiver<Result<Value>>)>,
    org_early: Option<(String, Result<Vec<crate::core::where_used::Use>>)>,
    /// This window's compare with a Git revision, while it is in one: it
    /// follows the window's participant's compare state.
    pub compare: Option<Compare>,
    /// How many diffs were asked for, and whether one is on its way; one
    /// asked for meanwhile waits for it (`compare_again`).
    compare_asked: u64,
    compare_waiting: bool,
    compare_again: bool,
    /// The merge Git is in the middle of, while it is in one: what is left
    /// to settle.
    pub merge: Option<Merge>,
    /// Every part's release state (ryupdm), for the tree's badges.
    pub states: States,
    /// ryupdm's settings (`pdm.config`): `project` is null until PDM is
    /// set up.
    pub pdm_config: Option<Value>,
    /// The Part section's form, for the part it shows (see `show_props`).
    pub props: Option<PartProps>,
    props_part: Option<String>,
    /// The Releases tab, and whether it was opened (it follows changes
    /// from then on).
    pub releases: ReleaseTab,
    releases_open: bool,
    /// The BOM tab, whether it was opened (it follows changes from then
    /// on), and whether a reading is on its way and another wanted after.
    pub bom: BomTab,
    bom_open: bool,
    bom_waiting: bool,
    bom_again: bool,
    /// The BOM tab's Export CSV…, while open.
    pub bom_export: Option<crate::core::bom::BomExport>,
    /// Where the Part section's part is used, once read, and its part.
    pub where_used: Option<crate::core::where_used::WhereUsed>,
    where_used_part: Option<String>,
    pub settings: Settings,
    /// The last doctor report, once asked for.
    pub doctor: Option<Value>,
    /// The shown assembly's interference, once asked for (Tools > Check
    /// interference); asked again as the assembly changes.
    pub interference: Option<Interference>,
    /// Tools > Measure: on or off, its picks and what they measured.
    pub measure: Tool,
    /// Tools > Mass properties, once asked for.
    pub mass: Option<Mass>,
    /// Tools > Clearance, once asked for: the shown assembly's.
    pub clearance: Option<Clearance>,
    /// What the disk cache held when last asked (`cache.stats`), for the
    /// Preferences window.
    pub cache: Option<Value>,
    /// The last answer to storing or removing the shared cache's token
    /// (`cache.login`, `cache.logout`), or its error as `{"error": …}`.
    pub cache_token: Option<Value>,
    /// File > Import STEP… or Update import…, while open.
    pub import: Option<crate::core::step::Import>,
    /// File > Export…, while open.
    pub export: Option<crate::core::export::Export>,
    /// What the last export chose, which the next one starts from.
    pub export_choices: crate::core::export::Choices,
    /// The instance being dragged, if any.
    pub drag: Option<Drag>,
    /// The Move tool, while it is on one instance.
    pub mover: Option<crate::core::mover::Mover>,
    /// When the Move tool's preview was last asked and for which value,
    /// and whether one is on its way.
    mover_asked: Option<(Instant, Option<crate::core::mover::Grip>, f64)>,
    mover_waiting: bool,
    /// The Mate dialog, while it is open.
    pub mating: Option<crate::core::mating::Mating>,
    /// The Markup bar, while it is open (design 423).
    pub markup: Option<crate::core::markup::Markup>,
    /// How many mate previews the window has asked for.
    mate_previews: u64,
    /// A tile the window should open (the app opens it next frame): the
    /// Annotations tile after a rough drag's proposal.
    pub wants_tile: Option<&'static str>,
    /// The named views of the part the window shows, for the cube's menu
    /// and the view selector.
    pub views: crate::core::views::NamedViews,
    /// A camera normal to a face, come from the daemon, for the view to
    /// turn to.
    pub looked: Option<Camera>,
    /// What the window shows changed (View > Show, or an agent's
    /// `ryucad focus`): the view frames it again, as it frames the first
    /// view (issue #211).
    pub refocused: bool,
    /// The Appearance window.
    pub appearance: crate::core::appearance::Editor,
    /// The planes this window cuts with, and the gizmo's drag.
    pub sections: crate::core::section::Sections,
    /// The shown assembly's explodes, the one shown, and a step being
    /// made.
    pub explodes: crate::core::explode::Explodes,
    /// How the shown bodies look, as the daemon last said.
    looks_read: BTreeMap<BodyKey, Arc<crate::core::appearance::Look>>,
    /// The last appearance reading asked for: an older answer is dropped.
    looks_asked: u64,
    /// Tests only: notifications are dropped as they arrive, as if lost,
    /// to see what the window does from its answers alone.
    #[doc(hidden)]
    pub deaf: bool,
    /// Why the last drag moved nothing, for the status line.
    pub drag_note: Option<String>,
    /// Whether a report is on its way, and whether the project changed
    /// meanwhile, so the badges need another.
    doctor_waiting: bool,
    doctor_again: bool,
    /// Someone asked for this window (`ryucad open`): bring it forward.
    pub raised: bool,
    /// Requests of the window's that failed, newest last.
    pub errors: Vec<Error>,
    /// When the newest of them came.
    error_at: Option<Instant>,
    /// The daemon is gone: the window shows its last state, greyed, and
    /// reconnects (see `tick`).
    pub lost: bool,
    /// Why the last attempt to reconnect failed.
    pub lost_why: Option<Error>,
    /// Long requests: the window's own, with their progress and Cancel,
    /// and other participants', from the `progress` topic (design 199).
    pub progress: Progress,
    /// The toasts to draw.
    pub toasts: Toasts,
    /// The failure whose error card is open, by its place in `errors`.
    pub card: Option<usize>,
    /// Whether the window has the system's focus: a long request of its
    /// own that finishes while it hasn't gets a toast.
    pub focused: bool,
    /// The activity feed's filter: one participant's ID, `external`, or
    /// everyone.
    pub feed_only: Option<String>,
    /// How many meshes were fetched: an unchanged body is never fetched
    /// twice.
    pub fetched: u64,
    /// How many bytes answers brought after their lines: meshes, mostly.
    pub received: u64,
    pending: HashMap<u64, Pending>,
    /// The camera to share, and when one was last shared.
    camera_due: Option<Camera>,
    camera_sent: Option<Instant>,
    /// The camera last shared, to share again with a new daemon.
    camera_last: Option<Camera>,
    daemon: Daemon,
    name: String,
    wake: Wake,
    retry: Option<Retry>,
}

impl Workspace {
    /// The project's default unit, for forms.
    /// Opens the project at `root` (canonical) as the human `name`, and
    /// starts loading it.
    pub fn open(root: &Path, daemon: &Daemon, name: &str, wake: Wake) -> Result<Workspace> {
        let link = Link::open(root, daemon, name, wake.clone())?;
        let mut people = People::default();
        people.reset(&link.session["participants"]);
        let mut ws = Workspace {
            root: root.to_path_buf(),
            link,
            mode: Default::default(),
            annotations: Default::default(),
            chosen_annotation: None,
            show_agent: false,
            drawn: Default::default(),
            annotation_filter: Default::default(),
            annotation_ui: Default::default(),
            model: Model::default(),
            loading: BTreeSet::new(),
            asked_at: BTreeMap::new(),
            wanted: Default::default(),
            people,
            following: None,
            form: None,
            made: None,
            made_for: None,
            swept: None,
            swept_at: None,
            sheets: Default::default(),
            flat_view: None,
            sheet_hover: Vec::new(),
            dimension: None,
            form_for: None,
            dimension_for: None,
            params: None,
            params_part: None,
            equations: None,
            equations_open: false,
            drawings: Default::default(),
            drawing_rows: BTreeMap::new(),
            activity: Activity::default(),
            history: None,
            history_at: None,
            history_waiting: false,
            history_again: false,
            branches: Branches::default(),
            branches_at: None,
            branches_waiting: false,
            deps: Deps::default(),
            deps_waiting: false,
            deps_again: false,
            picker: None,
            org: None,
            org_early: None,
            compare: None,
            compare_asked: 0,
            compare_waiting: false,
            compare_again: false,
            merge: None,
            states: States::default(),
            pdm_config: None,
            props: None,
            props_part: None,
            releases: ReleaseTab::default(),
            releases_open: false,
            bom: BomTab::default(),
            bom_open: false,
            bom_waiting: false,
            bom_again: false,
            bom_export: None,
            where_used: None,
            where_used_part: None,
            settings: Settings::default(),
            doctor: None,
            interference: None,
            measure: Tool::default(),
            mass: None,
            clearance: None,
            cache: None,
            cache_token: None,
            import: None,
            export: None,
            export_choices: Default::default(),
            drag: None,
            mover: None,
            mover_asked: None,
            mover_waiting: false,
            mating: None,
            markup: None,
            mate_previews: 0,
            wants_tile: None,
            views: Default::default(),
            sections: Default::default(),
            explodes: Default::default(),
            looked: None,
            refocused: false,
            appearance: Default::default(),
            looks_read: BTreeMap::new(),
            looks_asked: 0,
            deaf: false,
            drag_note: None,
            doctor_waiting: false,
            doctor_again: false,
            raised: false,
            camera_due: None,
            camera_sent: None,
            camera_last: None,
            errors: Vec::new(),
            error_at: None,
            lost: false,
            lost_why: None,
            progress: Progress::default(),
            toasts: Toasts::default(),
            card: None,
            focused: true,
            feed_only: None,
            fetched: 0,
            received: 0,
            pending: HashMap::new(),
            daemon: daemon.clone(),
            name: name.to_owned(),
            wake,
            retry: None,
        };
        ws.load();
        Ok(ws)
    }

    /// Asks for everything again: on opening, and after `resync`.
    fn load(&mut self) {
        // What was asked for before a resync may never be answered.
        self.loading.clear();
        self.asked_at.clear();
        self.send(Request::ProjectInfo {}, Pending::Info);
        self.send(Request::PartList {}, Pending::Parts);
        self.read_drawn();
        let log = Request::ActivityLog {
            limit: Some(LOADED),
            before: None,
        };
        self.send(log, Pending::Activity);
        self.load_settings();
        self.send(Request::MergeStatus {}, Pending::Merge);
        self.send(Request::ReleaseList { part: None }, Pending::States);
        self.send(Request::PdmConfig {}, Pending::PdmConfig);
        self.read_appearance();
        self.load_deps();
        let list = Request::AnnotationList {
            status: Vec::new(),
            part: None,
            by: None,
            open: false,
        };
        self.send(list, Pending::Annotations);
    }

    fn load_settings(&mut self) {
        let req = Request::ConfigGet {
            key: None,
            scope: None,
        };
        self.send(req, Pending::Config);
    }

    /// Changes a setting through `config.set`: a project setting against
    /// the revision it was read at. A failure lands in `errors`.
    pub fn set_config(&mut self, key: &str, value: Value) {
        let req = self.config_request(key, value);
        self.send(req, Pending::Config);
    }

    /// [`Workspace::set_config`], waiting up to `timeout` for the daemon's
    /// answer: a window about to close would otherwise go before its
    /// request reached the daemon. Whether the daemon answered; a lost
    /// or hung daemon gives up at `timeout`.
    pub fn set_config_waiting(&mut self, key: &str, value: Value, timeout: Duration) -> bool {
        let req = self.config_request(key, value);
        let ticket = self.link.send(req);
        self.link.wait_answer(ticket, timeout)
    }

    fn config_request(&self, key: &str, value: Value) -> Request {
        let project = self
            .settings
            .all
            .iter()
            .any(|s| s.key == key && s.scope == "project");
        Request::ConfigSet {
            key: key.to_owned(),
            value,
            scope: None,
            expected_revision: project.then(|| self.settings.project_revision.clone()),
        }
    }

    /// Undoes an activity entry, or this window's latest change. A failure
    /// (`undo_conflict`, `nothing_to_undo`) lands in `errors`.
    pub fn undo(&mut self, entry: Option<u64>) {
        self.send(Request::Undo { entry }, Pending::Quiet);
    }

    /// Redoes an undo, or this window's latest undo.
    pub fn redo(&mut self, entry: Option<u64>) {
        self.send(Request::Redo { entry }, Pending::Quiet);
    }

    /// The assembly the window shows, if it shows exactly one.
    pub fn shown_assembly(&self) -> Option<String> {
        match self.model.shown().as_slice() {
            [one] if self.model.parts.get(*one).is_some_and(|p| p.assembly) => {
                Some((*one).to_owned())
            }
            _ => None,
        }
    }

    /// Finds which instances of the shown assembly interfere
    /// (`interference`); nothing when it shows none.
    pub fn check_interference(&mut self) {
        let Some(part) = self.shown_assembly() else {
            return;
        };
        self.interference = Some(Interference::asked(&part));
        let req = Request::Interference {
            part: part.clone(),
            instances: Vec::new(),
        };
        self.send(req, Pending::Interference(part));
    }

    /// The part whose named views the window offers: the one it shows.
    pub fn views_part(&self) -> Option<String> {
        match self.model.shown().as_slice() {
            [one] => Some((*one).to_owned()),
            _ => None,
        }
    }

    /// Reads the named views of the part shown, when that changed.
    fn follow_views(&mut self) {
        let part = self.views_part();
        if part != self.views.part {
            self.views = crate::core::views::NamedViews {
                part,
                ..Default::default()
            };
            self.read_views();
        }
    }

    fn read_views(&mut self) {
        if let Some(p) = self.views.part.clone() {
            let req = Request::ViewList {
                part: Some(p.clone()),
            };
            self.send(req, Pending::Views(p));
        }
    }

    /// Saves `camera` as named view `id` of the part shown (`view.save`),
    /// with this window's section planes and explode: the camera is shared
    /// at once, then saved from this window's participant (#576).
    pub fn save_view(&mut self, id: &str, camera: &Camera) {
        let Some(part) = self.views.part.clone() else {
            return;
        };
        self.views.error = None;
        // The saved camera must be exactly this one, not the last shared.
        self.camera_due = None;
        self.camera_sent = Some(Instant::now());
        self.camera_last = Some(camera.clone());
        let share = Request::CameraSet {
            camera: Some(camera_arg(camera)),
            view: None,
            fit: Vec::new(),
            normal_to: None,
            part: None,
        };
        self.send(share, Pending::Quiet);
        let req = Request::ViewSave {
            part: part.clone(),
            view: id.to_owned(),
            camera: None,
            participant: Some(self.me().to_owned()),
            expected_revision: None,
        };
        self.send(req, Pending::ViewSaved(part));
    }

    /// Restores named view `id` of the part shown on this window's
    /// participant (`camera.set` with the view): the daemon puts back its
    /// camera, section planes and explode, which reach the window as they
    /// reach an agent's (#576).
    pub fn restore_named_view(&mut self, id: &str) {
        let Some(part) = self.views.part.clone() else {
            return;
        };
        let req = Request::CameraSet {
            camera: None,
            view: Some(id.to_owned()),
            fit: Vec::new(),
            normal_to: None,
            part: Some(part),
        };
        self.send(req, Pending::Quiet);
    }

    /// Asks the daemon for a camera normal to `face` (its geometry is the
    /// daemon's); the answer lands in `looked`.
    pub fn look_normal_to(&mut self, face: &str) {
        let req = Request::CameraSet {
            camera: None,
            view: None,
            fit: Vec::new(),
            normal_to: Some(face.to_owned()),
            part: None,
        };
        self.send(req, Pending::NormalTo);
    }

    /// Reads how the bodies the window shows look (`appearance.get`).
    fn read_appearance(&mut self) {
        self.looks_asked += 1;
        let req = Request::AppearanceGet {
            part: self.model.focus.clone(),
            participant: None,
            rev: self.model_rev(),
        };
        self.send(req, Pending::Appearance(self.looks_asked));
    }

    /// Lays what the Appearance window drags over the daemon's looks.
    fn refresh_looks(&mut self) {
        use crate::core::appearance::previewed;
        let looks = match (self.appearance.dragging, self.appearance.target()) {
            (Some(p), Some(t)) if self.appearance.open => {
                previewed(&self.looks_read, self.model.meshes.keys(), t, p)
            }
            _ => self.looks_read.clone(),
        };
        if *self.model.looks != looks {
            self.model.looks = Arc::new(looks);
            self.model.generation += 1;
        }
    }

    /// Opens the Appearance window on the selection's first entity, else
    /// the one part the window shows (an assembly colours only what it
    /// places).
    pub fn open_appearance(&mut self) {
        use crate::core::appearance::targets;
        let first = self.myself().and_then(|m| m.selection.first().cloned());
        let targets = match first {
            Some(e) => {
                let part = e.split('/').next().unwrap_or_default();
                let assembly = self.model.parts.get(part).is_some_and(|p| p.assembly);
                targets(&e, assembly)
            }
            None => match self.model.shown().as_slice() {
                [one] if !self.model.parts.get(*one).is_some_and(|p| p.assembly) => {
                    vec![(*one).to_owned()]
                }
                _ => Vec::new(),
            },
        };
        let now = self.look_of(targets.first().map(String::as_str));
        self.appearance
            .open_on(targets, now, ryucad::render::SURFACE);
    }

    /// How `target` (a reference) is drawn now: its face, else its first
    /// body's base.
    pub fn look_of(&self, target: Option<&str>) -> crate::core::appearance::Face {
        let Some(t) = target else {
            return Default::default();
        };
        let (part, rest) = t.split_once('/').unwrap_or((t, ""));
        let is_name = |s: &str| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let (prefix, face) = match rest.rsplit_once('/') {
            Some((s, n)) if is_name(n) => (s, Some(n)),
            _ => (rest, None),
        };
        let found = self.looks_read.iter().find(|((p, k), _)| {
            p == part && (prefix.is_empty() || k == prefix || k.starts_with(&format!("{prefix}/")))
        });
        match (found, face) {
            (Some((_, l)), Some(n)) => l.face(n),
            (Some((_, l)), None) => l.base,
            (None, _) => Default::default(),
        }
    }

    /// Shows `preview` over what the Appearance window colours, until it
    /// is written.
    pub fn preview_appearance(&mut self, preview: crate::core::appearance::Preview) {
        self.appearance.dragging = Some(preview);
        self.refresh_looks();
    }

    /// Writes what the Appearance window shows to its target: its colour
    /// (`colour`), its material's colour (`material`) or its opacity
    /// (`opacity`), one undoable change (`appearance.set`).
    pub fn write_appearance(&mut self, field: &str) {
        let Some(target) = self.appearance.target().map(str::to_owned) else {
            return;
        };
        self.appearance.error = None;
        let req = Request::AppearanceSet {
            entities: vec![target],
            colour: (field == "colour").then(|| self.appearance.hex()),
            material: field == "material",
            // To a thousandth: the slider's f32 isn't the decimal it shows.
            opacity: (field == "opacity")
                .then_some((f64::from(self.appearance.opacity) * 1000.0).round() / 1000.0),
            unset: Vec::new(),
            expected_revision: None,
        };
        self.send(req, Pending::AppearanceWritten);
    }

    /// Takes the Appearance window's target's entry out
    /// (`appearance.clear`).
    pub fn clear_appearance(&mut self) {
        let Some(target) = self.appearance.target().map(str::to_owned) else {
            return;
        };
        self.appearance.error = None;
        self.appearance.dragging = None;
        let req = Request::AppearanceClear {
            entities: vec![target],
            expected_revision: None,
        };
        self.send(req, Pending::AppearanceWritten);
    }

    /// Closes the Appearance window, dropping anything dragged.
    pub fn close_appearance(&mut self) {
        self.appearance.open = false;
        self.appearance.dragging = None;
        self.refresh_looks();
    }

    /// Takes a pick of the Measure tool and measures what it now holds.
    pub fn measure_pick(&mut self, entity: &str) {
        let picks = self.measure.pick(entity);
        let req = Request::Measure {
            entities: picks.clone(),
            rev: None,
        };
        self.send(req, Pending::Measure(picks));
    }

    /// Measures the Measure tool's picks again: what they name changed.
    fn measure_again(&mut self) {
        let picks = self.measure.picks.clone();
        if picks.is_empty() {
            return;
        }
        let req = Request::Measure {
            entities: picks.clone(),
            rev: self.model_rev(),
        };
        self.send(req, Pending::Measure(picks));
    }

    /// What Tools > Mass properties weighs: the solid or occurrence of the
    /// first selected entity, else the part or assembly shown.
    pub fn mass_target(&self) -> Option<String> {
        if let Some(e) = self.myself().and_then(|m| m.selection.first()) {
            let part = e.split('/').next().unwrap_or_default();
            let assembly = self.model.parts.get(part).is_some_and(|p| p.assembly);
            return Some(crate::core::measure::weighed(e, assembly));
        }
        match self.model.shown().as_slice() {
            [one] => Some((*one).to_owned()),
            _ => None,
        }
    }

    /// Weighs `of` (`mass`) for the Mass properties panel.
    pub fn weigh(&mut self, of: &str) {
        self.mass = Some(Mass::asked(of));
        let req = Request::Mass {
            of: of.to_owned(),
            tolerance: None,
            rev: self.model_rev(),
        };
        self.send(req, Pending::Mass(of.to_owned()));
    }

    /// Finds the shown assembly's clearances (`clearance`); nothing when it
    /// shows none.
    pub fn check_clearance(&mut self) {
        let Some(part) = self.shown_assembly() else {
            return;
        };
        self.clearance = Some(Clearance::asked(&part));
        let req = Request::Clearance {
            part: part.clone(),
            instances: Vec::new(),
            below: None,
        };
        self.send(req, Pending::Clearance(part));
    }

    /// Diagnoses the project (`doctor`).
    pub fn diagnose(&mut self) {
        self.doctor_waiting = true;
        self.send(Request::Doctor {}, Pending::Doctor);
    }

    /// Diagnoses the project again after it changed, so badges don't
    /// outlive the damage they show (a conflict record settled, say): at
    /// most one report on its way, and one more after it if the project
    /// changed meanwhile. Nothing until the window first asked.
    /// Moves the instance being dragged (starting a drag of `instance` of
    /// assembly `part` if none is) by `translate` from where it was: the
    /// daemon solves the rest, held there for a first solve, at most every
    /// [`DRAG_EVERY`], and the window draws where it puts them.
    pub fn drag_to(&mut self, part: &str, instance: &str, translate: [f64; 3]) {
        let same = self
            .drag
            .as_ref()
            .is_some_and(|d| d.part == part && d.instance == instance);
        if !same {
            self.drag = Some(Drag {
                part: part.to_owned(),
                instance: instance.to_owned(),
                translate,
                asked: None,
                waiting: false,
            });
        }
        let d = self.drag.as_mut().expect("just set");
        d.translate = translate;
        let due = d.asked.is_none_or(|(at, _)| at.elapsed() >= DRAG_EVERY);
        if !d.waiting && due {
            self.ask_drag();
        }
    }

    fn ask_drag(&mut self) {
        let Some(d) = self.drag.as_mut() else { return };
        d.asked = Some((Instant::now(), d.translate));
        d.waiting = true;
        let req = Request::AssemblySolve {
            part: d.part.clone(),
            moves: vec![ryucad::proto::InstanceMove {
                instance: d.instance.clone(),
                translate: Some(meters(d.translate)),
                ..Default::default()
            }],
            component: true,
            rev: None,
            mates: Vec::new(),
        };
        let part = d.part.clone();
        self.send(req, Pending::DragSolve(part));
    }

    /// Takes a preview solve's poses: each instance it moved, drawn there.
    fn preview_poses(&mut self, part: &str, result: &Value) {
        let Some(instances) = result["instances"].as_object() else {
            return;
        };
        let triple = |v: &Value| [0, 1, 2].map(|k| v[k].as_f64().unwrap_or_default());
        for (id, i) in instances {
            let p = &i["pose"];
            let pose = crate::core::model::Pose::of(
                triple(&p["origin_m"]),
                triple(&p["z"]),
                triple(&p["x"]),
            );
            self.model
                .preview
                .insert((part.to_owned(), id.clone()), pose);
        }
        self.model.generation += 1;
    }

    /// Ends a drag, writing where it left the instance with one
    /// `assembly.move` (logged and undoable, as an agent's), against the
    /// assembly's revision as the window last saw it.
    pub fn drop_drag(&mut self) {
        let Some(d) = self.drag.take() else { return };
        if d.translate == [0.0; 3] {
            self.model.preview.retain(|(p, _), _| *p != d.part);
            // Picking and drawing read poses through the picker, made
            // again when the generation moves.
            self.model.generation += 1;
            return;
        }
        let revision = self.model.parts.get(&d.part).map(|p| p.revision.clone());
        let req = Request::AssemblyMove {
            part: d.part.clone(),
            instance: d.instance.clone(),
            placement: None,
            translate: Some(meters(d.translate)),
            axis: None,
            angle: None,
            about: None,
            expected_revision: revision,
        };
        // In Annotate a freehand drag is a rough suggestion, its comment
        // for the person to say more in.
        self.send_with(
            req,
            Pending::DragMove(d.part),
            Proposal {
                comment: Some(ROUGH_COMMENT.into()),
                rough: true,
            },
        );
    }

    /// Puts the Move tool's handles on instance `instance` of assembly
    /// `part`, at the middle of its drawn boxes, or says why it can't
    /// move (fixed, a pattern's copy, no freedom left).
    pub fn start_move(&mut self, part: &str, instance: &str) -> Result<(), String> {
        let key = (part.to_owned(), format!("{instance}/"));
        self.model.movable(&key)?;
        let middle = self
            .model
            .middles(part)
            .get(instance)
            .copied()
            .ok_or_else(|| format!("{instance} isn't drawn yet"))?;
        let body = self
            .model
            .meshes
            .keys()
            .find(|k| k.0 == part && k.1.split('/').next() == Some(instance))
            .cloned();
        let pose = body.map_or(crate::core::model::Pose::IDENTITY, |k| self.model.pose(&k));
        let own = self.mover.as_ref().is_some_and(|m| m.own);
        self.cancel_move();
        self.mover = Some(crate::core::mover::Mover::new(
            part, instance, middle, pose, own,
        ));
        Ok(())
    }

    /// The project's default unit.
    pub fn unit(&self) -> ryucad::units::Unit {
        ryucad::units::Unit::parse(&self.model.default_unit).unwrap_or(ryucad::units::Unit::Mm)
    }

    /// Reads the Move tool's value field.
    pub fn type_move(&mut self, text: &str) {
        let unit = self.unit();
        if let Some(m) = self.mover.as_mut() {
            m.typed(text, unit);
        }
    }

    /// Asks where the Move tool's value puts what the instance's mates
    /// reach, at the drawn revision: when the value has changed since it
    /// last asked, one at a time, at most every [`DRAG_EVERY`]. The view
    /// calls it every frame the tool is on.
    pub fn preview_move(&mut self) {
        let Some(m) = &self.mover else { return };
        if self.mover_waiting {
            return;
        }
        let asked = (m.grip, m.value);
        if self
            .mover_asked
            .is_some_and(|(at, g, v)| (g, v) == asked || at.elapsed() < DRAG_EVERY)
        {
            return;
        }
        let part = m.part.clone();
        if m.grip.is_none() || m.value == 0.0 || m.error.is_some() {
            // Nothing to move: drawn where it is.
            self.mover_asked = Some((Instant::now(), asked.0, asked.1));
            if self.model.preview.keys().any(|(p, _)| *p == part) {
                self.model.preview.retain(|(p, _), _| *p != part);
                self.model.generation += 1;
            }
            return;
        }
        let req = m.preview(self.model_rev());
        self.mover_asked = Some((Instant::now(), asked.0, asked.1));
        self.mover_waiting = true;
        self.send(req, Pending::MoveSolve(part));
    }

    /// Sends the Move tool's value as `assembly.move` (proposed in
    /// Annotate) against the assembly's revision as the window last saw
    /// it; the handles go.
    pub fn send_move(&mut self) {
        let Some(m) = self.mover.take() else { return };
        if !m.ready() {
            self.mover = Some(m);
            return;
        }
        let revision = self.model.parts.get(&m.part).map(|p| p.revision.clone());
        let part = m.part.clone();
        self.mover_asked = None;
        self.mover_waiting = false;
        self.send(m.request(revision), Pending::MoveWrite(part));
    }

    /// Drops the Move tool's value and its preview; the handles stay.
    pub fn clear_move(&mut self) {
        let Some(m) = self.mover.as_mut() else { return };
        m.clear();
        let part = m.part.clone();
        self.model.preview.retain(|(p, _), _| *p != part);
        self.model.generation += 1;
    }

    /// Drops the Move tool and its preview.
    pub fn cancel_move(&mut self) {
        if let Some(m) = self.mover.take() {
            self.model.preview.retain(|(p, _), _| *p != m.part);
            self.model.generation += 1;
        }
        self.mover_asked = None;
        self.mover_waiting = false;
    }

    /// Opens the Mate dialog on assembly `part`, from instance `from`.
    pub fn open_mate(&mut self, part: &str, from: Option<&str>) {
        self.close_mate();
        self.mating = Some(crate::core::mating::Mating::new(part, from));
    }

    /// Takes a pick for the dialog's next side, previewing once both are
    /// in; a pick it refuses is said in the dialog.
    pub fn mate_pick(&mut self, entity: &str) -> std::result::Result<(), String> {
        let d = self.mating.as_mut().ok_or("no Mate dialog")?;
        match d.pick(entity) {
            Ok(_) => d.note = None,
            Err(why) => {
                d.note = Some(why.clone());
                return Err(why);
            }
        }
        self.preview_mate();
        Ok(())
    }

    /// Asks where the dialog's mate puts the instances, at the drawn
    /// revision, writing nothing.
    pub fn preview_mate(&mut self) {
        let rev = self.model_rev();
        let Some(d) = self.mating.as_mut() else {
            return;
        };
        let Some(req) = d.preview(rev) else { return };
        self.mate_previews += 1;
        d.asked = self.mate_previews;
        d.waiting = true;
        let part = d.part.clone();
        let asked = d.asked;
        self.send(req, Pending::MatePreview(part, asked));
    }

    /// Sends the dialog's mate (`feature.add`; proposed in Annotate)
    /// against the assembly's revision as the window last saw it.
    pub fn add_mate(&mut self) {
        let Some(d) = self.mating.take() else { return };
        if !d.addable() {
            self.mating = Some(d);
            return;
        }
        let revision = self.model.parts.get(&d.part).map(|p| p.revision.clone());
        let part = d.part.clone();
        let Some(req) = d.add(revision) else { return };
        self.model.preview.retain(|(p, _), _| *p != part);
        self.model.generation += 1;
        self.send(req, Pending::MateAdd(part));
    }

    /// Closes the Mate dialog and drops its preview.
    pub fn close_mate(&mut self) {
        if let Some(d) = self.mating.take() {
            self.model.preview.retain(|(p, _), _| *p != d.part);
            self.model.generation += 1;
        }
    }

    /// Opens the Markup bar on the shown part or assembly, looking through
    /// `camera`, which it locks (design 423).
    pub fn open_markup(&mut self, camera: Camera) -> std::result::Result<(), String> {
        let shown = self.model.shown();
        let [part] = shown.as_slice() else {
            return Err("Show one part or assembly to mark up (View > Show).".into());
        };
        let part = part.to_string();
        // One tool at a time.
        self.cancel_move();
        self.close_mate();
        self.markup = Some(crate::core::markup::Markup::new(&part, camera));
        Ok(())
    }

    /// Reopens the Markup bar on annotation `id`'s view and marks (its
    /// author's Edit), showing the model they were drawn on.
    pub fn edit_markup(&mut self, id: &str) -> std::result::Result<(), String> {
        let e = self
            .annotations
            .all
            .get(id)
            .ok_or_else(|| format!("No annotation {id}."))?;
        let m = crate::core::markup::Markup::editing(id, e)
            .ok_or("This annotation has no marks or no view to reopen.")?;
        if !self.model.shown().contains(&m.part.as_str()) {
            self.set_focus(Some(m.part.clone()));
        }
        self.markup = Some(m);
        Ok(())
    }

    /// Post: the marks and the comment as one annotation, or the edited
    /// one's; the bar stays open, its marks kept, until the daemon answers.
    pub fn post_markup(&mut self) {
        let Some(m) = self.markup.as_mut() else {
            return;
        };
        m.commit_text();
        let Some(req) = m.request() else {
            m.note = Some("Type what should change: a mark says where, words say what.".into());
            return;
        };
        m.posting = true;
        m.note = None;
        self.send(req, Pending::MarkupPost);
    }

    /// The view marks the viewport draws on the glass (design 423, When):
    /// the Markup bar's, with the mark under way, in this window's colour;
    /// else the chosen annotation's, in its author's, at
    /// [`ryucad::redline::CLOSED_ALPHA`] once closed; else none, since flat
    /// marks seen from another view mislead.
    pub fn view_marks(&self) -> Option<crate::core::markup::ViewMarks> {
        use crate::core::markup::ViewMarks;
        if let Some(m) = &self.markup {
            let mut marks = m.marks.clone();
            marks.extend(m.under_way());
            return Some(ViewMarks {
                plane: m.plane(),
                marks,
                colour: ryucad::render::participant(&self.name),
                alpha: 1.0,
            });
        }
        let e = self.annotations.all.get(self.chosen_annotation.as_ref()?)?;
        let marks = e.marks.as_ref()?;
        // Drawn only over the part they were made on (#730), as pins are.
        if !self.model.shown().contains(&marks.part.as_str()) {
            return None;
        }
        Some(ViewMarks {
            plane: e.plane()?,
            marks: marks.list.clone(),
            colour: ryucad::render::participant(&e.by),
            alpha: if e.is_open() {
                1.0
            } else {
                ryucad::redline::CLOSED_ALPHA
            },
        })
    }

    /// The annotation the Markup bar edits, if it edits one: the bar
    /// draws its marks, so the listing's are left out.
    pub fn markup_editing(&self) -> Option<&str> {
        self.markup.as_ref().and_then(|m| m.editing.as_deref())
    }

    /// The face strokes the window draws (design 423): open annotations'
    /// when `pins` are drawn, but the one the bar edits, then the bar's,
    /// with the mark under way, in this window's colour.
    pub fn face_strokes(&self, pins: bool) -> Vec<crate::core::annotations::Stroke> {
        let shown = self.model.shown();
        let place = |on: &str| self.model.placed_on(on);
        let mut strokes = if pins {
            self.annotations
                .face_strokes(&shown, place, self.markup_editing())
        } else {
            Vec::new()
        };
        if let Some(m) = &self.markup {
            let mut marks = m.marks.clone();
            marks.extend(m.under_way());
            let marks = ryucad::redline::Marks {
                part: ryucad::doc::id::Id::parse(&m.part, "part").expect("a part's ID"),
                list: marks,
            };
            strokes.extend(crate::core::annotations::face_strokes_of(
                &marks,
                self.my_name(),
                &place,
            ));
        }
        strokes
    }

    /// Cancel (or Esc with nothing under way): the bar closes and its
    /// marks go.
    pub fn cancel_markup(&mut self) {
        self.markup = None;
    }

    /// Asks what importing the open import's file would do: `import` as
    /// a dry run, an update's when it replaces a file the project holds.
    pub fn plan_import(&mut self) {
        let Some(i) = self.import.as_mut() else {
            return;
        };
        let key = i.key();
        if key.0.is_empty() {
            return;
        }
        i.waiting = true;
        i.error = None;
        i.done = None;
        let req = import_request(&key.0, i.update.clone(), true, key.1);
        self.send(req, Pending::ImportPlan(key));
    }

    /// Imports the open import's file, as its plan said: the same
    /// `import` an agent sends, one logged, undoable change.
    pub fn run_import(&mut self) {
        let Some(i) = self.import.as_mut() else {
            return;
        };
        let key = i.key();
        i.waiting = true;
        i.error = None;
        let req = import_request(&key.0, i.update.clone(), false, key.1);
        self.send(req, Pending::Import(key));
    }

    /// Converts import `feature` of `part` to a healed solid, or with
    /// `reference` back to a reference (`import.convert`): one logged,
    /// undoable change, a long request the status line shows. A body
    /// healing refuses is the part's rebuild's to report.
    pub fn convert_import(&mut self, part: &str, feature: &str, reference: bool) {
        let req = Request::ImportConvert {
            part: part.to_owned(),
            feature: Some(feature.to_owned()),
            reference,
            expected_revision: self.model.parts.get(part).map(|p| p.revision.clone()),
        };
        self.send(
            req,
            Pending::ImportConvert(part.to_owned(), feature.to_owned()),
        );
    }

    /// Opens File > Export… on `part`, with the last export's choices.
    pub fn open_export(&mut self, part: &str) {
        let mut e = crate::core::export::Export::new(part, self.export_choices.clone());
        // DXF only for sheet metal; the last export's DXF doesn't carry
        // over to a part with none.
        e.sheet = self.sheet_solid(part, None).map(|k| k.1);
        if e.sheet.is_none() && e.choices.format == crate::core::export::Format::Dxf {
            e.choices.format = crate::core::export::Format::Step;
        }
        // The drawing the Drawings tile shows of this part, if any.
        e.drawing = self
            .drawings
            .sheet
            .as_ref()
            .filter(|s| self.drawings.open && s.part == part)
            .map(|s| (s.drawing.clone(), s.sheet.clone()));
        if e.drawing.is_none() && e.choices.format == crate::core::export::Format::Drawing {
            e.choices.format = crate::core::export::Format::Step;
        }
        self.export = Some(e);
    }

    /// Exports the open export's part: the daemon, on this machine,
    /// writes the file where the dialog says. The choices are kept for
    /// the next export.
    pub fn run_export(&mut self) {
        let Some(e) = self.export.as_mut() else {
            return;
        };
        // The format's extension, where the typed path lacks it (#388).
        let path = absolute(&e.saved_path());
        e.path = path.display().to_string();
        e.waiting = true;
        e.error = None;
        e.done = None;
        e.faults.clear();
        self.export_choices = e.choices.clone();
        let req = e.request(&path);
        self.send(req, Pending::Export);
    }

    /// Asks what the disk cache holds.
    pub fn read_cache(&mut self) {
        self.send(Request::CacheStats {}, Pending::Cache);
    }

    /// Empties the disk cache; the answer says what it holds now.
    pub fn clear_cache(&mut self) {
        self.send(Request::CacheClear {}, Pending::Cache);
    }

    /// Stores the token the shared cache is sent (`cache.login`), then
    /// reads the cache's state again.
    pub fn login_cache(&mut self, token: &str) {
        let req = Request::CacheLogin {
            token: token.trim().to_owned(),
        };
        self.send(req, Pending::CacheToken);
    }

    /// Removes the stored token (`cache.logout`).
    pub fn logout_cache(&mut self) {
        self.send(Request::CacheLogout {}, Pending::CacheToken);
    }

    fn rediagnose(&mut self) {
        if self.doctor_waiting {
            self.doctor_again = true;
        } else if self.doctor.is_some() {
            self.diagnose();
        }
    }

    /// Applies repairs from the last report (`doctor.repair`); the answer
    /// brings the new report.
    pub fn repair(&mut self, repairs: Vec<String>, params: serde_json::Map<String, Value>) {
        self.doctor_waiting = true;
        self.send(Request::DoctorRepair { repairs, params }, Pending::Doctor);
    }

    /// Reads the project's Git history again.
    pub fn load_history(&mut self) {
        self.history_at = Some(Instant::now());
        self.history_waiting = true;
        let req = Request::HistoryLog {
            part: None,
            limit: Some(100),
        };
        self.send(req, Pending::History);
    }

    /// Reads the history again after the project changed, once it was
    /// asked for: at most one reading on its way, and one more after it.
    fn reload_history(&mut self) {
        if self.history_waiting {
            self.history_again = true;
        } else if self.history.is_some() {
            self.load_history();
        }
    }

    /// The History tab is shown: read the history again when the last
    /// reading is older than [`HISTORY_EVERY`]. Returns when to look again.
    pub fn history_shown(&mut self, now: Instant) -> Duration {
        let due = self.history_at.map_or(now, |t| t + HISTORY_EVERY);
        if now >= due && !self.history_waiting {
            self.load_history();
            return HISTORY_EVERY;
        }
        due.saturating_duration_since(now)
            .max(Duration::from_millis(100))
    }

    /// Reads the Branches tab's worktrees again (`worktree.list`); their
    /// previews follow, one at a time.
    pub fn load_branches(&mut self) {
        self.branches_at = Some(Instant::now());
        self.branches_waiting = true;
        self.send(Request::WorktreeList {}, Pending::Branches);
    }

    /// Reads the worktrees again after this project changed, once the tab
    /// has read them: this worktree's state moved, so its previews are
    /// made again.
    fn reload_branches(&mut self) {
        if self.branches.list.is_some() && !self.branches_waiting {
            self.load_branches();
        }
    }

    /// The Branches tab is shown: read the worktrees again when the last
    /// reading is older than [`crate::core::branches::BRANCHES_EVERY`].
    /// Returns when to look again.
    pub fn branches_shown(&mut self, now: Instant) -> Duration {
        let every = crate::core::branches::BRANCHES_EVERY;
        let due = self.branches_at.map_or(now, |t| t + every);
        if now >= due && !self.branches_waiting {
            self.load_branches();
            return every;
        }
        due.saturating_duration_since(now)
            .max(Duration::from_millis(100))
    }

    /// Asks for the next merge preview the tab lacks, if none is on its
    /// way.
    fn ask_preview(&mut self) {
        let Some((path, key)) = self.branches.next_preview() else {
            return;
        };
        self.branches.previewing = Some((path.clone(), key.clone()));
        let rev = crate::core::branches::compare_rev(&path);
        self.send(Request::MergePreview { rev }, Pending::Preview(path, key));
    }

    /// Opens the worktree at `path` in a new window of this app, or brings
    /// the windows already on it to the front: what `ryucad open` does.
    pub fn open_worktree(&mut self, path: &str) {
        let dir = Path::new(path);
        let app = std::env::current_exe().ok();
        if let Err(e) = ryucad::client::open_app(dir, Some(dir), app.as_deref()) {
            self.fail(e);
        }
    }

    /// Reads the dependencies and what is outdated again, once at a time:
    /// one asked for meanwhile follows.
    pub fn load_deps(&mut self) {
        if self.deps_waiting {
            self.deps_again = true;
            return;
        }
        self.deps_waiting = true;
        self.send(Request::DepsList {}, Pending::DepsList);
        self.send(Request::DepsOutdated {}, Pending::DepsOutdated);
    }

    /// Fetches every dependency, or those named.
    pub fn fetch_deps(&mut self, names: Vec<String>) {
        self.deps.busy = true;
        self.deps.failed = None;
        self.send_aside(Request::DepsFetch { names }, Pending::DepsWrite);
    }

    /// Adds the dependency typed into the tab.
    pub fn add_dep(&mut self) {
        let d = &self.deps;
        let name = d.add_name.trim();
        let req = Request::DepsAdd {
            url: d.add_url.trim().to_owned(),
            path: d.add_path.trim().to_owned(),
            name: (!name.is_empty()).then(|| name.to_owned()),
            expected_revision: d.revision.clone(),
        };
        self.deps.busy = true;
        self.deps.failed = None;
        self.send_aside(req, Pending::DepsWrite);
    }

    /// Removes dependency `name`.
    pub fn remove_dep(&mut self, name: &str) {
        let req = Request::DepsRemove {
            name: name.to_owned(),
            expected_revision: self.deps.revision.clone(),
        };
        self.deps.busy = true;
        self.deps.failed = None;
        self.send(req, Pending::DepsWrite);
    }

    /// Update's first step: a dry run (fetching first) that says what
    /// would move and what would fail, shown before anything is written.
    pub fn plan_update(&mut self, names: Vec<String>, part: Option<String>) {
        let req = Request::DepsUpdate {
            names: names.clone(),
            part: part.clone(),
            to: None,
            offline: false,
            dry_run: true,
            expected_revision: None,
        };
        self.deps.busy = true;
        self.deps.failed = None;
        self.deps.update = None;
        self.send_aside(req, Pending::DepsDry(names, part));
    }

    /// Writes the update its dry run showed, from what that fetched.
    pub fn run_update(&mut self) {
        let Some(u) = self.deps.update.take() else {
            return;
        };
        let req = Request::DepsUpdate {
            names: u.names,
            part: u.part,
            to: None,
            offline: true,
            dry_run: false,
            expected_revision: self.deps.revision.clone(),
        };
        self.deps.busy = true;
        self.send(req, Pending::DepsWrite);
    }

    /// Opens Place from dependency… on the shown assembly.
    pub fn open_picker(&mut self) {
        let Some(asm) = self.shown_assembly() else {
            return;
        };
        self.picker = Some(Picker::new(&asm));
    }

    /// The picker's dependency chosen: its released parts are read.
    pub fn pick_dependency(&mut self, name: &str) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        p.dependency = Some(name.to_owned());
        p.parts.clear();
        p.reading = true;
        let req = Request::DepsParts {
            name: name.to_owned(),
            part: None,
        };
        self.send(req, Pending::DepsParts(name.to_owned()));
    }

    /// Places the picker's part at its label, at the assembly's origin.
    pub fn place_picked(&mut self) {
        let Some(p) = self.picker.as_ref() else {
            return;
        };
        let (Some(of), Some(rev)) = (p.of(), p.label.clone()) else {
            return;
        };
        let asm = p.assembly.clone();
        let expected = self.part(&asm).map(|x| x.revision.clone());
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        p.placing = true;
        p.error = None;
        let req = Request::FeatureAdd {
            part: asm,
            ty: "instance".to_owned(),
            params: Default::default(),
            placement: None,
            fields: ryucad::proto::FeatureFields {
                of: Some(of),
                rev: Some(rev),
                ..Default::default()
            },
            id: None,
            position: Default::default(),
            expected_revision: expected,
        };
        self.send(req, Pending::DepsPlace);
    }

    /// Settles a merge conflict by `choice` (`ours`, `theirs`, or `repair`
    /// with `repair`), against the revision of its part the window last
    /// saw. The answer brings the new status; a failure is kept in the
    /// merge's `failed`.
    pub fn resolve(&mut self, conflict: &str, choice: &str, repair: Option<&str>) {
        let Some(m) = &mut self.merge else { return };
        let Some(c) = m.conflict(conflict) else {
            return;
        };
        let req = crate::core::merge::resolution(c, choice, repair, &self.model.parts);
        m.resolving = true;
        m.failed = None;
        self.send(req, Pending::Resolve);
    }

    /// Puts this window into a compare with `rev`, or (with `None`) out of
    /// one, through `compare.set`: the compare starts when the session
    /// says so, as one an agent starts does.
    pub fn set_compare(&mut self, rev: Option<String>) {
        let req = Request::CompareSet {
            off: rev.is_none(),
            rev,
            participant: Some(self.link.participant.clone()),
        };
        self.send(req, Pending::Quiet);
    }

    /// Shows `part` in this window, or (with `None`) the project's model,
    /// through `view.focus`: the window follows when the session says so,
    /// as it does when an agent sets it.
    pub fn set_focus(&mut self, part: Option<String>) {
        let req = Request::ViewFocus {
            project: part.is_none(),
            part,
            participant: Some(self.link.participant.clone()),
        };
        self.send(req, Pending::Quiet);
    }

    /// Cuts with the planes this window's participant has; the scene and
    /// picking read them from [`Workspace::sections`].
    fn follow_sections(&mut self) {
        let planes = self
            .myself()
            .map(|m| m.sections.clone())
            .unwrap_or_default();
        if self.sections.follow(&planes) {
            self.model.generation += 1;
        }
    }

    /// Cuts this window with exactly `planes` (`section.set`): drawn at
    /// once, as a drag left them, and written for the session.
    pub fn set_sections(&mut self, planes: Vec<ryucad::section::Plane>) {
        let req = crate::core::section::request(&planes, &self.link.participant);
        self.sections.planes = planes;
        self.model.generation += 1;
        self.send(req, Pending::Sections);
    }

    /// Adds the plane `plane` names (`xy`, `yz`, `zx` or a planar face's
    /// reference) to this window's.
    pub fn add_section(&mut self, plane: &str) {
        let req = crate::core::section::add(plane, &self.link.participant);
        self.send(req, Pending::Sections);
    }

    /// The explode this window's participant shows, its list read when it
    /// is another assembly's, and where it moves occurrences.
    fn follow_explode(&mut self) {
        let shown = self.myself().and_then(|m| m.explode.clone());
        if self.explodes.shown != shown {
            self.explodes.shown = shown;
            self.explodes.sliding = None;
        }
        let part = self
            .explodes
            .shown
            .as_ref()
            .map(|s| s.part.clone())
            .or_else(|| self.shown_assembly());
        if part.is_some() && part != self.explodes.part {
            self.explodes.part = part;
            self.explodes.list.clear();
            self.read_explodes();
        }
        self.sync_explode();
    }

    /// Reads the explodes of the assembly they are of (`explode.list`).
    pub fn read_explodes(&mut self) {
        if let Some(p) = self.explodes.part.clone() {
            let req = Request::ExplodeList {
                part: Some(p.clone()),
            };
            self.send(req, Pending::Explodes(p));
        }
    }

    /// Moves the model's occurrences where the explode drawn puts them.
    pub fn sync_explode(&mut self) {
        let moved = self
            .explodes
            .part
            .clone()
            .map(|p| (p.clone(), self.explodes.offsets(&p)));
        self.model.explode(moved);
    }

    /// Shows explode `explode` of the explodes' assembly in this window,
    /// `fraction` of the way (`explode.show`); with no explode, moves the
    /// one shown; `off` shows none.
    pub fn show_explode(&mut self, explode: Option<&str>, fraction: Option<f64>, off: bool) {
        let req = Request::ExplodeShow {
            part: explode.and(self.explodes.part.clone()),
            explode: explode.map(str::to_owned),
            fraction,
            off,
            participant: Some(self.link.participant.clone()),
        };
        self.send(req, Pending::ExplodeShown);
    }

    /// Draws the explode `fraction` of the way while the slider moves;
    /// written when let go ([`Workspace::show_explode`]).
    pub fn slide_explode(&mut self, fraction: f64) {
        self.explodes.sliding = Some(fraction.clamp(0.0, 1.0));
        self.sync_explode();
    }

    /// Starts a new step of the selected occurrences: their arrows show
    /// at their middle, to drag.
    pub fn start_step(&mut self) {
        let Some(part) = self.explodes.part.clone() else {
            return;
        };
        let selection = self
            .myself()
            .map(|m| m.selection.clone())
            .unwrap_or_default();
        let mut instances: Vec<String> = Vec::new();
        for r in &selection {
            if let Some((asm, path)) = self.model.occurrence_path(r)
                && asm == part
                && !instances.contains(&path)
            {
                instances.push(path);
            }
        }
        if instances.is_empty() {
            return;
        }
        let middles = self.model.middles(&part);
        let offsets = self.explodes.offsets(&part);
        let at: Vec<[f64; 3]> = instances
            .iter()
            .filter_map(|p| {
                let m = *middles.get(p)?;
                Some(offsets.get(p).map_or(m, |o| o.apply(m)))
            })
            .collect();
        let n = at.len().max(1) as f64;
        let centre = [0, 1, 2].map(|k| at.iter().map(|p| p[k]).sum::<f64>() / n);
        self.explodes.adding = Some(crate::core::explode::Adding {
            instances,
            centre,
            axis: None,
            distance: 0.0,
            from: [0.0; 2],
        });
    }

    /// Writes the step being made after the shown explode's steps, or as
    /// the first of a new explode named [`Explodes::name`].
    pub fn finish_step(&mut self) {
        let Some(a) = self.explodes.adding.take() else {
            return;
        };
        let Some(part) = self.explodes.part.clone() else {
            return;
        };
        let unit = self.unit();
        let Some(step) = a.written(unit.as_str(), unit.factor()) else {
            return;
        };
        let current = self.explodes.current(&part).map(|(e, _)| e.clone());
        let id = match &current {
            Some(e) => e.id.clone(),
            None if !self.explodes.name.is_empty() => self.explodes.name.clone(),
            None => return,
        };
        let req = crate::core::explode::save_with(
            &part,
            &id,
            current.as_ref(),
            step,
            self.explodes.revision.clone(),
        );
        self.sync_explode();
        self.send(req, Pending::ExplodeSaved(part, Some(id)));
    }

    /// Takes step `k` out of the shown explode, or deletes the explode
    /// with its last step.
    pub fn remove_step(&mut self, k: usize) {
        let Some(part) = self.explodes.part.clone() else {
            return;
        };
        let Some((e, _)) = self.explodes.current(&part) else {
            return;
        };
        let e = e.clone();
        let req = if e.steps.as_array().is_some_and(|s| s.len() <= 1) {
            Request::ExplodeDelete {
                part: part.clone(),
                explode: e.id.clone(),
                expected_revision: self.explodes.revision.clone(),
            }
        } else {
            crate::core::explode::save_without(&part, &e, k, self.explodes.revision.clone())
        };
        self.send(req, Pending::ExplodeSaved(part, Some(e.id)));
    }

    /// Shows what this window's participant's focus says, and hides what
    /// its hidden set says.
    fn follow_focus(&mut self) {
        let want = self.myself().and_then(|m| m.focus.clone());
        if self.model.focus != want {
            if let Some(f) = &want {
                let f = f.clone();
                self.load_part(&f);
            }
            self.model.focus = want;
            self.model.generation += 1;
            self.refocused = true;
            // The BOM tab shows what the window shows, and so does the
            // appearance.
            self.reload_bom();
            self.read_appearance();
        }
        let hidden: std::collections::BTreeSet<String> = self
            .myself()
            .map(|m| m.hidden.iter().cloned().collect())
            .unwrap_or_default();
        if self.model.hidden != hidden {
            self.model.hidden = hidden;
            self.model.generation += 1;
        }
    }

    /// Changes what this window hides (`visibility.set`): everything
    /// shown first with `all`, then everything else in their parts hidden
    /// for `isolate`, then `show`, then `hide`. An agent can do the same
    /// to this window.
    pub fn set_visibility(
        &mut self,
        hide: Vec<String>,
        show: Vec<String>,
        isolate: Vec<String>,
        all: bool,
    ) {
        let req = Request::VisibilitySet {
            hide,
            show,
            isolate,
            all,
            participant: Some(self.link.participant.clone()),
        };
        self.send(req, Pending::Hidden);
    }

    /// Starts, changes or ends the compare to match this window's
    /// participant.
    fn follow_compare(&mut self) {
        // While the person's draft is drawn, it is painted against the
        // agent's model (the working files); else the participant's own
        // compare. A draft's compare isn't sent to the session.
        let want = match self.model_rev() {
            Some(draft) => Some(("working".to_owned(), Some(draft))),
            None => self
                .myself()
                .and_then(|m| m.compare.clone())
                .map(|r| (r, None)),
        };
        match (&self.compare, want) {
            (Some(c), Some((rev, to))) if c.rev == rev && c.to == to => {}
            (_, Some((rev, to))) => {
                self.compare = Some(Compare::between(&rev, to));
                self.model.generation += 1;
                self.ask_diff();
                self.reload_bom();
            }
            (Some(_), None) => {
                self.compare = None;
                self.model.generation += 1;
                self.reload_bom();
            }
            (None, None) => {}
        }
    }

    /// Asks for the compare's diff, from its revision to the working
    /// files; with one on its way, once that one comes.
    fn ask_diff(&mut self) {
        let Some(c) = &self.compare else { return };
        if self.compare_waiting {
            self.compare_again = true;
            return;
        }
        let req = Request::Diff {
            from: Some(c.rev.clone()),
            to: c.to.clone(),
            part: None,
            model: true,
            positions: false,
            all: false,
        };
        self.compare_asked += 1;
        self.compare_waiting = true;
        self.compare_again = false;
        let n = self.compare_asked;
        self.send(req, Pending::Compare(n));
    }

    /// Fetches the ghosts the compare wants and doesn't have, one body
    /// per request, at the commit the diff was made from.
    fn fetch_ghosts(&mut self) {
        let Some(c) = &mut self.compare else { return };
        let rev = c.commit.clone().unwrap_or_else(|| c.rev.clone());
        let asked: Vec<(BodyKey, String, String)> = c
            .ask()
            .into_iter()
            .map(|(key, hash)| {
                let feature = c.ghost_request(&key);
                (key, hash, feature)
            })
            .collect();
        for (key, hash, feature) in asked {
            let req = Request::MeshGet {
                part: key.0.clone(),
                feature: Some(feature),
                features: Vec::new(),
                quality: None,
                transfer: Some(Transfer::Compact),
                rev: Some(rev.clone()),
                flat: None,
            };
            self.send(req, Pending::Ghost(key, hash));
        }
    }

    /// Sends `req`, as the window's mode allows: as it is, as a proposed
    /// change (`annotation.propose`, its answer the request's own), or
    /// not at all, its refusal answered where its answer would have gone
    /// (a form's error, else an error card), saying which mode allows it.
    fn send(&mut self, req: Request, why: Pending) {
        self.send_with(req, why, Proposal::default());
    }

    /// [`Workspace::send`], with what a proposal of it says: its comment,
    /// and whether it is rough (a freehand drag's).
    fn send_with(&mut self, req: Request, why: Pending, proposal: Proposal) {
        use crate::core::modes::{Gate, gate};
        match gate(self.mode, &req) {
            Gate::Send => self.send_now(req, why),
            Gate::Propose => {
                let request = json!({"method": req.method(), "params": req.params()});
                let rough = proposal.rough;
                let wrapped = Request::AnnotationPropose {
                    request,
                    comment: proposal.comment,
                    pins: Vec::new(),
                    rough,
                    author: None,
                };
                self.send_now(wrapped, Pending::Proposal(Box::new(why), rough));
            }
            Gate::Refuse(because) => {
                let method = req.method();
                let e = Error::new(Code::ParticipantMode, format!("{method}: {because}"))
                    .with("mode", self.mode.as_str())
                    .with("method", &method);
                self.answer(why, Err(e));
            }
        }
    }

    /// What a control that writes by `method` does in the window's mode
    /// (see `core::modes::control`).
    pub fn control(&self, method: &str) -> crate::core::modes::Gate {
        crate::core::modes::control(self.mode, method)
    }

    fn send_now(&mut self, req: Request, why: Pending) {
        let method = req.method();
        if !ryucad::daemon::LONG.contains(&method.as_str()) {
            let ticket = self.link.send(req);
            self.pending.insert(ticket, why);
            return;
        }
        // A long request: its progress for the status line, and a token
        // its Cancel names.
        let part = req.params()["part"].as_str().map(str::to_owned);
        let token = self.progress.token();
        let ask = Ask {
            progress: true,
            token: Some(token.clone()),
        };
        let ticket = self.link.send_job(req, ask);
        let label = crate::core::progress::label(&method, part.as_deref());
        self.progress
            .started(ticket, &token, &method, label, Instant::now());
        self.pending.insert(ticket, why);
    }

    /// [`Workspace::send`] on the link's second connection: for requests
    /// that may wait on another repository. Long, so the status line shows
    /// their progress and Cancel.
    fn send_aside(&mut self, req: Request, why: Pending) {
        let method = req.method();
        let token = self.progress.token();
        let ask = Ask {
            progress: true,
            token: Some(token.clone()),
        };
        let ticket = self.link.send_aside(req, ask);
        let label = crate::core::progress::label(&method, None);
        self.progress
            .started(ticket, &token, &method, label, Instant::now());
        self.pending.insert(ticket, why);
    }

    /// Cancels the long request the status line shows: it stops at its
    /// next boundary and answers `cancelled`.
    pub fn cancel(&mut self) {
        if let Some(token) = self.progress.cancel() {
            self.link.cancel(&token);
        }
    }

    /// Tries the lost daemon again now, not when the backoff says.
    pub fn retry_now(&mut self) {
        if let Some(r) = &mut self.retry
            && r.trying.is_none()
        {
            r.at = Instant::now();
            r.delay = RETRY_FIRST;
        }
        (self.wake)();
    }

    /// When something feedback shows is due to change: a long request's
    /// bar to show, a toast to go.
    pub fn feedback_due(&self, now: Instant) -> Option<Duration> {
        match (self.progress.due(now), self.toasts.due(now)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// This window's participant ID.
    pub fn me(&self) -> &str {
        &self.link.participant
    }

    /// Puts the window in `mode`: at once here, and for the daemon, which
    /// refuses a human's writes their mode doesn't allow. Sent on the
    /// request connection, so every request after it is judged in it.
    pub fn set_mode(&mut self, mode: crate::core::modes::Mode) {
        if mode == self.mode {
            return;
        }
        let was = std::mem::replace(&mut self.mode, mode);
        self.send(Request::ParticipantMode { mode }, Pending::Mode(was));
        if mode != crate::core::modes::Mode::Annotate {
            self.show_agent = false;
        }
        self.model.generation += 1;
        self.follow_draft();
    }

    /// This window's person's draft as a revision, while they have one in
    /// Annotate: `draft:<name>`.
    fn draft_rev(&self) -> Option<String> {
        (self.mode == crate::core::modes::Mode::Annotate
            && !self.annotations.draft(&self.name).is_empty())
        .then(|| format!("{}{}", ryucad::session::DRAFT, self.name))
    }

    /// The revision the window's model is read at: the draft, unless the
    /// person looks at the agent's model or compares with a revision;
    /// else the working files.
    pub fn model_rev(&self) -> Option<String> {
        let comparing = self.myself().is_some_and(|m| m.compare.is_some());
        self.draft_rev().filter(|_| !self.show_agent && !comparing)
    }

    /// The revision forms read at: the draft whenever there is one, since
    /// proposals run over it (and are guarded by its revisions).
    pub fn form_rev(&self) -> Option<String> {
        self.draft_rev()
    }

    /// Annotate's Show agent's model (`true`), or the person's suggestions.
    pub fn set_show_agent(&mut self, on: bool) {
        self.show_agent = on;
        self.follow_draft();
    }

    /// Reads the model again when what is drawn changed: the mode, Show
    /// agent's model, a compare, or the person's draft (a suggestion made,
    /// applied, closed or gone stale); and follows the compare.
    fn follow_draft(&mut self) {
        let now = (self.model_rev(), self.annotations.draft(&self.name));
        if now != self.drawn {
            let rev_moved = now.0 != self.drawn.0 || now.0.is_some();
            self.drawn = now;
            if rev_moved {
                let loaded: Vec<String> = self.model.parts.keys().cloned().collect();
                for id in loaded {
                    self.loading.remove(&id);
                    self.ask_part(&id);
                }
                for part in self.model.clear_sketches() {
                    self.fetch_sketches(&part);
                }
                self.read_appearance();
            }
            if let Some(f) = &self.form {
                let (part, id) = (f.part.clone(), f.feature.clone());
                self.read_form(part, id, None);
            }
        }
        self.follow_compare();
    }

    /// This window's participant name, as annotations name their author.
    pub fn my_name(&self) -> &str {
        &self.name
    }

    /// A comment pinned to `pins`, seen through `camera`.
    pub fn create_annotation(
        &mut self,
        pins: Vec<ryucad::proto::PinArg>,
        comment: String,
        camera: Option<Camera>,
    ) {
        let req = Request::AnnotationCreate {
            pins,
            comment,
            camera: camera.as_ref().map(camera_arg),
            with_view: false,
            marks: None,
            view: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    pub fn reply_annotation(&mut self, id: &str, text: String) {
        let req = Request::AnnotationReply {
            id: id.to_owned(),
            text,
            expect: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    pub fn edit_annotation(&mut self, id: &str, comment: String) {
        let req = Request::AnnotationEdit {
            id: id.to_owned(),
            comment: Some(comment),
            pins: None,
            marks: None,
            no_marks: false,
            expect: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    pub fn delete_annotation(&mut self, id: &str) {
        let req = Request::AnnotationDelete {
            id: id.to_owned(),
            expect: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    /// Takes an annotation's change into the working files (Direct edit).
    pub fn apply_annotation(&mut self, id: &str) {
        let req = Request::AnnotationApply {
            id: id.to_owned(),
            keep_open: false,
            expect: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    /// Chooses an annotation (or none): its view restored, and with a
    /// change, a compare against `annotation:<id>`; leaving one compared
    /// stops that compare.
    pub fn choose_annotation(&mut self, id: Option<&str>) {
        self.chosen_annotation = id.map(str::to_owned);
        let entry = id.and_then(|i| self.annotations.all.get(i)).cloned();
        let comparing_one = self
            .compare
            .as_ref()
            .is_some_and(|c| c.rev.starts_with(ryucad::session::ANNOTATION));
        match entry {
            Some(e) => {
                if let Some(c) = e.view {
                    self.looked = Some(c);
                }
                match e.change {
                    Some(_) => {
                        self.set_compare(Some(format!("{}{}", ryucad::session::ANNOTATION, e.id)))
                    }
                    None if comparing_one => self.set_compare(None),
                    None => {}
                }
            }
            None if comparing_one => self.set_compare(None),
            None => {}
        }
        self.model.generation += 1;
    }

    /// This window as the session knows it.
    pub fn myself(&self) -> Option<&Person> {
        self.people.get(&self.link.participant)
    }

    /// Changes this window's selection. It shows at once; the daemon's
    /// answer (and everyone else's view of it) follows.
    pub fn select(&mut self, entities: Vec<String>, mode: SelectMode) {
        let me = self.link.participant.clone();
        if let Some(p) = self.people.all.get_mut(&me) {
            match mode {
                SelectMode::Replace => p.selection.clone_from(&entities),
                SelectMode::Add => {
                    for e in &entities {
                        if !p.selection.contains(e) {
                            p.selection.push(e.clone());
                        }
                    }
                }
                SelectMode::Remove => p.selection.retain(|e| !entities.contains(e)),
            }
            self.people.generation += 1;
        }
        self.send(Request::SelectionSet { entities, mode }, Pending::Quiet);
    }

    /// Opens the form for a feature (or closes it, with `None`); a
    /// dimension's form closes.
    /// Chooses a feature in the tree (or none): what it made is read, to
    /// be painted.
    pub fn choose(&mut self, feature: Option<BodyKey>) {
        self.made = None;
        self.swept = None;
        self.swept_at = None;
        self.made_for.clone_from(&feature);
        if let Some(key) = feature {
            self.ask_swept(&key);
            self.ask_sheet_paint(&key);
            self.ask_made(key);
        }
    }

    /// Reads `key`'s sheet metal table (the part and the solid, `""` for
    /// its one sheet metal solid) unless it was read at the part's
    /// revision or is on its way: the Part tile's sheet metal section asks
    /// every frame it shows.
    pub fn show_sheet(&mut self, key: &BodyKey) {
        let Some(revision) = self.revision_of(&key.0) else {
            return;
        };
        if self.sheets.want_table(key, &revision) {
            let req = Request::SheetGet {
                part: key.0.clone(),
                solid: (!key.1.is_empty()).then(|| key.1.clone()),
                rev: self.model_rev(),
            };
            self.send(req, Pending::SheetTable(key.clone(), revision));
        }
    }

    /// The sheet metal solid the Part tile's section shows of `part`: the
    /// chosen feature's solid when it is in `part`, else the solid a base
    /// flange founds, else `""` (the part's one sheet metal solid, as the
    /// daemon finds it); `None` for a part with no sheet metal feature.
    pub fn sheet_solid(&self, part: &str, chosen: Option<&BodyKey>) -> Option<BodyKey> {
        let p = self.model.parts.get(part)?;
        let sheet = |f: &crate::core::model::Feature| f.sheet.is_some();
        if !p.features.iter().any(sheet) {
            return None;
        }
        if let Some(k) = chosen.filter(|k| k.0 == part)
            && p.feature(&k.1).is_some_and(sheet)
            && let Some(s) = self.model.solid_of(k)
        {
            return Some(s);
        }
        let founder = p
            .features
            .iter()
            .find(|f| f.sheet.as_ref().is_some_and(|s| s.kind == "base_flange"))
            .map_or_else(String::new, |f| f.id.clone());
        Some((part.to_owned(), founder))
    }

    /// Reads `key`'s flat bend lines, as [`Workspace::show_sheet`] its
    /// table.
    fn ask_flat_lines(&mut self, key: &BodyKey) {
        let Some(revision) = self.revision_of(&key.0) else {
            return;
        };
        if self.sheets.want_flat(key, &revision) {
            let req = Request::FlatGet {
                part: key.0.clone(),
                solid: (!key.1.is_empty()).then(|| key.1.clone()),
                rev: self.model_rev(),
                fixed: None,
                bends: Vec::new(),
            };
            self.send(req, Pending::FlatLines(key.clone(), revision));
        }
    }

    /// What a chosen unfold or refold paints needs: its target's table,
    /// and an unfold's flat too.
    fn ask_sheet_paint(&mut self, key: &BodyKey) {
        let Some(info) = self
            .model
            .parts
            .get(&key.0)
            .and_then(|p| p.feature(&key.1))
            .filter(|f| f.error.is_none())
            .and_then(|f| f.sheet.clone())
        else {
            return;
        };
        let Some(target) = info.target else {
            return;
        };
        let solid = (key.0.clone(), target);
        match info.kind.as_str() {
            "unfold" => {
                self.show_sheet(&solid);
                self.ask_flat_lines(&solid);
            }
            "refold" => self.show_sheet(&solid),
            _ => {}
        }
    }

    /// The faces and edges a chosen unfold or refold paints, from what
    /// the window has read ([`crate::core::sheet::painted`]).
    pub fn sheet_paint(&self, key: &BodyKey) -> (Vec<String>, Vec<String>) {
        let none = (Vec::new(), Vec::new());
        let Some(p) = self.model.parts.get(&key.0) else {
            return none;
        };
        let Some(info) = p.feature(&key.1).and_then(|f| f.sheet.as_ref()) else {
            return none;
        };
        let Some(target) = &info.target else {
            return none;
        };
        let solid = (key.0.clone(), target.clone());
        let table = match self.sheets.table(&solid, &p.revision) {
            Some(crate::core::sheet::Shown::Table(t)) => Some(t),
            _ => None,
        };
        let flat = self
            .sheets
            .flat(&solid, &p.revision)
            .and_then(|f| f.as_ref().ok());
        let unfold_only = info
            .unfold
            .as_ref()
            .and_then(|u| p.feature(u))
            .and_then(|f| f.sheet.as_ref())
            .map(|u| u.only.clone())
            .unwrap_or_default();
        crate::core::sheet::painted(&key.1, info, &unfold_only, table, flat)
    }

    /// Turns the flat view on, for `solid` (a part and its solid, `""` for
    /// the part's one sheet metal solid), or off.
    pub fn show_flat_view(&mut self, solid: Option<BodyKey>) {
        self.model.show_flat(None);
        self.flat_view = None;
        let Some(key) = solid else {
            return;
        };
        let Some(revision) = self.revision_of(&key.0) else {
            return;
        };
        self.flat_view = Some(crate::core::flat::FlatView {
            part: key.0.clone(),
            solid: key.1.clone(),
            revision: revision.clone(),
            lines: None,
            error: None,
            turned: false,
        });
        self.read_flat_view();
    }

    /// Reads the flat view's bend lines and body again, at its part's
    /// revision.
    fn read_flat_view(&mut self) {
        let Some(v) = &self.flat_view else {
            return;
        };
        let key = (v.part.clone(), v.solid.clone());
        let revision = v.revision.clone();
        self.ask_flat_lines(&key);
        let req = Request::MeshGet {
            part: key.0.clone(),
            feature: None,
            features: Vec::new(),
            quality: None,
            transfer: Some(Transfer::Compact),
            rev: self.model_rev(),
            flat: Some(key.1.clone()),
        };
        self.send(req, Pending::FlatMesh(key, revision));
    }

    /// Turns the camera to the flat view, once its body and lines have
    /// come: along the pattern's `z`, from its viewer side (where its bend
    /// lines lie), as `render --flat` looks at it; the view frames it.
    fn turn_to_flat(&mut self) {
        let Some(v) = self.flat_view.as_mut().filter(|v| !v.turned) else {
            return;
        };
        let (Some(l), Some((_, body))) = (&v.lines, &self.model.flat) else {
            return;
        };
        let points: Vec<[f64; 3]> = match &body.mesh {
            Ok(m) => m
                .positions
                .iter()
                .map(|p| [0, 1, 2].map(|k| m.origin[k] + f64::from(p[k])))
                .collect(),
            Err(_) => l.lines.iter().flat_map(|b| [b.from, b.to]).collect(),
        };
        v.turned = true;
        self.looked = Some(l.camera(&points, 1.0));
    }

    /// The solid View > Flat pattern lays flat: the chosen feature's
    /// solid, else the one part shown (not an assembly), its one sheet
    /// metal solid; `None` where neither is.
    pub fn flat_candidate(&self, chosen: Option<&BodyKey>) -> Option<BodyKey> {
        let shown = self.model.shown();
        if let Some(k) = chosen
            && shown.contains(&k.0.as_str())
            && let Some(s) = self.model.solid_of(k)
            && self.model.parts.get(&k.0).is_some_and(|p| !p.assembly)
        {
            return Some(s);
        }
        match shown.as_slice() {
            [one] if self.model.parts.get(*one).is_some_and(|p| !p.assembly) => {
                Some(((*one).to_owned(), String::new()))
            }
            _ => None,
        }
    }

    /// Reads what a sweep or loft that built is built from, to draw it;
    /// anything else draws nothing of the kind.
    fn ask_swept(&mut self, key: &BodyKey) {
        let Some(p) = self.model.parts.get(&key.0) else {
            return;
        };
        self.swept_at = Some(p.revision.clone());
        let Some(f) = p.feature(&key.1) else {
            return;
        };
        if !crate::core::sweeps::drawn(f.ty.as_deref()) || f.error.is_some() {
            return;
        }
        let revision = p.revision.clone();
        let req = Request::SweepGet {
            part: key.0.clone(),
            feature: key.1.clone(),
            quality: None,
            rev: self.model_rev(),
        };
        self.send(req, Pending::Sweep(key.clone(), revision));
    }

    /// Reads the faces `key` made, unless it is a sketch: it makes none. A
    /// mate's are the faces its connectors are on, known already.
    fn ask_made(&mut self, key: BodyKey) {
        let Some(p) = self.model.parts.get(&key.0) else {
            return;
        };
        if let Some(m) = p.feature(&key.1).and_then(|f| f.mate.as_ref()) {
            let mut faces: std::collections::BTreeMap<BodyKey, std::collections::BTreeSet<String>> =
                std::collections::BTreeMap::new();
            for c in [&m.a, &m.b] {
                if let Some((body, name)) = crate::core::model::MateInfo::face_of(c) {
                    faces.entry((key.0.clone(), body)).or_default().insert(name);
                }
            }
            self.made = Some(Made {
                feature: key.clone(),
                revision: p.revision.clone(),
                faces,
                ..Made::default()
            });
            self.model.generation += 1;
            return;
        }
        if p.feature(&key.1).is_none_or(|f| f.sketch.is_some()) {
            return;
        }
        let revision = p.revision.clone();
        // A tweak makes nothing an origin names: what it modified comes
        // from inspect of the feature itself.
        let tweak = p.feature(&key.1).is_some_and(|f| f.tweak.is_some());
        let req = Request::Inspect {
            part: key.0.clone(),
            feature: tweak.then(|| key.1.clone()),
            entity: None,
            rev: self.model_rev(),
        };
        self.send(req, Pending::Made(key, revision));
    }

    pub fn open_form(&mut self, feature: Option<(String, String)>) {
        self.form = None;
        self.dimension = None;
        self.dimension_for = None;
        self.form_for.clone_from(&feature);
        if let Some((part, id)) = feature {
            self.read_form(part, id, None);
        }
    }

    fn read_form(&mut self, part: String, id: String, before: Option<Box<Form>>) {
        let req = Request::FeatureShow {
            part: part.clone(),
            id: id.clone(),
            rev: self.form_rev(),
        };
        self.send(req, Pending::Form(part, id, before));
    }

    /// Opens the form for a sketch's dimension (or closes it, with
    /// `None`); the feature's form closes.
    pub fn open_dimension(&mut self, dimension: Option<(String, String, String)>) {
        self.dimension = None;
        self.dimension_for.clone_from(&dimension);
        if let Some((part, sketch, constraint)) = dimension {
            self.form = None;
            self.form_for = None;
            self.read_dimension(part, sketch, constraint, None);
        }
    }

    fn read_dimension(
        &mut self,
        part: String,
        sketch: String,
        constraint: String,
        before: Option<Box<DimForm>>,
    ) {
        let req = Request::SketchGet {
            part: part.clone(),
            sketch: sketch.clone(),
            quality: None,
            rev: self.form_rev(),
        };
        self.send(req, Pending::Dim(part, sketch, constraint, before));
    }

    /// Shows `part`'s parameters in the panel, read if they aren't yet.
    pub fn show_params(&mut self, part: Option<&str>) {
        if self.params_part.as_deref() == part {
            return;
        }
        self.params_part = part.map(str::to_owned);
        self.params = None;
        if let Some(p) = part {
            self.read_params(p.to_owned(), None);
        }
    }

    fn read_params(&mut self, part: String, before: Option<Box<Params>>) {
        let req = Request::ParamList {
            part: Some(part.clone()),
            scope: None,
            rev: self.form_rev(),
        };
        self.send(req, Pending::Params(part, before));
    }

    /// Shows the Equations tile's table, read the first time.
    pub fn show_equations(&mut self) {
        if !self.equations_open {
            self.equations_open = true;
            self.read_equations(false);
        }
    }

    fn read_equations(&mut self, conflicted: bool) {
        let req = Request::EquationsList {
            part: None,
            all: false,
            rev: None,
        };
        self.send(req, Pending::Equations(conflicted));
    }

    /// Reads the table again if it is open and nothing is being written,
    /// keeping its filter and what was typed.
    fn reload_equations(&mut self) {
        if !self.equations_open {
            return;
        }
        if !self.equations.as_ref().is_some_and(|e| e.saving.is_some()) {
            self.read_equations(false);
        }
    }

    /// Opens the Drawings tile on `part` (else the first part shown),
    /// reading its drawings the first time or when the part changes.
    pub fn show_drawings(&mut self, part: Option<&str>) {
        let part = part
            .map(str::to_owned)
            .or_else(|| self.drawings.part.clone())
            .or_else(|| self.model.shown().first().map(|p| p.to_string()));
        let Some(part) = part else { return };
        let fresh = !self.drawings.open || self.drawings.part.as_deref() != Some(&part);
        self.drawings.open = true;
        if fresh {
            self.drawings = crate::core::drawings::Tile {
                open: true,
                part: Some(part),
                ..Default::default()
            };
            self.read_drawing_list();
        }
    }

    /// Reads every part's drawings again, for the tree's rows.
    pub fn read_drawn(&mut self) {
        self.send(Request::DrawingList { part: None }, Pending::DrawingsAll);
    }

    /// Opens the error card on a refusal a form holds (#750): kept with
    /// the window's failures, which the card reads, without a toast or the
    /// status line.
    pub fn open_card(&mut self, e: Error) {
        self.errors.push(e);
        self.card = Some(self.errors.len() - 1);
    }

    /// A drawing's tree row chosen: the Drawings tile opens on it, its
    /// first sheet shown.
    pub fn open_drawing(&mut self, part: &str, drawing: &str) {
        self.show_drawings(Some(part));
        self.drawings.chosen = None;
        self.drawings.sheet = None;
        self.drawings.camera = None;
        self.drawings.want = Some(drawing.to_owned());
        if self.drawings.list.is_some() {
            self.read_drawing_sheet();
        }
        self.wants_tile = Some("drawings");
    }

    fn read_drawing_list(&mut self) {
        let Some(part) = self.drawings.part.clone() else {
            return;
        };
        self.send(
            Request::DrawingList {
                part: Some(part.clone()),
            },
            Pending::DrawingList(part),
        );
    }

    /// Shows `drawing`'s `sheet` in the Drawings tile.
    pub fn choose_drawing(&mut self, drawing: &str, sheet: &str) {
        let same = self
            .drawings
            .chosen
            .as_ref()
            .is_some_and(|(d, k)| d == drawing && k == sheet);
        self.drawings.chosen = Some((drawing.to_owned(), sheet.to_owned()));
        if !same {
            self.drawings.camera = None;
            self.drawings.choose(None);
            self.drawings.sheet = None;
        }
        self.read_drawing_sheet();
    }

    /// Reads the chosen sheet again (`drawing.get --items`), at the
    /// revision forms read: a person's draft in Annotate.
    fn read_drawing_sheet(&mut self) {
        let (Some(part), Some((drawing, sheet))) =
            (self.drawings.part.clone(), self.drawings.first_choice())
        else {
            return;
        };
        self.drawings.chosen = Some((drawing.clone(), sheet.clone()));
        self.drawings.loading = true;
        let req = Request::DrawingGet {
            part: part.clone(),
            drawing: drawing.clone(),
            rev: self.form_rev(),
            sheet: Some(sheet.clone()),
            items: true,
        };
        self.send(req, Pending::DrawingSheet(part, drawing, sheet));
    }

    /// Writes `ops` to the shown drawing, guarded by the part's revision
    /// it was built at: a drag's or the form's one write.
    fn write_drawing(&mut self, ops: Value) {
        let Some(s) = &self.drawings.sheet else {
            return;
        };
        let req = Request::DrawingEdit {
            part: s.part.clone(),
            drawing: s.drawing.clone(),
            ops,
            expected_revision: s.revision.clone(),
        };
        self.drawings.saving = true;
        self.drawings.error = None;
        self.drawings.note = None;
        self.send(req, Pending::DrawingWrite);
    }

    /// A drag of what was picked on the sheet, let go `d` mm from where it
    /// started: one `drawing.edit`, or a note saying why none.
    pub fn drag_drawing(&mut self, pick: &crate::core::drawings::Pick, d: [f64; 2]) {
        let Some(s) = &self.drawings.sheet else {
            return;
        };
        match crate::core::drawings::drag_ops(s, pick, d) {
            Ok(ops) => self.write_drawing(ops),
            Err(why) => self.drawings.note = Some(why.to_owned()),
        }
    }

    /// Writes the Drawings tile's dimension form as typed.
    pub fn save_drawing_dimension(&mut self) {
        let Some(f) = &self.drawings.form else { return };
        match f.op() {
            Ok(op) => self.write_drawing(json!([op])),
            Err(why) => self.drawings.note = Some(why),
        }
    }

    /// Writes row `i` of the Equations tile as typed, where it lives.
    pub fn save_equation(&mut self, i: usize) {
        let Some(e) = &mut self.equations else { return };
        let Some(req) = e.request(i) else { return };
        e.saving = Some(Some(i));
        e.error = None;
        self.send(req, Pending::EquationSave);
    }

    /// Adds the Equations tile's new parameter, to the project or a part.
    pub fn add_equation(&mut self) {
        let Some(e) = &mut self.equations else { return };
        let Some(req) = e.add_request() else { return };
        e.saving = Some(None);
        e.error = None;
        self.send(req, Pending::EquationSave);
    }

    /// Saves what was typed into the parameter panel.
    pub fn save_params(&mut self) {
        let Some(p) = &mut self.params else { return };
        p.saving = true;
        p.error = None;
        let req = p.request();
        self.send(req, Pending::ParamSave);
    }

    /// Shows `part`'s properties in the Part section, read if they aren't
    /// yet.
    pub fn show_props(&mut self, part: Option<&str>) {
        if self.props_part.as_deref() == part {
            return;
        }
        self.props_part = part.map(str::to_owned);
        self.props = None;
        if let Some(p) = part {
            self.read_props(p.to_owned(), None);
        }
    }

    fn read_props(&mut self, part: String, before: Option<Box<PartProps>>) {
        let req = Request::PropsGet {
            part: part.clone(),
            rev: self.form_rev(),
        };
        self.send(req, Pending::Props(part, before));
    }

    /// Saves what was typed into the Part section.
    pub fn save_props(&mut self) {
        let Some(p) = &mut self.props else { return };
        p.saving = true;
        p.error = None;
        let req = p.request();
        self.send(req, Pending::PropsSave);
    }

    /// Reads the release records the first time the Releases tile shows.
    pub fn open_releases(&mut self) {
        if !self.releases_open {
            self.load_releases();
        }
    }

    /// Reads the BOM the first time the BOM tile shows.
    pub fn open_bom(&mut self) {
        if !self.bom_open {
            self.load_bom();
        }
    }

    /// Reads the Releases tab: every record and the release checks; from
    /// now on they are read again as they change.
    pub fn load_releases(&mut self) {
        self.releases_open = true;
        self.send(Request::ReleaseList { part: None }, Pending::States);
        let req = Request::ReleaseCheck { results: false };
        self.send(req, Pending::ReleaseCheck);
    }

    /// The dry run of proposing `part`, for its Propose dialog: the label
    /// it takes and every reason it can't yet.
    pub fn propose_dry(&mut self, part: &str) {
        self.releases.proposal = None;
        let req = Request::ReleasePropose {
            part: part.to_owned(),
            note: None,
            expected_revision: None,
            dry_run: true,
        };
        self.send(req, Pending::ProposeDry(part.to_owned()));
    }

    /// The revision of `part` the window last saw, to guard a write.
    fn revision_of(&self, part: &str) -> Option<String> {
        self.model.parts.get(part).map(|p| p.revision.clone())
    }

    /// Proposes `part`'s next revision (`release.propose`), against the
    /// revision of it the window last saw.
    pub fn propose(&mut self, part: &str, note: Option<String>) {
        let req = Request::ReleasePropose {
            part: part.to_owned(),
            note,
            expected_revision: self.revision_of(part),
            dry_run: false,
        };
        self.release_write(req);
    }

    /// Makes `part`'s latest official revision, `label`, obsolete.
    pub fn obsolete(&mut self, part: &str, label: &str, note: Option<String>) {
        let req = Request::ReleaseObsolete {
            part: part.to_owned(),
            label: label.to_owned(),
            note,
            expected_revision: self.revision_of(part),
        };
        self.release_write(req);
    }

    /// Creates and moves the local `ryupdm/` tags (`release.sync`).
    pub fn sync_tags(&mut self) {
        self.release_write(Request::ReleaseSync {});
    }

    fn release_write(&mut self, req: Request) {
        self.releases.busy = true;
        self.releases.error = None;
        self.releases.done = None;
        self.send(req, Pending::ReleaseWrite);
    }

    /// The part whose BOM the BOM tab shows: the window's focus; else
    /// none, and the daemon finds the project's one root assembly, as the
    /// window shows it, or names the roots to choose from.
    pub fn bom_root(&self) -> Option<String> {
        self.model.focus.clone()
    }

    /// Reads the BOM tab (`bom.get`): the indented BOM, then the flat one
    /// when the tab shows it. From now on it is read again as what it
    /// reads changes; at most one reading is on its way, and one more
    /// after it.
    pub fn load_bom(&mut self) {
        self.bom_open = true;
        if self.bom_waiting {
            self.bom_again = true;
            return;
        }
        self.bom_waiting = true;
        self.ask_bom(false);
    }

    fn ask_bom(&mut self, flat: bool) {
        let req = Request::BomGet {
            part: self.bom_root(),
            rev: None,
            flat,
            depth: None,
            format: None,
        };
        self.send(req, Pending::Bom { flat });
    }

    /// While the window compares, asks for the BOM's diff from the
    /// compared revision to the working files; else drops any.
    fn ask_bom_diff(&mut self) {
        match (&self.compare, &self.bom.root) {
            (Some(c), Some(root)) => {
                let req = Request::BomDiff {
                    part: Some(root.clone()),
                    from: Some(c.rev.clone()),
                    to: None,
                };
                self.send(req, Pending::BomDiff);
            }
            _ => {
                self.bom.take_diff(None);
                self.bom_read();
            }
        }
    }

    /// A reading is done: the next one, if it was wanted meanwhile.
    fn bom_read(&mut self) {
        self.bom_waiting = false;
        if std::mem::take(&mut self.bom_again) {
            self.load_bom();
        }
    }

    /// Shows the flat BOM, or the indented one.
    pub fn set_bom_flat(&mut self, flat: bool) {
        self.bom.flat = flat;
        self.load_bom();
    }

    /// Shows where `part` is used in the Part section (`bom.where_used`),
    /// read if it isn't yet.
    pub fn show_where_used(&mut self, part: Option<&str>) {
        if self.where_used_part.as_deref() == part {
            return;
        }
        self.where_used_part = part.map(str::to_owned);
        self.where_used = None;
        self.read_where_used();
    }

    /// Reads where the Part section's part is used again, if it shows one.
    fn read_where_used(&mut self) {
        if let Some(part) = self.where_used_part.clone() {
            let req = Request::BomWhereUsed {
                part: part.clone(),
                label: None,
                working: false,
                direct: false,
                within: Vec::new(),
                no_dependents: false,
            };
            self.send(req, Pending::WhereUsed(part.clone()));
            self.ask_org(&part);
        }
    }

    /// Asks the review service where `part` is used across the
    /// organization, on a thread of its own, as `ryucad where-used --org`
    /// does: only when the project has an ID; the daemon takes no part, so
    /// its lock is never held on the network.
    fn ask_org(&mut self, part: &str) {
        let Some(project) = self
            .pdm_config
            .as_ref()
            .and_then(|c| c["project"].as_str())
            .and_then(|p| ryucad::doc::id::ProjectId::parse(p).ok())
        else {
            return;
        };
        let Ok(part_id) = ryucad::doc::id::Id::parse(part, "part") else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let answer = ryucad::review::token().and_then(|t| {
                let client = ryucad::review::Client::new(&ryucad::review::base_url(), &t);
                ryucad::review::where_used(
                    &client,
                    &ryucad::review::OrgQuery {
                        project,
                        part: part_id,
                        label: None,
                        direct: false,
                    },
                )
            });
            let _ = tx.send(answer);
            wake();
        });
        self.org = Some((part.to_owned(), rx));
        self.org_early = None;
    }

    /// The organization's answer, once the thread has it: into the Part
    /// section's where-used if that is still the part's, else kept for it.
    fn take_org(&mut self) {
        let Some((part, rx)) = &self.org else {
            return;
        };
        let Ok(answer) = rx.try_recv() else {
            return;
        };
        let part = part.clone();
        self.org = None;
        let rows = answer.map(|v| crate::core::where_used::WhereUsed::org_rows(&v));
        match self.where_used.as_mut().filter(|w| w.part == part) {
            Some(w) => w.org = Some(rows),
            None => self.org_early = Some((part, rows)),
        }
    }

    /// Opens Export CSV… for the BOM as the tab shows it.
    pub fn open_bom_export(&mut self) {
        self.bom_export = Some(crate::core::bom::BomExport {
            flat: self.bom.flat,
            ..Default::default()
        });
    }

    /// Writes the open export's CSV: `bom.get` with `format: csv`, the
    /// bytes written atomically where it says.
    pub fn run_bom_export(&mut self) {
        let part = self.bom.root.clone().or_else(|| self.bom_root());
        let Some(e) = self.bom_export.as_mut() else {
            return;
        };
        let path = absolute(e.path.trim());
        e.waiting = true;
        e.error = None;
        e.done = None;
        let req = Request::BomGet {
            part,
            rev: None,
            flat: e.flat,
            depth: None,
            format: Some("csv".to_owned()),
        };
        self.send(req, Pending::BomExport(path));
    }

    /// Saves the item number being typed in the BOM tab (`bom.renumber`),
    /// against the revision of its assembly the window last saw. Text
    /// that isn't an item number is refused here, and kept.
    pub fn save_item(&mut self) {
        let Some(e) = self.bom.editing.clone() else {
            return;
        };
        let n = match crate::core::bom::item_number(&e.text) {
            Ok(n) => n,
            Err(why) => {
                self.bom.edit_error = Some(why);
                return;
            }
        };
        if Some(n) == e.was {
            self.bom.editing = None;
            return;
        }
        let mut items = serde_json::Map::new();
        items.insert(e.part.clone(), json!(n));
        let req = Request::BomRenumber {
            part: e.parent.clone(),
            items,
            compact: false,
            expected_revision: self.part(&e.parent).map(|p| p.revision.clone()),
        };
        self.bom.saving = true;
        self.bom.edit_error = None;
        self.send(req, Pending::BomRenumber(e));
    }

    /// Reads the BOM again if the tab was opened.
    fn reload_bom(&mut self) {
        if self.bom_open {
            self.load_bom();
        }
    }

    /// Whether ryupdm is set up: the project has its ID.
    pub fn pdm_ready(&self) -> bool {
        self.pdm_config
            .as_ref()
            .is_some_and(|c| c["project"].is_string())
    }

    /// Sets up ryupdm (`pdm.init`): the project's ID and default
    /// settings. The answer is the new settings.
    pub fn pdm_init(&mut self) {
        self.send(Request::PdmInit {}, Pending::PdmConfig);
    }

    /// Saves the value typed into the dimension's form.
    pub fn save_dimension(&mut self) {
        let Some(f) = &mut self.dimension else { return };
        f.saving = true;
        f.error = None;
        let req = f.request();
        self.send(req, Pending::DimSave);
    }

    /// Saves what was typed into the form.
    pub fn save_form(&mut self) {
        let Some(form) = &mut self.form else { return };
        match form.request() {
            Err(e) => form.error = Some(e),
            Ok(req) => {
                form.saving = true;
                form.error = None;
                self.send(req, Pending::Save);
            }
        }
    }

    /// Shares this window's camera with the session, at most every
    /// [`CAMERA_EVERY`]; the last one always goes out (see `tick`).
    pub fn share_camera(&mut self, camera: &Camera) {
        self.camera_due = Some(camera.clone());
        self.tick(Instant::now());
    }

    fn fail(&mut self, e: Error) {
        let now = Instant::now();
        // A cancel the window asked for isn't a failure to dwell on.
        if e.code == Code::Cancelled {
            self.toasts
                .note(&format!("Cancelled: {}", e.message), false, now);
        } else {
            let title = crate::core::feedback::title(e.code);
            self.toasts.error(&title, &e, self.errors.len(), now);
        }
        self.errors.push(e);
        self.error_at = Some(now);
    }

    /// The failure the status line shows at `now`: the newest, for
    /// [`ERROR_SHOWN`] after it came.
    pub fn shown_error(&self, now: Instant) -> Option<&Error> {
        self.error_at
            .filter(|t| now.saturating_duration_since(*t) < ERROR_SHOWN)
            .and(self.errors.last())
    }

    /// How long until the shown failure goes, to repaint then.
    pub fn error_due(&self, now: Instant) -> Option<Duration> {
        self.error_at
            .map(|t| ERROR_SHOWN.saturating_sub(now.saturating_duration_since(t)))
            .filter(|d| !d.is_zero())
    }

    /// Sends a camera that is due, and reconnects a lost window when it is
    /// time. Call it every frame.
    pub fn tick(&mut self, now: Instant) {
        self.toasts.expire(now);
        self.take_org();
        if self.lost {
            self.reconnect(now);
            return;
        }
        self.follow_views();
        let ready = self
            .camera_sent
            .is_none_or(|t| now.saturating_duration_since(t) >= CAMERA_EVERY);
        if ready && let Some(c) = self.camera_due.take() {
            self.camera_sent = Some(now);
            self.camera_last = Some(c.clone());
            let req = Request::CameraSet {
                camera: Some(camera_arg(&c)),
                view: None,
                fit: Vec::new(),
                normal_to: None,
                part: None,
            };
            self.send(req, Pending::Quiet);
        }
    }

    /// When a lost window tries again next, for the window to wake then.
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry.as_ref().map(|r| r.at)
    }

    fn reconnect(&mut self, now: Instant) {
        let Some(retry) = &mut self.retry else {
            return;
        };
        if let Some(rx) = &retry.trying {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => return,
                Ok(Ok(link)) => {
                    self.retry = None;
                    self.reconnected(link);
                    return;
                }
                Ok(Err(e)) => self.lost_why = Some(e),
                Err(TryRecvError::Disconnected) => {}
            }
            retry.trying = None;
            retry.at = now + retry.delay;
            retry.delay = (retry.delay * 2).min(RETRY_MAX);
            return;
        }
        if now < retry.at {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (root, daemon, name, wake) = (
            self.root.clone(),
            self.daemon.clone(),
            self.name.clone(),
            self.wake.clone(),
        );
        std::thread::spawn(move || {
            let link = Link::open(&root, &daemon, &name, wake.clone());
            let _ = tx.send(link);
            wake();
        });
        retry.trying = Some(rx);
    }

    /// A new link after a lost one: everything is fetched again; meshes of
    /// bodies that didn't change are kept.
    fn reconnected(&mut self, link: Link) {
        self.people.reset(&link.session["participants"]);
        self.link = link;
        // A reconnected window starts in View, as the daemon starts every
        // participant: Direct edit isn't kept across a daemon's restart.
        self.mode = Default::default();
        self.lost = false;
        self.lost_why = None;
        self.toasts.found(Instant::now());
        // A new session knows nothing of this window's camera: tell it.
        if self.camera_due.is_none() {
            self.camera_due = self.camera_last.clone();
        }
        self.camera_sent = None;
        if self
            .following
            .as_ref()
            .is_some_and(|f| self.people.get(f).is_none())
        {
            self.following = None;
        }
        self.load();
        // A new session knows nothing of this window's compare or focus
        // either.
        // (A draft's compare is the window's own: the window is in View
        // again, so it goes.)
        if let Some(rev) = self
            .compare
            .as_ref()
            .filter(|c| c.to.is_none())
            .map(|c| c.rev.clone())
        {
            self.set_compare(Some(rev));
            self.ask_diff();
        }
        if let Some(part) = self.model.focus.clone() {
            self.set_focus(Some(part));
        }
        // A new daemon may see damage, and commits, the old one didn't.
        self.rediagnose();
        self.reload_history();
        // Panels asked for whose answers were lost are asked for again.
        if let Some(part) = self.params_part.clone()
            && self.params.is_none()
        {
            self.read_params(part, None);
        }
        if self.equations_open && self.equations.is_none() {
            self.read_equations(false);
        }
        if let Some((part, id)) = self.form_for.clone()
            && self.form.is_none()
        {
            self.read_form(part, id, None);
        }
        if let Some(part) = self.props_part.clone()
            && self.props.is_none()
        {
            self.read_props(part, None);
        }
        if self.releases_open {
            self.load_releases();
        }
        self.reload_bom();
        if self.where_used.is_none() {
            self.read_where_used();
        }
        if let Some((part, sketch, constraint)) = self.dimension_for.clone()
            && self.dimension.is_none()
        {
            self.read_dimension(part, sketch, constraint, None);
        }
    }

    /// Whether a camera is waiting to be shared.
    pub fn camera_due(&self) -> bool {
        self.camera_due.is_some()
    }

    /// The camera of the participant this window follows, if they have one.
    pub fn followed_camera(&self) -> Option<&Camera> {
        self.people.get(self.following.as_deref()?)?.camera.as_ref()
    }

    /// Handles every event that has arrived. Returns whether there were any.
    pub fn pump(&mut self) -> bool {
        let mut any = false;
        while let Some(e) = self.link.try_event() {
            self.handle(e);
            any = true;
        }
        any
    }

    /// Handles events until `done` holds or `timeout` passes; returns
    /// whether it holds. For tests and the smoke run.
    pub fn wait_until(
        &mut self,
        timeout: Duration,
        mut done: impl FnMut(&Workspace) -> bool,
    ) -> bool {
        let start = Instant::now();
        loop {
            if done(self) {
                return true;
            }
            let left = timeout.saturating_sub(start.elapsed());
            match self.link.wait_event(left) {
                Some(e) => self.handle(e),
                None => return done(self),
            }
            if self.lost {
                return done(self);
            }
        }
    }

    /// Whether every answer the window waits for has arrived.
    /// The daemon this window talks to, for a connection of its own (a
    /// thumbnail's render).
    pub fn daemon(&self) -> &Daemon {
        &self.daemon
    }

    pub fn idle(&self) -> bool {
        self.pending.is_empty()
    }

    fn handle(&mut self, e: Event) {
        match e {
            Event::Note { .. } if self.deaf => {}
            Event::Answer { ticket, answer } => {
                if let Some(m) = self.progress.answered(ticket) {
                    let now = Instant::now();
                    let long = now.saturating_duration_since(m.started) >= DONE_TOAST_AFTER;
                    if !self.focused && long && answer.is_ok() {
                        self.toasts.note(&format!("{} done", m.label), false, now);
                    }
                }
                if let Some(why) = self.pending.remove(&ticket) {
                    self.answered(why, answer);
                }
            }
            Event::Progress { ticket, params } => {
                self.progress.line(ticket, &params, Instant::now());
            }
            Event::Note { method, params } => self.noted(&method, &params),
            Event::Lost if !self.lost => {
                self.lost = true;
                self.pending.clear();
                self.progress.lost();
                self.toasts.lost(Instant::now());
                // Saves that went unanswered can be tried again.
                if let Some(f) = &mut self.form {
                    f.saving = false;
                }
                if let Some(f) = &mut self.dimension {
                    f.saving = false;
                }
                if let Some(p) = &mut self.params {
                    p.saving = false;
                }
                if let Some(e) = &mut self.equations {
                    e.saving = None;
                }
                if let Some(p) = &mut self.props {
                    p.saving = false;
                }
                self.releases.busy = false;
                self.bom_waiting = false;
                self.bom_again = false;
                self.bom.saving = false;
                self.model.asked.clear();
                self.model.sketches_asked.clear();
                self.compare_waiting = false;
                self.doctor_waiting = false;
                self.doctor_again = false;
                self.history_waiting = false;
                self.history_again = false;
                self.branches_waiting = false;
                self.branches.previewing = None;
                if let Some(c) = &mut self.compare {
                    c.asked.clear();
                }
                self.retry = Some(Retry {
                    at: Instant::now() + RETRY_FIRST,
                    delay: RETRY_FIRST,
                    trying: None,
                });
            }
            Event::Lost => {}
        }
    }

    fn answered(&mut self, why: Pending, answer: Result<Answer>) {
        if let Ok(Answer {
            binary: Some(b), ..
        }) = &answer
        {
            self.received += b.len() as u64;
        }
        let (doctor, history) = (
            matches!(why, Pending::Doctor),
            matches!(why, Pending::History),
        );
        self.answer(why, answer);
        if doctor {
            self.doctor_waiting = false;
            if std::mem::take(&mut self.doctor_again) {
                self.diagnose();
            }
        }
        if history {
            self.history_waiting = false;
            if std::mem::take(&mut self.history_again) {
                self.load_history();
            }
        }
    }

    fn answer(&mut self, why: Pending, answer: Result<Answer>) {
        match (why, answer) {
            (Pending::Proposal(inner, rough), r) => {
                // A rough suggestion's comment opens to edit, its
                // annotation chosen in the Annotations tile.
                if rough
                    && let Ok(a) = &r
                    && let Some(id) = a.result["annotation"]["id"].as_str()
                {
                    let id = id.to_owned();
                    let comment = a.result["annotation"]["comment"]
                        .as_str()
                        .unwrap_or(ROUGH_COMMENT)
                        .to_owned();
                    self.choose_annotation(Some(&id));
                    self.annotation_ui.editing = Some((id, comment));
                    self.wants_tile = Some("annotations");
                }
                let r = r.map(|a| Answer {
                    result: a.result["answer"].clone(),
                    binary: None,
                    raw: None,
                });
                if r.is_ok() {
                    self.toasts
                        .note("Suggested: the agent will see it", false, Instant::now());
                }
                self.answer(*inner, r);
            }
            (Pending::Annotations, Ok(a)) => {
                self.annotations.reset(&a.result);
                self.model.generation += 1;
                self.follow_draft();
            }
            (Pending::AnnotationWrite, Ok(_)) => {}
            (Pending::Mode(_), Ok(_)) => {}
            (Pending::Mode(was), Err(e)) => {
                self.mode = was;
                self.fail(e);
            }
            (Pending::Info, Ok(a)) => {
                self.model.default_unit = a.result["default_unit"].as_str().unwrap_or("mm").into();
            }
            (Pending::Parts, Ok(a)) => {
                let listed: BTreeMap<String, Listed> = a.result["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| Some((p["part"].as_str()?.to_owned(), Listed::from_json(p))))
                    .collect();
                // Parts deleted while the window wasn't listening go.
                let gone: Vec<String> = self
                    .model
                    .parts
                    .keys()
                    .filter(|id| !listed.contains_key(*id))
                    .cloned()
                    .collect();
                for id in gone {
                    self.model.remove_part(&id);
                }
                self.model.model_parts = a.result["model"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.as_str().map(str::to_owned))
                    .collect();
                // A project of a few parts loads whole; a larger one, what
                // the window shows, and the rest as its tree asks.
                let whole = listed.len() <= LOAD_WHOLE;
                let shown: Vec<String> = self.model.shown().iter().map(|s| s.to_string()).collect();
                for (id, l) in &listed {
                    match &l.error {
                        Some(e) => {
                            self.model.apply_part(
                                &json!({"part": id, "revision": l.revision, "error": e}),
                            );
                        }
                        None if whole
                            || shown.contains(id)
                            || self.model.parts.contains_key(id) =>
                        {
                            self.ask_part(id);
                        }
                        None => {}
                    }
                }
                self.model.listed = listed;
                self.model.generation += 1;
            }
            (Pending::Rebuild(id, rev), r) => {
                // Asked at a revision the window no longer draws: its
                // answer is dropped, and the part asked for at the one it
                // draws unless that is on its way already (a part not yet
                // loaded when the revision moved isn't asked again then).
                let now = self.model_rev();
                if rev != now {
                    if self.asked_at.get(&id) != Some(&now) {
                        self.loading.remove(&id);
                        self.ask_part(&id);
                    }
                    return;
                }
                self.loading.remove(&id);
                self.asked_at.remove(&id);
                // A failed feature fails the rebuild; the report is in the details.
                let report = match r {
                    Ok(a) => a.result["parts"][0].clone(),
                    Err(e) if e.code == Code::RebuildFailed => e.details["parts"][0].clone(),
                    Err(e) => json!({"part": id, "error": e}),
                };
                self.part_changed(&report);
            }
            (Pending::People, Ok(a)) => {
                self.people.reset(&a.result["participants"]);
                self.follow_draft();
                self.follow_focus();
            }
            (Pending::Compare(n), r) => {
                if n == self.compare_asked {
                    self.compare_waiting = false;
                }
                if let Some(c) = self.compare.as_mut().filter(|_| n == self.compare_asked) {
                    match r {
                        Ok(a) => c.take_diff(&a.result),
                        Err(e) => c.failed(e),
                    }
                    self.model.generation += 1;
                    self.fetch_ghosts();
                }
                if std::mem::take(&mut self.compare_again) {
                    self.ask_diff();
                }
            }
            (Pending::States, Ok(a)) => {
                self.states.reset(&a.result);
                self.releases.list = Some(a.result);
            }
            (Pending::ReleaseCheck, r) => match r {
                Ok(a) => self.releases.findings = findings(&a.result),
                // A failing check is still a report.
                Err(e) if e.code == Code::CheckFailed => {
                    self.releases.findings = findings(&e.details);
                }
                Err(e) => self.releases.error = Some(e),
            },
            (Pending::ProposeDry(part), r) => match r {
                Ok(a) => self.releases.proposal = Some((part, a.result)),
                Err(e) => self.releases.error = Some(e),
            },
            (Pending::ReleaseWrite, r) => {
                self.releases.busy = false;
                match r {
                    Ok(a) => {
                        self.releases.proposal = None;
                        self.releases.confirm = None;
                        self.releases.note.clear();
                        self.releases.done = Some(match a.result.get("path") {
                            Some(p) => format!("wrote {}", p.as_str().unwrap_or_default()),
                            None => crate::core::releases::synced(&a.result),
                        });
                        self.load_releases();
                    }
                    Err(e) => self.releases.error = Some(e),
                }
            }
            (Pending::Bom { flat }, r) => {
                match r {
                    Ok(a) => self.bom.take(&a.result),
                    Err(e) => self.bom.failed(e),
                }
                // The flat BOM follows the indented one, whose lines give
                // it its occurrences, and a compare's diff follows both.
                if !flat && self.bom.flat && self.bom.error.is_none() {
                    self.ask_bom(true);
                } else {
                    self.ask_bom_diff();
                }
            }
            (Pending::BomDiff, r) => {
                match r {
                    Ok(a) => self.bom.take_diff(Some(&a.result)),
                    Err(e) => self.bom.diff_failed(e),
                }
                self.bom_read();
            }
            (Pending::WhereUsed(part), r) => {
                if self.where_used_part.as_deref() == Some(part.as_str()) {
                    use crate::core::where_used::WhereUsed;
                    let mut w = match r {
                        Ok(a) => WhereUsed::from_answer(&part, &a.result),
                        Err(e) => WhereUsed::failed(&part, e),
                    };
                    // The organization's answer may have come first; else
                    // the last one stands until the new one comes.
                    match self.org_early.take() {
                        Some((p, rows)) if p == part => w.org = Some(rows),
                        _ => {
                            w.org = self
                                .where_used
                                .take()
                                .filter(|old| old.part == part)
                                .and_then(|old| old.org);
                        }
                    }
                    self.where_used = Some(w);
                }
            }
            (Pending::BomExport(path), a) => {
                let Some(e) = self.bom_export.as_mut() else {
                    return;
                };
                e.waiting = false;
                let written = a.and_then(|a| {
                    let csv = a.result["csv"].as_str().ok_or_else(|| {
                        Error::new(Code::Internal, "the BOM came without its CSV")
                    })?;
                    ryucad::doc::project::write_atomic(&path, csv)?;
                    Ok(csv.len())
                });
                match written {
                    Ok(n) => e.done = Some((path.display().to_string(), n)),
                    Err(err) => e.error = Some(err),
                }
            }
            (Pending::BomRenumber(edit), r) => {
                self.bom.saving = false;
                match r {
                    Ok(_) => self.bom.editing = None,
                    // Someone renumbered first: read again, and say what
                    // changed.
                    Err(e) if e.code == Code::Conflict => {
                        self.bom.editing = None;
                        self.bom.conflict = Some(edit);
                    }
                    Err(e) => self.bom.edit_error = Some(e.message),
                }
                self.load_bom();
            }
            (Pending::PdmConfig, Ok(a)) => {
                let was = self.pdm_ready();
                self.pdm_config = Some(a.result);
                // Set up just now: the properties' faults may have moved.
                if !was
                    && self.pdm_ready()
                    && let Some(part) = self.props_part.clone()
                {
                    self.read_props(part, None);
                }
            }
            (Pending::Props(part, before), Ok(a)) => {
                if self.props_part.as_deref() == Some(part.as_str()) {
                    self.props = Some(match (before, &self.props) {
                        (Some(b), _) => b.reloaded(&a.result),
                        // Read again under someone typing (after Set up
                        // PDM): what they typed stays.
                        (None, Some(p)) if p.part == part => p.refreshed(&a.result),
                        (None, _) => PartProps::from_get(&a.result),
                    });
                }
            }
            (Pending::PropsSave, Ok(_)) => {
                if let Some(p) = &self.props {
                    let part = p.part.clone();
                    self.read_props(part, None);
                }
            }
            (Pending::PropsSave, Err(e)) => {
                if let Some(p) = &mut self.props {
                    p.saving = false;
                    if e.code == Code::Conflict {
                        let before = Box::new(p.clone());
                        let part = p.part.clone();
                        self.read_props(part, Some(before));
                    } else {
                        p.error = Some(e);
                    }
                }
            }
            (Pending::Merge, Ok(a)) => {
                // A failure read again after keeps saying why.
                let failed = self.merge.as_mut().and_then(|m| m.failed.take());
                let mut m = Merge::of(a.result);
                m.failed = failed;
                self.merge = Some(m);
            }
            // No merge: nothing to show.
            (Pending::Merge, Err(e)) if matches!(e.code, Code::NotMerging | Code::NotAGitRepo) => {
                self.merge = None;
            }
            (Pending::Merge, Err(e)) => self.merge = Some(Merge::broken(e)),
            (Pending::Resolve, Ok(a)) => {
                if let Some(m) = &mut self.merge {
                    m.resolving = false;
                    m.status = a.result["status"].clone();
                }
            }
            (Pending::Resolve, Err(e)) => {
                // Changed underneath: read the status again.
                let again = matches!(e.code, Code::Conflict | Code::RepairConflict);
                if let Some(m) = &mut self.merge {
                    m.resolving = false;
                    m.failed = Some(e);
                }
                if again {
                    self.send(Request::MergeStatus {}, Pending::Merge);
                }
            }
            (Pending::Ghost(key, hash), a) => {
                let Some(c) = &mut self.compare else { return };
                c.answered(&key, &hash);
                match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(bodies))) => {
                        if !bodies.iter().any(|b| b.feature == key.1) {
                            c.no_ghost(&key, &hash);
                        }
                        for b in bodies {
                            if c.take_ghost(&key.0, b) {
                                self.model.generation += 1;
                            }
                        }
                    }
                    Ok(Some(Err(e))) | Err(e) => self.fail(e),
                    Ok(None) => {
                        self.fail(Error::new(Code::Daemon, "a mesh came without its bytes"));
                    }
                }
            }
            (Pending::Quiet, Ok(_)) => {}
            // Drawn as its own request answers, not only when the
            // notification comes (#143).
            (Pending::Hidden, Ok(a)) => {
                let me = self.link.participant.clone();
                if let Some(p) = self.people.all.get_mut(&me) {
                    p.hidden = a.result["hidden"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect();
                    self.people.generation += 1;
                }
                self.follow_focus();
            }
            (Pending::Activity, Ok(a)) => self.activity.reset(&a.result["entries"]),
            (Pending::History, Ok(a)) => self.history = Some(a.result),
            (Pending::Branches, a) => {
                self.branches_waiting = false;
                match a {
                    Ok(a) => {
                        self.branches.listed(a.result);
                        self.ask_preview();
                    }
                    // Said in the tab: outside a repository, there are none.
                    Err(e) => self.branches.error = Some(e),
                }
            }
            (Pending::DepsList, a) => {
                match a {
                    Ok(a) => self.deps.listed(&a.result),
                    Err(e) => self.deps.error = Some(e),
                }
                self.model.set_marks(self.deps.marks());
            }
            (Pending::DepsOutdated, a) => {
                self.deps_waiting = false;
                if let Ok(a) = a {
                    self.deps.outdated(&a.result);
                    self.model.set_marks(self.deps.marks());
                }
                if std::mem::take(&mut self.deps_again) {
                    self.load_deps();
                }
            }
            (Pending::DepsWrite, a) => {
                self.deps.busy = false;
                match a {
                    Ok(a) => {
                        if let Some(f) = a.result["fetched"].as_array() {
                            let changed = f
                                .iter()
                                .flat_map(|r| r["branches"].as_array().into_iter().flatten())
                                .filter(|b| b["changed"] == true)
                                .count();
                            let what = if f.len() == 1 {
                                "repository"
                            } else {
                                "repositories"
                            };
                            self.deps.fetched =
                                Some(format!("fetched {} {what}, {changed} changed", f.len()));
                        }
                        if a.result["dependency"].is_object() {
                            self.deps.add_url.clear();
                            self.deps.add_path.clear();
                            self.deps.add_name.clear();
                        }
                    }
                    Err(e) => self.deps.failed = Some(e),
                }
                // A fetch writes no project file: nothing else says so.
                self.load_deps();
            }
            (Pending::DepsDry(names, part), a) => {
                self.deps.busy = false;
                match a {
                    Ok(a) => {
                        let mut u = Update::from_answer(&a.result);
                        u.names = names;
                        u.part = part;
                        self.deps.update = Some(u);
                    }
                    Err(e) => self.deps.failed = Some(e),
                }
                self.load_deps();
            }
            (Pending::DepsParts(name), a) => {
                if let Some(p) = self
                    .picker
                    .as_mut()
                    .filter(|p| p.dependency.as_deref() == Some(name.as_str()))
                {
                    match a {
                        Ok(a) => p.parts_read(&a.result),
                        Err(e) => {
                            p.reading = false;
                            p.error = Some(e);
                        }
                    }
                }
            }
            (Pending::DepsPlace, a) => match a {
                Ok(_) => self.picker = None,
                Err(e) => {
                    if let Some(p) = self.picker.as_mut() {
                        p.placing = false;
                        p.error = Some(e);
                    }
                }
            },
            (Pending::Preview(path, key), a) => {
                self.branches.previewed(path, key, a.map(|a| a.result));
                self.ask_preview();
            }
            // Said in the tab, not left spinning.
            (Pending::History, Err(e)) => {
                self.history = Some(json!({"error": e.message}));
                self.fail(e);
            }
            (Pending::Cache, Ok(a)) => self.cache = Some(a.result),
            (Pending::CacheToken, Ok(a)) => {
                self.cache_token = Some(a.result);
                self.read_cache();
            }
            // Said beside the field, not only in the status line.
            (Pending::CacheToken, Err(e)) => self.cache_token = Some(json!({"error": e})),
            // Only the dialog for that file takes an answer: one opened
            // since asks again.
            (Pending::ImportPlan(key), a) => {
                let Some(i) = self.import.as_mut().filter(|i| i.key() == key) else {
                    return;
                };
                i.waiting = false;
                match a {
                    Ok(a) => i.plan = Some((key, crate::core::step::Plan::from_answer(&a.result))),
                    Err(e) => i.error = Some(e),
                }
            }
            (Pending::Import(key), a) => {
                let Some(i) = self.import.as_mut().filter(|i| i.key() == key) else {
                    return;
                };
                i.waiting = false;
                match a {
                    Ok(a) => {
                        i.done = Some(crate::core::step::Plan::from_answer(&a.result));
                        i.plan = None;
                    }
                    Err(e) => i.error = Some(e),
                }
            }
            (Pending::Export, a) => {
                if let Some(e) = self.export.as_mut() {
                    e.answered(a.map(|a| a.result));
                }
            }
            (Pending::DragSolve(part), a) => {
                let mut again = false;
                if let Some(d) = self.drag.as_mut().filter(|d| d.part == part) {
                    d.waiting = false;
                    again = d.asked.is_some_and(|(_, t)| t != d.translate);
                }
                match a {
                    // Only while it is still dragged: a late answer after
                    // the drop would undo the written move's look.
                    Ok(a) if self.drag.is_some() => self.preview_poses(&part, &a.result),
                    Ok(_) => {}
                    Err(e) => self.fail(e),
                }
                if again {
                    self.ask_drag();
                }
            }
            (Pending::MoveSolve(part), a) => {
                self.mover_waiting = false;
                let on = self.mover.as_ref().is_some_and(|m| m.part == part);
                match a {
                    // A late answer after the tool went would undo the
                    // written move's look.
                    Ok(a) if on => self.preview_poses(&part, &a.result),
                    Ok(_) => {}
                    Err(e) => {
                        if let Some(m) = self.mover.as_mut().filter(|_| on) {
                            m.error = Some(e.message.clone());
                        }
                    }
                }
            }
            (Pending::MatePreview(part, asked), a) => {
                let Some(d) = self
                    .mating
                    .as_mut()
                    .filter(|d| d.part == part && d.asked == asked)
                else {
                    return;
                };
                match a {
                    Ok(a) => {
                        d.previewed(Ok(a.result["converged"].as_bool().unwrap_or(false)));
                        self.model.preview.retain(|(p, _), _| *p != part);
                        self.preview_poses(&part, &a.result);
                    }
                    Err(e) => {
                        let code = serde_json::to_value(e.code)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned));
                        d.previewed(Err((code.unwrap_or_default(), e.message.clone())));
                        self.model.preview.retain(|(p, _), _| *p != part);
                        self.model.generation += 1;
                    }
                }
            }
            (Pending::MarkupPost, a) => match a {
                // Posted: the bar closes, and the annotation is chosen, so
                // its marks are drawn in its view.
                Ok(a) => {
                    self.markup = None;
                    if let Some(id) = a.result["annotation"]["id"].as_str() {
                        let id = id.to_owned();
                        self.choose_annotation(Some(&id));
                    }
                }
                // Refused: said on the bar, the marks kept.
                Err(e) => match self.markup.as_mut() {
                    Some(m) => {
                        m.posting = false;
                        m.note = Some(e.message.clone());
                    }
                    None => self.fail(e),
                },
            },
            (Pending::MateAdd(_), Err(e)) => self.fail(e),
            (Pending::MateAdd(_), Ok(_)) => {}
            (Pending::MoveWrite(part), a) => {
                self.model.preview.retain(|(p, _), _| *p != part);
                self.model.generation += 1;
                if let Err(e) = a {
                    self.fail(e);
                }
            }
            (Pending::DragMove(part), a) => {
                if let Err(e) = a {
                    self.model.preview.retain(|(p, _), _| *p != part);
                    self.model.generation += 1;
                    self.fail(e);
                }
            }
            (Pending::Sections, Ok(a)) => {
                let me = self.link.participant.clone();
                if let Some(p) = self.people.all.get_mut(&me) {
                    p.sections =
                        serde_json::from_value(a.result["sections"].clone()).unwrap_or_default();
                    self.people.generation += 1;
                }
                self.follow_sections();
            }
            (Pending::Explodes(part), Ok(a)) => {
                if self.explodes.part.as_deref() == Some(part.as_str()) {
                    self.explodes.listed(&part, &a.result);
                    self.sync_explode();
                }
            }
            (Pending::ExplodeShown, Ok(a)) => {
                let me = self.link.participant.clone();
                if let Some(p) = self.people.all.get_mut(&me) {
                    p.explode = crate::core::explode::Shown::from_json(&a.result["explode"]);
                    self.people.generation += 1;
                }
                self.explodes.sliding = None;
                self.follow_explode();
            }
            (Pending::ExplodeSaved(part, show), r) => match r {
                Ok(a) => {
                    if self.explodes.part.as_deref() == Some(part.as_str()) {
                        self.read_explodes();
                    }
                    // A new explode, or one still there, is shown.
                    if let Some(id) = show
                        && a.result.get("steps").is_some()
                        && self.explodes.shown.as_ref().is_none_or(|s| s.explode != id)
                    {
                        self.show_explode(Some(&id), Some(1.0), false);
                    }
                    self.explodes.name.clear();
                }
                Err(e) => {
                    self.sync_explode();
                    self.fail(e);
                }
            },
            (Pending::Views(part), Ok(a)) => {
                if self.views.part.as_deref() == Some(part.as_str()) {
                    self.views = crate::core::views::NamedViews::of(&part, &a.result);
                }
            }
            (Pending::ViewSaved(part), a) => match a {
                Ok(_) => {
                    if self.views.part.as_deref() == Some(part.as_str()) {
                        self.read_views();
                    }
                }
                Err(e) => {
                    self.views.error = Some(e.clone());
                    self.fail(e);
                }
            },
            (Pending::Appearance(n), Ok(a)) => {
                if n == self.looks_asked {
                    self.looks_read = crate::core::appearance::read(&a.result);
                    self.refresh_looks();
                    // An open window shows its target as it is now.
                    let now = self.look_of(self.appearance.target());
                    self.appearance.reread(now, ryucad::render::SURFACE);
                }
            }
            // The rebuild follows as `model.changed`; an open form on the
            // import is read again, its mode with it.
            (Pending::ImportConvert(part, feature), a) => match a {
                Ok(_) => {
                    let key = (part, feature);
                    if self.form_for.as_ref() == Some(&key) {
                        self.open_form(Some(key));
                    }
                }
                Err(e) => self.fail(e),
            },
            (Pending::AppearanceWritten, a) => {
                // What was dragged stays drawn until the new reading.
                self.appearance.dragging = None;
                match a {
                    Ok(_) => self.read_appearance(),
                    Err(e) => {
                        self.appearance.error = Some(e.clone());
                        self.refresh_looks();
                        self.fail(e);
                    }
                }
            }
            (Pending::NormalTo, Ok(a)) => {
                self.looked = serde_json::from_value(a.result["camera"].clone()).ok();
            }
            (Pending::Measure(picks), a) => {
                match a {
                    Ok(a) => self.measure.take(&picks, &a.result),
                    Err(e) => self.measure.failed(&picks, e),
                }
                self.model.generation += 1;
            }
            (Pending::Mass(of), a) => {
                let Some(m) = self.mass.as_mut().filter(|m| m.of == of) else {
                    return;
                };
                match a {
                    Ok(a) => m.take(&a.result),
                    Err(e) => m.failed(e),
                }
                self.model.generation += 1;
            }
            (Pending::Clearance(part), a) => {
                let Some(c) = self.clearance.as_mut().filter(|c| c.part == part) else {
                    return;
                };
                match a {
                    Ok(a) => c.take(&a.result),
                    Err(e) => c.failed(e),
                }
                self.model.generation += 1;
            }
            (Pending::Interference(part), a) => {
                let Some(i) = self.interference.as_mut().filter(|i| i.part == part) else {
                    return;
                };
                match a {
                    Ok(a) => i.take(&a.result),
                    Err(e) => i.failed(e),
                }
                self.model.generation += 1;
            }
            (Pending::Doctor, Ok(a)) => {
                self.doctor = Some(crate::core::doctor::report_of(&a.result))
            }
            // A whole reading, or one changed setting: read them all again,
            // which also brings the project revision up to date.
            (Pending::Config, Ok(a)) if a.result.get("settings").is_some() => {
                self.settings.reset(&a.result);
                self.model.base = self.display_quality();
            }
            (Pending::Config, Ok(_)) => self.load_settings(),
            (Pending::Form(part, id, before), Ok(a)) => {
                // Only the form last asked for opens (a dimension may have
                // been chosen since); a reload after a conflict only if
                // that form is still open.
                let asked = self.form_for.as_ref() == Some(&(part.clone(), id));
                let wanted = asked
                    && match (&before, &self.form) {
                        (Some(b), Some(f)) => b.feature == f.feature && b.part == f.part,
                        (None, _) => true,
                        (Some(_), None) => false,
                    };
                if wanted {
                    let mut form = match before {
                        Some(b) => b.reloaded(&a.result),
                        None => Form::from_show_in(&part, &a.result, self.unit()),
                    };
                    if let Some(p) = self.model.parts.get(&part) {
                        // A reference import's solid is never offered to
                        // act on (design 364).
                        form.offer(&p.modellable());
                    }
                    // An instance may take its part's latest release; another
                    // project's, any of its dependency's official labels.
                    let placed = a.result["part"].as_str().unwrap_or_default();
                    if let Some(project) = form.foreign.clone() {
                        let name = self.model.marks.name(&project);
                        let req = Request::DepsParts {
                            name,
                            part: Some(placed.to_owned()),
                        };
                        let why = Pending::FormLabels(part.clone(), form.feature.clone());
                        self.send(req, why);
                    } else if let Some(l) = self
                        .states
                        .by_part
                        .get(placed)
                        .and_then(|s| s["label"].as_str())
                    {
                        form.offer_revs(&[l.to_owned()]);
                    }
                    self.form = Some(form);
                }
            }
            (Pending::FormLabels(part, id), Ok(a)) => {
                if let Some(f) = self
                    .form
                    .as_mut()
                    .filter(|f| f.part == part && f.feature == id)
                {
                    let labels: Vec<String> = a.result["parts"][0]["labels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|l| l["label"].as_str().map(str::to_owned))
                        .collect();
                    f.offer_revs(&labels);
                }
            }
            // Not fetched: the label stays as read; the Dependencies tab says why.
            (Pending::FormLabels(..), Err(_)) => {}
            (Pending::Dim(part, sketch, constraint, before), Ok(a)) => {
                // Only the dimension last asked for opens, as for features.
                let key = (part.clone(), sketch.clone(), constraint.clone());
                let wanted = self.dimension_for.as_ref() == Some(&key)
                    && match (&before, &self.dimension) {
                        (Some(_), Some(_)) | (None, _) => true,
                        (Some(_), None) => false,
                    };
                if wanted {
                    self.dimension = match before {
                        Some(b) => b.reloaded(&a.result),
                        None => DimForm::from_get(&part, &sketch, &constraint, &a.result),
                    };
                }
            }
            (Pending::Params(part, before), Ok(a)) => {
                if self.params_part.as_deref() == Some(part.as_str()) {
                    let unit = self.model.default_unit.clone();
                    self.params = Some(match before {
                        Some(b) => b.reloaded(&a.result, &unit),
                        None => Params::from_list(&a.result, &unit),
                    });
                }
            }
            (Pending::ParamSave, Ok(_)) => {
                if let Some(p) = &self.params {
                    let part = p.part.clone();
                    self.read_params(part, None);
                }
            }
            (Pending::Equations(conflicted), Ok(a)) => {
                let unit = self.model.default_unit.clone();
                self.equations = Some(match &self.equations {
                    Some(now) => now.reloaded(&a.result, &unit, conflicted),
                    None => Equations::from_list(&a.result, &unit),
                });
            }
            (Pending::Equations(_), Err(e)) => {
                let mut table = self.equations.take().unwrap_or_default();
                table.saving = None;
                table.error = Some(e);
                self.equations = Some(table);
            }
            (Pending::DrawingList(part), Ok(a)) => {
                if self.drawings.part.as_deref() == Some(&part) {
                    let list = crate::core::drawings::List::from_answer(&part, &a.result);
                    // A drawing or sheet that went: the first left.
                    if let Some((d, k)) = &self.drawings.chosen
                        && !list
                            .entries
                            .iter()
                            .any(|e| &e.drawing == d && e.sheets.iter().any(|(s, _)| s == k))
                    {
                        self.drawings.chosen = None;
                        self.drawings.sheet = None;
                        self.drawings.camera = None;
                    }
                    self.drawings.list = Some(list);
                    self.read_drawing_sheet();
                }
            }
            (Pending::DrawingList(_), Err(e)) => self.drawings.error = Some(e),
            (Pending::DrawingsAll, Ok(a)) => {
                let mut drawn: BTreeMap<String, Vec<(String, String, bool)>> = BTreeMap::new();
                for d in a.result["drawings"].as_array().into_iter().flatten() {
                    let (Some(p), Some(id)) = (d["part"].as_str(), d["drawing"].as_str()) else {
                        continue;
                    };
                    drawn.entry(p.to_owned()).or_default().push((
                        id.to_owned(),
                        d["name"].as_str().unwrap_or("").to_owned(),
                        d.get("error").is_some(),
                    ));
                }
                if drawn != self.drawing_rows {
                    self.drawing_rows = drawn;
                    self.model.generation += 1;
                }
            }
            (Pending::DrawingsAll, Err(_)) => {}
            (Pending::DrawingSheet(part, drawing, sheet), r) => {
                let current = self.drawings.part.as_deref() == Some(&part)
                    && self.drawings.chosen == Some((drawing, sheet));
                if current {
                    self.drawings.loading = false;
                    match r {
                        Ok(a) => {
                            self.drawings.sheet =
                                crate::core::drawings::Sheet::from_answer(&a.result);
                            // What was chosen, chosen again in what was read:
                            // a form reads the file as it is now.
                            let keep = self.drawings.selected.take();
                            if !self.drawings.form.as_ref().is_some_and(|f| f.dirty()) {
                                self.drawings.choose(keep);
                            } else {
                                self.drawings.selected = keep;
                            }
                        }
                        Err(e) => self.drawings.error = Some(e),
                    }
                }
            }
            (Pending::DrawingWrite, r) => {
                self.drawings.saving = false;
                match r {
                    Ok(_) => {
                        self.drawings.form = None;
                        self.read_drawing_sheet();
                    }
                    Err(e) if e.code == Code::Conflict => {
                        self.drawings.note = Some(
                            "Someone changed the drawing while you edited it: read again".into(),
                        );
                        self.drawings.form = None;
                        self.read_drawing_list();
                    }
                    Err(e) => self.drawings.error = Some(e),
                }
            }
            (Pending::EquationSave, Ok(_)) => {
                if let Some(e) = &mut self.equations {
                    // What was written reads back as written.
                    match e.saving.take() {
                        Some(Some(i)) => {
                            if let Some(r) = e.rows.get_mut(i) {
                                r.field.read = r.field.text.trim().to_owned();
                            }
                        }
                        Some(None) => {
                            e.new_name.clear();
                            e.new_value.clear();
                        }
                        None => {}
                    }
                    self.read_equations(false);
                }
            }
            (Pending::EquationSave, Err(err)) => {
                if let Some(e) = &mut self.equations {
                    e.saving = None;
                    if err.code == Code::Conflict {
                        self.read_equations(true);
                    } else {
                        e.error = Some(err);
                    }
                }
            }
            (Pending::ParamSave, Err(e)) => {
                if let Some(p) = &mut self.params {
                    p.saving = false;
                    if e.code == Code::Conflict {
                        let before = Box::new(p.clone());
                        let part = p.part.clone();
                        self.read_params(part, Some(before));
                    } else {
                        p.error = Some(e);
                    }
                }
            }
            (Pending::DimSave, Ok(_)) => {
                // Read it back as the files now have it.
                if let Some(f) = &self.dimension {
                    let (p, s, c) = (f.part.clone(), f.sketch.clone(), f.constraint.clone());
                    self.read_dimension(p, s, c, None);
                }
            }
            (Pending::DimSave, Err(e)) => {
                if let Some(f) = &mut self.dimension {
                    f.saving = false;
                    if e.code == Code::Conflict {
                        let before = Box::new(f.clone());
                        let (p, s, c) = (f.part.clone(), f.sketch.clone(), f.constraint.clone());
                        self.read_dimension(p, s, c, Some(before));
                    } else {
                        f.error = Some(e);
                    }
                }
            }
            (Pending::Save, Ok(a)) => {
                if let Some(f) = &self.form {
                    let mut v = a.result["feature"].clone();
                    v["revision"] = a.result["revision"].clone();
                    self.form = Some(Form::from_show_in(&f.part, &v, self.unit()));
                }
            }
            (Pending::Save, Err(e)) => {
                // A feature that would change a reference import: the
                // card offers Convert to solid, beside the form's own.
                if e.code == Code::ReferenceBody {
                    self.fail(e.clone());
                }
                if let Some(f) = &mut self.form {
                    f.saving = false;
                    if e.code == Code::Conflict {
                        // Read it again and show what changed underneath.
                        let (part, id) = (f.part.clone(), f.feature.clone());
                        let before = Box::new(f.clone());
                        self.read_form(part, id, Some(before));
                    } else {
                        f.refuse(e);
                    }
                }
            }
            (Pending::Sketch(key, revision), a) => {
                self.model.sketch_answered(&key, &revision);
                match a {
                    Ok(a) => {
                        self.model
                            .take_sketch(&key, crate::core::sketch::Drawing::from_get(&a.result));
                    }
                    Err(e) => self.fail(e),
                }
            }
            (Pending::Path(key, revision), a) => {
                self.model.path_answered(&key, &revision);
                match a {
                    Ok(a) => {
                        self.model
                            .take_path(&key, crate::core::datums::PathDrawing::from_get(&a.result));
                    }
                    Err(e) => self.fail(e),
                }
            }
            (Pending::Sweep(key, _), a) => {
                if self.made_for.as_ref() == Some(&key) {
                    match a {
                        Ok(a) => {
                            self.swept =
                                Some(crate::core::sweeps::SweepDrawing::from_get(key, &a.result));
                        }
                        Err(e) => self.fail(e),
                    }
                    self.model.generation += 1;
                }
            }
            (Pending::SheetTable(key, revision), a) => {
                use crate::core::sheet::{SheetTable, Shown};
                let t = match a {
                    Ok(a) => Shown::Table(SheetTable::from_answer(&revision, &a.result)),
                    Err(e) => Shown::Failed(e),
                };
                self.sheets.take_table(key, revision, t);
                self.model.generation += 1;
            }
            (Pending::FlatLines(key, revision), a) => {
                let f = a.map(|a| crate::core::flat::FlatLines::from_answer(&a.result));
                if let Some(v) = self
                    .flat_view
                    .as_mut()
                    .filter(|v| (v.part.clone(), v.solid.clone()) == key && v.revision == revision)
                {
                    match &f {
                        Ok(l) => v.lines = Some(l.clone()),
                        Err(e) => {
                            // The flat no longer reads: draw the folded
                            // body again, not the last revision's flat.
                            v.lines = None;
                            v.error = Some(e.clone());
                            self.model.show_flat(None);
                        }
                    }
                }
                self.sheets.take_flat(key, revision, f);
                self.turn_to_flat();
                self.model.generation += 1;
            }
            (Pending::FlatMesh(key, revision), a) => {
                let wanted = self.flat_view.as_ref().is_some_and(|v| {
                    (v.part.clone(), v.solid.clone()) == key && v.revision == revision
                });
                if !wanted {
                    return;
                }
                match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(mut bodies))) if bodies.len() == 1 => {
                        let body = bodies.remove(0);
                        // The solid it is, named now the daemon has chosen it.
                        let solid = (key.0.clone(), body.feature.clone());
                        if let Some(v) = &mut self.flat_view {
                            v.solid.clone_from(&body.feature);
                        }
                        self.model.show_flat(Some((solid, body)));
                        self.turn_to_flat();
                    }
                    Ok(Some(Err(e))) | Err(e) => {
                        // Refused (an edit left the solid unfolding no
                        // more): the folded body is drawn again, and the
                        // view keeps the refusal, not the stale flat.
                        if let Some(v) = &mut self.flat_view {
                            v.lines = None;
                            v.error = Some(e.clone());
                        }
                        self.model.show_flat(None);
                        self.fail(e);
                    }
                    Ok(_) => {
                        self.model.show_flat(None);
                        self.fail(Error::new(Code::Daemon, "a flat came without its body"));
                    }
                }
            }
            (Pending::Made(key, revision), a) => {
                if self.made_for.as_ref() == Some(&key) {
                    match a {
                        Ok(a) => self.made = Some(Made::from_inspect(&key, &revision, &a.result)),
                        Err(e) => self.fail(e),
                    }
                    self.model.generation += 1;
                }
            }
            (Pending::Level(hash, level), a) => {
                let bodies = match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(bodies))) => bodies,
                    Ok(Some(Err(e))) | Err(e) => {
                        self.fail(e);
                        Vec::new()
                    }
                    Ok(None) => Vec::new(),
                };
                let mut taken = false;
                for b in bodies.into_iter().filter(|b| b.hash == hash) {
                    taken |= self.model.take_level(b, level);
                }
                if !taken {
                    self.model.levels_asked.remove(&(hash, level));
                }
            }
            (Pending::Mesh(key, hash), a) => {
                self.answered_meshes(vec![(key, hash)], a);
            }
            (Pending::Meshes(asked), a) => self.answered_meshes(asked, a),
            (_, Err(e)) => self.fail(e),
        }
    }

    /// Takes the meshes a `mesh.get` for `asked` (bodies of one part) brought.
    fn answered_meshes(&mut self, asked: Vec<(BodyKey, String)>, a: Result<Answer>) {
        for (key, hash) in &asked {
            self.model.answered(key, hash);
        }
        let Some(part) = asked.first().map(|(k, _)| k.0.clone()) else {
            return;
        };
        match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
            Ok(Some(Ok(bodies))) => {
                for b in bodies {
                    self.model.take_mesh(&part, b);
                }
            }
            Ok(Some(Err(e))) | Err(e) => self.fail(e),
            Ok(None) => {
                self.fail(Error::new(Code::Daemon, "a mesh came without its bytes"));
            }
        }
    }

    fn noted(&mut self, method: &str, params: &Value) {
        match method {
            "annotation.changed" => {
                let me = self.name.clone();
                // A closed annotation loses its number: the one it had
                // names it where it has no comment (a suggestion, #744).
                let before = self.annotations.numbers();
                let news = self.annotations.changed(params, &me);
                let numbers = self.annotations.numbers();
                let tag = |id: &str| {
                    numbers
                        .get(id)
                        .map_or_else(String::new, |n| format!(" #{n}"))
                };
                let now = Instant::now();
                for (id, r) in &news.replies {
                    let text = format!("replied on{}", tag(id));
                    self.toasts
                        .annotation(&r.by, r.agent, &text, &r.text, id, now);
                }
                for (id, _, note) in &news.closed {
                    let Some(e) = self.annotations.all.get(id) else {
                        continue;
                    };
                    let by = e
                        .resolution
                        .as_ref()
                        .map_or("agent", |r| r.by.as_str())
                        .to_owned();
                    let text = crate::core::annotations::closed_text(e, before.get(id).copied());
                    let note = note.clone().unwrap_or_default();
                    self.toasts.annotation(&by, true, &text, &note, id, now);
                }
                if self
                    .chosen_annotation
                    .as_ref()
                    .is_some_and(|c| !self.annotations.all.contains_key(c))
                {
                    self.choose_annotation(None);
                }
                self.model.generation += 1;
                self.follow_draft();
            }
            "model.changed" => {
                // What the BOM reads changed, or parts went.
                let parts = params["parts"].as_array().into_iter().flatten();
                if parts
                    .filter_map(|r| r["part"].as_str())
                    .any(|p| self.bom.reads(p))
                    || params["removed"].as_array().is_some_and(|r| !r.is_empty())
                    || self.bom.root.is_none()
                {
                    self.reload_bom();
                }
                // Any assembly may now reach the part, or no longer.
                self.read_where_used();
                // The Drawings tile's part's revision (its drawings') moved.
                if self.drawings.open
                    && let Some(part) = self.drawings.part.clone()
                {
                    let read = self.drawings.list.as_ref().and_then(|l| l.revision.clone());
                    let moved = params["parts"].as_array().into_iter().flatten().any(|r| {
                        r["part"] == part.as_str() && r["revision"].as_str() != read.as_deref()
                    });
                    if moved {
                        self.read_drawing_list();
                    }
                }
                for report in params["parts"].as_array().into_iter().flatten() {
                    let Some(id) = report["part"].as_str() else {
                        continue;
                    };
                    let mut l = Listed::from_json(report);
                    l.features = report["features"].as_array().map_or(0, Vec::len);
                    self.model.listed.insert(id.to_owned(), l);
                    // What the window hasn't loaded and doesn't show stays
                    // a line in the tree.
                    let shown = self.model.shown().contains(&id);
                    if !self.model.parts.contains_key(id) && !shown {
                        self.model.generation += 1;
                        continue;
                    }
                    // An assembly's occurrences come only when asked for;
                    // the draft is rebuilt over the files that moved.
                    if report["kind"] == "assembly" && report["occurrences"].is_null() {
                        self.ask_part(id);
                    } else if self.model_rev().is_some() {
                        self.loading.remove(id);
                        self.ask_part(id);
                    } else {
                        self.part_changed(report);
                    }
                }
                for id in params["removed"].as_array().into_iter().flatten() {
                    self.model.remove_part(id.as_str().unwrap_or_default());
                }
                // What the working files differ by changed with them, and
                // so may their damage (a conflict record comes and goes
                // with a file) and the history's uncommitted changes.
                self.ask_diff();
                // A proposal may be stale now.
                if self.releases_open {
                    let req = Request::ReleaseCheck { results: false };
                    self.send(req, Pending::ReleaseCheck);
                }
                self.rediagnose();
                self.reload_history();
                self.read_appearance();
                self.reload_branches();
                self.load_deps();
                self.reload_equations();
                // A drawing came, went or changed its name.
                self.read_drawn();
            }
            "project.changed" => {
                // The project's parameters may have changed.
                self.reload_equations();
                // Parts came or went: the list, and what the window shows.
                self.send(Request::PartList {}, Pending::Parts);
                // The manifest may have its ID now, or ryupdm.json changed.
                self.send(Request::PdmConfig {}, Pending::PdmConfig);
                // Parts came or went, or the declared properties changed.
                self.reload_bom();
                self.read_where_used();
                // Parts came or went: damage may have too.
                self.rediagnose();
                self.reload_history();
                self.reload_branches();
                self.load_deps();
                if let Some(u) = params["default_unit"].as_str()
                    && u != self.model.default_unit
                {
                    self.model.default_unit = u.to_owned();
                    self.model.generation += 1;
                    // Dimensions' labels say their values in it.
                    for part in self.model.clear_sketches() {
                        self.fetch_sketches(&part);
                    }
                }
            }
            "config.changed" => {
                self.settings.changed(params);
                if params["key"] == "display.quality" {
                    self.model.base = self.display_quality();
                    let all = self.model.clear_meshes();
                    self.fetch(all);
                    for part in self.model.clear_sketches() {
                        self.fetch_sketches(&part);
                    }
                    if let Some(c) = &mut self.compare {
                        c.clear_ghosts();
                        self.fetch_ghosts();
                    }
                }
                if params["scope"] == "project" {
                    self.load_settings();
                }
            }
            "merge.changed" => {
                // A resolution in hand, and why the last one failed, outlast
                // a status the files' moving made.
                self.merge = match (params["merging"].as_bool(), params.get("status")) {
                    (Some(true), Some(s)) => {
                        Some(Merge::of(s.clone()).keeping(self.merge.as_ref()))
                    }
                    (Some(true), None) => Some(Merge::broken(
                        serde_json::from_value(params["error"].clone())
                            .unwrap_or_else(|_| Error::new(Code::Internal, "no merge status")),
                    )),
                    _ => None,
                };
                self.rediagnose();
            }
            "pdm.changed" => {
                self.states.changed(params);
                if self.releases_open {
                    self.load_releases();
                }
                // Labels resolve differently, and rows' states moved; a
                // release may have become official.
                self.reload_bom();
                self.read_where_used();
            }
            "progress" => {
                let me = self.link.participant.clone();
                self.progress.published(&me, params, Instant::now());
            }
            "activity.appended" => {
                self.activity.appended(params);
                let me = self.link.participant.clone();
                self.toasts.activity(params, &me, Instant::now());
                // The material library defines no part, yet colours them.
                if params
                    .to_string()
                    .contains(ryucad::doc::materials::MATERIALS)
                {
                    self.read_appearance();
                }
            }
            // The worktree this window compares with changed: its diff,
            // and (once read) its row and preview.
            "compare.changed" => {
                self.ask_diff();
                self.reload_branches();
            }
            "app.open" => self.raised = true,
            "participant.changed" => {
                self.people.changed(params);
                self.follow_draft();
                self.follow_focus();
                self.follow_sections();
                self.follow_explode();
                // Following someone who left ends.
                if self
                    .following
                    .as_ref()
                    .is_some_and(|f| self.people.get(f).is_none())
                {
                    self.following = None;
                }
            }
            // Too much was missed: start again from the daemon's state.
            "resync" => {
                self.load();
                self.send(Request::ParticipantList {}, Pending::People);
            }
            _ => {}
        }
    }

    fn part_changed(&mut self, report: &Value) {
        // Its explodes may have changed with it.
        if self
            .explodes
            .part
            .as_deref()
            .is_some_and(|p| report["part"] == p)
        {
            self.read_explodes();
        }
        // Its named views may have changed with it.
        if self
            .views
            .part
            .as_deref()
            .is_some_and(|p| report["part"] == p)
        {
            self.read_views();
        }
        // Interference found for this assembly is found again.
        if self
            .interference
            .as_ref()
            .is_some_and(|i| report["part"] == i.part.as_str())
        {
            self.check_interference();
        }
        // So are clearances, a weight and the Measure tool's picks that
        // read the part.
        let changed = report["part"].as_str().unwrap_or_default();
        let reads = |r: &str| r == changed || r.starts_with(&format!("{changed}/"));
        if self.clearance.as_ref().is_some_and(|c| c.part == changed) {
            self.check_clearance();
        }
        if let Some(of) = self
            .mass
            .as_ref()
            .map(|m| m.of.clone())
            .filter(|of| reads(of))
        {
            self.weigh(&of);
        }
        if self.measure.picks.iter().any(|p| reads(p)) {
            self.measure_again();
        }
        // A form nobody is typing in follows the files.
        if let Some(f) = &self.form
            && report["part"] == f.part.as_str()
            && report["revision"] != f.revision.as_str()
            && !f.dirty()
            && !f.saving
        {
            let (part, id) = (f.part.clone(), f.feature.clone());
            self.read_form(part, id, None);
        }
        // So do the parameters, and a dimension's form.
        if let Some(p) = &self.params
            && report["part"] == p.part.as_str()
            && report["revision"] != p.revision.as_str()
            && !p.dirty()
            && !p.saving
        {
            let part = p.part.clone();
            self.read_params(part, None);
        }
        if let Some(p) = &self.props
            && report["part"] == p.part.as_str()
            && report["revision"] != p.revision.as_str()
            && !p.dirty()
            && !p.saving
        {
            let part = p.part.clone();
            self.read_props(part, None);
        }
        if let Some(f) = &self.dimension
            && report["part"] == f.part.as_str()
            && report["revision"] != f.revision.as_str()
            && !f.dirty()
            && !f.saving
        {
            let (p, s, c) = (f.part.clone(), f.sketch.clone(), f.constraint.clone());
            self.read_dimension(p, s, c, None);
        }
        let wanted = self.model.apply_part(report);
        self.fetch(wanted);
        // What the chosen feature made moves with its part.
        if let Some(key) = self.made_for.clone()
            && report["part"] == key.0.as_str()
            && self
                .made
                .as_ref()
                .is_none_or(|m| report["revision"] != m.revision.as_str())
        {
            self.ask_made(key);
        }
        // So does what a chosen unfold or refold paints.
        if let Some(key) = self.made_for.clone()
            && report["part"] == key.0.as_str()
        {
            self.ask_sheet_paint(&key);
        }
        // And the flat view, read again at the part's new revision.
        if let Some(v) = &mut self.flat_view
            && report["part"] == v.part.as_str()
            && let Some(r) = report["revision"].as_str()
            && r != v.revision
        {
            v.revision = r.to_owned();
            v.lines = None;
            v.error = None;
            self.read_flat_view();
        }
        // So does how the chosen sweep or loft is drawn.
        if let Some(key) = self.made_for.clone()
            && report["part"] == key.0.as_str()
            && self.swept_at.as_deref() != report["revision"].as_str()
        {
            self.swept = None;
            self.ask_swept(&key);
        }
        if let Some(part) = report["part"].as_str() {
            self.fetch_sketches(part);
        }
    }

    /// Fetches the drawings of `part`'s sketches, paths and helices that
    /// are missing or older than the part, one per request.
    fn fetch_sketches(&mut self, part: &str) {
        for (key, revision) in self.model.ask_sketches(part) {
            let req = Request::SketchGet {
                part: key.0.clone(),
                sketch: key.1.clone(),
                quality: None,
                rev: self.model_rev(),
            };
            self.send(req, Pending::Sketch(key, revision));
        }
        for (key, revision) in self.model.ask_paths(part) {
            let req = Request::PathGet {
                part: key.0.clone(),
                path: key.1.clone(),
                quality: None,
                rev: self.model_rev(),
            };
            self.send(req, Pending::Path(key, revision));
        }
    }

    /// Fetches display meshes, one body per request (an assembly's by the
    /// batch), at [`Model::fetch_level`]: a part with one changed feature
    /// sends one mesh, and a body already asked for at its current hash
    /// isn't asked for again.
    fn fetch(&mut self, bodies: Vec<BodyKey>) {
        // Coarse first in a large view: finer levels follow where they show.
        let level = self.model.fetch_level();
        let quality = (level != self.model.base).then(|| level.as_str().to_owned());
        let mut batches: BTreeMap<String, Vec<(BodyKey, String)>> = BTreeMap::new();
        for (key, hash) in self.model.ask(bodies) {
            self.fetched += 1;
            // An assembly's bodies come by occurrence, many to a request,
            // meshed side by side: an assembly may place 10,000.
            let assembly = self.model.parts.get(&key.0).is_some_and(|p| p.assembly);
            match key.1.rsplit_once('/') {
                Some(_) if assembly => {
                    batches.entry(key.0.clone()).or_default().push((key, hash));
                }
                _ => {
                    let req = Request::MeshGet {
                        part: key.0.clone(),
                        feature: Some(key.1.clone()),
                        features: Vec::new(),
                        quality: quality.clone(),
                        transfer: Some(Transfer::Compact),
                        rev: self.model_rev(),
                        flat: None,
                    };
                    self.send(req, Pending::Mesh(key, hash));
                }
            }
        }
        for (part, asked) in batches {
            for batch in asked.chunks(MESH_BATCH) {
                let features = batch
                    .iter()
                    .filter_map(|(k, _)| Some(k.1.rsplit_once('/')?.0.to_owned()))
                    .collect();
                let req = Request::MeshGet {
                    part: part.clone(),
                    feature: None,
                    features,
                    quality: quality.clone(),
                    transfer: Some(Transfer::Compact),
                    rev: self.model_rev(),
                    flat: None,
                };
                self.send(req, Pending::Meshes(batch.to_vec()));
            }
        }
    }

    /// The user's display quality, as the settings say.
    fn display_quality(&self) -> ryucad::mesh::Quality {
        ryucad::mesh::Quality::parse(self.settings.text("display.quality", "normal"))
            .unwrap_or_default()
    }

    /// Fetches bodies' meshes at the levels of detail the view wants,
    /// in the order given (nearest first), each body and level once.
    pub fn fetch_levels(&mut self, wanted: Vec<(BodyKey, ryucad::mesh::Quality)>) {
        for (key, hash, level) in self.model.ask_levels(wanted) {
            self.fetched += 1;
            let assembly = self.model.parts.get(&key.0).is_some_and(|p| p.assembly);
            let feature = match key.1.rsplit_once('/') {
                Some((path, _)) if assembly => path.to_owned(),
                _ => key.1.clone(),
            };
            let req = Request::MeshGet {
                part: key.0.clone(),
                feature: Some(feature),
                features: Vec::new(),
                quality: Some(level.as_str().to_owned()),
                transfer: Some(Transfer::Compact),
                rev: self.model_rev(),
                flat: None,
            };
            self.send(req, Pending::Level(hash, level));
        }
    }

    /// The part `id`, if the window knows it.
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.model.parts.get(id)
    }

    /// Asks for `id`'s report unless it is loaded or asked for: when its
    /// row in the tree opens, the window shows it, or something in it is
    /// chosen.
    pub fn load_part(&mut self, id: &str) {
        if !self.model.parts.contains_key(id) {
            self.ask_part(id);
        }
    }

    /// Notes, while the tree draws, that `id` should be loaded.
    pub fn want(&self, id: &str) {
        if !self.model.parts.contains_key(id) {
            self.wanted.borrow_mut().insert(id.to_owned());
        }
    }

    /// Loads what the tree wanted as it drew.
    pub fn load_wanted(&mut self) {
        let wanted = std::mem::take(&mut *self.wanted.borrow_mut());
        for id in wanted {
            self.load_part(&id);
        }
    }

    /// Asks for `id`'s report again, unless it is being asked for.
    fn ask_part(&mut self, id: &str) {
        if !self.loading.insert(id.to_owned()) {
            return;
        }
        let rev = self.model_rev();
        self.asked_at.insert(id.to_owned(), rev.clone());
        let req = Request::Rebuild {
            part: Some(id.to_owned()),
            rev: rev.clone(),
        };
        self.send(req, Pending::Rebuild(id.to_owned(), rev));
    }
}

/// An `import` of the file at `path`, replacing `update` when given; a
/// dry run plans without writing. Bodies are references unless `solid`.
fn import_request(path: &str, update: Option<String>, dry_run: bool, solid: bool) -> Request {
    Request::Import {
        path: absolute(path).display().to_string(),
        part: None,
        into: None,
        product: None,
        dry_run,
        expected_revision: None,
        update,
        solid,
        flatten: false,
    }
}

/// `path` made absolute against the app's own folder: the daemon reads a
/// relative path against the project's.
fn absolute(path: &str) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A body of an inspect answer: faces by name and origin, edges by the
    /// faces they bound.
    fn body(solid: &str, faces: &[(&str, &str)], edges: &[(&str, [&str; 2])]) -> Value {
        json!({
            "solid": solid,
            "faces": faces.iter().map(|(n, f)| json!({"name": n, "origin": {"feature": f}}))
                .collect::<Vec<_>>(),
            "edges": edges.iter().map(|(n, fs)| json!({"name": n, "faces": fs}))
                .collect::<Vec<_>>(),
        })
    }

    #[test]
    fn a_shells_rims_are_the_edges_between_made_and_unmade_faces() {
        let key = ("p".to_owned(), "sh".to_owned());
        // An open shell: walls `w1`, `w2` and floor `fl` the shell made, the
        // outside `o1`, `o2` the box's, the opening's edges `r1`, `r2`
        // between them; `e1` joins two made faces, `e2` two unmade ones.
        let open = body(
            "b",
            &[
                ("w1", "sh"),
                ("w2", "sh"),
                ("fl", "sh"),
                ("o1", "b"),
                ("o2", "b"),
            ],
            &[
                ("r1", ["w1", "o1"]),
                ("r2", ["o2", "w2"]),
                ("e1", ["w1", "fl"]),
                ("e2", ["o1", "o2"]),
            ],
        );
        // A closed shell: its cavity's faces meet only each other.
        let cavity = body(
            "c",
            &[("i1", "sh"), ("i2", "sh"), ("o3", "c")],
            &[("e3", ["i1", "i2"]), ("e4", ["o3", "o3"])],
        );
        // A body the shell made nothing of.
        let other = body("d", &[("x", "d")], &[("e5", ["x", "x"])]);
        let v = json!({"bodies": [open, cavity, other]});
        let m = Made::from_inspect(&key, "r1", &v);
        let names = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<BTreeSet<_>>();
        let b = ("p".to_owned(), "b".to_owned());
        let c = ("p".to_owned(), "c".to_owned());
        assert_eq!(m.faces[&b], names(&["w1", "w2", "fl"]));
        assert_eq!(m.faces[&c], names(&["i1", "i2"]));
        assert!(!m.faces.contains_key(&("p".to_owned(), "d".to_owned())));
        assert_eq!(m.rims[&b], names(&["r1", "r2"]));
        // The cavity has no opening, so no rim.
        assert!(!m.rims.contains_key(&c));
        assert_eq!(m.rims.len(), 1);
        assert_eq!((m.feature, m.revision.as_str()), (key, "r1"));
        assert!(m.lines.is_empty() && m.walls.is_empty());
    }

    #[test]
    fn a_fills_sides_a_sews_seams_and_a_thickens_walls_are_found_by_origin() {
        let names = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<BTreeSet<_>>();
        let edge = |n: &str, origin: Value| json!({"name": n, "faces": ["a"], "origin": origin});
        // A fill: its patch and its sides, laminar, from its own origins.
        let fill = json!({"bodies": [{"solid": "f",
            "faces": [{"name": "a", "origin": {"feature": "f", "fill": "patch"}}],
            "edges": [edge("s1", json!({"feature": "f", "side": 0})),
                      edge("s2", json!({"feature": "f", "side": 1, "piece": 0})),
                      edge("i1", json!({"feature": "f"}))]}]});
        let m = Made::from_inspect(&("p".into(), "f".into()), "r", &fill);
        let f = ("p".to_owned(), "f".to_owned());
        assert_eq!(m.faces[&f], names(&["a"]));
        assert_eq!(m.lines[&f], names(&["s1", "s2"]));
        // A sew: the sheets' faces keep their own origins, only the edges
        // it sewed are its.
        let sew = json!({"bodies": [{"solid": "sw",
            "faces": [{"name": "a", "origin": {"feature": "top"}}],
            "edges": [edge("e1", json!({"feature": "sw", "sewn": ["x", "y"]})),
                      edge("e2", json!({"feature": "top"}))]}]});
        let m = Made::from_inspect(&("p".into(), "sw".into()), "r", &sew);
        let sw = ("p".to_owned(), "sw".to_owned());
        assert!(m.faces.is_empty());
        assert_eq!(m.lines[&sw], names(&["e1"]));
        // A thicken: its walls apart from its sides' faces.
        let thicken = json!({"bodies": [{"solid": "t",
            "faces": [{"name": "w", "origin": {"feature": "t", "wall_of": ["e"]}},
                      {"name": "s", "origin": {"feature": "t", "front_of": "a"}}],
            "edges": []}]});
        let m = Made::from_inspect(&("p".into(), "t".into()), "r", &thicken);
        let t = ("p".to_owned(), "t".to_owned());
        assert_eq!(m.faces[&t], names(&["w", "s"]));
        assert_eq!(m.walls[&t], names(&["w"]));
    }

    #[test]
    fn a_tweaks_made_faces_are_the_faces_it_modified() {
        let key = ("p".to_owned(), "m".to_owned());
        // The tweak's own inspect: every face keeps its origin, the box's.
        let v = json!({"bodies": [{"feature": "m", "solid": "b",
            "faces": [{"name": "t", "origin": {"feature": "b"}},
                      {"name": "s", "origin": {"feature": "b"}}],
            "edges": [{"name": "e", "faces": ["t", "s"]}]}],
            "move_face": {"modified": {"faces": 1, "edges": 1, "vertices": 0,
                                       "references": ["p/b/e", "p/b/t"]}}});
        let m = Made::from_inspect(&key, "r1", &v);
        let b = ("p".to_owned(), "b".to_owned());
        assert_eq!(m.faces[&b], BTreeSet::from(["t".to_owned()]));
        assert!(m.rims.is_empty());
    }
}
