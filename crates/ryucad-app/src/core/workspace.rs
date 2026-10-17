//! An open project: the link to its daemon and what the window knows, kept
//! up to date from answers and notifications.

use crate::core::link::{Event, Link, Wake};
use crate::core::model::{BodyKey, Model, Part};
use ryucad::client::Daemon;
use ryucad::error::{Code, Error, Result};
use ryucad::proto::{Answer, Request, Transfer};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What a request was for, so its answer lands in the right place.
#[derive(Clone, Debug, PartialEq)]
enum Pending {
    Info,
    Parts,
    Rebuild(String),
    Mesh(String),
}

pub struct Workspace {
    pub root: PathBuf,
    pub link: Link,
    pub model: Model,
    /// Requests of the window's that failed, newest last.
    pub errors: Vec<Error>,
    /// The daemon is gone.
    pub lost: bool,
    /// How many meshes were fetched: an unchanged body is never fetched
    /// twice.
    pub fetched: u64,
    pending: HashMap<u64, Pending>,
}

impl Workspace {
    /// Opens the project at `root` (canonical) as the human `name`, and
    /// starts loading it.
    pub fn open(root: &Path, daemon: &Daemon, name: &str, wake: Wake) -> Result<Workspace> {
        let link = Link::open(root, daemon, name, wake)?;
        let mut ws = Workspace {
            root: root.to_path_buf(),
            link,
            model: Model::default(),
            errors: Vec::new(),
            lost: false,
            fetched: 0,
            pending: HashMap::new(),
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
            Event::Lost => self.lost = true,
        }
    }

    fn answered(&mut self, why: Pending, answer: Result<Answer>) {
        match (why, answer) {
            (Pending::Info, Ok(a)) => {
                self.model.default_unit = a.result["default_unit"].as_str().unwrap_or("mm").into();
            }
            (Pending::Parts, Ok(a)) => {
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
            // Too much was missed: start again from the daemon's state.
            "resync" => self.load(),
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
