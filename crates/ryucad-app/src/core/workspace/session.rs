//! The window's place in the session, a sub-state of the window's
//! workspace (design 541, "The app's workspace"): its mode, the
//! participants and what this window's participant shows (visibility,
//! sections, explode, motion), settings, the doctor and the cache.

use super::*;

/// This window's session state.
#[derive(Default)]
pub struct Session {
    /// The window's mode (design 374): View when it opens and whenever it
    /// connects again; Annotate and Direct edit by the person's choice.
    pub mode: crate::core::modes::Mode,
    pub people: People,
    /// The participant whose camera this window follows.
    pub following: Option<String>,
    pub settings: Settings,
    /// The last doctor report, once asked for.
    pub doctor: Option<Value>,
    /// Whether a report is on its way, and whether the project changed
    /// meanwhile, so the badges need another.
    pub(super) doctor_waiting: bool,
    pub(super) doctor_again: bool,
    /// What the disk cache held when last asked (`cache.stats`), for the
    /// Preferences window.
    pub cache: Option<Value>,
    /// The last answer to storing or removing the shared cache's token
    /// (`cache.login`, `cache.logout`), or its error as `{"error": …}`.
    pub cache_token: Option<Value>,
}

impl Workspace {
    pub(super) fn load_settings(&mut self) {
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
            .session
            .settings
            .all
            .iter()
            .any(|s| s.key == key && s.scope == "project");
        Request::ConfigSet {
            key: key.to_owned(),
            value,
            scope: None,
            expected_revision: project.then(|| self.session.settings.project_revision.clone()),
        }
    }

    /// Diagnoses the project (`doctor`).
    pub fn diagnose(&mut self) {
        self.session.doctor_waiting = true;
        self.send(Request::Doctor {}, Pending::Doctor);
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

    pub(super) fn rediagnose(&mut self) {
        if self.session.doctor_waiting {
            self.session.doctor_again = true;
        } else if self.session.doctor.is_some() {
            self.diagnose();
        }
    }

    /// Applies repairs from the last report (`doctor.repair`); the answer
    /// brings the new report.
    pub fn repair(&mut self, repairs: Vec<String>, params: serde_json::Map<String, Value>) {
        self.session.doctor_waiting = true;
        self.send(Request::DoctorRepair { repairs, params }, Pending::Doctor);
    }

    /// Cuts this window with exactly `planes` (`section.set`): drawn at
    /// once, as a drag left them, and written for the session.
    pub fn set_sections(&mut self, planes: Vec<ryucad::section::Plane>) {
        let req = crate::core::section::request(&planes, &self.link.participant);
        self.view.sections.planes = planes;
        self.model.generation += 1;
        self.send(req, Pending::Sections);
    }

    /// Adds the plane `plane` names (`xy`, `yz`, `zx` or a planar face's
    /// reference) to this window's.
    pub fn add_section(&mut self, plane: &str) {
        let req = crate::core::section::add(plane, &self.link.participant);
        self.send(req, Pending::Sections);
    }

    /// Shows explode `explode` of the explodes' assembly in this window,
    /// `fraction` of the way (`explode.show`); with no explode, moves the
    /// one shown; `off` shows none.
    pub fn show_explode(&mut self, explode: Option<&str>, fraction: Option<f64>, off: bool) {
        let req = Request::ExplodeShow {
            part: explode.and(self.assembly.explodes.part.clone()),
            explode: explode.map(str::to_owned),
            fraction,
            off,
            participant: Some(self.link.participant.clone()),
        };
        self.send(req, Pending::ExplodeShown);
    }

    /// Shows motion `motion` of the motions' assembly in this window
    /// (`motion.show`); with no motion, moves or plays the one shown;
    /// `off` shows none.
    pub fn show_motion(
        &mut self,
        motion: Option<&str>,
        fraction: Option<f64>,
        playing: Option<bool>,
        off: bool,
    ) {
        let req = Request::MotionShow {
            part: motion.and(self.assembly.motions.part.clone()),
            motion: motion.map(str::to_owned),
            fraction,
            playing,
            off,
            participant: Some(self.link.participant.clone()),
        };
        self.send(req, Pending::MotionShown);
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

    /// Puts the window in `mode`: at once here, and for the daemon, which
    /// refuses a human's writes their mode doesn't allow. Sent on the
    /// request connection, so every request after it is judged in it.
    pub fn set_mode(&mut self, mode: crate::core::modes::Mode) {
        if mode == self.session.mode {
            return;
        }
        let was = std::mem::replace(&mut self.session.mode, mode);
        self.send(Request::ParticipantMode { mode }, Pending::Mode(was));
        if mode != crate::core::modes::Mode::Annotate {
            self.annotations.show_agent = false;
        }
        self.model.generation += 1;
        self.follow_draft();
    }

    /// This window as the session knows it.
    pub fn myself(&self) -> Option<&Person> {
        self.session.people.get(&self.link.participant)
    }

    /// Changes this window's selection. It shows at once; the daemon's
    /// answer (and everyone else's view of it) follows.
    pub fn select(&mut self, entities: Vec<String>, mode: SelectMode) {
        let me = self.link.participant.clone();
        if let Some(p) = self.session.people.all.get_mut(&me) {
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
            self.session.people.generation += 1;
        }
        self.send(Request::SelectionSet { entities, mode }, Pending::Quiet);
    }

    /// The camera of the participant this window follows, if they have one.
    pub fn followed_camera(&self) -> Option<&Camera> {
        self.session
            .people
            .get(self.session.following.as_deref()?)?
            .camera
            .as_ref()
    }

    /// The user's display quality, as the settings say.
    pub(super) fn display_quality(&self) -> ryucad::mesh::Quality {
        ryucad::mesh::Quality::parse(self.session.settings.text("display.quality", "normal"))
            .unwrap_or_default()
    }
}
