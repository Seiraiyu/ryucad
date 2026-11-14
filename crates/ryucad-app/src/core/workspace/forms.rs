//! The property panel's forms, a sub-state of the window's workspace
//! (design 541, "The app's workspace"): the chosen feature's form, a
//! dimension's, an insert's preview, and each one's save and conflict.

use super::*;

/// The forms: what they hold, the requests they send (the `impl
/// Workspace` below) and their answers (`Forms::answer`).
#[derive(Default)]
pub struct Forms {
    /// The property panel's form, for the feature chosen in the tree.
    pub form: Option<Form>,
    /// The property panel's form for a dimension chosen by its label;
    /// shown instead of `form`.
    pub dimension: Option<DimForm>,
    /// What the property panel was last asked to show, a feature or a
    /// dimension: only its answer opens a form.
    pub(super) form_for: Option<(String, String)>,
    pub(super) dimension_for: Option<(String, String, String)>,
    /// What moving the insert form's chosen pin changes, once asked.
    pub insert_preview: Option<crate::core::inserts::Preview>,
}

impl Workspace {
    /// Converts import `feature` of `part` to a healed solid, or with
    /// `reference` back to a reference (`import.convert`): one logged,
    /// undoable change, a long request the status line shows. A body
    /// healing refuses is the part's rebuild's to report.
    pub fn convert_import(&mut self, part: &str, feature: &str, reference: bool) {
        let req = Request::ImportConvert {
            part: part.to_owned(),
            feature: Some(feature.to_owned()),
            reference,
            expected_revision: self.model.parts.get(part).map(|p| p.revision.clone()),
        };
        self.send(
            req,
            Pending::ImportConvert(part.to_owned(), feature.to_owned()),
        );
    }

    pub fn open_form(&mut self, feature: Option<(String, String)>) {
        self.forms.form = None;
        self.forms.dimension = None;
        self.forms.dimension_for = None;
        self.forms.form_for.clone_from(&feature);
        if let Some((part, id)) = feature {
            self.read_form(part, id, None);
        }
    }

    pub(super) fn read_form(&mut self, part: String, id: String, before: Option<Box<Form>>) {
        let req = Request::FeatureShow {
            part: part.clone(),
            id: id.clone(),
            rev: self.form_rev(),
        };
        self.send(req, Pending::Form(part, id, before));
    }

    /// Opens the form for a sketch's dimension (or closes it, with
    /// `None`); the feature's form closes.
    pub fn open_dimension(&mut self, dimension: Option<(String, String, String)>) {
        self.forms.dimension = None;
        self.forms.dimension_for.clone_from(&dimension);
        if let Some((part, sketch, constraint)) = dimension {
            self.forms.form = None;
            self.forms.form_for = None;
            self.read_dimension(part, sketch, constraint, None);
        }
    }

    pub(super) fn read_dimension(
        &mut self,
        part: String,
        sketch: String,
        constraint: String,
        before: Option<Box<DimForm>>,
    ) {
        let req = Request::SketchGet {
            part: part.clone(),
            sketch: sketch.clone(),
            quality: None,
            rev: self.form_rev(),
        };
        self.send(req, Pending::Dim(part, sketch, constraint, before));
    }

    /// Saves the value typed into the dimension's form.
    pub fn save_dimension(&mut self) {
        let Some(f) = &mut self.forms.dimension else {
            return;
        };
        f.saving = true;
        f.error = None;
        let req = f.request();
        self.send(req, Pending::DimSave);
    }

    /// Saves what was typed into the form.
    /// While an insert's form has a pin chosen other than the one read,
    /// asks once what moving it there changes (`insert.update`'s dry run;
    /// its `expected_revision` is the project's, so none is sent).
    pub fn preview_insert(&mut self) {
        let chosen = self.forms.form.as_ref().and_then(|f| {
            let r = f.rev.as_ref().filter(|r| f.ty == "insert" && r.dirty())?;
            let to = match r.chosen.as_str() {
                "working" => "working".to_owned(),
                l => format!("label:{l}"),
            };
            Some((f.part.clone(), f.feature.clone(), to))
        });
        let Some(key) = chosen else {
            self.forms.insert_preview = None;
            return;
        };
        if self
            .forms
            .insert_preview
            .as_ref()
            .is_some_and(|p| p.key == key)
        {
            return;
        }
        self.forms.insert_preview = Some(crate::core::inserts::Preview::asked(key.clone()));
        let req = Request::InsertUpdate {
            part: key.0.clone(),
            feature: key.1.clone(),
            to: Some(key.2.clone()),
            dry_run: true,
            expected_revision: None,
        };
        self.send(req, Pending::InsertPreview(key));
    }

    pub fn save_form(&mut self) {
        let Some(form) = &mut self.forms.form else {
            return;
        };
        match form.request() {
            Err(e) => form.error = Some(e),
            Ok(req) => {
                form.saving = true;
                form.error = None;
                self.send(req, Pending::Save);
            }
        }
    }
}
