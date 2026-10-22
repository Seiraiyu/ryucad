//! An open project: the link to its daemon and what the window knows, kept
//! up to date from answers and notifications.

use crate::core::activity::{Activity, LOADED};
use crate::core::compare::Compare;
use crate::core::edit::{DimForm, Form};
use crate::core::link::{Event, Link, Wake};
use crate::core::model::{BodyKey, Model, Part};
use crate::core::params::Params;
use crate::core::people::{People, Person};
use crate::core::settings::Settings;
use ryucad::client::Daemon;
use ryucad::error::{Code, Error, Result};
use ryucad::live::SelectMode;
use ryucad::proto::{Answer, Request, Transfer};
use ryucad::view::{Camera, CameraArg, Projection, ProjectionArg};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// What a request was for, so its answer lands in the right place.
#[derive(Clone, Debug, PartialEq)]
enum Pending {
    Info,
    Parts,
    Rebuild(String),
    /// A body's mesh, asked for at this hash.
    Mesh(BodyKey, String),
    /// A sketch's drawing, asked for at this part revision.
    Sketch(BodyKey, String),
    People,
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
    Config,
    /// The diff of a compare, the `n`th asked for.
    Compare(u64),
    /// A ghost's mesh, asked for at this hash.
    Ghost(BodyKey, String),
    Doctor,
    /// Only a failure matters.
    Quiet,
}

/// How often the window's camera is shared at most while it moves: often
/// enough for a follower, rarely enough not to flood watching agents.
pub const CAMERA_EVERY: Duration = Duration::from_millis(150);
/// How long the status line shows a failed request.
pub const ERROR_SHOWN: Duration = Duration::from_secs(8);

/// How long a window that lost its daemon waits before trying again, at
/// first and at most: each failure doubles the wait.
pub const RETRY_FIRST: Duration = Duration::from_millis(250);
pub const RETRY_MAX: Duration = Duration::from_secs(5);

/// A lost window's next attempt to reconnect.
struct Retry {
    at: Instant,
    delay: Duration,
    /// An attempt in progress, on a thread: connecting may start a daemon.
    trying: Option<Receiver<Result<Link>>>,
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
    pub people: People,
    /// The participant whose camera this window follows.
    pub following: Option<String>,
    /// The property panel's form, for the feature chosen in the tree.
    pub form: Option<Form>,
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
    /// This window's compare with a Git revision, while it is in one: it
    /// follows the window's participant's compare state.
    pub compare: Option<Compare>,
    /// How many diffs were asked for, and whether one is on its way; one
    /// asked for meanwhile waits for it (`compare_again`).
    compare_asked: u64,
    compare_waiting: bool,
    compare_again: bool,
    pub settings: Settings,
    /// The last doctor report, once asked for.
    pub doctor: Option<Value>,
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
    /// How many meshes were fetched: an unchanged body is never fetched
    /// twice.
    pub fetched: u64,
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
            people,
            following: None,
            form: None,
            dimension: None,
            form_for: None,
            dimension_for: None,
            params: None,
            params_part: None,
            activity: Activity::default(),
            history: None,
            compare: None,
            compare_asked: 0,
            compare_waiting: false,
            compare_again: false,
            settings: Settings::default(),
            doctor: None,
            raised: false,
            camera_due: None,
            camera_sent: None,
            camera_last: None,
            errors: Vec::new(),
            error_at: None,
            lost: false,
            lost_why: None,
            fetched: 0,
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
        self.send(Request::ProjectInfo {}, Pending::Info);
        self.send(Request::PartList {}, Pending::Parts);
        let log = Request::ActivityLog {
            limit: Some(LOADED),
            before: None,
        };
        self.send(log, Pending::Activity);
        self.load_settings();
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
        let project = self
            .settings
            .all
            .iter()
            .any(|s| s.key == key && s.scope == "project");
        let req = Request::ConfigSet {
            key: key.to_owned(),
            value,
            scope: None,
            expected_revision: project.then(|| self.settings.project_revision.clone()),
        };
        self.send(req, Pending::Config);
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

    /// Diagnoses the project (`doctor`).
    pub fn diagnose(&mut self) {
        self.send(Request::Doctor {}, Pending::Doctor);
    }

    /// Applies repairs from the last report (`doctor.repair`); the answer
    /// brings the new report.
    pub fn repair(&mut self, repairs: Vec<String>, params: serde_json::Map<String, Value>) {
        self.send(Request::DoctorRepair { repairs, params }, Pending::Doctor);
    }

    /// Reads the project's Git history again.
    pub fn load_history(&mut self) {
        let req = Request::HistoryLog {
            part: None,
            limit: Some(100),
        };
        self.send(req, Pending::History);
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
            }
            (Some(_), None) => {
                self.compare = None;
                self.model.generation += 1;
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
        for (key, hash) in c.ask() {
            let req = Request::MeshGet {
                part: key.0.clone(),
                feature: Some(key.1.clone()),
                quality: None,
                transfer: Some(Transfer::Binary),
                rev: Some(rev.clone()),
            };
            self.send(req, Pending::Ghost(key, hash));
        }
    }

    fn send(&mut self, req: Request, why: Pending) {
        let ticket = self.link.send(req);
        self.pending.insert(ticket, why);
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
        self.errors.push(e);
        self.error_at = Some(Instant::now());
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
        if self.lost {
            self.reconnect(now);
            return;
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
        // A new session knows nothing of this window's compare either.
        if let Some(rev) = self.compare.as_ref().map(|c| c.rev.clone()) {
            self.set_compare(Some(rev));
            self.ask_diff();
        }
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
            Event::Answer { ticket, answer } => {
                if let Some(why) = self.pending.remove(&ticket) {
                    self.answered(why, answer);
                }
            }
            Event::Note { method, params } => self.noted(&method, &params),
            Event::Lost if !self.lost => {
                self.lost = true;
                self.pending.clear();
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
                self.model.asked.clear();
                self.model.sketches_asked.clear();
                self.compare_waiting = false;
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
        match (why, answer) {
            (Pending::Info, Ok(a)) => {
                self.model.default_unit = a.result["default_unit"].as_str().unwrap_or("mm").into();
            }
            (Pending::Parts, Ok(a)) => {
                let listed: Vec<&str> = a.result["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p["part"].as_str())
                    .collect();
                // Parts deleted while the window wasn't listening go.
                let gone: Vec<String> = self
                    .model
                    .parts
                    .keys()
                    .filter(|id| !listed.contains(&id.as_str()))
                    .cloned()
                    .collect();
                for id in gone {
                    self.model.remove_part(&id);
                }
                for p in a.result["parts"].as_array().into_iter().flatten() {
                    let id = p["part"].as_str().unwrap_or_default().to_owned();
                    match p.get("error") {
                        Some(e) => {
                            self.model.apply_part(
                                &json!({"part": id, "revision": p["revision"], "error": e}),
                            );
                        }
                        None => self.send(
                            Request::Rebuild {
                                part: Some(id.clone()),
                                rev: None,
                            },
                            Pending::Rebuild(id),
                        ),
                    }
                }
            }
            (Pending::Rebuild(id), r) => {
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
            (Pending::Ghost(key, hash), a) => {
                let Some(c) = &mut self.compare else { return };
                c.answered(&key, &hash);
                match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(bodies))) => {
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
            (Pending::Activity, Ok(a)) => self.activity.reset(&a.result["entries"]),
            (Pending::History, Ok(a)) => self.history = Some(a.result),
            // Said in the tab, not left spinning.
            (Pending::History, Err(e)) => {
                self.history = Some(json!({"error": e.message}));
                self.fail(e);
            }
            (Pending::Doctor, Ok(a)) => {
                self.doctor = Some(crate::core::doctor::report_of(&a.result))
            }
            // A whole reading, or one changed setting: read them all again,
            // which also brings the project revision up to date.
            (Pending::Config, Ok(a)) if a.result.get("settings").is_some() => {
                self.settings.reset(&a.result);
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
                    self.form = Some(match before {
                        Some(b) => b.reloaded(&a.result),
                        None => Form::from_show(&part, &a.result),
                    });
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
                    self.form = Some(Form::from_show(&f.part, &v));
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
            (Pending::Mesh(key, hash), a) => {
                self.model.answered(&key, &hash);
                match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(bodies))) => {
                        for b in bodies {
                            self.model.take_mesh(&key.0, b);
                        }
                    }
                    Ok(Some(Err(e))) | Err(e) => self.fail(e),
                    Ok(None) => {
                        self.fail(Error::new(Code::Daemon, "a mesh came without its bytes"));
                    }
                }
            }
            (_, Err(e)) => self.fail(e),
        }
    }

    fn noted(&mut self, method: &str, params: &Value) {
        match method {
            "model.changed" => {
                for report in params["parts"].as_array().into_iter().flatten() {
                    self.part_changed(report);
                }
                for id in params["removed"].as_array().into_iter().flatten() {
                    self.model.remove_part(id.as_str().unwrap_or_default());
                }
                // What the working files differ by changed with them.
                self.ask_diff();
            }
            "project.changed" => {
                // Parts came or went: damage may have too.
                if self.doctor.is_some() {
                    self.diagnose();
                }
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
            "activity.appended" => self.activity.appended(params),
            "app.open" => self.raised = true,
            "participant.changed" => {
                self.people.changed(params);
                self.follow_compare();
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

    /// Fetches display meshes, one body per request: a part with one
    /// changed feature sends one mesh, and a body already asked for at its
    /// current hash isn't asked for again.
    fn fetch(&mut self, bodies: Vec<BodyKey>) {
        for (key, hash) in self.model.ask(bodies) {
            self.fetched += 1;
            let req = Request::MeshGet {
                part: key.0.clone(),
                feature: Some(key.1.clone()),
                quality: None,
                transfer: Some(Transfer::Binary),
                rev: None,
            };
            self.send(req, Pending::Mesh(key, hash));
        }
    }

    /// The part `id`, if the window knows it.
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.model.parts.get(id)
    }
}
