//! An open project: the link to its daemon and what the window knows, kept
//! up to date from answers and notifications.

use crate::core::activity::{Activity, LOADED};
use crate::core::bom::BomTab;
use crate::core::branches::Branches;
use crate::core::compare::Compare;
use crate::core::edit::{DimForm, Form};
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
    Parts,
    Rebuild(String),
    /// A body's mesh, asked for at this hash.
    Mesh(BodyKey, String),
    /// Several bodies of one assembly's occurrences, each at its hash.
    Meshes(Vec<(BodyKey, String)>),
    /// A body's mesh at another level of detail.
    Level(String, ryucad::mesh::Quality),
    /// A sketch's drawing, asked for at this part revision.
    Sketch(BodyKey, String),
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
    Activity,
    History,
    /// The Branches tab's worktrees (`worktree.list`).
    Branches,
    /// A merge preview with the worktree at this path, made from this key.
    Preview(String, String),
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
    /// An import's dry run, for the file at this path.
    ImportPlan(String),
    /// An import or update of the file at this path, written.
    Import(String),
    /// An export's bytes, to write at this path.
    Export(PathBuf),
    /// A drag's preview solve (`assembly.solve`), for this assembly.
    DragSolve(String),
    /// A drag's move, written (`assembly.move`).
    DragMove(String),
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
    /// This window's own `section.set`.
    Sections,
    /// An assembly's explodes (`explode.list`).
    Explodes(String),
    /// This window's own `explode.show`.
    ExplodeShown,
    /// An assembly's explode written or deleted, to show `explode` next
    /// if it is still there.
    ExplodeSaved(String, Option<String>),
    /// Only a failure matters.
    Quiet,
}

/// How often a drag asks the daemon to solve at most.
pub const DRAG_EVERY: Duration = Duration::from_millis(50);

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
/// (hex), at a part revision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Made {
    pub feature: BodyKey,
    pub revision: String,
    pub faces: std::collections::BTreeMap<BodyKey, std::collections::BTreeSet<String>>,
}

impl Made {
    /// The faces of an `inspect` of the model whose origin is `feature`.
    pub fn from_inspect(feature: &BodyKey, revision: &str, v: &Value) -> Made {
        let mut faces = std::collections::BTreeMap::new();
        for b in v["bodies"].as_array().into_iter().flatten() {
            let Some(solid) = b["solid"].as_str() else {
                continue;
            };
            let names: std::collections::BTreeSet<String> = b["faces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|f| f["origin"]["feature"] == feature.1.as_str())
                .filter_map(|f| f["name"].as_str().map(str::to_owned))
                .collect();
            if !names.is_empty() {
                faces.insert((feature.0.clone(), solid.to_owned()), names);
            }
        }
        Made {
            feature: feature.clone(),
            revision: revision.to_owned(),
            faces,
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
    pub model: Model,
    /// Parts whose reports are asked for and not yet answered.
    pub loading: BTreeSet<String>,
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
    /// File > Export STEP…, while open.
    pub export: Option<crate::core::step::Export>,
    /// The instance being dragged, if any.
    pub drag: Option<Drag>,
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
    fn unit(&self) -> ryucad::units::Unit {
        ryucad::units::Unit::parse(&self.model.default_unit).unwrap_or(ryucad::units::Unit::Mm)
    }

    /// Opens the project at `root` (canonical) as the human `name`, and
    /// starts loading it.
    pub fn open(root: &Path, daemon: &Daemon, name: &str, wake: Wake) -> Result<Workspace> {
        let link = Link::open(root, daemon, name, wake.clone())?;
        let mut people = People::default();
        people.reset(&link.session["participants"]);
        let mut ws = Workspace {
            root: root.to_path_buf(),
            link,
            model: Model::default(),
            loading: BTreeSet::new(),
            wanted: Default::default(),
            people,
            following: None,
            form: None,
            made: None,
            made_for: None,
            dimension: None,
            form_for: None,
            dimension_for: None,
            params: None,
            params_part: None,
            activity: Activity::default(),
            history: None,
            history_at: None,
            history_waiting: false,
            history_again: false,
            branches: Branches::default(),
            branches_at: None,
            branches_waiting: false,
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
            drag: None,
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
        self.send(Request::ProjectInfo {}, Pending::Info);
        self.send(Request::PartList {}, Pending::Parts);
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

    /// Saves `camera` as named view `id` of the part shown (`view.save`).
    pub fn save_view(&mut self, id: &str, camera: &Camera) {
        let Some(part) = self.views.part.clone() else {
            return;
        };
        self.views.error = None;
        let req = Request::ViewSave {
            part: part.clone(),
            view: id.to_owned(),
            camera: Some(camera_arg(camera)),
            participant: None,
            expected_revision: None,
        };
        self.send(req, Pending::ViewSaved(part));
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
            rev: None,
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
            rev: None,
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
            rev: None,
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
            expected_revision: revision,
        };
        self.send(req, Pending::DragMove(d.part));
    }

    /// Asks what importing the open import's file would do: `import` as
    /// a dry run, an update's when it replaces a file the project holds.
    pub fn plan_import(&mut self) {
        let Some(i) = self.import.as_mut() else {
            return;
        };
        let path = i.path.trim().to_owned();
        if path.is_empty() {
            return;
        }
        i.waiting = true;
        i.error = None;
        i.done = None;
        let req = import_request(&path, i.update.clone(), true);
        self.send(req, Pending::ImportPlan(path));
    }

    /// Imports the open import's file, as its plan said: the same
    /// `import` an agent sends, one logged, undoable change.
    pub fn run_import(&mut self) {
        let Some(i) = self.import.as_mut() else {
            return;
        };
        let path = i.path.trim().to_owned();
        i.waiting = true;
        i.error = None;
        let req = import_request(&path, i.update.clone(), false);
        self.send(req, Pending::Import(path));
    }

    /// Exports the open export's part, writing the bytes where it says.
    pub fn run_export(&mut self) {
        let Some(e) = self.export.as_mut() else {
            return;
        };
        let path = absolute(e.path.trim());
        e.waiting = true;
        e.error = None;
        e.done = None;
        let req = Request::Export {
            part: e.part.clone(),
            rev: None,
            format: None,
            out: None,
            schema: Some(e.schema.clone()),
            unit: Some(e.unit.clone()),
            time_stamp: None,
            file_name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
            chord: None,
            angle: None,
            max_edge: None,
            ascii: false,
            brep: None,
            mesh: None,
            pipe_tolerance: None,
        };
        self.send(req, Pending::Export(path));
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
        let want = self.myself().and_then(|m| m.compare.clone());
        match (&self.compare, want) {
            (Some(c), Some(w)) if c.rev == w => {}
            (_, Some(w)) => {
                self.compare = Some(Compare::new(&w));
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
            to: None,
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
            };
            self.send(req, Pending::Ghost(key, hash));
        }
    }

    fn send(&mut self, req: Request, why: Pending) {
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
        self.made_for.clone_from(&feature);
        if let Some(key) = feature {
            self.ask_made(key);
        }
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
            });
            self.model.generation += 1;
            return;
        }
        if p.feature(&key.1).is_none_or(|f| f.sketch.is_some()) {
            return;
        }
        let revision = p.revision.clone();
        let req = Request::Inspect {
            part: key.0.clone(),
            feature: None,
            entity: None,
            rev: None,
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
        let req = Request::ParamList { part: part.clone() };
        self.send(req, Pending::Params(part, before));
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
        let req = Request::PropsGet { part: part.clone() };
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
            };
            self.send(req, Pending::WhereUsed(part));
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
        self.lost = false;
        self.lost_why = None;
        self.toasts
            .note("Connected to the daemon again", false, Instant::now());
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
        if let Some(rev) = self.compare.as_ref().map(|c| c.rev.clone()) {
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
                self.toasts
                    .note("Lost the daemon: reconnecting", true, Instant::now());
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
            (Pending::Rebuild(id), r) => {
                self.loading.remove(&id);
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
                self.follow_compare();
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
                    self.where_used = Some(match r {
                        Ok(a) => WhereUsed::from_answer(&part, &a.result),
                        Err(e) => WhereUsed::failed(&part, e),
                    });
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
            (Pending::ImportPlan(path), a) => {
                let Some(i) = self.import.as_mut().filter(|i| i.path.trim() == path) else {
                    return;
                };
                i.waiting = false;
                match a {
                    Ok(a) => i.plan = Some((path, crate::core::step::Plan::from_answer(&a.result))),
                    Err(e) => i.error = Some(e),
                }
            }
            (Pending::Import(path), a) => {
                let Some(i) = self.import.as_mut().filter(|i| i.path.trim() == path) else {
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
            (Pending::Export(path), a) => {
                let Some(e) = self.export.as_mut() else {
                    return;
                };
                e.waiting = false;
                let written = a.and_then(|a| {
                    let bytes = a.binary.ok_or_else(|| {
                        Error::new(Code::Internal, "the export came without its bytes")
                    })?;
                    ryucad::doc::project::write_atomic(&path, &bytes)?;
                    Ok(bytes.len())
                });
                match written {
                    Ok(n) => e.done = Some((path.display().to_string(), n)),
                    Err(err) => e.error = Some(err),
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
                }
            }
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
                        form.offer(&p.founders());
                    }
                    // An instance may take its part's latest release.
                    let placed = a.result["part"].as_str().unwrap_or_default();
                    if let Some(l) = self
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
                if let Some(f) = &mut self.form {
                    f.saving = false;
                    if e.code == Code::Conflict {
                        // Read it again and show what changed underneath.
                        let (part, id) = (f.part.clone(), f.feature.clone());
                        let before = Box::new(f.clone());
                        self.read_form(part, id, Some(before));
                    } else {
                        f.error = Some(e);
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
                    // An assembly's occurrences come only when asked for.
                    if report["kind"] == "assembly" && report["occurrences"].is_null() {
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
            }
            "project.changed" => {
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
                self.merge = match (params["merging"].as_bool(), params.get("status")) {
                    (Some(true), Some(s)) => Some(Merge::of(s.clone())),
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
                self.follow_compare();
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
        if let Some(part) = report["part"].as_str() {
            self.fetch_sketches(part);
        }
    }

    /// Fetches the drawings of `part`'s sketches that are missing or
    /// older than the part, one sketch per request.
    fn fetch_sketches(&mut self, part: &str) {
        for (key, revision) in self.model.ask_sketches(part) {
            let req = Request::SketchGet {
                part: key.0.clone(),
                sketch: key.1.clone(),
                quality: None,
            };
            self.send(req, Pending::Sketch(key, revision));
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
                        rev: None,
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
                    rev: None,
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
                rev: None,
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
        let req = Request::Rebuild {
            part: Some(id.to_owned()),
            rev: None,
        };
        self.send(req, Pending::Rebuild(id.to_owned()));
    }
}

/// An `import` of the file at `path`, replacing `update` when given; a
/// dry run plans without writing.
fn import_request(path: &str, update: Option<String>, dry_run: bool) -> Request {
    Request::Import {
        path: absolute(path).display().to_string(),
        part: None,
        into: None,
        product: None,
        dry_run,
        expected_revision: None,
        update,
    }
}

/// `path` made absolute against the app's own folder: the daemon reads a
/// relative path against the project's.
fn absolute(path: &str) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path))
}
