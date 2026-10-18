//! An open project: the link to its daemon and what the window knows, kept
//! up to date from answers and notifications.

use crate::core::activity::{Activity, LOADED};
use crate::core::edit::Form;
use crate::core::link::{Event, Link, Wake};
use crate::core::model::{BodyKey, Model, Part};
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
    Mesh(String),
    People,
    /// A feature read for the form; after a conflict, with the form as it
    /// was when the save failed.
    Form(String, Option<Box<Form>>),
    Save,
    Activity,
    History,
    Config,
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
    pub activity: Activity,
    /// The project's Git history (`history.log`), once asked for.
    pub history: Option<Value>,
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
            activity: Activity::default(),
            history: None,
            settings: Settings::default(),
            doctor: None,
            raised: false,
            camera_due: None,
            camera_sent: None,
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

    /// Opens the form for a feature (or closes it, with `None`).
    pub fn open_form(&mut self, feature: Option<(String, String)>) {
        self.form = None;
        if let Some((part, id)) = feature {
            self.send(
                Request::FeatureShow {
                    part: part.clone(),
                    id,
                },
                Pending::Form(part, None),
            );
        }
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

    /// Sends a camera that is due, and reconnects a lost window when it is
    /// time. Call it every frame.
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
        if self
            .following
            .as_ref()
            .is_some_and(|f| self.people.get(f).is_none())
        {
            self.following = None;
        }
        self.load();
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
            (Pending::People, Ok(a)) => self.people.reset(&a.result["participants"]),
            (Pending::Quiet, Ok(_)) => {}
            (Pending::Activity, Ok(a)) => self.activity.reset(&a.result["entries"]),
            (Pending::History, Ok(a)) => self.history = Some(a.result),
            (Pending::Doctor, Ok(a)) => {
                self.doctor = Some(crate::core::doctor::report_of(&a.result))
            }
            // A whole reading, or one changed setting: read them all again,
            // which also brings the project revision up to date.
            (Pending::Config, Ok(a)) if a.result.get("settings").is_some() => {
                self.settings.reset(&a.result);
            }
            (Pending::Config, Ok(_)) => self.load_settings(),
            (Pending::Form(part, before), Ok(a)) => {
                // Answers come in order, so the last form asked for wins;
                // a reload after a conflict only if that form is still open.
                let wanted = match (&before, &self.form) {
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
                        self.send(
                            Request::FeatureShow {
                                part: part.clone(),
                                id,
                            },
                            Pending::Form(part, Some(before)),
                        );
                    } else {
                        f.error = Some(e);
                    }
                }
            }
            (Pending::Mesh(part), Ok(a)) => match a.binary.as_deref().map(ryucad::mesh::decode) {
                Some(Ok(bodies)) => {
                    for b in bodies {
                        self.model.take_mesh(&part, b);
                    }
                }
                Some(Err(e)) => self.fail(e),
                None => self.fail(Error::new(Code::Daemon, "a mesh came without its bytes")),
            },
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
            }
            "project.changed" => {
                // Parts came or went: damage may have too.
                if self.doctor.is_some() {
                    self.diagnose();
                }
                if let Some(u) = params["default_unit"].as_str() {
                    self.model.default_unit = u.to_owned();
                    self.model.generation += 1;
                }
            }
            "config.changed" => {
                self.settings.changed(params);
                if params["key"] == "display.quality" {
                    let all = self.model.clear_meshes();
                    self.fetch(all);
                }
                if params["scope"] == "project" {
                    self.load_settings();
                }
            }
            "activity.appended" => self.activity.appended(params),
            "app.open" => self.raised = true,
            "participant.changed" => {
                self.people.changed(params);
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
            self.send(
                Request::FeatureShow {
                    part: part.clone(),
                    id,
                },
                Pending::Form(part, None),
            );
        }
        let wanted = self.model.apply_part(report);
        self.fetch(wanted);
    }

    /// Fetches display meshes, one body per request: a part with one
    /// changed feature sends one mesh.
    fn fetch(&mut self, bodies: Vec<BodyKey>) {
        for (part, feature) in bodies {
            self.fetched += 1;
            let req = Request::MeshGet {
                part: part.clone(),
                feature: Some(feature),
                quality: None,
                transfer: Some(Transfer::Binary),
            };
            self.send(req, Pending::Mesh(part));
        }
    }

    /// The part `id`, if the window knows it.
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.model.parts.get(id)
    }
}
