//! The Dependencies tab: what the window asks the daemon for it (an `impl Workspace`
//! block) and, from 27.F's Task 7 on, what it does with the answers.
//! Its fields live on `core::deps::Deps`.

use super::*;

impl Workspace {
    /// Reads the dependencies and what is outdated again, once at a time:
    /// one asked for meanwhile follows.
    pub fn load_deps(&mut self) {
        if self.deps.deps_waiting {
            self.deps.deps_again = true;
            return;
        }
        self.deps.deps_waiting = true;
        self.send(Request::DepsList {}, Route::Deps(deps::Asked::DepsList));
        self.send(
            Request::DepsOutdated {},
            Route::Deps(deps::Asked::DepsOutdated),
        );
    }

    /// Fetches every dependency, or those named.
    pub fn fetch_deps(&mut self, names: Vec<String>) {
        self.deps.busy = true;
        self.deps.failed = None;
        self.send_aside(
            Request::DepsFetch { names },
            Route::Deps(deps::Asked::DepsWrite),
        );
    }

    /// Adds the dependency typed into the tab.
    pub fn add_dep(&mut self) {
        let d = &self.deps;
        let name = d.add_name.trim();
        let req = Request::DepsAdd {
            url: d.add_url.trim().to_owned(),
            path: d.add_path.trim().to_owned(),
            name: (!name.is_empty()).then(|| name.to_owned()),
            expected_revision: d.revision.clone(),
        };
        self.deps.busy = true;
        self.deps.failed = None;
        self.send_aside(req, Route::Deps(deps::Asked::DepsWrite));
    }

    /// Removes dependency `name`.
    pub fn remove_dep(&mut self, name: &str) {
        let req = Request::DepsRemove {
            name: name.to_owned(),
            expected_revision: self.deps.revision.clone(),
        };
        self.deps.busy = true;
        self.deps.failed = None;
        self.send(req, Route::Deps(deps::Asked::DepsWrite));
    }

    /// Update's first step: a dry run (fetching first) that says what
    /// would move and what would fail, shown before anything is written.
    pub fn plan_update(&mut self, names: Vec<String>, part: Option<String>) {
        let req = Request::DepsUpdate {
            names: names.clone(),
            part: part.clone(),
            to: None,
            offline: false,
            dry_run: true,
            expected_revision: None,
        };
        self.deps.busy = true;
        self.deps.failed = None;
        self.deps.update = None;
        self.send_aside(req, Route::Deps(deps::Asked::DepsDry(names, part)));
    }

    /// Writes the update its dry run showed, from what that fetched.
    pub fn run_update(&mut self) {
        let Some(u) = self.deps.update.take() else {
            return;
        };
        let req = Request::DepsUpdate {
            names: u.names,
            part: u.part,
            to: None,
            offline: true,
            dry_run: false,
            expected_revision: self.deps.revision.clone(),
        };
        self.deps.busy = true;
        self.send(req, Route::Deps(deps::Asked::DepsWrite));
    }

    /// Opens Place from dependency… on the shown assembly.
    pub fn open_picker(&mut self) {
        let Some(asm) = self.shown_assembly() else {
            return;
        };
        self.deps.picker = Some(Picker::new(&asm));
    }

    /// The picker's dependency chosen: its released parts are read.
    pub fn pick_dependency(&mut self, name: &str) {
        let Some(p) = self.deps.picker.as_mut() else {
            return;
        };
        p.dependency = Some(name.to_owned());
        p.parts.clear();
        p.reading = true;
        let req = Request::DepsParts {
            name: name.to_owned(),
            part: None,
        };
        self.send(req, Route::Deps(deps::Asked::DepsParts(name.to_owned())));
    }

    /// Places the picker's part at its label, at the assembly's origin.
    pub fn place_picked(&mut self) {
        let Some(p) = self.deps.picker.as_ref() else {
            return;
        };
        let (Some(of), Some(rev)) = (p.of(), p.label.clone()) else {
            return;
        };
        let asm = p.assembly.clone();
        let expected = self.part(&asm).map(|x| x.revision.clone());
        let Some(p) = self.deps.picker.as_mut() else {
            return;
        };
        p.placing = true;
        p.error = None;
        let req = Request::FeatureAdd {
            part: asm,
            ty: "instance".to_owned(),
            params: Default::default(),
            placement: None,
            fields: ryucad::proto::FeatureFields {
                of: Some(of),
                rev: Some(rev),
                ..Default::default()
            },
            id: None,
            position: Default::default(),
            expected_revision: expected,
            dry_run: false,
        };
        self.send(req, Route::Deps(deps::Asked::DepsPlace));
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
// The variants keep the names they had in `Pending` before 27.F.
#[allow(clippy::enum_variant_names)]
pub(crate) enum Asked {
    /// `deps.list` and `deps.outdated`, for the tab and the marks.
    DepsList,
    DepsOutdated,
    /// A fetch, an add, an update or a removal from the tab.
    DepsWrite,
    /// An update's dry run, for the names and part it was asked for.
    DepsDry(Vec<String>, Option<String>),
    /// The picker's `deps.parts`, for this dependency.
    DepsParts(String),
    /// The picker's `feature.add`.
    DepsPlace,
}

impl Deps {
    /// Takes the answer to one of this sub-state's requests (design
    /// 541, "Typed handles"); what it does elsewhere goes in `fx`.
    pub(super) fn answer(&mut self, why: Asked, answer: Result<Answer>, fx: &mut Effects) {
        match (why, answer) {
            (Asked::DepsList, a) => {
                match a {
                    Ok(a) => self.listed(&a.result),
                    Err(e) => self.error = Some(e),
                }
                fx.marks(self.marks());
            }
            (Asked::DepsOutdated, a) => {
                self.deps_waiting = false;
                if let Ok(a) = a {
                    self.outdated(&a.result);
                    fx.marks(self.marks());
                }
                if std::mem::take(&mut self.deps_again) {
                    fx.then(Then::LoadDeps);
                }
            }
            (Asked::DepsWrite, a) => {
                self.busy = false;
                match a {
                    Ok(a) => {
                        if let Some(f) = a.result["fetched"].as_array() {
                            let changed = f
                                .iter()
                                .flat_map(|r| r["branches"].as_array().into_iter().flatten())
                                .filter(|b| b["changed"] == true)
                                .count();
                            let what = if f.len() == 1 {
                                "repository"
                            } else {
                                "repositories"
                            };
                            self.fetched =
                                Some(format!("fetched {} {what}, {changed} changed", f.len()));
                        }
                        if a.result["dependency"].is_object() {
                            self.add_url.clear();
                            self.add_path.clear();
                            self.add_name.clear();
                        }
                    }
                    Err(e) => self.failed = Some(e),
                }
                // A fetch writes no project file: nothing else says so.
                fx.then(Then::LoadDeps);
            }
            (Asked::DepsDry(names, part), a) => {
                self.busy = false;
                match a {
                    Ok(a) => {
                        let mut u = Update::from_answer(&a.result);
                        u.names = names;
                        u.part = part;
                        self.update = Some(u);
                    }
                    Err(e) => self.failed = Some(e),
                }
                fx.then(Then::LoadDeps);
            }
            (Asked::DepsParts(name), a) => {
                if let Some(p) = self
                    .picker
                    .as_mut()
                    .filter(|p| p.dependency.as_deref() == Some(name.as_str()))
                {
                    match a {
                        Ok(a) => p.parts_read(&a.result),
                        Err(e) => {
                            p.reading = false;
                            p.error = Some(e);
                        }
                    }
                }
            }
            (Asked::DepsPlace, a) => match a {
                Ok(_) => self.picker = None,
                Err(e) => {
                    if let Some(p) = self.picker.as_mut() {
                        p.placing = false;
                        p.error = Some(e);
                    }
                }
            },
        }
    }
}
