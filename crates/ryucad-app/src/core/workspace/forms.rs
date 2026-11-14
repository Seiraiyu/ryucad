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
            Pending::Forms(forms::Asked::ImportConvert(
                part.to_owned(),
                feature.to_owned(),
            )),
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
        self.send(req, Pending::Forms(forms::Asked::Form(part, id, before)));
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
        self.send(
            req,
            Pending::Forms(forms::Asked::Dim(part, sketch, constraint, before)),
        );
    }

    /// Saves the value typed into the dimension's form.
    pub fn save_dimension(&mut self) {
        let Some(f) = &mut self.forms.dimension else {
            return;
        };
        f.saving = true;
        f.error = None;
        let req = f.request();
        self.send(req, Pending::Forms(forms::Asked::DimSave));
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
        self.send(req, Pending::Forms(forms::Asked::InsertPreview(key)));
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
                self.send(req, Pending::Forms(forms::Asked::Save));
            }
        }
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    /// A feature read for the form: its part and ID; after a conflict,
    /// with the form as it was when the save failed.
    Form(String, String, Option<Box<Form>>),
    Save,
    /// A sketch read for a dimension's form: its part, sketch and
    /// constraint; after a conflict, with the form as it was.
    Dim(String, String, String, Option<Box<DimForm>>),
    DimSave,
    /// A foreign instance's form's labels (`deps.parts`), for this
    /// assembly and instance.
    FormLabels(String, String),
    /// An insert update's dry run for the form: part, insert and `to`.
    InsertPreview((String, String, String)),
    /// An import's mode set (`import.convert`): its part and feature.
    ImportConvert(String, String),
}

impl Forms {
    /// Takes the answer to one of this sub-state's requests (design
    /// 541, "Typed handles"); what it does elsewhere goes in `fx`.
    pub(super) fn answer(
        &mut self,
        why: Asked,
        answer: Result<Answer>,
        model: &Model,
        states: &crate::core::pdm::States,
        fx: &mut Effects,
    ) {
        match (why, answer) {
            (Asked::InsertPreview(key), a) => {
                if let Some(p) = self.insert_preview.as_mut().filter(|p| p.key == key) {
                    p.answer = Some(a.map(|a| crate::core::inserts::Moved::from_answer(&a.result)));
                }
            }
            // The rebuild follows as `model.changed`; an open form on the
            // import is read again, its mode with it.
            (Asked::ImportConvert(part, feature), a) => match a {
                Ok(_) => {
                    let key = (part, feature);
                    if self.form_for.as_ref() == Some(&key) {
                        fx.then(Then::OpenForm(key));
                    }
                }
                Err(e) => fx.fail(e),
            },
            (Asked::Form(part, id, before), Ok(a)) => {
                // Only the form last asked for opens (a dimension may have
                // been chosen since); a reload after a conflict only if
                // that form is still open.
                let asked = self.form_for.as_ref() == Some(&(part.clone(), id));
                let wanted = asked
                    && match (&before, &self.form) {
                        (Some(b), Some(f)) => b.feature == f.feature && b.part == f.part,
                        (None, _) => true,
                        (Some(_), None) => false,
                    };
                if wanted {
                    let mut form = match before {
                        Some(b) => b.reloaded(&a.result),
                        None => Form::from_show_in(&part, &a.result, model.unit()),
                    };
                    if let Some(p) = model.parts.get(&part) {
                        // A reference import's solid is never offered to
                        // act on (design 364).
                        form.offer(&p.modellable());
                    }
                    // An instance may take its part's latest release; another
                    // project's, any of its dependency's official labels.
                    let placed = a.result["part"].as_str().unwrap_or_default();
                    if let Some(project) = form.foreign.clone() {
                        let name = model.marks.name(&project);
                        let req = Request::DepsParts {
                            name,
                            part: Some(placed.to_owned()),
                        };
                        let why =
                            Pending::Forms(Asked::FormLabels(part.clone(), form.feature.clone()));
                        fx.send(req, why);
                    } else if let Some(l) =
                        states.by_part.get(placed).and_then(|s| s["label"].as_str())
                    {
                        form.offer_revs(&[l.to_owned()]);
                    }
                    // An insert of this project's part may take any of its
                    // official labels (design 521): every one, read now.
                    if form.ty == "insert" && form.foreign.is_none() {
                        let req = Request::ReleaseList {
                            part: Some(placed.to_owned()),
                        };
                        fx.send(
                            req,
                            Pending::Forms(Asked::FormLabels(part.clone(), form.feature.clone())),
                        );
                    }
                    self.form = Some(form);
                }
            }
            (Asked::FormLabels(part, id), Ok(a)) => {
                if let Some(f) = self
                    .form
                    .as_mut()
                    .filter(|f| f.part == part && f.feature == id)
                {
                    // An insert's from `release.list`, an instance's of
                    // another project from `deps.parts`.
                    let of = f.of.clone().unwrap_or_default();
                    let labels = crate::core::inserts::official_labels(&a.result, &of);
                    f.offer_revs(&labels);
                }
            }
            // Not fetched: the label stays as read; the Dependencies tab says why.
            (Asked::FormLabels(..), Err(_)) => {}
            (Asked::Dim(part, sketch, constraint, before), Ok(a)) => {
                // Only the dimension last asked for opens, as for features.
                let key = (part.clone(), sketch.clone(), constraint.clone());
                let wanted = self.dimension_for.as_ref() == Some(&key)
                    && match (&before, &self.dimension) {
                        (Some(_), Some(_)) | (None, _) => true,
                        (Some(_), None) => false,
                    };
                if wanted {
                    self.dimension = match before {
                        Some(b) => b.reloaded(&a.result),
                        None => DimForm::from_get(&part, &sketch, &constraint, &a.result),
                    };
                }
            }
            (Asked::DimSave, Ok(_)) => {
                // Read it back as the files now have it.
                if let Some(f) = &self.dimension {
                    let (p, s, c) = (f.part.clone(), f.sketch.clone(), f.constraint.clone());
                    fx.then(Then::ReadDimension(p, s, c, None));
                }
            }
            (Asked::DimSave, Err(e)) => {
                if let Some(f) = &mut self.dimension {
                    f.saving = false;
                    if e.code == Code::Conflict {
                        let before = Box::new(f.clone());
                        let (p, s, c) = (f.part.clone(), f.sketch.clone(), f.constraint.clone());
                        fx.then(Then::ReadDimension(p, s, c, Some(before)));
                    } else {
                        f.error = Some(e);
                    }
                }
            }
            (Asked::Save, Ok(a)) => {
                if let Some(f) = &self.form {
                    let mut v = a.result["feature"].clone();
                    v["revision"] = a.result["revision"].clone();
                    self.form = Some(Form::from_show_in(&f.part, &v, model.unit()));
                }
            }
            (Asked::Save, Err(e)) => {
                // A feature that would change a reference import: the
                // card offers Convert to solid, beside the form's own.
                if e.code == Code::ReferenceBody {
                    fx.fail(e.clone());
                }
                if let Some(f) = &mut self.form {
                    f.saving = false;
                    if e.code == Code::Conflict {
                        // Read it again and show what changed underneath.
                        let (part, id) = (f.part.clone(), f.feature.clone());
                        let before = Box::new(f.clone());
                        fx.then(Then::ReadForm(part, id, Some(before)));
                    } else {
                        f.refuse(e);
                    }
                }
            }
            (_, Err(e)) => fx.fail(e),
        }
    }
}
