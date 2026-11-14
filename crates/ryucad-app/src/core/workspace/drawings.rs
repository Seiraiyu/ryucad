//! The Drawings tile: what the window asks the daemon for it (an `impl Workspace`
//! block) and, from 27.F's Task 7 on, what it does with the answers.
//! Its fields live on `core::drawings::Tile`.

use super::*;

impl Workspace {
    /// Opens the Drawings tile on `part` (else the first part shown),
    /// reading its drawings the first time or when the part changes.
    pub fn show_drawings(&mut self, part: Option<&str>) {
        let part = part
            .map(str::to_owned)
            .or_else(|| self.drawings.part.clone())
            .or_else(|| self.model.shown().first().map(|p| p.to_string()));
        let Some(part) = part else { return };
        let fresh = !self.drawings.open || self.drawings.part.as_deref() != Some(&part);
        self.drawings.open = true;
        if fresh {
            self.drawings = crate::core::drawings::Tile {
                open: true,
                part: Some(part),
                ..Default::default()
            };
            self.read_drawing_list();
        }
    }

    /// Reads every part's drawings again, for the tree's rows.
    pub fn read_drawn(&mut self) {
        self.send(
            Request::DrawingList { part: None },
            Pending::Drawings(drawings::Asked::DrawingsAll),
        );
    }

    /// A drawing's tree row chosen: the Drawings tile opens on it, its
    /// first sheet shown.
    pub fn open_drawing(&mut self, part: &str, drawing: &str) {
        self.show_drawings(Some(part));
        self.drawings.chosen = None;
        self.drawings.sheet = None;
        self.drawings.camera = None;
        self.drawings.want = Some(drawing.to_owned());
        if self.drawings.list.is_some() {
            self.read_drawing_sheet();
        }
        self.wants_tile = Some("drawings");
    }

    pub(super) fn read_drawing_list(&mut self) {
        let Some(part) = self.drawings.part.clone() else {
            return;
        };
        self.send(
            Request::DrawingList {
                part: Some(part.clone()),
            },
            Pending::Drawings(drawings::Asked::DrawingList(part)),
        );
    }

    /// Shows `drawing`'s `sheet` in the Drawings tile.
    pub fn choose_drawing(&mut self, drawing: &str, sheet: &str) {
        let same = self
            .drawings
            .chosen
            .as_ref()
            .is_some_and(|(d, k)| d == drawing && k == sheet);
        self.drawings.chosen = Some((drawing.to_owned(), sheet.to_owned()));
        if !same {
            self.drawings.camera = None;
            self.drawings.choose(None);
            self.drawings.sheet = None;
        }
        self.read_drawing_sheet();
    }

    /// Reads the chosen sheet again (`drawing.get --items`), at the
    /// revision forms read: a person's draft in Annotate.
    pub(super) fn read_drawing_sheet(&mut self) {
        let (Some(part), Some((drawing, sheet))) =
            (self.drawings.part.clone(), self.drawings.first_choice())
        else {
            return;
        };
        self.drawings.chosen = Some((drawing.clone(), sheet.clone()));
        self.drawings.loading = true;
        let req = Request::DrawingGet {
            part: part.clone(),
            drawing: drawing.clone(),
            rev: self.form_rev(),
            sheet: Some(sheet.clone()),
            items: true,
        };
        self.send(
            req,
            Pending::Drawings(drawings::Asked::DrawingSheet(part, drawing, sheet)),
        );
    }

    /// Writes `ops` to the shown drawing, guarded by the part's revision
    /// it was built at: a drag's or the form's one write.
    fn write_drawing(&mut self, ops: Value) {
        let Some(s) = &self.drawings.sheet else {
            return;
        };
        let req = Request::DrawingEdit {
            part: s.part.clone(),
            drawing: s.drawing.clone(),
            ops,
            expected_revision: s.revision.clone(),
        };
        self.drawings.saving = true;
        self.drawings.error = None;
        self.drawings.note = None;
        self.send(req, Pending::Drawings(drawings::Asked::DrawingWrite));
    }

    /// A drag of what was picked on the sheet, let go `d` mm from where it
    /// started: one `drawing.edit`, or a note saying why none.
    pub fn drag_drawing(&mut self, pick: &crate::core::drawings::Pick, d: [f64; 2]) {
        let Some(s) = &self.drawings.sheet else {
            return;
        };
        match crate::core::drawings::drag_ops(s, pick, d) {
            Ok(ops) => self.write_drawing(ops),
            Err(why) => self.drawings.note = Some(why.to_owned()),
        }
    }

    /// Writes the Drawings tile's dimension form as typed.
    pub fn save_drawing_dimension(&mut self) {
        let Some(f) = &self.drawings.form else { return };
        match f.op() {
            Ok(op) => self.write_drawing(json!([op])),
            Err(why) => self.drawings.note = Some(why),
        }
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    /// The Drawings tile's list of this part's drawings.
    DrawingList(String),
    /// Every part's drawings, for the tree.
    DrawingsAll,
    /// The Drawings tile's sheet: part, drawing and sheet asked for.
    DrawingSheet(String, String, String),
    /// A drag or form of the Drawings tile written.
    DrawingWrite,
}

impl Workspace {
    pub(super) fn answer_drawings(&mut self, why: Asked, answer: Result<Answer>) {
        match (why, answer) {
            (Asked::DrawingList(part), Ok(a)) => {
                if self.drawings.part.as_deref() == Some(&part) {
                    let list = crate::core::drawings::List::from_answer(&part, &a.result);
                    // A drawing or sheet that went: the first left.
                    if let Some((d, k)) = &self.drawings.chosen
                        && !list
                            .entries
                            .iter()
                            .any(|e| &e.drawing == d && e.sheets.iter().any(|(s, _)| s == k))
                    {
                        self.drawings.chosen = None;
                        self.drawings.sheet = None;
                        self.drawings.camera = None;
                    }
                    self.drawings.list = Some(list);
                    self.read_drawing_sheet();
                }
            }
            (Asked::DrawingList(_), Err(e)) => self.drawings.error = Some(e),
            (Asked::DrawingsAll, Ok(a)) => {
                let mut drawn: BTreeMap<String, Vec<(String, String, bool)>> = BTreeMap::new();
                for d in a.result["drawings"].as_array().into_iter().flatten() {
                    let (Some(p), Some(id)) = (d["part"].as_str(), d["drawing"].as_str()) else {
                        continue;
                    };
                    drawn.entry(p.to_owned()).or_default().push((
                        id.to_owned(),
                        d["name"].as_str().unwrap_or("").to_owned(),
                        d.get("error").is_some(),
                    ));
                }
                if drawn != self.drawings.drawing_rows {
                    self.drawings.drawing_rows = drawn;
                    self.model.generation += 1;
                }
            }
            (Asked::DrawingsAll, Err(_)) => {}
            (Asked::DrawingSheet(part, drawing, sheet), r) => {
                let current = self.drawings.part.as_deref() == Some(&part)
                    && self.drawings.chosen == Some((drawing, sheet));
                if current {
                    self.drawings.loading = false;
                    match r {
                        Ok(a) => {
                            self.drawings.sheet =
                                crate::core::drawings::Sheet::from_answer(&a.result);
                            // What was chosen, chosen again in what was read:
                            // a form reads the file as it is now.
                            let keep = self.drawings.selected.take();
                            if !self.drawings.form.as_ref().is_some_and(|f| f.dirty()) {
                                self.drawings.choose(keep);
                            } else {
                                self.drawings.selected = keep;
                            }
                        }
                        Err(e) => self.drawings.error = Some(e),
                    }
                }
            }
            (Asked::DrawingWrite, r) => {
                self.drawings.saving = false;
                match r {
                    Ok(_) => {
                        self.drawings.form = None;
                        self.read_drawing_sheet();
                    }
                    Err(e) if e.code == Code::Conflict => {
                        self.drawings.note = Some(
                            "Someone changed the drawing while you edited it: read again".into(),
                        );
                        self.drawings.form = None;
                        self.read_drawing_list();
                    }
                    Err(e) => self.drawings.error = Some(e),
                }
            }
        }
    }
}
