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
        self.send(Request::DrawingList { part: None }, Pending::DrawingsAll);
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
            Pending::DrawingList(part),
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
        self.send(req, Pending::DrawingSheet(part, drawing, sheet));
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
        self.send(req, Pending::DrawingWrite);
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
