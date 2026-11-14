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
        self.send(req, Pending::Params(part, before));
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
        self.send(req, Pending::Equations(conflicted));
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
        self.send(req, Pending::EquationSave);
    }

    /// Adds the Equations tile's new parameter, to the project or a part.
    pub fn add_equation(&mut self) {
        let Some(e) = &mut self.parameters.equations else {
            return;
        };
        let Some(req) = e.add_request() else { return };
        e.saving = Some(None);
        e.error = None;
        self.send(req, Pending::EquationSave);
    }

    /// Saves what was typed into the parameter panel.
    pub fn save_params(&mut self) {
        let Some(p) = &mut self.parameters.params else {
            return;
        };
        p.saving = true;
        p.error = None;
        let req = p.request();
        self.send(req, Pending::ParamSave);
    }
}
