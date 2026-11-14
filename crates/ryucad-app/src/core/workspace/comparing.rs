//! A compare with a Git revision, a sub-state of the window's workspace
//! (design 541, "The app's workspace"): the diff, its ghosts and which
//! side is drawn.

use super::*;

/// The window's compare, and the diffs it has asked for.
#[derive(Default)]
pub struct Comparing {
    /// This window's compare with a Git revision, while it is in one: it
    /// follows the window's participant's compare state.
    pub compare: Option<Compare>,
    /// Which side of the compare is drawn, and whether the other is too:
    /// the window's own, never the session's (design 447).
    pub blink: Blink,
    /// How many diffs were asked for, and whether one is on its way; one
    /// asked for meanwhile waits for it (`compare_again`).
    pub(super) compare_asked: u64,
    pub(super) compare_waiting: bool,
    pub(super) compare_again: bool,
}

impl Workspace {
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
    /// participant and its blink.
    pub(super) fn follow_compare(&mut self) {
        // The participant's compare, from the revision to the working
        // files; swapped, from the working files to the revision drawn;
        // with no compare, the person's draft painted against the agent's
        // model. Neither of the last two is sent to the session.
        let compared = self.myself().and_then(|m| m.compare.clone());
        let want = match (compared, self.model_rev()) {
            (Some(rev), Some(at)) if self.comparing.blink.draws_rev() => {
                Some(("working".to_owned(), Some(at), Some(rev)))
            }
            (Some(rev), _) => Some((rev, None, None)),
            (None, Some(draft)) => Some(("working".to_owned(), Some(draft), None)),
            (None, None) => None,
        };
        match (&self.comparing.compare, want) {
            (Some(c), Some((rev, to, swapped)))
                if c.rev == rev && c.to == to && c.swapped == swapped => {}
            (_, Some((rev, to, swapped))) => {
                self.comparing.compare = Some(Compare {
                    swapped,
                    ..Compare::between(&rev, to)
                });
                self.model.generation += 1;
                self.ask_diff();
                self.reload_bom();
            }
            (Some(_), None) => {
                self.comparing.compare = None;
                self.model.generation += 1;
                self.reload_bom();
            }
            (None, None) => {}
        }
    }

    /// Swaps the compare's sides (View > Swap compared sides, `B`).
    pub fn swap_compare(&mut self) {
        self.comparing.blink.swap();
        self.blinked();
    }

    /// Shows one side of the compare alone, or both again when it is.
    pub fn compare_only(&mut self, front: Front) {
        self.comparing.blink.only(front);
        self.blinked();
    }

    /// Shows both sides of the compare again.
    pub fn compare_both(&mut self) {
        self.comparing.blink.both();
        self.blinked();
    }

    /// The blink moved: the picker and the paint are made again, and the
    /// model and the diff follow the side drawn.
    fn blinked(&mut self) {
        self.model.generation += 1;
        self.follow_draft();
    }

    /// The compare as the viewport paints and ghosts it: none while one
    /// side is shown alone.
    pub fn painted(&self) -> Option<&Compare> {
        self.comparing
            .compare
            .as_ref()
            .filter(|_| !self.comparing.blink.alone)
    }

    /// Asks for the compare's diff, from its revision to the working
    /// files; with one on its way, once that one comes.
    pub(super) fn ask_diff(&mut self) {
        let Some(c) = &self.comparing.compare else {
            return;
        };
        if self.comparing.compare_waiting {
            self.comparing.compare_again = true;
            return;
        }
        let req = Request::Diff {
            from: Some(c.rev.clone()),
            to: c.to.clone(),
            part: None,
            model: true,
            positions: false,
            all: false,
        };
        self.comparing.compare_asked += 1;
        self.comparing.compare_waiting = true;
        self.comparing.compare_again = false;
        let n = self.comparing.compare_asked;
        self.send(req, Pending::Comparing(comparing::Asked::Compare(n)));
    }

    /// Fetches the ghosts the compare wants and doesn't have, one body
    /// per request, at the commit the diff was made from.
    pub(super) fn fetch_ghosts(&mut self) {
        let Some(c) = &mut self.comparing.compare else {
            return;
        };
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
                flat: None,
                before: None,
            };
            self.send(req, Pending::Comparing(comparing::Asked::Ghost(key, hash)));
        }
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    /// The diff of a compare, the `n`th asked for.
    Compare(u64),
    /// A ghost's mesh, asked for at this hash.
    Ghost(BodyKey, String),
}

impl Workspace {
    pub(super) fn answer_comparing(&mut self, why: Asked, answer: Result<Answer>) {
        match (why, answer) {
            (Asked::Compare(n), r) => {
                if n == self.comparing.compare_asked {
                    self.comparing.compare_waiting = false;
                }
                if let Some(c) = self
                    .comparing
                    .compare
                    .as_mut()
                    .filter(|_| n == self.comparing.compare_asked)
                {
                    match r {
                        Ok(a) => c.take_diff(&a.result),
                        Err(e) => c.failed(e),
                    }
                    self.model.generation += 1;
                    self.fetch_ghosts();
                }
                if std::mem::take(&mut self.comparing.compare_again) {
                    self.ask_diff();
                }
            }
            (Asked::Ghost(key, hash), a) => {
                let Some(c) = &mut self.comparing.compare else {
                    return;
                };
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
        }
    }
}
