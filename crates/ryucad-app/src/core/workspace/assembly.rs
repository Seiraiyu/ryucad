//! The assembly tools, a sub-state of the window's workspace: drags, the
//! Move tool, the Mate dialog, interference and clearance (only when
//! asked), explodes and motions.

use super::*;

/// The shown assembly's tools.
#[derive(Default)]
pub struct Assembly {
    /// The shown assembly's interference, once asked for (Tools > Check
    /// interference); marked stale when the assembly changes, and asked
    /// again only by Check again (design 525).
    pub interference: Option<Interference>,
    /// Tools > Clearance, once asked for: the shown assembly's.
    pub clearance: Option<Clearance>,
    /// The instance being dragged, if any.
    pub drag: Option<Drag>,
    /// Why the last drag moved nothing, for the status line.
    pub drag_note: Option<String>,
    /// The Move tool, while it is on one instance.
    pub mover: Option<crate::core::mover::Mover>,
    /// When the Move tool's preview was last asked and for which value,
    /// and whether one is on its way.
    pub(super) mover_asked: Option<(Instant, Option<crate::core::mover::Grip>, f64)>,
    pub(super) mover_waiting: bool,
    /// The Mate dialog, while it is open.
    pub mating: Option<crate::core::mating::Mating>,
    /// How many mate previews the window has asked for.
    pub(super) mate_previews: u64,
    /// The shown assembly's explodes, the one shown, and a step being
    /// made.
    pub explodes: crate::core::explode::Explodes,
    /// The shown assembly's motions, the one shown, its frames, Play and
    /// the last sweep.
    pub motions: crate::core::motion::Motions,
}

impl Workspace {
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
        self.assembly.interference = Some(Interference::asked(&part));
        let req = Request::Interference {
            part: part.clone(),
            instances: Vec::new(),
        };
        self.send(req, Route::Assembly(assembly::Asked::Interference(part)));
    }

    /// Finds the shown assembly's clearances (`clearance`); nothing when it
    /// shows none.
    pub fn check_clearance(&mut self) {
        let Some(part) = self.shown_assembly() else {
            return;
        };
        self.assembly.clearance = Some(Clearance::asked(&part));
        let req = Request::Clearance {
            part: part.clone(),
            instances: Vec::new(),
            below: None,
        };
        self.send(req, Route::Assembly(assembly::Asked::Clearance(part)));
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
            .assembly
            .drag
            .as_ref()
            .is_some_and(|d| d.part == part && d.instance == instance);
        if !same {
            self.assembly.drag = Some(Drag {
                part: part.to_owned(),
                instance: instance.to_owned(),
                translate,
                asked: None,
                waiting: false,
            });
        }
        let d = self.assembly.drag.as_mut().expect("just set");
        d.translate = translate;
        let due = d.asked.is_none_or(|(at, _)| at.elapsed() >= DRAG_EVERY);
        if !d.waiting && due {
            self.ask_drag();
        }
    }

    pub(super) fn ask_drag(&mut self) {
        let Some(d) = self.assembly.drag.as_mut() else {
            return;
        };
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
            rev: None,
            mates: Vec::new(),
        };
        let part = d.part.clone();
        self.send(req, Route::Assembly(assembly::Asked::DragSolve(part)));
    }

    /// Ends a drag, writing where it left the instance with one
    /// `assembly.move` (logged and undoable, as an agent's), against the
    /// assembly's revision as the window last saw it.
    pub fn drop_drag(&mut self) {
        let Some(d) = self.assembly.drag.take() else {
            return;
        };
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
            about: None,
            expected_revision: revision,
        };
        // In Annotate a freehand drag is a rough suggestion, its comment
        // for the person to say more in.
        self.send_with(
            req,
            Route::Assembly(assembly::Asked::DragMove(d.part)),
            Proposal {
                comment: Some(ROUGH_COMMENT.into()),
                rough: true,
            },
        );
    }

    /// Puts the Move tool's handles on instance `instance` of assembly
    /// `part`, at the middle of its drawn boxes, or says why it can't
    /// move (fixed, a pattern's copy, no freedom left).
    pub fn start_move(&mut self, part: &str, instance: &str) -> Result<(), String> {
        if self.comparing.blink.draws_rev() {
            return Err(crate::core::compare::SWAPPED.to_owned());
        }
        let key = (part.to_owned(), format!("{instance}/"));
        self.model.movable(&key)?;
        let middle = self
            .model
            .middles(part)
            .get(instance)
            .copied()
            .ok_or_else(|| format!("{instance} isn't drawn yet"))?;
        let body = self
            .model
            .meshes
            .keys()
            .find(|k| k.0 == part && k.1.split('/').next() == Some(instance))
            .cloned();
        let pose = body.map_or(crate::core::model::Pose::IDENTITY, |k| self.model.pose(&k));
        let own = self.assembly.mover.as_ref().is_some_and(|m| m.own);
        self.cancel_move();
        self.assembly.mover = Some(crate::core::mover::Mover::new(
            part, instance, middle, pose, own,
        ));
        Ok(())
    }

    /// Reads the Move tool's value field.
    pub fn type_move(&mut self, text: &str) {
        let unit = self.unit();
        if let Some(m) = self.assembly.mover.as_mut() {
            m.typed(text, unit);
        }
    }

    /// Asks where the Move tool's value puts what the instance's mates
    /// reach, at the drawn revision: when the value has changed since it
    /// last asked, one at a time, at most every [`DRAG_EVERY`]. The view
    /// calls it every frame the tool is on.
    pub fn preview_move(&mut self) {
        let Some(m) = &self.assembly.mover else {
            return;
        };
        if self.assembly.mover_waiting {
            return;
        }
        let asked = (m.grip, m.value);
        if self
            .assembly
            .mover_asked
            .is_some_and(|(at, g, v)| (g, v) == asked || at.elapsed() < DRAG_EVERY)
        {
            return;
        }
        let part = m.part.clone();
        if m.grip.is_none() || m.value == 0.0 || m.error.is_some() {
            // Nothing to move: drawn where it is.
            self.assembly.mover_asked = Some((Instant::now(), asked.0, asked.1));
            if self.model.preview.keys().any(|(p, _)| *p == part) {
                self.model.preview.retain(|(p, _), _| *p != part);
                self.model.generation += 1;
            }
            return;
        }
        let req = m.preview(self.model_rev());
        self.assembly.mover_asked = Some((Instant::now(), asked.0, asked.1));
        self.assembly.mover_waiting = true;
        self.send(req, Route::Assembly(assembly::Asked::MoveSolve(part)));
    }

    /// Sends the Move tool's value as `assembly.move` (proposed in
    /// Annotate) against the assembly's revision as the window last saw
    /// it; the handles go.
    pub fn send_move(&mut self) {
        let Some(m) = self.assembly.mover.take() else {
            return;
        };
        if !m.ready() {
            self.assembly.mover = Some(m);
            return;
        }
        let revision = self.model.parts.get(&m.part).map(|p| p.revision.clone());
        let part = m.part.clone();
        self.assembly.mover_asked = None;
        self.assembly.mover_waiting = false;
        self.send(
            m.request(revision),
            Route::Assembly(assembly::Asked::MoveWrite(part)),
        );
    }

    /// Drops the Move tool's value and its preview; the handles stay.
    pub fn clear_move(&mut self) {
        let Some(m) = self.assembly.mover.as_mut() else {
            return;
        };
        m.clear();
        let part = m.part.clone();
        self.model.preview.retain(|(p, _), _| *p != part);
        self.model.generation += 1;
    }

    /// Drops the Move tool and its preview.
    pub fn cancel_move(&mut self) {
        if let Some(m) = self.assembly.mover.take() {
            self.model.preview.retain(|(p, _), _| *p != m.part);
            self.model.generation += 1;
        }
        self.assembly.mover_asked = None;
        self.assembly.mover_waiting = false;
    }

    /// Opens the Mate dialog on assembly `part`, from instance `from`.
    pub fn open_mate(&mut self, part: &str, from: Option<&str>) {
        self.close_mate();
        self.assembly.mating = Some(crate::core::mating::Mating::new(part, from));
    }

    /// Takes a pick for the dialog's next side, previewing once both are
    /// in; a pick it refuses is said in the dialog.
    pub fn mate_pick(&mut self, entity: &str) -> std::result::Result<(), String> {
        let d = self.assembly.mating.as_mut().ok_or("no Mate dialog")?;
        match d.pick(entity) {
            Ok(_) => d.note = None,
            Err(why) => {
                d.note = Some(why.clone());
                return Err(why);
            }
        }
        self.preview_mate();
        Ok(())
    }

    /// Asks where the dialog's mate puts the instances, at the drawn
    /// revision, writing nothing.
    pub fn preview_mate(&mut self) {
        let rev = self.model_rev();
        let Some(d) = self.assembly.mating.as_mut() else {
            return;
        };
        let Some(req) = d.preview(rev) else { return };
        self.assembly.mate_previews += 1;
        d.asked = self.assembly.mate_previews;
        d.waiting = true;
        let part = d.part.clone();
        let asked = d.asked;
        self.send(
            req,
            Route::Assembly(assembly::Asked::MatePreview(part, asked)),
        );
    }

    /// Sends the dialog's mate (`feature.add`; proposed in Annotate)
    /// against the assembly's revision as the window last saw it.
    pub fn add_mate(&mut self) {
        let Some(d) = self.assembly.mating.take() else {
            return;
        };
        if !d.addable() {
            self.assembly.mating = Some(d);
            return;
        }
        let revision = self.model.parts.get(&d.part).map(|p| p.revision.clone());
        let part = d.part.clone();
        let Some(req) = d.add(revision) else { return };
        self.model.preview.retain(|(p, _), _| *p != part);
        self.model.generation += 1;
        self.send(req, Route::Assembly(assembly::Asked::MateAdd(part)));
    }

    /// Closes the Mate dialog and drops its preview.
    pub fn close_mate(&mut self) {
        if let Some(d) = self.assembly.mating.take() {
            self.model.preview.retain(|(p, _), _| *p != d.part);
            self.model.generation += 1;
        }
    }

    /// Reads the explodes of the assembly they are of (`explode.list`).
    pub fn read_explodes(&mut self) {
        if let Some(p) = self.assembly.explodes.part.clone() {
            let req = Request::ExplodeList {
                part: Some(p.clone()),
            };
            self.send(req, Route::Assembly(assembly::Asked::Explodes(p)));
        }
    }

    /// Moves the model's occurrences where the explode drawn puts them.
    pub fn sync_explode(&mut self) {
        self.sync_moved();
    }

    /// Draws the explode `fraction` of the way while the slider moves;
    /// written when let go ([`Workspace::show_explode`]).
    pub fn slide_explode(&mut self, fraction: f64) {
        self.assembly.explodes.sliding = Some(fraction.clamp(0.0, 1.0));
        self.sync_explode();
    }

    /// Starts a new step of the selected occurrences: their arrows show
    /// at their middle, to drag.
    pub fn start_step(&mut self) {
        let Some(part) = self.assembly.explodes.part.clone() else {
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
        let offsets = self.assembly.explodes.offsets(&part);
        let at: Vec<[f64; 3]> = instances
            .iter()
            .filter_map(|p| {
                let m = *middles.get(p)?;
                Some(offsets.get(p).map_or(m, |o| o.apply(m)))
            })
            .collect();
        let n = at.len().max(1) as f64;
        let centre = [0, 1, 2].map(|k| at.iter().map(|p| p[k]).sum::<f64>() / n);
        self.assembly.explodes.adding = Some(crate::core::explode::Adding {
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
        let Some(a) = self.assembly.explodes.adding.take() else {
            return;
        };
        let Some(part) = self.assembly.explodes.part.clone() else {
            return;
        };
        let unit = self.unit();
        let Some(step) = a.written(unit.as_str(), unit.factor()) else {
            return;
        };
        let current = self
            .assembly
            .explodes
            .current(&part)
            .map(|(e, _)| e.clone());
        let id = match &current {
            Some(e) => e.id.clone(),
            None if !self.assembly.explodes.name.is_empty() => self.assembly.explodes.name.clone(),
            None => return,
        };
        let req = crate::core::explode::save_with(
            &part,
            &id,
            current.as_ref(),
            step,
            self.assembly.explodes.revision.clone(),
        );
        self.sync_explode();
        self.send(
            req,
            Route::Assembly(assembly::Asked::ExplodeSaved(part, Some(id))),
        );
    }

    /// Takes step `k` out of the shown explode, or deletes the explode
    /// with its last step.
    pub fn remove_step(&mut self, k: usize) {
        let Some(part) = self.assembly.explodes.part.clone() else {
            return;
        };
        let Some((e, _)) = self.assembly.explodes.current(&part) else {
            return;
        };
        let e = e.clone();
        let req = if e.steps.as_array().is_some_and(|s| s.len() <= 1) {
            Request::ExplodeDelete {
                part: part.clone(),
                explode: e.id.clone(),
                expected_revision: self.assembly.explodes.revision.clone(),
            }
        } else {
            crate::core::explode::save_without(
                &part,
                &e,
                k,
                self.assembly.explodes.revision.clone(),
            )
        };
        self.send(
            req,
            Route::Assembly(assembly::Asked::ExplodeSaved(part, Some(e.id))),
        );
    }

    /// Reads the motions of the assembly they are of (`motion.list`).
    pub fn read_motions(&mut self) {
        if let Some(p) = self.assembly.motions.part.clone() {
            self.send(
                Request::MotionList {
                    part: Some(p.clone()),
                },
                Route::Assembly(assembly::Asked::Motions(p)),
            );
        }
    }

    /// Fetches the shown motion's frames (`motion.frames`, 120 of them).
    pub(super) fn read_frames(&mut self) {
        if let Some(s) = self.assembly.motions.shown.clone() {
            let req = Request::MotionFrames {
                part: s.part.clone(),
                motion: Some(s.motion.clone()),
                drives: None,
                steps: None,
                frames: None,
            };
            self.send(
                req,
                Route::Assembly(assembly::Asked::MotionFrames(s.part, s.motion)),
            );
        }
    }

    /// Draws the motion `fraction` of the way while the slider moves
    /// (Play stops); written when let go.
    pub fn slide_motion(&mut self, fraction: f64) {
        self.assembly.motions.started = None;
        self.assembly.motions.sliding = Some(fraction.clamp(0.0, 1.0));
        self.sync_moved();
    }

    /// Plays the shown motion from where it is, or pauses it there.
    pub fn play_motion(&mut self, play: bool) {
        let at = self.assembly.motions.fraction(Instant::now());
        if play {
            self.assembly.motions.started = Some((Instant::now(), at.unwrap_or(0.0)));
        } else {
            self.assembly.motions.started = None;
            self.assembly.motions.sliding = at;
        }
        self.show_motion(None, at, Some(play), false);
    }

    /// Sweeps the shown motion with interference (`assembly.sweep`): its
    /// collisions marked on the slider and listed.
    pub fn sweep_motion(&mut self) {
        let Some(s) = self.assembly.motions.shown.clone() else {
            return;
        };
        self.assembly.motions.sweep = Some(crate::core::motion::Sweep::asked(&s.part, &s.motion));
        let req = Request::AssemblySweep {
            part: s.part.clone(),
            motion: Some(s.motion.clone()),
            drives: None,
            mate: None,
            from: None,
            to: None,
            steps: None,
            interference: true,
            clearance: false,
            below: None,
            poses: false,
            refine: None,
        };
        self.send(
            req,
            Route::Assembly(assembly::Asked::MotionSwept(s.part, s.motion)),
        );
    }

    /// Jumps the slider to `fraction` and writes it (a sweep's Show).
    pub fn jump_motion(&mut self, fraction: f64) {
        self.slide_motion(fraction);
        self.show_motion(None, Some(fraction), Some(false), false);
    }

    /// Writes the shown motion's ranges and steps as edited
    /// (`motion.save`); a refusal is the daemon's, shown as any.
    pub fn save_motion(&mut self) {
        let (Some(part), Some((id, ranges, steps))) = (
            self.assembly.motions.part.clone(),
            self.assembly.motions.editing.clone(),
        ) else {
            return;
        };
        let Some(entry) = self
            .assembly
            .motions
            .list
            .iter()
            .find(|e| e.id == id)
            .cloned()
        else {
            return;
        };
        match crate::core::motion::save(
            &part,
            &id,
            &entry.drives,
            &ranges,
            &steps,
            self.assembly.motions.revision.clone(),
        ) {
            Ok(req) => self.send(req, Route::Assembly(assembly::Asked::MotionSaved(part))),
            Err(why) => self.fail(Error::new(Code::InvalidParam, why)),
        }
    }

    /// Moves the model's occurrences where the explode drawn puts them,
    /// else where the motion drawn does: a window shows one or neither.
    pub(super) fn sync_moved(&mut self) {
        let explode = self
            .assembly
            .explodes
            .part
            .clone()
            .map(|p| {
                let o = self.assembly.explodes.offsets(&p);
                (p, o)
            })
            .filter(|(_, o)| !o.is_empty());
        let moved = explode.or_else(|| {
            let p = self.assembly.motions.part.clone()?;
            let part = self.model.parts.get(&p)?;
            let offsets = self.assembly.motions.offsets(
                &p,
                part.occurrences.iter().map(|o| o.path.as_str()),
                Instant::now(),
            );
            Some((p, offsets))
        });
        self.model.explode(moved);
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    /// An assembly's interference (`interference`).
    Interference(String),
    /// An assembly's clearances (`clearance`).
    Clearance(String),
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
    /// An assembly's explodes (`explode.list`).
    Explodes(String),
    /// An assembly's explode written or deleted, to show `explode` next
    /// if it is still there.
    ExplodeSaved(String, Option<String>),
    /// An assembly's motions (`motion.list`).
    Motions(String),
    /// A motion's frames (`motion.frames`): its assembly and ID.
    MotionFrames(String, String),
    /// A motion written (`motion.save`), of an assembly.
    MotionSaved(String),
    /// A motion swept with interference: its assembly and ID.
    MotionSwept(String, String),
}

impl Assembly {
    /// Takes the answer to one of this sub-state's requests (design
    /// 541, "Typed handles"); what it does elsewhere goes in `fx`.
    pub(super) fn answer(&mut self, why: Asked, answer: Result<Answer>, fx: &mut Effects) {
        match (why, answer) {
            (Asked::DragSolve(part), a) => {
                let mut again = false;
                if let Some(d) = self.drag.as_mut().filter(|d| d.part == part) {
                    d.waiting = false;
                    again = d.asked.is_some_and(|(_, t)| t != d.translate);
                }
                match a {
                    // Only while it is still dragged: a late answer after
                    // the drop would undo the written move's look.
                    Ok(a) if self.drag.is_some() => fx.preview_poses(&part, &a.result),
                    Ok(_) => {}
                    Err(e) => fx.fail(e),
                }
                if again {
                    fx.then(Then::AskDrag);
                }
            }
            (Asked::MoveSolve(part), a) => {
                self.mover_waiting = false;
                let on = self.mover.as_ref().is_some_and(|m| m.part == part);
                match a {
                    // A late answer after the tool went would undo the
                    // written move's look.
                    Ok(a) if on => fx.preview_poses(&part, &a.result),
                    Ok(_) => {}
                    Err(e) => {
                        if let Some(m) = self.mover.as_mut().filter(|_| on) {
                            m.error = Some(e.message.clone());
                        }
                    }
                }
            }
            (Asked::MatePreview(part, asked), a) => {
                let Some(d) = self
                    .mating
                    .as_mut()
                    .filter(|d| d.part == part && d.asked == asked)
                else {
                    return;
                };
                match a {
                    Ok(a) => {
                        d.previewed(Ok(a.result["converged"].as_bool().unwrap_or(false)));
                        fx.drop_preview(&part);
                        fx.preview_poses(&part, &a.result);
                    }
                    Err(e) => {
                        let code = serde_json::to_value(e.code)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned));
                        d.previewed(Err((code.unwrap_or_default(), e.message.clone())));
                        fx.drop_preview(&part);
                        fx.redraw();
                    }
                }
            }
            (Asked::MateAdd(_), Err(e)) => fx.fail(e),
            (Asked::MateAdd(_), Ok(_)) => {}
            (Asked::MoveWrite(part), a) => {
                fx.drop_preview(&part);
                fx.redraw();
                if let Err(e) = a {
                    fx.fail(e);
                }
            }
            (Asked::DragMove(part), a) => {
                if let Err(e) = a {
                    fx.drop_preview(&part);
                    fx.redraw();
                    fx.fail(e);
                }
            }
            (Asked::Explodes(part), Ok(a)) => {
                if self.explodes.part.as_deref() == Some(part.as_str()) {
                    self.explodes.listed(&part, &a.result);
                    fx.then(Then::SyncExplode);
                }
            }
            (Asked::Motions(part), Ok(a)) => {
                if self.motions.part.as_deref() == Some(part.as_str()) {
                    self.motions.listed(&part, &a.result);
                    if self.motions.shown.is_some() && !self.motions.frames_current() {
                        fx.then(Then::ReadFrames);
                    }
                    fx.then(Then::SyncMoved);
                }
            }
            (Asked::MotionFrames(part, id), Ok(a)) => {
                let wanted = self
                    .motions
                    .shown
                    .as_ref()
                    .is_some_and(|s| s.part == part && s.motion == id);
                if wanted {
                    self.motions.frames = crate::core::motion::Frames::from_json(&a.result);
                    fx.then(Then::SyncMoved);
                }
            }
            (Asked::MotionSaved(part), r) => match r {
                Ok(_) => {
                    if self.motions.part.as_deref() == Some(part.as_str()) {
                        self.motions.editing = None;
                        fx.then(Then::ReadMotions);
                    }
                }
                Err(e) => fx.fail(e),
            },
            (Asked::MotionSwept(part, id), r) => {
                let Some(w) = self
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
                fx.redraw();
            }
            (Asked::ExplodeSaved(part, show), r) => match r {
                Ok(a) => {
                    if self.explodes.part.as_deref() == Some(part.as_str()) {
                        fx.then(Then::ReadExplodes);
                    }
                    // A new explode, or one still there, is shown.
                    if let Some(id) = show
                        && a.result.get("steps").is_some()
                        && self.explodes.shown.as_ref().is_none_or(|s| s.explode != id)
                    {
                        fx.then(Then::ShowExplode(id));
                    }
                    self.explodes.name.clear();
                }
                Err(e) => {
                    fx.then(Then::SyncExplode);
                    fx.fail(e);
                }
            },
            (Asked::Clearance(part), a) => {
                let Some(c) = self.clearance.as_mut().filter(|c| c.part == part) else {
                    return;
                };
                match a {
                    Ok(a) => c.take(&a.result),
                    Err(e) => c.failed(e),
                }
                fx.redraw();
            }
            (Asked::Interference(part), a) => {
                let Some(i) = self.interference.as_mut().filter(|i| i.part == part) else {
                    return;
                };
                match a {
                    Ok(a) => i.take(&a.result),
                    Err(e) => i.failed(e),
                }
                fx.redraw();
            }
            (_, Err(e)) => fx.fail(e),
        }
    }
}
