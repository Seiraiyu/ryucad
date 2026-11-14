//! What the view shows beyond the model, a sub-state of the window's
//! workspace: named views, a face's normal, appearance, section planes,
//! Measure and Mass properties.

use super::*;

/// The view's tools.
#[derive(Default)]
pub struct View {
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
    /// How the shown bodies look, as the daemon last said.
    pub(super) looks_read: BTreeMap<BodyKey, Arc<crate::core::appearance::Look>>,
    /// The last appearance reading asked for: an older answer is dropped.
    pub(super) looks_asked: u64,
    /// The planes this window cuts with, and the gizmo's drag.
    pub sections: crate::core::section::Sections,
    /// Tools > Measure: on or off, its picks and what they measured.
    pub measure: Tool,
    /// Tools > Mass properties, once asked for.
    pub mass: Option<Mass>,
}

impl Workspace {
    /// The part whose named views the window offers: the one it shows.
    pub fn views_part(&self) -> Option<String> {
        match self.model.shown().as_slice() {
            [one] => Some((*one).to_owned()),
            _ => None,
        }
    }

    /// Reads the named views of the part shown, when that changed.
    pub(super) fn follow_views(&mut self) {
        let part = self.views_part();
        if part != self.view.views.part {
            self.view.views = crate::core::views::NamedViews {
                part,
                ..Default::default()
            };
            self.read_views();
        }
    }

    pub(super) fn read_views(&mut self) {
        if let Some(p) = self.view.views.part.clone() {
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
        let Some(part) = self.view.views.part.clone() else {
            return;
        };
        self.view.views.error = None;
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
        let Some(part) = self.view.views.part.clone() else {
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
    pub(super) fn read_appearance(&mut self) {
        self.view.looks_asked += 1;
        let req = Request::AppearanceGet {
            part: self.model.focus.clone(),
            participant: None,
            rev: self.model_rev(),
        };
        self.send(req, Pending::Appearance(self.view.looks_asked));
    }

    /// Lays what the Appearance window drags over the daemon's looks.
    pub(super) fn refresh_looks(&mut self) {
        use crate::core::appearance::previewed;
        let looks = match (self.view.appearance.dragging, self.view.appearance.target()) {
            (Some(p), Some(t)) if self.view.appearance.open => {
                previewed(&self.view.looks_read, self.model.meshes.keys(), t, p)
            }
            _ => self.view.looks_read.clone(),
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
        self.view
            .appearance
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
        let found = self.view.looks_read.iter().find(|((p, k), _)| {
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
        self.view.appearance.dragging = Some(preview);
        self.refresh_looks();
    }

    /// Writes what the Appearance window shows to its target: its colour
    /// (`colour`), its material's colour (`material`) or its opacity
    /// (`opacity`), one undoable change (`appearance.set`).
    pub fn write_appearance(&mut self, field: &str) {
        let Some(target) = self.view.appearance.target().map(str::to_owned) else {
            return;
        };
        self.view.appearance.error = None;
        let req = Request::AppearanceSet {
            entities: vec![target],
            colour: (field == "colour").then(|| self.view.appearance.hex()),
            material: field == "material",
            // To a thousandth: the slider's f32 isn't the decimal it shows.
            opacity: (field == "opacity")
                .then_some((f64::from(self.view.appearance.opacity) * 1000.0).round() / 1000.0),
            unset: Vec::new(),
            file_styles: None,
            expected_revision: None,
        };
        self.send(req, Pending::AppearanceWritten);
    }

    /// Takes the Appearance window's target's entry out
    /// (`appearance.clear`).
    pub fn clear_appearance(&mut self) {
        let Some(target) = self.view.appearance.target().map(str::to_owned) else {
            return;
        };
        self.view.appearance.error = None;
        self.view.appearance.dragging = None;
        let req = Request::AppearanceClear {
            entities: vec![target],
            expected_revision: None,
        };
        self.send(req, Pending::AppearanceWritten);
    }

    /// Closes the Appearance window, dropping anything dragged.
    pub fn close_appearance(&mut self) {
        self.view.appearance.open = false;
        self.view.appearance.dragging = None;
        self.refresh_looks();
    }

    /// Takes a pick of the Measure tool and measures what it now holds.
    pub fn measure_pick(&mut self, entity: &str) {
        let picks = self.view.measure.pick(entity);
        let req = Request::Measure {
            entities: picks.clone(),
            rev: None,
        };
        self.send(req, Pending::Measure(picks));
    }

    /// Measures the Measure tool's picks again: what they name changed.
    pub(super) fn measure_again(&mut self) {
        let picks = self.view.measure.picks.clone();
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
        self.view.mass = Some(Mass::asked(of));
        let req = Request::Mass {
            of: of.to_owned(),
            tolerance: None,
            rev: self.model_rev(),
        };
        self.send(req, Pending::Mass(of.to_owned()));
    }
}
