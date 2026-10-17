//! An open project: the link to its daemon and what the window knows, kept
//! up to date from answers and notifications.

use crate::core::link::{Event, Link, Wake};
use crate::core::model::{BodyKey, Model, Part};
use crate::core::people::{People, Person};
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
    /// Only a failure matters.
    Quiet,
}

/// How often the window's camera is shared at most while it moves: often
/// enough for a follower, rarely enough not to flood watching agents.
pub const CAMERA_EVERY: Duration = Duration::from_millis(150);

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
    /// Requests of the window's that failed, newest last.
    pub errors: Vec<Error>,
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
            camera_due: None,
            camera_sent: None,
            errors: Vec::new(),
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

    /// Shares this window's camera with the session, at most every
    /// [`CAMERA_EVERY`]; the last one always goes out (see `tick`).
    pub fn share_camera(&mut self, camera: &Camera) {
        self.camera_due = Some(camera.clone());
        self.tick(Instant::now());
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
            (Pending::Mesh(part), Ok(a)) => match a.binary.as_deref().map(ryucad::mesh::decode) {
                Some(Ok(bodies)) => {
                    for b in bodies {
                        self.model.take_mesh(&part, b);
                    }
                }
                Some(Err(e)) => self.errors.push(e),
                None => self
                    .errors
                    .push(Error::new(Code::Daemon, "a mesh came without its bytes")),
            },
            (_, Err(e)) => self.errors.push(e),
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
                if let Some(u) = params["default_unit"].as_str() {
                    self.model.default_unit = u.to_owned();
                    self.model.generation += 1;
                }
            }
            "config.changed" if params["key"] == "display.quality" => {
                let all = self.model.clear_meshes();
                self.fetch(all);
            }
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
