//! An open project: the link to its daemon and what the window knows, kept
//! up to date from answers and notifications.

use crate::core::activity::{Activity, LOADED};
use crate::core::bom::BomTab;
use crate::core::branches::Branches;
use crate::core::compare::{Blink, Compare, Front};
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
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub mod activity;
pub mod annotations;
pub mod assembly;
pub mod branches;
pub mod comparing;
pub mod deps;
pub mod drawings;
pub mod exchange;
pub mod forms;
pub mod model;
pub mod parameters;
pub mod pdm;
pub mod session;
pub mod view;

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
    /// A part's holes, at its revision.
    Holes(String, String),
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
    /// The Branches tab's branches (`branch.list`).
    BranchList,
    /// The Check out dialog's `branch.switch`.
    Switch,
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
    /// A chosen delete's ghost: the delete it was asked for.
    Deleted(BodyKey),
    /// An insert update's dry run for the form: part, insert and `to`.
    InsertPreview((String, String, String)),
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
    /// An assembly's motions (`motion.list`).
    Motions(String),
    /// A motion's frames (`motion.frames`): its assembly and ID.
    MotionFrames(String, String),
    /// This window's own `motion.show`.
    MotionShown,
    /// A motion written (`motion.save`), of an assembly.
    MotionSaved(String),
    /// A motion swept with interference: its assembly and ID.
    MotionSwept(String, String),
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
    /// of a tweak's or direct edit's own `inspect`, the faces it modified
    /// and made (`tweaks::modified`).
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
    /// The project's annotations (design 374), read when the window opens
    /// and followed from `annotation.changed`.
    pub annotations: crate::core::annotations::Annotations,
    pub model: Model,
    /// The property panel's forms (design 541's sub-states, 27.F).
    pub forms: forms::Forms,
    /// The parameter panel and the Equations tile.
    pub parameters: parameters::Parameters,
    /// Releases, PDM settings, properties, where-used and the BOM.
    pub pdm: pdm::Pdm,
    /// A compare with a Git revision.
    pub comparing: comparing::Comparing,
    /// This window's mode, participants, settings, doctor and cache.
    pub session: session::Session,
    /// The shown assembly's tools.
    pub assembly: assembly::Assembly,
    /// Named views, appearance, sections, Measure and Mass.
    pub view: view::View,
    /// Import and export.
    pub exchange: exchange::Exchange,
    /// The Drawings tile (see `show_drawings`).
    pub drawings: crate::core::drawings::Tile,
    pub activity: Activity,
    /// The Branches tab: the worktrees and their merge previews, once
    /// asked for; when they were last read and whether a reading is on its
    /// way.
    pub branches: Branches,
    /// The project's dependencies, the lock's labels and what is
    /// outdated: the Dependencies tab's, and the marks' in the tree.
    pub deps: Deps,
    /// A tile the window should open (the app opens it next frame): the
    /// Annotations tile after a rough drag's proposal.
    pub wants_tile: Option<&'static str>,
    /// Tests only: notifications are dropped as they arrive, as if lost,
    /// to see what the window does from its answers alone.
    #[doc(hidden)]
    pub deaf: bool,
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
            annotations: Default::default(),
            model: Model::default(),
            forms: Default::default(),
            parameters: Default::default(),
            pdm: Default::default(),
            comparing: Default::default(),
            session: session::Session {
                people,
                ..Default::default()
            },
            assembly: Default::default(),
            view: Default::default(),
            exchange: Default::default(),
            drawings: Default::default(),
            activity: Activity::default(),
            branches: Branches::default(),
            deps: Deps::default(),
            wants_tile: None,
            deaf: false,
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
        self.model.loading.clear();
        self.model.asked_at.clear();
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

    /// Undoes an activity entry, or this window's latest change. A failure
    /// (`undo_conflict`, `nothing_to_undo`) lands in `errors`.
    pub fn undo(&mut self, entry: Option<u64>) {
        self.send(Request::Undo { entry }, Pending::Quiet);
    }

    /// Redoes an undo, or this window's latest undo.
    pub fn redo(&mut self, entry: Option<u64>) {
        self.send(Request::Redo { entry }, Pending::Quiet);
    }

    /// The project's default unit.
    pub fn unit(&self) -> ryucad::units::Unit {
        ryucad::units::Unit::parse(&self.model.default_unit).unwrap_or(ryucad::units::Unit::Mm)
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
        if self.view.sections.follow(&planes) {
            self.model.generation += 1;
        }
    }

    /// The explode this window's participant shows, its list read when it
    /// is another assembly's, and where it moves occurrences.
    fn follow_explode(&mut self) {
        let shown = self.myself().and_then(|m| m.explode.clone());
        if self.assembly.explodes.shown != shown {
            self.assembly.explodes.shown = shown;
            self.assembly.explodes.sliding = None;
        }
        let part = self
            .assembly
            .explodes
            .shown
            .as_ref()
            .map(|s| s.part.clone())
            .or_else(|| self.shown_assembly());
        if part.is_some() && part != self.assembly.explodes.part {
            self.assembly.explodes.part = part;
            self.assembly.explodes.list.clear();
            self.read_explodes();
        }
        self.sync_explode();
    }

    /// The motion this window's participant shows: its list read when it
    /// is another assembly's, its frames fetched when they aren't this
    /// motion's at this revision, Play started or stopped as the session
    /// says, and where it moves occurrences.
    fn follow_motion(&mut self) {
        let shown = self.myself().and_then(|m| m.motion.clone());
        if self.assembly.motions.shown != shown {
            let was_playing = self
                .assembly
                .motions
                .shown
                .as_ref()
                .is_some_and(|s| s.playing);
            self.assembly.motions.shown = shown.clone();
            self.assembly.motions.sliding = None;
            match &shown {
                Some(s) if s.playing && !was_playing => {
                    self.assembly.motions.started = Some((Instant::now(), s.fraction));
                }
                Some(s) if s.playing => {}
                _ => self.assembly.motions.started = None,
            }
        }
        let part = shown
            .as_ref()
            .map(|s| s.part.clone())
            .or_else(|| self.shown_assembly());
        if part.is_some() && part != self.assembly.motions.part {
            self.assembly.motions.part = part;
            self.assembly.motions.list.clear();
            self.assembly.motions.frames = None;
            self.read_motions();
        }
        if shown.is_some() && !self.assembly.motions.frames_current() {
            self.read_frames();
        }
        self.sync_moved();
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
            self.view.refocused = true;
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
        // While the compared revision is drawn, nothing writes: the model
        // on screen isn't the files (design 447).
        if self.comparing.blink.draws_rev() && req.needs().is_some() {
            let method = req.method();
            let e = Error::new(
                Code::ParticipantMode,
                format!("{method}: {}", crate::core::compare::SWAPPED),
            )
            .with("showing", &self.comparing.blink.of)
            .with("method", &method);
            self.answer(why, Err(e));
            return;
        }
        match gate(self.session.mode, &req) {
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
                    .with("mode", self.session.mode.as_str())
                    .with("method", &method);
                self.answer(why, Err(e));
            }
        }
    }

    /// What a control that writes by `method` does in the window's mode
    /// (see `core::modes::control`); nothing while the compared revision
    /// is drawn.
    pub fn control(&self, method: &str) -> crate::core::modes::Gate {
        if self.comparing.blink.draws_rev() {
            return crate::core::modes::Gate::Refuse(crate::core::compare::SWAPPED);
        }
        crate::core::modes::control(self.session.mode, method)
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

    /// This window's person's draft as a revision, while they have one in
    /// Annotate: `draft:<name>`.
    fn draft_rev(&self) -> Option<String> {
        (self.session.mode == crate::core::modes::Mode::Annotate
            && !self.annotations.draft(&self.name).is_empty())
        .then(|| format!("{}{}", ryucad::session::DRAFT, self.name))
    }

    /// The revision the window's model is read at: the draft, unless the
    /// person looks at the agent's model or compares with a revision;
    /// the compared revision while it is swapped in front; else the
    /// working files.
    pub fn model_rev(&self) -> Option<String> {
        // Swapped: the compared revision, as the participant gave it.
        if self.comparing.blink.draws_rev() {
            return self.comparing.blink.of.clone();
        }
        let comparing = self.myself().is_some_and(|m| m.compare.is_some());
        self.draft_rev()
            .filter(|_| !self.annotations.show_agent && !comparing)
    }

    /// The revision forms read at: the draft whenever there is one, since
    /// proposals run over it (and are guarded by its revisions).
    pub fn form_rev(&self) -> Option<String> {
        self.draft_rev()
    }

    /// Reads the model again when what is drawn changed: the mode, Show
    /// agent's model, a compare, or the person's draft (a suggestion made,
    /// applied, closed or gone stale); and follows the compare.
    fn follow_draft(&mut self) {
        // A compare that changed or ended starts the blink again: both
        // sides, the working files in front.
        let compared = self.myself().and_then(|m| m.compare.clone());
        if self.comparing.blink.follow(compared.as_deref()) {
            self.model.generation += 1;
        }
        let now = (self.model_rev(), self.annotations.draft(&self.name));
        if now != self.model.drawn {
            let rev_moved = now.0 != self.model.drawn.0 || now.0.is_some();
            self.model.drawn = now;
            if rev_moved {
                let loaded: Vec<String> = self.model.parts.keys().cloned().collect();
                for id in loaded {
                    self.model.loading.remove(&id);
                    self.ask_part(&id);
                }
                for part in self.model.clear_sketches() {
                    self.fetch_sketches(&part);
                }
                self.read_appearance();
            }
            if let Some(f) = &self.forms.form {
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

    /// Opens the error card on a refusal a form holds (#750): kept with
    /// the window's failures, which the card reads, without a toast or the
    /// status line.
    pub fn open_card(&mut self, e: Error) {
        self.errors.push(e);
        self.card = Some(self.errors.len() - 1);
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
        // Play moves the model on every frame.
        if self.assembly.motions.playing() {
            self.sync_moved();
        }
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
        self.session.people.reset(&link.session["participants"]);
        self.link = link;
        // A reconnected window starts in View, as the daemon starts every
        // participant: Direct edit isn't kept across a daemon's restart.
        self.session.mode = Default::default();
        self.lost = false;
        self.lost_why = None;
        self.toasts.found(Instant::now());
        // A new session knows nothing of this window's camera: tell it.
        if self.camera_due.is_none() {
            self.camera_due = self.camera_last.clone();
        }
        self.camera_sent = None;
        if self
            .session
            .following
            .as_ref()
            .is_some_and(|f| self.session.people.get(f).is_none())
        {
            self.session.following = None;
        }
        self.load();
        // A new session knows nothing of this window's compare or focus
        // either.
        // (A draft's compare is the window's own: the window is in View
        // again, so it goes.)
        if let Some(rev) = self
            .comparing
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
        if let Some(part) = self.parameters.params_part.clone()
            && self.parameters.params.is_none()
        {
            self.read_params(part, None);
        }
        if self.parameters.equations_open && self.parameters.equations.is_none() {
            self.read_equations(false);
        }
        if let Some((part, id)) = self.forms.form_for.clone()
            && self.forms.form.is_none()
        {
            self.read_form(part, id, None);
        }
        if let Some(part) = self.pdm.props_part.clone()
            && self.pdm.props.is_none()
        {
            self.read_props(part, None);
        }
        if self.pdm.releases_open {
            self.load_releases();
        }
        self.reload_bom();
        if self.pdm.where_used.is_none() {
            self.read_where_used();
        }
        if let Some((part, sketch, constraint)) = self.forms.dimension_for.clone()
            && self.forms.dimension.is_none()
        {
            self.read_dimension(part, sketch, constraint, None);
        }
    }

    /// Whether a camera is waiting to be shared.
    pub fn camera_due(&self) -> bool {
        self.camera_due.is_some()
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
                if let Some(f) = &mut self.forms.form {
                    f.saving = false;
                }
                if let Some(f) = &mut self.forms.dimension {
                    f.saving = false;
                }
                if let Some(p) = &mut self.parameters.params {
                    p.saving = false;
                }
                if let Some(e) = &mut self.parameters.equations {
                    e.saving = None;
                }
                if let Some(p) = &mut self.pdm.props {
                    p.saving = false;
                }
                self.pdm.releases.busy = false;
                self.pdm.bom_waiting = false;
                self.pdm.bom_again = false;
                self.pdm.bom.saving = false;
                self.model.asked.clear();
                self.model.sketches_asked.clear();
                self.comparing.compare_waiting = false;
                self.session.doctor_waiting = false;
                self.session.doctor_again = false;
                self.activity.history_waiting = false;
                self.activity.history_again = false;
                self.branches.branches_waiting = false;
                self.branches.previewing = None;
                if let Some(c) = &mut self.comparing.compare {
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
            self.model.received += b.len() as u64;
        }
        let (doctor, history) = (
            matches!(why, Pending::Doctor),
            matches!(why, Pending::History),
        );
        self.answer(why, answer);
        if doctor {
            self.session.doctor_waiting = false;
            if std::mem::take(&mut self.session.doctor_again) {
                self.diagnose();
            }
        }
        if history {
            self.activity.history_waiting = false;
            if std::mem::take(&mut self.activity.history_again) {
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
                    self.annotations.annotation_ui.editing = Some((id, comment));
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
                self.session.mode = was;
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
                    if self.model.asked_at.get(&id) != Some(&now) {
                        self.model.loading.remove(&id);
                        self.ask_part(&id);
                    }
                    return;
                }
                self.model.loading.remove(&id);
                self.model.asked_at.remove(&id);
                // A failed feature fails the rebuild; the report is in the details.
                let report = match r {
                    Ok(a) => a.result["parts"][0].clone(),
                    Err(e) if e.code == Code::RebuildFailed => e.details["parts"][0].clone(),
                    Err(e) => json!({"part": id, "error": e}),
                };
                self.part_changed(&report);
            }
            (Pending::People, Ok(a)) => {
                self.session.people.reset(&a.result["participants"]);
                self.follow_draft();
                self.follow_focus();
            }
            (Pending::Compare(n), r) => {
                if n == self.comparing.compare_asked {
                    self.comparing.compare_waiting = false;
                }
                if let Some(c) = self
                    .comparing
                    .compare
                    .as_mut()
                    .filter(|_| n == self.comparing.compare_asked)
                {
                    match r {
                        Ok(a) => c.take_diff(&a.result),
                        Err(e) => c.failed(e),
                    }
                    self.model.generation += 1;
                    self.fetch_ghosts();
                }
                if std::mem::take(&mut self.comparing.compare_again) {
                    self.ask_diff();
                }
            }
            (Pending::States, Ok(a)) => {
                self.pdm.states.reset(&a.result);
                self.pdm.releases.list = Some(a.result);
            }
            (Pending::ReleaseCheck, r) => match r {
                Ok(a) => self.pdm.releases.findings = findings(&a.result),
                // A failing check is still a report.
                Err(e) if e.code == Code::CheckFailed => {
                    self.pdm.releases.findings = findings(&e.details);
                }
                Err(e) => self.pdm.releases.error = Some(e),
            },
            (Pending::ProposeDry(part), r) => match r {
                Ok(a) => self.pdm.releases.proposal = Some((part, a.result)),
                Err(e) => self.pdm.releases.error = Some(e),
            },
            (Pending::ReleaseWrite, r) => {
                self.pdm.releases.busy = false;
                match r {
                    Ok(a) => {
                        self.pdm.releases.proposal = None;
                        self.pdm.releases.confirm = None;
                        self.pdm.releases.note.clear();
                        self.pdm.releases.done = Some(match a.result.get("path") {
                            Some(p) => format!("wrote {}", p.as_str().unwrap_or_default()),
                            None => crate::core::releases::synced(&a.result),
                        });
                        self.load_releases();
                    }
                    Err(e) => self.pdm.releases.error = Some(e),
                }
            }
            (Pending::Bom { flat }, r) => {
                match r {
                    Ok(a) => self.pdm.bom.take(&a.result),
                    Err(e) => self.pdm.bom.failed(e),
                }
                // The flat BOM follows the indented one, whose lines give
                // it its occurrences, and a compare's diff follows both.
                if !flat && self.pdm.bom.flat && self.pdm.bom.error.is_none() {
                    self.ask_bom(true);
                } else {
                    self.ask_bom_diff();
                }
            }
            (Pending::BomDiff, r) => {
                match r {
                    Ok(a) => self.pdm.bom.take_diff(Some(&a.result)),
                    Err(e) => self.pdm.bom.diff_failed(e),
                }
                self.bom_read();
            }
            (Pending::WhereUsed(part), r) => {
                if self.pdm.where_used_part.as_deref() == Some(part.as_str()) {
                    use crate::core::where_used::WhereUsed;
                    let mut w = match r {
                        Ok(a) => WhereUsed::from_answer(&part, &a.result),
                        Err(e) => WhereUsed::failed(&part, e),
                    };
                    // The organization's answer may have come first; else
                    // the last one stands until the new one comes.
                    match self.pdm.org_early.take() {
                        Some((p, rows)) if p == part => w.org = Some(rows),
                        _ => {
                            w.org = self
                                .pdm
                                .where_used
                                .take()
                                .filter(|old| old.part == part)
                                .and_then(|old| old.org);
                        }
                    }
                    self.pdm.where_used = Some(w);
                }
            }
            (Pending::BomExport(path), a) => {
                let Some(e) = self.pdm.bom_export.as_mut() else {
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
                self.pdm.bom.saving = false;
                match r {
                    Ok(_) => self.pdm.bom.editing = None,
                    // Someone renumbered first: read again, and say what
                    // changed.
                    Err(e) if e.code == Code::Conflict => {
                        self.pdm.bom.editing = None;
                        self.pdm.bom.conflict = Some(edit);
                    }
                    Err(e) => self.pdm.bom.edit_error = Some(e.message),
                }
                self.load_bom();
            }
            (Pending::PdmConfig, Ok(a)) => {
                let was = self.pdm_ready();
                self.pdm.pdm_config = Some(a.result);
                // Set up just now: the properties' faults may have moved.
                if !was
                    && self.pdm_ready()
                    && let Some(part) = self.pdm.props_part.clone()
                {
                    self.read_props(part, None);
                }
            }
            (Pending::Props(part, before), Ok(a)) => {
                if self.pdm.props_part.as_deref() == Some(part.as_str()) {
                    self.pdm.props = Some(match (before, &self.pdm.props) {
                        (Some(b), _) => b.reloaded(&a.result),
                        // Read again under someone typing (after Set up
                        // PDM): what they typed stays.
                        (None, Some(p)) if p.part == part => p.refreshed(&a.result),
                        (None, _) => PartProps::from_get(&a.result),
                    });
                }
            }
            (Pending::PropsSave, Ok(_)) => {
                if let Some(p) = &self.pdm.props {
                    let part = p.part.clone();
                    self.read_props(part, None);
                }
            }
            (Pending::PropsSave, Err(e)) => {
                if let Some(p) = &mut self.pdm.props {
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
                let failed = self.branches.merge.as_mut().and_then(|m| m.failed.take());
                let mut m = Merge::of(a.result);
                m.failed = failed;
                self.branches.merge = Some(m);
            }
            // No merge: nothing to show.
            (Pending::Merge, Err(e)) if matches!(e.code, Code::NotMerging | Code::NotAGitRepo) => {
                self.branches.merge = None;
            }
            (Pending::Merge, Err(e)) => self.branches.merge = Some(Merge::broken(e)),
            (Pending::Resolve, Ok(a)) => {
                if let Some(m) = &mut self.branches.merge {
                    m.resolving = false;
                    m.status = a.result["status"].clone();
                }
            }
            (Pending::Resolve, Err(e)) => {
                // Changed underneath: read the status again.
                let again = matches!(e.code, Code::Conflict | Code::RepairConflict);
                if let Some(m) = &mut self.branches.merge {
                    m.resolving = false;
                    m.failed = Some(e);
                }
                if again {
                    self.send(Request::MergeStatus {}, Pending::Merge);
                }
            }
            (Pending::Ghost(key, hash), a) => {
                let Some(c) = &mut self.comparing.compare else {
                    return;
                };
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
            // A delete chosen before this one answers too late to matter:
            // its ghosts and its failures alike are dropped.
            (Pending::Deleted(key), _)
                if !self.model.deleted.as_ref().is_some_and(|d| d.is_for(&key)) => {}
            (Pending::Deleted(key), a) => {
                match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(bodies))) => {
                        for b in bodies {
                            if self.model.deleted.as_mut().is_some_and(|d| d.take(&key, b)) {
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
            (Pending::InsertPreview(key), a) => {
                if let Some(p) = self.forms.insert_preview.as_mut().filter(|p| p.key == key) {
                    p.answer = Some(a.map(|a| crate::core::inserts::Moved::from_answer(&a.result)));
                }
            }
            (Pending::Quiet, Ok(_)) => {}
            // Drawn as its own request answers, not only when the
            // notification comes (#143).
            (Pending::Hidden, Ok(a)) => {
                let me = self.link.participant.clone();
                if let Some(p) = self.session.people.all.get_mut(&me) {
                    p.hidden = a.result["hidden"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect();
                    self.session.people.generation += 1;
                }
                self.follow_focus();
            }
            (Pending::Activity, Ok(a)) => self.activity.reset(&a.result["entries"]),
            (Pending::History, Ok(a)) => self.activity.history = Some(a.result),
            (Pending::Branches, a) => {
                self.branches.branches_waiting = false;
                match a {
                    Ok(a) => {
                        self.branches.listed(a.result);
                        self.ask_preview();
                    }
                    // Said in the tab: outside a repository, there are none.
                    Err(e) => self.branches.error = Some(e),
                }
            }
            (Pending::BranchList, a) => match a {
                Ok(a) => {
                    self.branches.branch_list = Some(a.result);
                    self.branches.branch_error = None;
                }
                // Said in the tab: outside a repository, there are none.
                Err(e) => self.branches.branch_error = Some(e),
            },
            (Pending::Switch, a) => {
                match a {
                    Ok(_) => self.branches.checkout = None,
                    Err(e) => {
                        if let Some(c) = self.branches.checkout.as_mut() {
                            c.busy = false;
                            c.refused = Some(e);
                        }
                    }
                }
                // The head moved, or the list was stale: read both again.
                // The files' changes come as model.changed and
                // project.changed, as an outside change's do.
                self.load_branches();
                self.reload_history();
            }
            (Pending::DepsList, a) => {
                match a {
                    Ok(a) => self.deps.listed(&a.result),
                    Err(e) => self.deps.error = Some(e),
                }
                self.model.set_marks(self.deps.marks());
            }
            (Pending::DepsOutdated, a) => {
                self.deps.deps_waiting = false;
                if let Ok(a) = a {
                    self.deps.outdated(&a.result);
                    self.model.set_marks(self.deps.marks());
                }
                if std::mem::take(&mut self.deps.deps_again) {
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
                    .deps
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
                Ok(_) => self.deps.picker = None,
                Err(e) => {
                    if let Some(p) = self.deps.picker.as_mut() {
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
                self.activity.history = Some(json!({"error": e.message}));
                self.fail(e);
            }
            (Pending::Cache, Ok(a)) => self.session.cache = Some(a.result),
            (Pending::CacheToken, Ok(a)) => {
                self.session.cache_token = Some(a.result);
                self.read_cache();
            }
            // Said beside the field, not only in the status line.
            (Pending::CacheToken, Err(e)) => self.session.cache_token = Some(json!({"error": e})),
            // Only the dialog for that file takes an answer: one opened
            // since asks again.
            (Pending::ImportPlan(key), a) => {
                let Some(i) = self.exchange.import.as_mut().filter(|i| i.key() == key) else {
                    return;
                };
                i.waiting = false;
                match a {
                    Ok(a) => i.plan = Some((key, crate::core::step::Plan::from_answer(&a.result))),
                    Err(e) => i.error = Some(e),
                }
            }
            (Pending::Import(key), a) => {
                let Some(i) = self.exchange.import.as_mut().filter(|i| i.key() == key) else {
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
                if let Some(e) = self.exchange.export.as_mut() {
                    e.answered(a.map(|a| a.result));
                }
            }
            (Pending::DragSolve(part), a) => {
                let mut again = false;
                if let Some(d) = self.assembly.drag.as_mut().filter(|d| d.part == part) {
                    d.waiting = false;
                    again = d.asked.is_some_and(|(_, t)| t != d.translate);
                }
                match a {
                    // Only while it is still dragged: a late answer after
                    // the drop would undo the written move's look.
                    Ok(a) if self.assembly.drag.is_some() => self.preview_poses(&part, &a.result),
                    Ok(_) => {}
                    Err(e) => self.fail(e),
                }
                if again {
                    self.ask_drag();
                }
            }
            (Pending::MoveSolve(part), a) => {
                self.assembly.mover_waiting = false;
                let on = self.assembly.mover.as_ref().is_some_and(|m| m.part == part);
                match a {
                    // A late answer after the tool went would undo the
                    // written move's look.
                    Ok(a) if on => self.preview_poses(&part, &a.result),
                    Ok(_) => {}
                    Err(e) => {
                        if let Some(m) = self.assembly.mover.as_mut().filter(|_| on) {
                            m.error = Some(e.message.clone());
                        }
                    }
                }
            }
            (Pending::MatePreview(part, asked), a) => {
                let Some(d) = self
                    .assembly
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
                    self.annotations.markup = None;
                    if let Some(id) = a.result["annotation"]["id"].as_str() {
                        let id = id.to_owned();
                        self.choose_annotation(Some(&id));
                    }
                }
                // Refused: said on the bar, the marks kept.
                Err(e) => match self.annotations.markup.as_mut() {
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
                if let Some(p) = self.session.people.all.get_mut(&me) {
                    p.sections =
                        serde_json::from_value(a.result["sections"].clone()).unwrap_or_default();
                    self.session.people.generation += 1;
                }
                self.follow_sections();
            }
            (Pending::Explodes(part), Ok(a)) => {
                if self.assembly.explodes.part.as_deref() == Some(part.as_str()) {
                    self.assembly.explodes.listed(&part, &a.result);
                    self.sync_explode();
                }
            }
            (Pending::ExplodeShown, Ok(a)) => {
                let me = self.link.participant.clone();
                if let Some(p) = self.session.people.all.get_mut(&me) {
                    p.explode = crate::core::explode::Shown::from_json(&a.result["explode"]);
                    if p.explode.is_some() {
                        p.motion = None;
                    }
                    self.session.people.generation += 1;
                }
                self.assembly.explodes.sliding = None;
                self.follow_explode();
                self.follow_motion();
            }
            (Pending::Motions(part), Ok(a)) => {
                if self.assembly.motions.part.as_deref() == Some(part.as_str()) {
                    self.assembly.motions.listed(&part, &a.result);
                    if self.assembly.motions.shown.is_some()
                        && !self.assembly.motions.frames_current()
                    {
                        self.read_frames();
                    }
                    self.sync_moved();
                }
            }
            (Pending::MotionFrames(part, id), Ok(a)) => {
                let wanted = self
                    .assembly
                    .motions
                    .shown
                    .as_ref()
                    .is_some_and(|s| s.part == part && s.motion == id);
                if wanted {
                    self.assembly.motions.frames =
                        crate::core::motion::Frames::from_json(&a.result);
                    self.sync_moved();
                }
            }
            (Pending::MotionShown, Ok(a)) => {
                let me = self.link.participant.clone();
                if let Some(p) = self.session.people.all.get_mut(&me) {
                    p.motion = crate::core::motion::Shown::from_json(&a.result["motion"]);
                    if p.motion.is_some() {
                        p.explode = None;
                    }
                    self.session.people.generation += 1;
                }
                self.assembly.motions.sliding = None;
                self.follow_explode();
                self.follow_motion();
            }
            (Pending::MotionSaved(part), r) => match r {
                Ok(_) => {
                    if self.assembly.motions.part.as_deref() == Some(part.as_str()) {
                        self.assembly.motions.editing = None;
                        self.read_motions();
                    }
                }
                Err(e) => self.fail(e),
            },
            (Pending::MotionSwept(part, id), r) => {
                let Some(w) = self
                    .assembly
                    .motions
                    .sweep
                    .as_mut()
                    .filter(|w| w.part == part && w.motion == id)
                else {
                    return;
                };
                match r {
                    Ok(a) => *w = crate::core::motion::Sweep::from_json(&a.result),
                    Err(e) => {
                        w.loaded = true;
                        w.error = Some(e.message.clone());
                    }
                }
                self.model.generation += 1;
            }
            (Pending::ExplodeSaved(part, show), r) => match r {
                Ok(a) => {
                    if self.assembly.explodes.part.as_deref() == Some(part.as_str()) {
                        self.read_explodes();
                    }
                    // A new explode, or one still there, is shown.
                    if let Some(id) = show
                        && a.result.get("steps").is_some()
                        && self
                            .assembly
                            .explodes
                            .shown
                            .as_ref()
                            .is_none_or(|s| s.explode != id)
                    {
                        self.show_explode(Some(&id), Some(1.0), false);
                    }
                    self.assembly.explodes.name.clear();
                }
                Err(e) => {
                    self.sync_explode();
                    self.fail(e);
                }
            },
            (Pending::Views(part), Ok(a)) => {
                if self.view.views.part.as_deref() == Some(part.as_str()) {
                    self.view.views = crate::core::views::NamedViews::of(&part, &a.result);
                }
            }
            (Pending::ViewSaved(part), a) => match a {
                Ok(_) => {
                    if self.view.views.part.as_deref() == Some(part.as_str()) {
                        self.read_views();
                    }
                }
                Err(e) => {
                    self.view.views.error = Some(e.clone());
                    self.fail(e);
                }
            },
            (Pending::Appearance(n), Ok(a)) => {
                if n == self.view.looks_asked {
                    self.view.looks_read = crate::core::appearance::read(&a.result);
                    self.refresh_looks();
                    // An open window shows its target as it is now.
                    let now = self.look_of(self.view.appearance.target());
                    self.view.appearance.reread(now, ryucad::render::SURFACE);
                }
            }
            // The rebuild follows as `model.changed`; an open form on the
            // import is read again, its mode with it.
            (Pending::ImportConvert(part, feature), a) => match a {
                Ok(_) => {
                    let key = (part, feature);
                    if self.forms.form_for.as_ref() == Some(&key) {
                        self.open_form(Some(key));
                    }
                }
                Err(e) => self.fail(e),
            },
            (Pending::AppearanceWritten, a) => {
                // What was dragged stays drawn until the new reading.
                self.view.appearance.dragging = None;
                match a {
                    Ok(_) => self.read_appearance(),
                    Err(e) => {
                        self.view.appearance.error = Some(e.clone());
                        self.refresh_looks();
                        self.fail(e);
                    }
                }
            }
            (Pending::NormalTo, Ok(a)) => {
                self.view.looked = serde_json::from_value(a.result["camera"].clone()).ok();
            }
            (Pending::Measure(picks), a) => {
                match a {
                    Ok(a) => self.view.measure.take(&picks, &a.result),
                    Err(e) => self.view.measure.failed(&picks, e),
                }
                self.model.generation += 1;
            }
            (Pending::Mass(of), a) => {
                let Some(m) = self.view.mass.as_mut().filter(|m| m.of == of) else {
                    return;
                };
                match a {
                    Ok(a) => m.take(&a.result),
                    Err(e) => m.failed(e),
                }
                self.model.generation += 1;
            }
            (Pending::Clearance(part), a) => {
                let Some(c) = self.assembly.clearance.as_mut().filter(|c| c.part == part) else {
                    return;
                };
                match a {
                    Ok(a) => c.take(&a.result),
                    Err(e) => c.failed(e),
                }
                self.model.generation += 1;
            }
            (Pending::Interference(part), a) => {
                let Some(i) = self
                    .assembly
                    .interference
                    .as_mut()
                    .filter(|i| i.part == part)
                else {
                    return;
                };
                match a {
                    Ok(a) => i.take(&a.result),
                    Err(e) => i.failed(e),
                }
                self.model.generation += 1;
            }
            (Pending::Doctor, Ok(a)) => {
                self.session.doctor = Some(crate::core::doctor::report_of(&a.result))
            }
            // A whole reading, or one changed setting: read them all again,
            // which also brings the project revision up to date.
            (Pending::Config, Ok(a)) if a.result.get("settings").is_some() => {
                self.session.settings.reset(&a.result);
                self.model.base = self.display_quality();
            }
            (Pending::Config, Ok(_)) => self.load_settings(),
            (Pending::Form(part, id, before), Ok(a)) => {
                // Only the form last asked for opens (a dimension may have
                // been chosen since); a reload after a conflict only if
                // that form is still open.
                let asked = self.forms.form_for.as_ref() == Some(&(part.clone(), id));
                let wanted = asked
                    && match (&before, &self.forms.form) {
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
                        .pdm
                        .states
                        .by_part
                        .get(placed)
                        .and_then(|s| s["label"].as_str())
                    {
                        form.offer_revs(&[l.to_owned()]);
                    }
                    // An insert of this project's part may take any of its
                    // official labels (design 521): every one, read now.
                    if form.ty == "insert" && form.foreign.is_none() {
                        let req = Request::ReleaseList {
                            part: Some(placed.to_owned()),
                        };
                        self.send(req, Pending::FormLabels(part.clone(), form.feature.clone()));
                    }
                    self.forms.form = Some(form);
                }
            }
            (Pending::FormLabels(part, id), Ok(a)) => {
                if let Some(f) = self
                    .forms
                    .form
                    .as_mut()
                    .filter(|f| f.part == part && f.feature == id)
                {
                    // An insert's from `release.list`, an instance's of
                    // another project from `deps.parts`.
                    let of = f.of.clone().unwrap_or_default();
                    let labels = crate::core::inserts::official_labels(&a.result, &of);
                    f.offer_revs(&labels);
                }
            }
            // Not fetched: the label stays as read; the Dependencies tab says why.
            (Pending::FormLabels(..), Err(_)) => {}
            (Pending::Dim(part, sketch, constraint, before), Ok(a)) => {
                // Only the dimension last asked for opens, as for features.
                let key = (part.clone(), sketch.clone(), constraint.clone());
                let wanted = self.forms.dimension_for.as_ref() == Some(&key)
                    && match (&before, &self.forms.dimension) {
                        (Some(_), Some(_)) | (None, _) => true,
                        (Some(_), None) => false,
                    };
                if wanted {
                    self.forms.dimension = match before {
                        Some(b) => b.reloaded(&a.result),
                        None => DimForm::from_get(&part, &sketch, &constraint, &a.result),
                    };
                }
            }
            (Pending::Params(part, before), Ok(a)) => {
                if self.parameters.params_part.as_deref() == Some(part.as_str()) {
                    let unit = self.model.default_unit.clone();
                    self.parameters.params = Some(match before {
                        Some(b) => b.reloaded(&a.result, &unit),
                        None => Params::from_list(&a.result, &unit),
                    });
                }
            }
            (Pending::ParamSave, Ok(_)) => {
                if let Some(p) = &self.parameters.params {
                    let part = p.part.clone();
                    self.read_params(part, None);
                }
            }
            (Pending::Equations(conflicted), Ok(a)) => {
                let unit = self.model.default_unit.clone();
                self.parameters.equations = Some(match &self.parameters.equations {
                    Some(now) => now.reloaded(&a.result, &unit, conflicted),
                    None => Equations::from_list(&a.result, &unit),
                });
            }
            (Pending::Equations(_), Err(e)) => {
                let mut table = self.parameters.equations.take().unwrap_or_default();
                table.saving = None;
                table.error = Some(e);
                self.parameters.equations = Some(table);
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
                if drawn != self.drawings.drawing_rows {
                    self.drawings.drawing_rows = drawn;
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
                if let Some(e) = &mut self.parameters.equations {
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
                if let Some(e) = &mut self.parameters.equations {
                    e.saving = None;
                    if err.code == Code::Conflict {
                        self.read_equations(true);
                    } else {
                        e.error = Some(err);
                    }
                }
            }
            (Pending::ParamSave, Err(e)) => {
                if let Some(p) = &mut self.parameters.params {
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
                if let Some(f) = &self.forms.dimension {
                    let (p, s, c) = (f.part.clone(), f.sketch.clone(), f.constraint.clone());
                    self.read_dimension(p, s, c, None);
                }
            }
            (Pending::DimSave, Err(e)) => {
                if let Some(f) = &mut self.forms.dimension {
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
                if let Some(f) = &self.forms.form {
                    let mut v = a.result["feature"].clone();
                    v["revision"] = a.result["revision"].clone();
                    self.forms.form = Some(Form::from_show_in(&f.part, &v, self.unit()));
                }
            }
            (Pending::Save, Err(e)) => {
                // A feature that would change a reference import: the
                // card offers Convert to solid, beside the form's own.
                if e.code == Code::ReferenceBody {
                    self.fail(e.clone());
                }
                if let Some(f) = &mut self.forms.form {
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
            // A part that doesn't rebuild has no holes to show.
            (Pending::Holes(part, revision), a) => {
                self.model.holes_answered(&part, &revision);
                let mut t = match a {
                    Ok(a) => crate::core::threads::HoleTable::from_list(&a.result),
                    Err(_) => crate::core::threads::HoleTable::default(),
                };
                t.revision = revision;
                self.model.take_holes(&part, t);
            }
            (Pending::Sweep(key, _), a) => {
                if self.model.made_for.as_ref() == Some(&key) {
                    match a {
                        Ok(a) => {
                            self.model.swept =
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
                self.model.sheets.take_table(key, revision, t);
                self.model.generation += 1;
            }
            (Pending::FlatLines(key, revision), a) => {
                let f = a.map(|a| crate::core::flat::FlatLines::from_answer(&a.result));
                if let Some(v) =
                    self.model.flat_view.as_mut().filter(|v| {
                        (v.part.clone(), v.solid.clone()) == key && v.revision == revision
                    })
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
                self.model.sheets.take_flat(key, revision, f);
                self.turn_to_flat();
                self.model.generation += 1;
            }
            (Pending::FlatMesh(key, revision), a) => {
                let wanted = self.model.flat_view.as_ref().is_some_and(|v| {
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
                        if let Some(v) = &mut self.model.flat_view {
                            v.solid.clone_from(&body.feature);
                        }
                        self.model.show_flat(Some((solid, body)));
                        self.turn_to_flat();
                    }
                    Ok(Some(Err(e))) | Err(e) => {
                        // Refused (an edit left the solid unfolding no
                        // more): the folded body is drawn again, and the
                        // view keeps the refusal, not the stale flat.
                        if let Some(v) = &mut self.model.flat_view {
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
                if self.model.made_for.as_ref() == Some(&key) {
                    match a {
                        Ok(a) => {
                            self.model.made = Some(Made::from_inspect(&key, &revision, &a.result))
                        }
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
                    .annotations
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
                    .any(|p| self.pdm.bom.reads(p))
                    || params["removed"].as_array().is_some_and(|r| !r.is_empty())
                    || self.pdm.bom.root.is_none()
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
                        self.model.loading.remove(id);
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
                if self.pdm.releases_open {
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
                self.session.settings.changed(params);
                if params["key"] == "display.quality" {
                    self.model.base = self.display_quality();
                    let all = self.model.clear_meshes();
                    self.fetch(all);
                    for part in self.model.clear_sketches() {
                        self.fetch_sketches(&part);
                    }
                    if let Some(c) = &mut self.comparing.compare {
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
                self.branches.merge = match (params["merging"].as_bool(), params.get("status")) {
                    (Some(true), Some(s)) => {
                        Some(Merge::of(s.clone()).keeping(self.branches.merge.as_ref()))
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
                self.pdm.states.changed(params);
                if self.pdm.releases_open {
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
                self.session.people.changed(params);
                self.follow_draft();
                self.follow_focus();
                self.follow_sections();
                self.follow_explode();
                self.follow_motion();
                // Following someone who left ends.
                if self
                    .session
                    .following
                    .as_ref()
                    .is_some_and(|f| self.session.people.get(f).is_none())
                {
                    self.session.following = None;
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
            .assembly
            .explodes
            .part
            .as_deref()
            .is_some_and(|p| report["part"] == p)
        {
            self.read_explodes();
        }
        // Its motions and their frames may have changed with it; the last
        // sweep goes stale (asked again only when someone asks).
        if self
            .assembly
            .motions
            .part
            .as_deref()
            .is_some_and(|p| report["part"] == p)
        {
            self.assembly.motions.frames = None;
            self.read_motions();
            if let Some(w) = self.assembly.motions.sweep.as_mut() {
                w.stale = true;
            }
        }
        // Its named views may have changed with it.
        if self
            .view
            .views
            .part
            .as_deref()
            .is_some_and(|p| report["part"] == p)
        {
            self.read_views();
        }
        // Interference found for this assembly goes stale: it is asked
        // again only when someone asks (design 525).
        if let Some(i) = self
            .assembly
            .interference
            .as_mut()
            .filter(|i| report["part"] == i.part.as_str())
        {
            i.mark_stale();
            self.model.generation += 1;
        }
        // So are clearances, a weight and the Measure tool's picks that
        // read the part.
        let changed = report["part"].as_str().unwrap_or_default();
        let reads = |r: &str| r == changed || r.starts_with(&format!("{changed}/"));
        if self
            .assembly
            .clearance
            .as_ref()
            .is_some_and(|c| c.part == changed)
        {
            self.check_clearance();
        }
        if let Some(of) = self
            .view
            .mass
            .as_ref()
            .map(|m| m.of.clone())
            .filter(|of| reads(of))
        {
            self.weigh(&of);
        }
        if self.view.measure.picks.iter().any(|p| reads(p)) {
            self.measure_again();
        }
        // A form nobody is typing in follows the files.
        if let Some(f) = &self.forms.form
            && report["part"] == f.part.as_str()
            && report["revision"] != f.revision.as_str()
            && !f.dirty()
            && !f.saving
        {
            let (part, id) = (f.part.clone(), f.feature.clone());
            self.read_form(part, id, None);
        }
        // So do the parameters, and a dimension's form.
        if let Some(p) = &self.parameters.params
            && report["part"] == p.part.as_str()
            && report["revision"] != p.revision.as_str()
            && !p.dirty()
            && !p.saving
        {
            let part = p.part.clone();
            self.read_params(part, None);
        }
        if let Some(p) = &self.pdm.props
            && report["part"] == p.part.as_str()
            && report["revision"] != p.revision.as_str()
            && !p.dirty()
            && !p.saving
        {
            let part = p.part.clone();
            self.read_props(part, None);
        }
        if let Some(f) = &self.forms.dimension
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
        if let Some(key) = self.model.made_for.clone()
            && report["part"] == key.0.as_str()
            && self
                .model
                .made
                .as_ref()
                .is_none_or(|m| report["revision"] != m.revision.as_str())
        {
            self.ask_made(key);
        }
        // So does what a chosen unfold or refold paints.
        if let Some(key) = self.model.made_for.clone()
            && report["part"] == key.0.as_str()
        {
            self.ask_sheet_paint(&key);
        }
        // And the flat view, read again at the part's new revision.
        if let Some(v) = &mut self.model.flat_view
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
        if let Some(key) = self.model.made_for.clone()
            && report["part"] == key.0.as_str()
            && self.model.swept_at.as_deref() != report["revision"].as_str()
        {
            self.model.swept = None;
            self.ask_swept(&key);
        }
        if let Some(part) = report["part"].as_str() {
            self.fetch_sketches(part);
        }
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
        search: Vec::new(),
    }
}

/// `path` made absolute against the app's own folder: the daemon reads a
/// relative path against the project's.
fn absolute(path: &str) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path))
}

#[cfg(test)]
mod tests;
