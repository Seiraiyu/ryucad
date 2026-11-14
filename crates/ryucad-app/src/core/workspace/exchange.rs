//! Import and export, a sub-state of the window's workspace: File > Import
//! STEP…, Update import… and Export….

use super::*;

/// The import and export dialogs.
#[derive(Default)]
pub struct Exchange {
    /// File > Import STEP… or Update import…, while open.
    pub import: Option<crate::core::step::Import>,
    /// File > Export…, while open.
    pub export: Option<crate::core::export::Export>,
    /// What the last export chose, which the next one starts from.
    pub export_choices: crate::core::export::Choices,
}

impl Workspace {
    /// Asks what importing the open import's file would do: `import` as
    /// a dry run, an update's when it replaces a file the project holds.
    pub fn plan_import(&mut self) {
        let Some(i) = self.exchange.import.as_mut() else {
            return;
        };
        let key = i.key();
        if key.0.is_empty() {
            return;
        }
        i.waiting = true;
        i.error = None;
        i.done = None;
        let req = import_request(&key.0, i.update.clone(), true, key.1);
        self.send(req, Pending::Exchange(exchange::Asked::ImportPlan(key)));
    }

    /// Imports the open import's file, as its plan said: the same
    /// `import` an agent sends, one logged, undoable change.
    pub fn run_import(&mut self) {
        let Some(i) = self.exchange.import.as_mut() else {
            return;
        };
        let key = i.key();
        i.waiting = true;
        i.error = None;
        let req = import_request(&key.0, i.update.clone(), false, key.1);
        self.send(req, Pending::Exchange(exchange::Asked::Import(key)));
    }

    /// Opens File > Export… on `part`, with the last export's choices.
    pub fn open_export(&mut self, part: &str) {
        let mut e = crate::core::export::Export::new(part, self.exchange.export_choices.clone());
        // DXF only for sheet metal; the last export's DXF doesn't carry
        // over to a part with none.
        e.sheet = self.sheet_solid(part, None).map(|k| k.1);
        if e.sheet.is_none() && e.choices.format == crate::core::export::Format::Dxf {
            e.choices.format = crate::core::export::Format::Step;
        }
        // The drawing the Drawings tile shows of this part, if any.
        e.drawing = self
            .drawings
            .sheet
            .as_ref()
            .filter(|s| self.drawings.open && s.part == part)
            .map(|s| (s.drawing.clone(), s.sheet.clone()));
        if e.drawing.is_none() && e.choices.format == crate::core::export::Format::Drawing {
            e.choices.format = crate::core::export::Format::Step;
        }
        self.exchange.export = Some(e);
    }

    /// Exports the open export's part: the daemon, on this machine,
    /// writes the file where the dialog says. The choices are kept for
    /// the next export.
    pub fn run_export(&mut self) {
        let Some(e) = self.exchange.export.as_mut() else {
            return;
        };
        // The format's extension, where the typed path lacks it (#388).
        let path = absolute(&e.saved_path());
        e.path = path.display().to_string();
        e.waiting = true;
        e.error = None;
        e.done = None;
        e.faults.clear();
        self.exchange.export_choices = e.choices.clone();
        let req = e.request(&path);
        self.send(req, Pending::Exchange(exchange::Asked::Export));
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    /// An import's dry run, for the file at this path, as solids or not.
    ImportPlan((String, bool)),
    /// An import or update of the file at this path, written.
    Import((String, bool)),
    /// An export, which the daemon writes where the dialog says.
    Export,
}

impl Workspace {
    pub(super) fn answer_exchange(&mut self, why: Asked, answer: Result<Answer>) {
        match (why, answer) {
            // Only the dialog for that file takes an answer: one opened
            // since asks again.
            (Asked::ImportPlan(key), a) => {
                let Some(i) = self.exchange.import.as_mut().filter(|i| i.key() == key) else {
                    return;
                };
                i.waiting = false;
                match a {
                    Ok(a) => i.plan = Some((key, crate::core::step::Plan::from_answer(&a.result))),
                    Err(e) => i.error = Some(e),
                }
            }
            (Asked::Import(key), a) => {
                let Some(i) = self.exchange.import.as_mut().filter(|i| i.key() == key) else {
                    return;
                };
                i.waiting = false;
                match a {
                    Ok(a) => {
                        i.done = Some(crate::core::step::Plan::from_answer(&a.result));
                        i.plan = None;
                    }
                    Err(e) => i.error = Some(e),
                }
            }
            (Asked::Export, a) => {
                if let Some(e) = self.exchange.export.as_mut() {
                    e.answered(a.map(|a| a.result));
                }
            }
        }
    }
}
