//! Releases, PDM settings, part properties, where-used and the BOM, a
//! sub-state of the window's workspace (design 541, "The app's
//! workspace", its `releases`).

use super::*;

/// What the Releases and BOM tabs and the Part section show of ryupdm.
#[derive(Default)]
pub struct Pdm {
    /// The Releases tab, and whether it was opened (it follows changes
    /// from then on).
    pub releases: ReleaseTab,
    pub(super) releases_open: bool,
    /// Every part's release state (ryupdm), for the tree's badges.
    pub states: States,
    /// ryupdm's settings (`pdm.config`): `project` is null until PDM is
    /// set up.
    pub pdm_config: Option<Value>,
    /// The Part section's form, for the part it shows (see `show_props`).
    pub props: Option<PartProps>,
    pub(super) props_part: Option<String>,
    /// The BOM tab, whether it was opened (it follows changes from then
    /// on), and whether a reading is on its way and another wanted after.
    pub bom: BomTab,
    pub(super) bom_open: bool,
    pub(super) bom_waiting: bool,
    pub(super) bom_again: bool,
    /// The BOM tab's Export CSV…, while open.
    pub bom_export: Option<crate::core::bom::BomExport>,
    /// Where the Part section's part is used, once read, and its part.
    pub where_used: Option<crate::core::where_used::WhereUsed>,
    pub(super) where_used_part: Option<String>,
    /// The review service's organization-wide where-used on its way, for
    /// this part; and one that came before `bom.where_used` answered.
    pub(super) org: Option<(String, std::sync::mpsc::Receiver<Result<Value>>)>,
    pub(super) org_early: Option<(String, Result<Vec<crate::core::where_used::Use>>)>,
}

impl Pdm {
    /// Whether ryupdm is set up: the project has an ID.
    pub fn ready(&self) -> bool {
        self.pdm_config
            .as_ref()
            .is_some_and(|c| c["project"].is_string())
    }
}

impl Workspace {
    /// Shows `part`'s properties in the Part section, read if they aren't
    /// yet.
    pub fn show_props(&mut self, part: Option<&str>) {
        if self.pdm.props_part.as_deref() == part {
            return;
        }
        self.pdm.props_part = part.map(str::to_owned);
        self.pdm.props = None;
        if let Some(p) = part {
            self.read_props(p.to_owned(), None);
        }
    }

    pub(super) fn read_props(&mut self, part: String, before: Option<Box<PartProps>>) {
        let req = Request::PropsGet {
            part: part.clone(),
            rev: self.form_rev(),
        };
        self.send(req, Route::Pdm(pdm::Asked::Props(part, before)));
    }

    /// Saves what was typed into the Part section.
    pub fn save_props(&mut self) {
        let Some(p) = &mut self.pdm.props else { return };
        p.saving = true;
        p.error = None;
        let req = p.request();
        self.send(req, Route::Pdm(pdm::Asked::PropsSave));
    }

    /// Reads the release records the first time the Releases tile shows.
    pub fn open_releases(&mut self) {
        if !self.pdm.releases_open {
            self.load_releases();
        }
    }

    /// Reads the BOM the first time the BOM tile shows.
    pub fn open_bom(&mut self) {
        if !self.pdm.bom_open {
            self.load_bom();
        }
    }

    /// Reads the Releases tab: every record and the release checks; from
    /// now on they are read again as they change.
    pub fn load_releases(&mut self) {
        self.pdm.releases_open = true;
        self.send(
            Request::ReleaseList { part: None },
            Route::Pdm(pdm::Asked::States),
        );
        let req = Request::ReleaseCheck { results: false };
        self.send(req, Route::Pdm(pdm::Asked::ReleaseCheck));
    }

    /// The dry run of proposing `part`, for its Propose dialog: the label
    /// it takes and every reason it can't yet.
    pub fn propose_dry(&mut self, part: &str) {
        self.pdm.releases.proposal = None;
        let req = Request::ReleasePropose {
            part: part.to_owned(),
            note: None,
            expected_revision: None,
            dry_run: true,
        };
        self.send(req, Route::Pdm(pdm::Asked::ProposeDry(part.to_owned())));
    }

    /// The revision of `part` the window last saw, to guard a write.
    pub(super) fn revision_of(&self, part: &str) -> Option<String> {
        self.model.parts.get(part).map(|p| p.revision.clone())
    }

    /// Proposes `part`'s next revision (`release.propose`), against the
    /// revision of it the window last saw.
    pub fn propose(&mut self, part: &str, note: Option<String>) {
        let req = Request::ReleasePropose {
            part: part.to_owned(),
            note,
            expected_revision: self.revision_of(part),
            dry_run: false,
        };
        self.release_write(req);
    }

    /// Makes `part`'s latest official revision, `label`, obsolete.
    pub fn obsolete(&mut self, part: &str, label: &str, note: Option<String>) {
        let req = Request::ReleaseObsolete {
            part: part.to_owned(),
            label: label.to_owned(),
            note,
            expected_revision: self.revision_of(part),
        };
        self.release_write(req);
    }

    /// Creates and moves the local `ryupdm/` tags (`release.sync`).
    pub fn sync_tags(&mut self) {
        self.release_write(Request::ReleaseSync {});
    }

    fn release_write(&mut self, req: Request) {
        self.pdm.releases.busy = true;
        self.pdm.releases.error = None;
        self.pdm.releases.done = None;
        self.send(req, Route::Pdm(pdm::Asked::ReleaseWrite));
    }

    /// The part whose BOM the BOM tab shows: the window's focus; else
    /// none, and the daemon finds the project's one root assembly, as the
    /// window shows it, or names the roots to choose from.
    pub fn bom_root(&self) -> Option<String> {
        self.model.focus.clone()
    }

    /// Reads the BOM tab (`bom.get`): the indented BOM, then the flat one
    /// when the tab shows it. From now on it is read again as what it
    /// reads changes; at most one reading is on its way, and one more
    /// after it.
    pub fn load_bom(&mut self) {
        self.pdm.bom_open = true;
        if self.pdm.bom_waiting {
            self.pdm.bom_again = true;
            return;
        }
        self.pdm.bom_waiting = true;
        self.ask_bom(false);
    }

    pub(super) fn ask_bom(&mut self, flat: bool) {
        let req = Request::BomGet {
            part: self.bom_root(),
            rev: None,
            flat,
            depth: None,
            format: None,
        };
        self.send(req, Route::Pdm(pdm::Asked::Bom { flat }));
    }

    /// While the window compares, asks for the BOM's diff from the
    /// compared revision to the working files; else drops any.
    pub(super) fn ask_bom_diff(&mut self) {
        match (&self.comparing.compare, &self.pdm.bom.root) {
            (Some(c), Some(root)) => {
                let req = Request::BomDiff {
                    part: Some(root.clone()),
                    from: Some(c.named().to_owned()),
                    to: None,
                };
                self.send(req, Route::Pdm(pdm::Asked::BomDiff));
            }
            _ => {
                self.pdm.bom.take_diff(None);
                self.bom_read();
            }
        }
    }

    /// A reading is done: the next one, if it was wanted meanwhile.
    pub(super) fn bom_read(&mut self) {
        self.pdm.bom_waiting = false;
        if std::mem::take(&mut self.pdm.bom_again) {
            self.load_bom();
        }
    }

    /// Shows the flat BOM, or the indented one.
    pub fn set_bom_flat(&mut self, flat: bool) {
        self.pdm.bom.flat = flat;
        self.load_bom();
    }

    /// Shows where `part` is used in the Part section (`bom.where_used`),
    /// read if it isn't yet.
    pub fn show_where_used(&mut self, part: Option<&str>) {
        if self.pdm.where_used_part.as_deref() == part {
            return;
        }
        self.pdm.where_used_part = part.map(str::to_owned);
        self.pdm.where_used = None;
        self.read_where_used();
    }

    /// Reads where the Part section's part is used again, if it shows one.
    pub(super) fn read_where_used(&mut self) {
        if let Some(part) = self.pdm.where_used_part.clone() {
            let req = Request::BomWhereUsed {
                part: part.clone(),
                label: None,
                working: false,
                direct: false,
                within: Vec::new(),
                no_dependents: false,
            };
            self.send(req, Route::Pdm(pdm::Asked::WhereUsed(part.clone())));
            self.ask_org(&part);
        }
    }

    /// Asks the review service where `part` is used across the
    /// organization, on a thread of its own, as `ryucad where-used --org`
    /// does: only when the project has an ID; the daemon takes no part, so
    /// its lock is never held on the network.
    fn ask_org(&mut self, part: &str) {
        let Some(project) = self
            .pdm
            .pdm_config
            .as_ref()
            .and_then(|c| c["project"].as_str())
            .and_then(|p| ryucad::doc::id::ProjectId::parse(p).ok())
        else {
            return;
        };
        let Ok(part_id) = ryucad::doc::id::Id::parse(part, "part") else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let answer = ryucad::review::token().and_then(|t| {
                let client = ryucad::review::Client::new(&ryucad::review::base_url(), &t);
                ryucad::review::where_used(
                    &client,
                    &ryucad::review::OrgQuery {
                        project,
                        part: part_id,
                        label: None,
                        direct: false,
                    },
                )
            });
            let _ = tx.send(answer);
            wake();
        });
        self.pdm.org = Some((part.to_owned(), rx));
        self.pdm.org_early = None;
    }

    /// The organization's answer, once the thread has it: into the Part
    /// section's where-used if that is still the part's, else kept for it.
    pub(super) fn take_org(&mut self) {
        let Some((part, rx)) = &self.pdm.org else {
            return;
        };
        let Ok(answer) = rx.try_recv() else {
            return;
        };
        let part = part.clone();
        self.pdm.org = None;
        let rows = answer.map(|v| crate::core::where_used::WhereUsed::org_rows(&v));
        match self.pdm.where_used.as_mut().filter(|w| w.part == part) {
            Some(w) => w.org = Some(rows),
            None => self.pdm.org_early = Some((part, rows)),
        }
    }

    /// Opens Export CSV… for the BOM as the tab shows it.
    pub fn open_bom_export(&mut self) {
        self.pdm.bom_export = Some(crate::core::bom::BomExport {
            flat: self.pdm.bom.flat,
            ..Default::default()
        });
    }

    /// Writes the open export's CSV: `bom.get` with `format: csv`, the
    /// bytes written atomically where it says.
    pub fn run_bom_export(&mut self) {
        let part = self.pdm.bom.root.clone().or_else(|| self.bom_root());
        let Some(e) = self.pdm.bom_export.as_mut() else {
            return;
        };
        let path = absolute(e.path.trim());
        e.waiting = true;
        e.error = None;
        e.done = None;
        let req = Request::BomGet {
            part,
            rev: None,
            flat: e.flat,
            depth: None,
            format: Some("csv".to_owned()),
        };
        self.send(req, Route::Pdm(pdm::Asked::BomExport(path)));
    }

    /// Saves the item number being typed in the BOM tab (`bom.renumber`),
    /// against the revision of its assembly the window last saw. Text
    /// that isn't an item number is refused here, and kept.
    pub fn save_item(&mut self) {
        let Some(e) = self.pdm.bom.editing.clone() else {
            return;
        };
        let n = match crate::core::bom::item_number(&e.text) {
            Ok(n) => n,
            Err(why) => {
                self.pdm.bom.edit_error = Some(why);
                return;
            }
        };
        if Some(n) == e.was {
            self.pdm.bom.editing = None;
            return;
        }
        let mut items = serde_json::Map::new();
        items.insert(e.part.clone(), json!(n));
        let req = Request::BomRenumber {
            part: e.parent.clone(),
            items,
            compact: false,
            expected_revision: self.part(&e.parent).map(|p| p.revision.clone()),
        };
        self.pdm.bom.saving = true;
        self.pdm.bom.edit_error = None;
        self.send(req, Route::Pdm(pdm::Asked::BomRenumber(e)));
    }

    /// Reads the BOM again if the tab was opened.
    pub(super) fn reload_bom(&mut self) {
        if self.pdm.bom_open {
            self.load_bom();
        }
    }

    /// Whether ryupdm is set up: the project has its ID.
    pub fn pdm_ready(&self) -> bool {
        self.pdm.ready()
    }

    /// Sets up ryupdm (`pdm.init`): the project's ID and default
    /// settings. The answer is the new settings.
    pub fn pdm_init(&mut self) {
        self.send(Request::PdmInit {}, Route::Pdm(pdm::Asked::PdmConfig));
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    /// Every release record and part's release state (`release.list`).
    States,
    /// The release checks' findings, for the Releases tab.
    ReleaseCheck,
    /// A proposal's dry run, for `part`'s Propose dialog.
    ProposeDry(String),
    /// A proposal, an obsolete record or a tag sync, from the Releases tab.
    ReleaseWrite,
    /// ryupdm's settings, and whether the project has an ID yet.
    PdmConfig,
    /// The BOM tab's indented BOM, or (`flat`) its flat one.
    Bom {
        flat: bool,
    },
    /// An item number set from the BOM tab (`bom.renumber`).
    BomRenumber(crate::core::bom::ItemEdit),
    /// The BOM's CSV, to write at this path.
    BomExport(PathBuf),
    /// Where a part is used, for the Part section.
    WhereUsed(String),
    /// The BOM tab's `bom.diff` from the compared revision.
    BomDiff,
    /// A part's properties for the Part section; after a conflict, with
    /// the form as it was.
    Props(String, Option<Box<PartProps>>),
    PropsSave,
}

impl Pdm {
    /// Takes the answer to one of this sub-state's requests (design
    /// 541, "Typed handles"); what it does elsewhere goes in `fx`.
    pub(super) fn answer(&mut self, why: Asked, answer: Result<Answer>, fx: &mut Effects) {
        match (why, answer) {
            (Asked::States, Ok(a)) => {
                self.states.reset(&a.result);
                self.releases.list = Some(a.result);
            }
            (Asked::ReleaseCheck, r) => match r {
                Ok(a) => self.releases.findings = findings(&a.result),
                // A failing check is still a report.
                Err(e) if e.code == Code::CheckFailed => {
                    self.releases.findings = findings(&e.details);
                }
                Err(e) => self.releases.error = Some(e),
            },
            (Asked::ProposeDry(part), r) => match r {
                Ok(a) => self.releases.proposal = Some((part, a.result)),
                Err(e) => self.releases.error = Some(e),
            },
            (Asked::ReleaseWrite, r) => {
                self.releases.busy = false;
                match r {
                    Ok(a) => {
                        self.releases.proposal = None;
                        self.releases.confirm = None;
                        self.releases.note.clear();
                        self.releases.done = Some(match a.result.get("path") {
                            Some(p) => format!("wrote {}", p.as_str().unwrap_or_default()),
                            None => crate::core::releases::synced(&a.result),
                        });
                        fx.then(Then::LoadReleases);
                    }
                    Err(e) => self.releases.error = Some(e),
                }
            }
            (Asked::Bom { flat }, r) => {
                match r {
                    Ok(a) => self.bom.take(&a.result),
                    Err(e) => self.bom.failed(e),
                }
                // The flat BOM follows the indented one, whose lines give
                // it its occurrences, and a compare's diff follows both.
                if !flat && self.bom.flat && self.bom.error.is_none() {
                    fx.then(Then::AskBom(true));
                } else {
                    fx.then(Then::AskBomDiff);
                }
            }
            (Asked::BomDiff, r) => {
                match r {
                    Ok(a) => self.bom.take_diff(Some(&a.result)),
                    Err(e) => self.bom.diff_failed(e),
                }
                fx.then(Then::BomRead);
            }
            (Asked::WhereUsed(part), r) => {
                if self.where_used_part.as_deref() == Some(part.as_str()) {
                    use crate::core::where_used::WhereUsed;
                    let mut w = match r {
                        Ok(a) => WhereUsed::from_answer(&part, &a.result),
                        Err(e) => WhereUsed::failed(&part, e),
                    };
                    // The organization's answer may have come first; else
                    // the last one stands until the new one comes.
                    match self.org_early.take() {
                        Some((p, rows)) if p == part => w.org = Some(rows),
                        _ => {
                            w.org = self
                                .where_used
                                .take()
                                .filter(|old| old.part == part)
                                .and_then(|old| old.org);
                        }
                    }
                    self.where_used = Some(w);
                }
            }
            (Asked::BomExport(path), a) => {
                let Some(e) = self.bom_export.as_mut() else {
                    return;
                };
                e.waiting = false;
                let written = a.and_then(|a| {
                    let csv = a.result["csv"].as_str().ok_or_else(|| {
                        Error::new(Code::Internal, "the BOM came without its CSV")
                    })?;
                    ryucad::doc::project::write_atomic(&path, csv)?;
                    Ok(csv.len())
                });
                match written {
                    Ok(n) => e.done = Some((path.display().to_string(), n)),
                    Err(err) => e.error = Some(err),
                }
            }
            (Asked::BomRenumber(edit), r) => {
                self.bom.saving = false;
                match r {
                    Ok(_) => self.bom.editing = None,
                    // Someone renumbered first: read again, and say what
                    // changed.
                    Err(e) if e.code == Code::Conflict => {
                        self.bom.editing = None;
                        self.bom.conflict = Some(edit);
                    }
                    Err(e) => self.bom.edit_error = Some(e.message),
                }
                fx.then(Then::LoadBom);
            }
            (Asked::PdmConfig, Ok(a)) => {
                let was = self.ready();
                self.pdm_config = Some(a.result);
                // Set up just now: the properties' faults may have moved.
                if !was
                    && self.ready()
                    && let Some(part) = self.props_part.clone()
                {
                    fx.then(Then::ReadProps(part, None));
                }
            }
            (Asked::Props(part, before), Ok(a)) => {
                if self.props_part.as_deref() == Some(part.as_str()) {
                    self.props = Some(match (before, &self.props) {
                        (Some(b), _) => b.reloaded(&a.result),
                        // Read again under someone typing (after Set up
                        // PDM): what they typed stays.
                        (None, Some(p)) if p.part == part => p.refreshed(&a.result),
                        (None, _) => PartProps::from_get(&a.result),
                    });
                }
            }
            (Asked::PropsSave, Ok(_)) => {
                if let Some(p) = &self.props {
                    let part = p.part.clone();
                    fx.then(Then::ReadProps(part, None));
                }
            }
            (Asked::PropsSave, Err(e)) => {
                if let Some(p) = &mut self.props {
                    p.saving = false;
                    if e.code == Code::Conflict {
                        let before = Box::new(p.clone());
                        let part = p.part.clone();
                        fx.then(Then::ReadProps(part, Some(before)));
                    } else {
                        p.error = Some(e);
                    }
                }
            }
            (_, Err(e)) => fx.fail(e),
        }
    }
}

impl Pdm {
    pub(super) fn on_lost(&mut self) {
        if let Some(p) = &mut self.props {
            p.saving = false;
        }
        self.releases.busy = false;
        self.bom_waiting = false;
        self.bom_again = false;
        self.bom.saving = false;
    }
}
