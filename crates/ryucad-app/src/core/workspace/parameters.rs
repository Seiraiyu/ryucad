//! The parameter panel and the Equations tile, a sub-state of the
//! window's workspace (design 541, "The app's workspace").

use super::*;

/// The parameters: the panel of the part shown and the Equations table.
#[derive(Default)]
pub struct Parameters {
    /// The parameter panel, for the part it shows (see `show_params`).
    pub params: Option<Params>,
    pub(super) params_part: Option<String>,
    /// The Equations tile's table, once it is open (see `show_equations`).
    pub equations: Option<Equations>,
    pub(super) equations_open: bool,
}

impl Workspace {
    /// Shows `part`'s parameters in the panel, read if they aren't yet.
    pub fn show_params(&mut self, part: Option<&str>) {
        if self.parameters.params_part.as_deref() == part {
            return;
        }
        self.parameters.params_part = part.map(str::to_owned);
        self.parameters.params = None;
        if let Some(p) = part {
            self.read_params(p.to_owned(), None);
        }
    }

    pub(super) fn read_params(&mut self, part: String, before: Option<Box<Params>>) {
        let req = Request::ParamList {
            part: Some(part.clone()),
            scope: None,
            rev: self.form_rev(),
        };
        self.send(
            req,
            Pending::Parameters(parameters::Asked::Params(part, before)),
        );
    }

    /// Shows the Equations tile's table, read the first time.
    pub fn show_equations(&mut self) {
        if !self.parameters.equations_open {
            self.parameters.equations_open = true;
            self.read_equations(false);
        }
    }

    pub(super) fn read_equations(&mut self, conflicted: bool) {
        let req = Request::EquationsList {
            part: None,
            all: false,
            rev: None,
        };
        self.send(
            req,
            Pending::Parameters(parameters::Asked::Equations(conflicted)),
        );
    }

    /// Reads the table again if it is open and nothing is being written,
    /// keeping its filter and what was typed.
    pub(super) fn reload_equations(&mut self) {
        if !self.parameters.equations_open {
            return;
        }
        if !self
            .parameters
            .equations
            .as_ref()
            .is_some_and(|e| e.saving.is_some())
        {
            self.read_equations(false);
        }
    }

    /// Writes row `i` of the Equations tile as typed, where it lives.
    pub fn save_equation(&mut self, i: usize) {
        let Some(e) = &mut self.parameters.equations else {
            return;
        };
        let Some(req) = e.request(i) else { return };
        e.saving = Some(Some(i));
        e.error = None;
        self.send(req, Pending::Parameters(parameters::Asked::EquationSave));
    }

    /// Adds the Equations tile's new parameter, to the project or a part.
    pub fn add_equation(&mut self) {
        let Some(e) = &mut self.parameters.equations else {
            return;
        };
        let Some(req) = e.add_request() else { return };
        e.saving = Some(None);
        e.error = None;
        self.send(req, Pending::Parameters(parameters::Asked::EquationSave));
    }

    /// Saves what was typed into the parameter panel.
    pub fn save_params(&mut self) {
        let Some(p) = &mut self.parameters.params else {
            return;
        };
        p.saving = true;
        p.error = None;
        let req = p.request();
        self.send(req, Pending::Parameters(parameters::Asked::ParamSave));
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    /// A part's parameters for the panel; after a conflict, with the
    /// panel as it was.
    Params(String, Option<Box<Params>>),
    ParamSave,
    /// The Equations tile's table (`equations.list`), and whether a
    /// write conflicted. Read again over the table as it is when the
    /// answer comes, never as it was when it was asked for: what was
    /// typed meanwhile stays.
    Equations(bool),
    /// A row of the Equations tile written.
    EquationSave,
}

impl Parameters {
    /// Takes the answer to one of this sub-state's requests (design
    /// 541, "Typed handles"); what it does elsewhere goes in `fx`.
    pub(super) fn answer(
        &mut self,
        why: Asked,
        answer: Result<Answer>,
        unit: &str,
        fx: &mut Effects,
    ) {
        match (why, answer) {
            (Asked::Params(part, before), Ok(a)) => {
                if self.params_part.as_deref() == Some(part.as_str()) {
                    let unit = unit.to_owned();
                    self.params = Some(match before {
                        Some(b) => b.reloaded(&a.result, &unit),
                        None => Params::from_list(&a.result, &unit),
                    });
                }
            }
            (Asked::ParamSave, Ok(_)) => {
                if let Some(p) = &self.params {
                    let part = p.part.clone();
                    fx.then(Then::ReadParams(part, None));
                }
            }
            (Asked::Equations(conflicted), Ok(a)) => {
                let unit = unit.to_owned();
                self.equations = Some(match &self.equations {
                    Some(now) => now.reloaded(&a.result, &unit, conflicted),
                    None => Equations::from_list(&a.result, &unit),
                });
            }
            (Asked::Equations(_), Err(e)) => {
                let mut table = self.equations.take().unwrap_or_default();
                table.saving = None;
                table.error = Some(e);
                self.equations = Some(table);
            }
            (Asked::EquationSave, Ok(_)) => {
                if let Some(e) = &mut self.equations {
                    // What was written reads back as written.
                    match e.saving.take() {
                        Some(Some(i)) => {
                            if let Some(r) = e.rows.get_mut(i) {
                                r.field.read = r.field.text.trim().to_owned();
                            }
                        }
                        Some(None) => {
                            e.new_name.clear();
                            e.new_value.clear();
                        }
                        None => {}
                    }
                    fx.then(Then::ReadEquations(false));
                }
            }
            (Asked::EquationSave, Err(err)) => {
                if let Some(e) = &mut self.equations {
                    e.saving = None;
                    if err.code == Code::Conflict {
                        fx.then(Then::ReadEquations(true));
                    } else {
                        e.error = Some(err);
                    }
                }
            }
            (Asked::ParamSave, Err(e)) => {
                if let Some(p) = &mut self.params {
                    p.saving = false;
                    if e.code == Code::Conflict {
                        let before = Box::new(p.clone());
                        let part = p.part.clone();
                        fx.then(Then::ReadParams(part, Some(before)));
                    } else {
                        p.error = Some(e);
                    }
                }
            }
            (_, Err(e)) => fx.fail(e),
        }
    }
}

impl Parameters {
    pub(super) fn on_lost(&mut self) {
        if let Some(p) = &mut self.params {
            p.saving = false;
        }
        if let Some(e) = &mut self.equations {
            e.saving = None;
        }
    }
}
