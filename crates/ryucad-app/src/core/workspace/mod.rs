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
use route::{Effects, Then};
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
mod route;
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
    /// A model edit sent as a proposal in Annotate: what its own answer
    /// was for.
    Proposal(Box<Pending>, bool),
    /// Only a failure matters.
    Quiet,
    Model(model::Asked),
    Forms(forms::Asked),
    Parameters(parameters::Asked),
    Drawings(drawings::Asked),
    Activity(activity::Asked),
    Branches(branches::Asked),
    Deps(deps::Asked),
    Pdm(pdm::Asked),
    Annotations(annotations::Asked),
    Comparing(comparing::Asked),
    Session(session::Asked),
    Assembly(assembly::Asked),
    View(view::Asked),
    Exchange(exchange::Asked),
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
        self.send(Request::ProjectInfo {}, Pending::Model(model::Asked::Info));
        self.send(Request::PartList {}, Pending::Model(model::Asked::Parts));
        self.read_drawn();
        let log = Request::ActivityLog {
            limit: Some(LOADED),
            before: None,
        };
        self.send(log, Pending::Activity(activity::Asked::Activity));
        self.load_settings();
        self.send(
            Request::MergeStatus {},
            Pending::Branches(branches::Asked::Merge),
        );
        self.send(
            Request::ReleaseList { part: None },
            Pending::Pdm(pdm::Asked::States),
        );
        self.send(Request::PdmConfig {}, Pending::Pdm(pdm::Asked::PdmConfig));
        self.read_appearance();
        self.load_deps();
        let list = Request::AnnotationList {
            status: Vec::new(),
            part: None,
            by: None,
            open: false,
        };
        self.send(list, Pending::Annotations(annotations::Asked::Annotations));
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
        self.model.unit()
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
            matches!(why, Pending::Session(session::Asked::Doctor)),
            matches!(why, Pending::Activity(activity::Asked::History)),
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
            (Pending::Quiet, Ok(_)) => {}
            (Pending::Model(a), r) => self.answer_model(a, r),
            (Pending::Forms(a), r) => {
                let mut fx = Effects::default();
                self.forms
                    .answer(a, r, &self.model, &self.pdm.states, &mut fx);
                self.apply(fx);
            }
            (Pending::Parameters(a), r) => {
                let mut fx = Effects::default();
                self.parameters
                    .answer(a, r, &self.model.default_unit, &mut fx);
                self.apply(fx);
            }
            (Pending::Drawings(a), r) => {
                let mut fx = Effects::default();
                self.drawings.answer(a, r, &mut fx);
                self.apply(fx);
            }
            (Pending::Activity(a), r) => self.answer_activity(a, r),
            (Pending::Branches(a), r) => self.answer_branches(a, r),
            (Pending::Deps(a), r) => self.answer_deps(a, r),
            (Pending::Pdm(a), r) => {
                let mut fx = Effects::default();
                self.pdm.answer(a, r, &mut fx);
                self.apply(fx);
            }
            (Pending::Annotations(a), r) => self.answer_annotations(a, r),
            (Pending::Comparing(a), r) => self.answer_comparing(a, r),
            (Pending::Session(a), r) => self.answer_session(a, r),
            (Pending::Assembly(a), r) => self.answer_assembly(a, r),
            (Pending::View(a), r) => self.answer_view(a, r),
            (Pending::Exchange(a), r) => {
                let mut fx = Effects::default();
                self.exchange.answer(a, r, &mut fx);
                self.apply(fx);
            }
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
                    self.send(req, Pending::Pdm(pdm::Asked::ReleaseCheck));
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
                self.send(Request::PartList {}, Pending::Model(model::Asked::Parts));
                // The manifest may have its ID now, or ryupdm.json changed.
                self.send(Request::PdmConfig {}, Pending::Pdm(pdm::Asked::PdmConfig));
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
                self.send(
                    Request::ParticipantList {},
                    Pending::Session(session::Asked::People),
                );
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
