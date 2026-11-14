//! What an answer does beyond its own sub-state (design 541, "Typed
//! handles"). A sub-state's `answer` touches only its own fields and
//! records the rest here. `Workspace::apply` carries it out, in the order
//! recorded, once the answer is taken.

// Used as 27.F's Tasks 9 to 11 move each sub-state's answers here;
// Task 12 removes this.
#![allow(dead_code)]

use super::*;
use crate::core::deps::Marks;

/// What one answer leaves for the workspace, in order.
#[derive(Debug, Default)]
pub(super) struct Effects(Vec<Effect>);

/// One thing an answer leaves for the workspace.
#[derive(Debug)]
pub(super) enum Effect {
    /// A request failed: [`Workspace::fail`].
    Fail(Error),
    /// The model draws differently (`model.generation += 1`).
    Redraw,
    /// An assembly's previewed poses go (`model.preview` of that part).
    DropPreview(String),
    /// An assembly's instances drawn at the poses an `assembly.solve`
    /// answered.
    PreviewPoses(String, Value),
    /// What the tree marks of dependencies (`Model::set_marks`).
    Marks(Marks),
    /// The model's base level of detail, from the settings.
    Base(ryucad::mesh::Quality),
    /// A read the answer asks for next, with its route (boxed: a
    /// `Request` is large).
    Send(Box<Request>, Pending),
    /// One of the workspace's own steps.
    Then(Then),
}

/// The workspace's steps an answer asks for. Each is the `Workspace`
/// method of the same name, called as the answer called it before 27.F.
#[derive(Debug)]
pub(super) enum Then {
    AskBom(bool),
    AskBomDiff,
    AskDiff,
    AskDrag,
    AskPart(String),
    AskPreview,
    BomRead,
    ChooseAnnotation(String),
    Diagnose,
    /// This window's explode shown: the explode's slider lets go, and the
    /// window follows its explode and motion.
    ExplodeShown,
    FetchGhosts,
    FollowDraft,
    FollowFocus,
    FollowSections,
    LoadBom,
    LoadBranches,
    LoadDeps,
    LoadHistory,
    LoadReleases,
    LoadSettings,
    /// This window's motion shown: likewise, the motion's slider.
    MotionShown,
    OpenForm((String, String)),
    PartChanged(Value),
    ReadAppearance,
    ReadCache,
    ReadDimension(String, String, String, Option<Box<DimForm>>),
    ReadDrawingList,
    ReadDrawingSheet,
    ReadEquations(bool),
    ReadExplodes,
    ReadForm(String, String, Option<Box<Form>>),
    ReadFrames,
    ReadMotions,
    ReadParams(String, Option<Box<Params>>),
    ReadProps(String, Option<Box<PartProps>>),
    ReadViews,
    RefreshLooks,
    ReloadHistory,
    /// `show_explode(Some(id), Some(1.0), false)`: a saved explode shown
    /// whole.
    ShowExplode(String),
    SyncExplode,
    SyncMoved,
    TurnToFlat,
}

impl Effects {
    pub(super) fn fail(&mut self, e: Error) {
        self.0.push(Effect::Fail(e));
    }

    pub(super) fn redraw(&mut self) {
        self.0.push(Effect::Redraw);
    }

    pub(super) fn drop_preview(&mut self, part: &str) {
        self.0.push(Effect::DropPreview(part.to_owned()));
    }

    pub(super) fn preview_poses(&mut self, part: &str, result: &Value) {
        self.0
            .push(Effect::PreviewPoses(part.to_owned(), result.clone()));
    }

    pub(super) fn marks(&mut self, marks: Marks) {
        self.0.push(Effect::Marks(marks));
    }

    pub(super) fn base(&mut self, q: ryucad::mesh::Quality) {
        self.0.push(Effect::Base(q));
    }

    pub(super) fn send(&mut self, req: Request, why: Pending) {
        self.0.push(Effect::Send(Box::new(req), why));
    }

    pub(super) fn then(&mut self, t: Then) {
        self.0.push(Effect::Then(t));
    }

    /// What was recorded, in order (for tests).
    #[cfg(test)]
    pub(super) fn recorded(&self) -> &[Effect] {
        &self.0
    }
}

impl Workspace {
    /// Carries out what an answer left, in the order its sub-state
    /// recorded it.
    pub(super) fn apply(&mut self, fx: Effects) {
        for e in fx.0 {
            match e {
                Effect::Fail(e) => self.fail(e),
                Effect::Redraw => self.model.generation += 1,
                Effect::DropPreview(part) => self.model.preview.retain(|(p, _), _| *p != part),
                Effect::PreviewPoses(part, result) => self.preview_poses(&part, &result),
                Effect::Marks(marks) => self.model.set_marks(marks),
                Effect::Base(q) => self.model.base = q,
                Effect::Send(req, why) => self.send(*req, why),
                Effect::Then(t) => self.then(t),
            }
        }
    }

    fn then(&mut self, t: Then) {
        match t {
            Then::AskBom(flat) => self.ask_bom(flat),
            Then::AskBomDiff => self.ask_bom_diff(),
            Then::AskDiff => self.ask_diff(),
            Then::AskDrag => self.ask_drag(),
            Then::AskPart(id) => self.ask_part(&id),
            Then::AskPreview => self.ask_preview(),
            Then::BomRead => self.bom_read(),
            Then::ChooseAnnotation(id) => self.choose_annotation(Some(&id)),
            Then::Diagnose => self.diagnose(),
            Then::ExplodeShown => {
                self.assembly.explodes.sliding = None;
                self.follow_explode();
                self.follow_motion();
            }
            Then::FetchGhosts => self.fetch_ghosts(),
            Then::FollowDraft => self.follow_draft(),
            Then::FollowFocus => self.follow_focus(),
            Then::FollowSections => self.follow_sections(),
            Then::LoadBom => self.load_bom(),
            Then::LoadBranches => self.load_branches(),
            Then::LoadDeps => self.load_deps(),
            Then::LoadHistory => self.load_history(),
            Then::LoadReleases => self.load_releases(),
            Then::LoadSettings => self.load_settings(),
            Then::MotionShown => {
                self.assembly.motions.sliding = None;
                self.follow_explode();
                self.follow_motion();
            }
            Then::OpenForm(key) => self.open_form(Some(key)),
            Then::PartChanged(report) => self.part_changed(&report),
            Then::ReadAppearance => self.read_appearance(),
            Then::ReadCache => self.read_cache(),
            Then::ReadDimension(part, sketch, constraint, before) => {
                self.read_dimension(part, sketch, constraint, before)
            }
            Then::ReadDrawingList => self.read_drawing_list(),
            Then::ReadDrawingSheet => self.read_drawing_sheet(),
            Then::ReadEquations(conflicted) => self.read_equations(conflicted),
            Then::ReadExplodes => self.read_explodes(),
            Then::ReadForm(part, id, before) => self.read_form(part, id, before),
            Then::ReadFrames => self.read_frames(),
            Then::ReadMotions => self.read_motions(),
            Then::ReadParams(part, before) => self.read_params(part, before),
            Then::ReadProps(part, before) => self.read_props(part, before),
            Then::ReadViews => self.read_views(),
            Then::RefreshLooks => self.refresh_looks(),
            Then::ReloadHistory => self.reload_history(),
            Then::ShowExplode(id) => self.show_explode(Some(&id), Some(1.0), false),
            Then::SyncExplode => self.sync_explode(),
            Then::SyncMoved => self.sync_moved(),
            Then::TurnToFlat => self.turn_to_flat(),
        }
    }
}
