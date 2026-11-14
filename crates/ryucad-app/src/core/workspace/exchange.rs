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
        self.send(req, Pending::ImportPlan(key));
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
        self.send(req, Pending::Import(key));
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
        self.send(req, Pending::Export);
    }
}
