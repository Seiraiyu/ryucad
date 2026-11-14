//! Annotations and the Markup bar: what the window asks the daemon for it (an `impl Workspace`
//! block) and, from 27.F's Task 7 on, what it does with the answers.
//! Its fields live on `core::annotations::Annotations`.

use super::*;

impl Workspace {
    /// Opens the Markup bar on the shown part or assembly, looking through
    /// `camera`, which it locks (design 423).
    pub fn open_markup(&mut self, camera: Camera) -> std::result::Result<(), String> {
        let shown = self.model.shown();
        let [part] = shown.as_slice() else {
            return Err("Show one part or assembly to mark up (View > Show).".into());
        };
        let part = part.to_string();
        // One tool at a time.
        self.cancel_move();
        self.close_mate();
        self.annotations.markup = Some(crate::core::markup::Markup::new(&part, camera));
        Ok(())
    }

    /// Reopens the Markup bar on annotation `id`'s view and marks (its
    /// author's Edit), showing the model they were drawn on.
    pub fn edit_markup(&mut self, id: &str) -> std::result::Result<(), String> {
        let e = self
            .annotations
            .all
            .get(id)
            .ok_or_else(|| format!("No annotation {id}."))?;
        let m = crate::core::markup::Markup::editing(id, e)
            .ok_or("This annotation has no marks or no view to reopen.")?;
        if !self.model.shown().contains(&m.part.as_str()) {
            self.set_focus(Some(m.part.clone()));
        }
        self.annotations.markup = Some(m);
        Ok(())
    }

    /// Post: the marks and the comment as one annotation, or the edited
    /// one's; the bar stays open, its marks kept, until the daemon answers.
    pub fn post_markup(&mut self) {
        let Some(m) = self.annotations.markup.as_mut() else {
            return;
        };
        m.commit_text();
        let Some(req) = m.request() else {
            m.note = Some("Type what should change: a mark says where, words say what.".into());
            return;
        };
        m.posting = true;
        m.note = None;
        self.send(req, Pending::MarkupPost);
    }

    /// The view marks the viewport draws on the glass (design 423, When):
    /// the Markup bar's, with the mark under way, in this window's colour;
    /// else the chosen annotation's, in its author's, at
    /// [`ryucad::redline::CLOSED_ALPHA`] once closed; else none, since flat
    /// marks seen from another view mislead.
    pub fn view_marks(&self) -> Option<crate::core::markup::ViewMarks> {
        use crate::core::markup::ViewMarks;
        if let Some(m) = &self.annotations.markup {
            let mut marks = m.marks.clone();
            marks.extend(m.under_way());
            return Some(ViewMarks {
                plane: m.plane(),
                marks,
                colour: ryucad::render::participant(&self.name),
                alpha: 1.0,
            });
        }
        let e = self
            .annotations
            .all
            .get(self.annotations.chosen_annotation.as_ref()?)?;
        let marks = e.marks.as_ref()?;
        // Drawn only over the part they were made on (#730), as pins are.
        if !self.model.shown().contains(&marks.part.as_str()) {
            return None;
        }
        Some(ViewMarks {
            plane: e.plane()?,
            marks: marks.list.clone(),
            colour: ryucad::render::participant(&e.by),
            alpha: if e.is_open() {
                1.0
            } else {
                ryucad::redline::CLOSED_ALPHA
            },
        })
    }

    /// The annotation the Markup bar edits, if it edits one: the bar
    /// draws its marks, so the listing's are left out.
    pub fn markup_editing(&self) -> Option<&str> {
        self.annotations
            .markup
            .as_ref()
            .and_then(|m| m.editing.as_deref())
    }

    /// The face strokes the window draws (design 423): open annotations'
    /// when `pins` are drawn, but the one the bar edits, then the bar's,
    /// with the mark under way, in this window's colour.
    pub fn face_strokes(&self, pins: bool) -> Vec<crate::core::annotations::Stroke> {
        let shown = self.model.shown();
        let place = |on: &str| self.model.placed_on(on);
        let mut strokes = if pins {
            self.annotations
                .face_strokes(&shown, place, self.markup_editing())
        } else {
            Vec::new()
        };
        if let Some(m) = &self.annotations.markup {
            let mut marks = m.marks.clone();
            marks.extend(m.under_way());
            let marks = ryucad::redline::Marks {
                part: ryucad::doc::id::Id::parse(&m.part, "part").expect("a part's ID"),
                list: marks,
            };
            strokes.extend(crate::core::annotations::face_strokes_of(
                &marks,
                self.my_name(),
                &place,
            ));
        }
        strokes
    }

    /// Cancel (or Esc with nothing under way): the bar closes and its
    /// marks go.
    pub fn cancel_markup(&mut self) {
        self.annotations.markup = None;
    }

    /// Annotate's Show agent's model (`true`), or the person's suggestions.
    pub fn set_show_agent(&mut self, on: bool) {
        self.annotations.show_agent = on;
        self.follow_draft();
    }

    /// A comment pinned to `pins`, seen through `camera`.
    pub fn create_annotation(
        &mut self,
        pins: Vec<ryucad::proto::PinArg>,
        comment: String,
        camera: Option<Camera>,
    ) {
        let req = Request::AnnotationCreate {
            pins,
            comment,
            camera: camera.as_ref().map(camera_arg),
            with_view: false,
            marks: None,
            view: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    pub fn reply_annotation(&mut self, id: &str, text: String) {
        let req = Request::AnnotationReply {
            id: id.to_owned(),
            text,
            expect: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    pub fn edit_annotation(&mut self, id: &str, comment: String) {
        let req = Request::AnnotationEdit {
            id: id.to_owned(),
            comment: Some(comment),
            pins: None,
            marks: None,
            no_marks: false,
            expect: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    pub fn delete_annotation(&mut self, id: &str) {
        let req = Request::AnnotationDelete {
            id: id.to_owned(),
            expect: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    /// Takes an annotation's change into the working files (Direct edit).
    pub fn apply_annotation(&mut self, id: &str) {
        let req = Request::AnnotationApply {
            id: id.to_owned(),
            keep_open: false,
            expect: None,
            author: None,
        };
        self.send(req, Pending::AnnotationWrite);
    }

    /// Chooses an annotation (or none): its view restored, and with a
    /// change, a compare against `annotation:<id>`; leaving one compared
    /// stops that compare.
    pub fn choose_annotation(&mut self, id: Option<&str>) {
        self.annotations.chosen_annotation = id.map(str::to_owned);
        let entry = id.and_then(|i| self.annotations.all.get(i)).cloned();
        let comparing_one = self
            .comparing
            .compare
            .as_ref()
            .is_some_and(|c| c.rev.starts_with(ryucad::session::ANNOTATION));
        match entry {
            Some(e) => {
                if let Some(c) = e.view {
                    self.view.looked = Some(c);
                }
                match e.change {
                    Some(_) => {
                        self.set_compare(Some(format!("{}{}", ryucad::session::ANNOTATION, e.id)))
                    }
                    None if comparing_one => self.set_compare(None),
                    None => {}
                }
            }
            None if comparing_one => self.set_compare(None),
            None => {}
        }
        self.model.generation += 1;
    }
}
