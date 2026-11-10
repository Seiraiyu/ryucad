//! The Markup tool (design 423, Making marks in the app): redline marks
//! drawn over the model in one locked view, and on its faces, kept as
//! window state until Post sends them with a comment as one annotation
//! (`annotation.create`, or `annotation.edit` of the annotation they came
//! from). The daemon works out what each mark touches; nothing here reads
//! a mark as an instruction. No egui: points come in the view's plane
//! (metres from its target, `x` right and `y` up), or, for the pen on
//! faces, on a solid in its part's frame.

use crate::core::nav::Projector;
use ryucad::proto::{MarksArg, PinArg, Request};
use ryucad::redline::{Anchor, Kind, Mark, Plane};
use ryucad::view::Camera;

/// What a press draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Arrow,
    Line,
    Ellipse,
    Rectangle,
    /// A freehand stroke in the view's plane.
    Pen,
    /// A freehand stroke on the solid under the press.
    PenFaces,
    Text,
}

impl Tool {
    pub const ALL: [Tool; 7] = [
        Tool::Arrow,
        Tool::Line,
        Tool::Ellipse,
        Tool::Rectangle,
        Tool::Pen,
        Tool::PenFaces,
        Tool::Text,
    ];

    /// Its command in the registry (`markup.arrow`, …).
    pub fn command(self) -> &'static str {
        match self {
            Tool::Arrow => "markup.arrow",
            Tool::Line => "markup.line",
            Tool::Ellipse => "markup.ellipse",
            Tool::Rectangle => "markup.rectangle",
            Tool::Pen => "markup.pen",
            Tool::PenFaces => "markup.pen_faces",
            Tool::Text => "markup.text",
        }
    }

    pub fn of_command(id: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.command() == id)
    }

    fn kind(self) -> Kind {
        match self {
            Tool::Arrow => Kind::Arrow,
            Tool::Line => Kind::Line,
            Tool::Ellipse => Kind::Ellipse,
            Tool::Rectangle => Kind::Rectangle,
            Tool::Pen | Tool::PenFaces => Kind::Stroke,
            Tool::Text => Kind::Text,
        }
    }
}

/// A Shift-snapped line turns in steps of this many degrees.
pub const SNAP_DEG: f64 = 15.0;
/// A pen stroke is simplified to within this many pixels when let go.
pub const SIMPLIFY_PX: f64 = 0.5;

/// A mark under way: what the press began.
#[derive(Clone, Debug, PartialEq)]
pub enum Drawing {
    /// A line, arrow, ellipse or rectangle: from where the press was to
    /// where the pointer is, in the plane.
    Two { from: [f64; 2], to: [f64; 2] },
    /// The pen in the view's plane.
    Pen(Vec<[f64; 2]>),
    /// The pen on faces: the solid it was pressed on (its body key and
    /// what a mark's `on` names) and its points in that part's frame.
    Faces {
        key: (String, String),
        on: String,
        points: Vec<[f64; 3]>,
        /// The pointer left the solid: the stroke ends there.
        left: bool,
    },
}

/// The Markup tool's state, from opening its bar to Post or Cancel.
#[derive(Clone, Debug, PartialEq)]
pub struct Markup {
    /// The part or assembly the marks are drawn on: the shown model.
    pub part: String,
    /// The view every view mark lies in, locked while the bar is open.
    pub camera: Camera,
    pub tool: Tool,
    /// Marks made, in the order drawn.
    pub marks: Vec<Mark>,
    pub drawing: Option<Drawing>,
    /// A text mark being typed: where it goes and what is typed.
    pub typing: Option<([f64; 2], String)>,
    pub comment: String,
    /// What the person also pinned (Annotate… while the bar is open).
    pub pins: Vec<PinArg>,
    /// The annotation whose marks this edits; `None` for a new one.
    pub editing: Option<String>,
    /// Why Post was refused, or what can't be drawn, said on the bar.
    pub note: Option<String>,
    /// Post was sent and not yet answered.
    pub posting: bool,
}

impl Markup {
    /// The bar opened on `part` looking through `camera`.
    pub fn new(part: &str, camera: Camera) -> Markup {
        Markup {
            part: part.to_owned(),
            camera,
            tool: Tool::default(),
            marks: Vec::new(),
            drawing: None,
            typing: None,
            comment: String::new(),
            pins: Vec::new(),
            editing: None,
            note: None,
            posting: false,
        }
    }

    /// The bar reopened on annotation `id` (its author's Edit): its view,
    /// its marks without their touches (the daemon works them out again
    /// over the author's draft) and its comment.
    pub fn editing(id: &str, e: &crate::core::annotations::Entry) -> Option<Markup> {
        let marks = e.marks.as_ref()?;
        let mut m = Markup::new(marks.part.as_str(), e.view.clone()?);
        m.marks = marks
            .list
            .iter()
            .cloned()
            .map(|mut k| {
                k.touches = None;
                k
            })
            .collect();
        m.comment.clone_from(&e.comment);
        m.editing = Some(id.to_owned());
        Some(m)
    }

    pub fn plane(&self) -> Plane {
        Plane::of(&self.camera)
    }

    pub fn full(&self) -> bool {
        self.marks.len() >= ryucad::doc::redline::MAX_MARKS
    }

    /// Chooses a tool; what was under way is left.
    pub fn choose(&mut self, tool: Tool) {
        self.tool = tool;
        self.drawing = None;
        self.commit_text();
    }

    /// A press at `at` in the plane: a two-point mark or a pen stroke
    /// starts, Text takes its anchor. The pen on faces starts with
    /// [`Markup::press_face`].
    pub fn press(&mut self, at: [f64; 2]) {
        self.commit_text();
        if self.full() {
            self.note = Some(format!(
                "An annotation holds at most {} marks.",
                ryucad::doc::redline::MAX_MARKS
            ));
            return;
        }
        self.note = None;
        self.drawing = match self.tool {
            Tool::Pen => Some(Drawing::Pen(vec![at])),
            Tool::Text => {
                self.typing = Some((at, String::new()));
                None
            }
            Tool::PenFaces => None,
            _ => Some(Drawing::Two { from: at, to: at }),
        };
    }

    /// The pointer at `at` in the plane while pressed; `shift` snaps a
    /// line or arrow to [`SNAP_DEG`] and makes an ellipse a circle and a
    /// rectangle a square.
    pub fn drag(&mut self, at: [f64; 2], shift: bool) {
        match &mut self.drawing {
            Some(Drawing::Two { from, to }) => {
                *to = match (shift, self.tool) {
                    (false, _) => at,
                    (true, Tool::Line | Tool::Arrow) => snap_angle(*from, at),
                    (true, _) => square(*from, at),
                };
            }
            Some(Drawing::Pen(points)) if points.last() != Some(&at) => points.push(at),
            _ => {}
        }
    }

    /// The pen on faces pressed on a solid: its body `key`, what `on`
    /// names and the point in its part's frame.
    pub fn press_face(&mut self, key: (String, String), on: String, at: [f64; 3]) {
        self.commit_text();
        if self.full() {
            self.note = Some(format!(
                "An annotation holds at most {} marks.",
                ryucad::doc::redline::MAX_MARKS
            ));
            return;
        }
        self.note = None;
        self.drawing = Some(Drawing::Faces {
            key,
            on,
            points: vec![at],
            left: false,
        });
    }

    /// The pen on faces dragged: `at` the solid under the pointer and the
    /// point on it in its part's frame. Leaving the solid it began on ends
    /// the stroke there.
    pub fn drag_face(&mut self, at: Option<((String, String), [f64; 3])>) {
        if let Some(Drawing::Faces {
            key, points, left, ..
        }) = &mut self.drawing
        {
            if *left {
                return;
            }
            match at {
                Some((k, p)) if k == *key => {
                    if points.last() != Some(&p) {
                        points.push(p);
                    }
                }
                _ => *left = true,
            }
        }
    }

    /// The press let go: the mark under way is made, a pen stroke
    /// simplified to [`SIMPLIFY_PX`] of `pixel` (metres a pixel spans at
    /// the plane). A two-point mark of no size, or a stroke of one point,
    /// makes nothing.
    pub fn release(&mut self, pixel: f64) {
        let Some(d) = self.drawing.take() else {
            return;
        };
        let kind = self.tool.kind();
        let mark = match d {
            Drawing::Two { from, to } if from != to => Some(Mark {
                kind,
                anchor: Anchor::View(vec![from, to]),
                text: None,
                touches: None,
            }),
            Drawing::Two { .. } => None,
            Drawing::Pen(points) => {
                let points = simplified(&points, SIMPLIFY_PX * pixel);
                (points.len() >= 2).then_some(Mark {
                    kind: Kind::Stroke,
                    anchor: Anchor::View(points),
                    text: None,
                    touches: None,
                })
            }
            Drawing::Faces { on, points, .. } => {
                let points = simplified(&points, SIMPLIFY_PX * pixel);
                (points.len() >= 2).then_some(Mark {
                    kind: Kind::Stroke,
                    anchor: Anchor::Face {
                        on,
                        points_m: points,
                    },
                    text: None,
                    touches: None,
                })
            }
        };
        if let Some(m) = mark {
            self.marks.push(m);
        }
    }

    /// Ends the text being typed: one line, cut to the file's limit,
    /// made a mark unless empty.
    pub fn commit_text(&mut self) {
        let Some((at, text)) = self.typing.take() else {
            return;
        };
        let line: String = text
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .chars()
            .take(ryucad::doc::redline::MAX_TEXT)
            .collect();
        if !line.is_empty() && !self.full() {
            self.marks.push(Mark {
                kind: Kind::Text,
                anchor: Anchor::View(vec![at]),
                text: Some(line),
                touches: None,
            });
        }
    }

    /// Esc: leaves the mark under way (or the text being typed), and says
    /// whether there was one. With none, Esc leaves the tool (the
    /// caller's).
    pub fn escape(&mut self) -> bool {
        self.drawing.take().is_some() || self.typing.take().is_some()
    }

    /// Undo mark: the mark under way, else the text being typed, else the
    /// last mark made.
    pub fn undo(&mut self) {
        if self.drawing.take().is_none() && self.typing.take().is_none() {
            self.marks.pop();
        }
    }

    pub fn clear(&mut self) {
        self.drawing = None;
        self.typing = None;
        self.marks.clear();
    }

    /// What Post sends the marks as.
    pub fn marks_arg(&self) -> Option<MarksArg> {
        (!self.marks.is_empty()).then(|| MarksArg {
            part: self.part.clone(),
            list: self.marks.iter().map(Mark::to_json).collect(),
            short: Vec::new(),
        })
    }

    /// Post's request: a new annotation with the marks, the comment, the
    /// pins and the locked view (`annotation.create`), or the edited
    /// annotation's comment and whole list of marks (`annotation.edit`,
    /// `no_marks` when every mark was undone). `None` until something is
    /// typed: a mark says where, words say what (design 423).
    pub fn request(&self) -> Option<Request> {
        let comment = self.comment.trim();
        if comment.is_empty() {
            return None;
        }
        Some(match &self.editing {
            Some(id) => Request::AnnotationEdit {
                id: id.clone(),
                comment: Some(comment.to_owned()),
                pins: None,
                marks: self.marks_arg(),
                no_marks: self.marks.is_empty(),
                expect: None,
                author: None,
            },
            None => Request::AnnotationCreate {
                pins: self.pins.clone(),
                comment: comment.to_owned(),
                camera: Some(crate::core::workspace::camera_arg(&self.camera)),
                with_view: false,
                marks: self.marks_arg(),
                view: None,
                author: None,
            },
        })
    }

    /// The mark under way as a mark, to draw while it is drawn.
    pub fn under_way(&self) -> Option<Mark> {
        let kind = self.tool.kind();
        match self.drawing.as_ref()? {
            Drawing::Two { from, to } => Some(Mark {
                kind,
                anchor: Anchor::View(vec![*from, *to]),
                text: None,
                touches: None,
            }),
            Drawing::Pen(p) => Some(Mark {
                kind: Kind::Stroke,
                anchor: Anchor::View(p.clone()),
                text: None,
                touches: None,
            }),
            Drawing::Faces { on, points, .. } => Some(Mark {
                kind: Kind::Stroke,
                anchor: Anchor::Face {
                    on: on.clone(),
                    points_m: points.clone(),
                },
                text: None,
                touches: None,
            }),
        }
    }
}

/// View marks as the viewport draws them on the glass (design 423,
/// Drawing): in their plane, one colour, how opaque.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewMarks {
    pub plane: Plane,
    pub marks: Vec<Mark>,
    pub colour: ryucad::render::Rgb,
    pub alpha: f32,
}

/// `to` turned about `from` to the nearest [`SNAP_DEG`], its length kept.
pub fn snap_angle(from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return to;
    }
    let step = SNAP_DEG.to_radians();
    let a = (ryu_math::atan2(dy, dx) / step).round() * step;
    let (s, c) = ryu_math::sin_cos(a);
    let snap = |x: f64| if x.abs() < 1e-15 { 0.0 } else { x };
    [from[0] + snap(len * c), from[1] + snap(len * s)]
}

/// `to` moved so the box from `from` to it is square: its longer side
/// both ways, each keeping its sign.
pub fn square(from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let side = dx.abs().max(dy.abs());
    let sign = |d: f64| if d < 0.0 { -1.0 } else { 1.0 };
    [from[0] + sign(dx) * side, from[1] + sign(dy) * side]
}

/// `points` simplified by Douglas–Peucker to within `tolerance`, the
/// first and last kept, then halved again until at most the file's
/// [`ryucad::doc::redline::MAX_POINTS`].
pub fn simplified<const N: usize>(points: &[[f64; N]], tolerance: f64) -> Vec<[f64; N]> {
    let mut tol = tolerance.max(0.0);
    loop {
        let out = douglas_peucker(points, tol);
        if out.len() <= ryucad::doc::redline::MAX_POINTS {
            return out;
        }
        tol = if tol > 0.0 { tol * 2.0 } else { 1e-9 };
    }
}

fn douglas_peucker<const N: usize>(points: &[[f64; N]], tol: f64) -> Vec<[f64; N]> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (mut far, mut at) = (0.0, a);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let d = off_segment(*p, points[a], points[b]);
            if d > far {
                (far, at) = (d, i);
            }
        }
        if far > tol {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

/// How far `p` lies from the segment `a`–`b`.
fn off_segment<const N: usize>(p: [f64; N], a: [f64; N], b: [f64; N]) -> f64 {
    let d: [f64; N] = std::array::from_fn(|k| b[k] - a[k]);
    let w: [f64; N] = std::array::from_fn(|k| p[k] - a[k]);
    let dd: f64 = d.iter().map(|x| x * x).sum();
    let t = if dd > 0.0 {
        (d.iter().zip(&w).map(|(x, y)| x * y).sum::<f64>() / dd).clamp(0.0, 1.0)
    } else {
        0.0
    };
    w.iter()
        .zip(&d)
        .map(|(w, d)| (w - t * d) * (w - t * d))
        .sum::<f64>()
        .sqrt()
}

/// Where the pixel `at` (the viewport's, y down) of a viewport `size`
/// pixels looks onto the plane of `pr`'s camera.
pub fn plane_at(pr: &Projector, size: [f64; 2], at: [f64; 2]) -> [f64; 2] {
    let ndc = [2.0 * at[0] / size[0] - 1.0, 1.0 - 2.0 * at[1] / size[1]];
    let (o, d) = pr.ray(ndc);
    let p = [0, 1, 2].map(|k| o[k] + d[k]);
    Plane::of(&pr.camera).xy(p)
}

/// The viewport's pixel (y down) a point of `plane` is drawn at, if in
/// front of the eye.
pub fn pixel_of(pr: &Projector, size: [f64; 2], plane: &Plane, xy: [f64; 2]) -> Option<[f64; 2]> {
    let n = pr.ndc(plane.point(xy))?;
    Some([(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::view::Projection;
    use serde_json::json;

    fn camera() -> Camera {
        Camera {
            eye: [0.0, -1.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
            projection: Projection::Orthographic { height: 0.1 },
        }
    }

    #[test]
    fn each_tool_makes_its_mark() {
        let mut m = Markup::new("p", camera());
        for (tool, kind) in [
            (Tool::Arrow, Kind::Arrow),
            (Tool::Line, Kind::Line),
            (Tool::Ellipse, Kind::Ellipse),
            (Tool::Rectangle, Kind::Rectangle),
        ] {
            m.choose(tool);
            m.press([0.0, 0.0]);
            m.drag([0.01, 0.005], false);
            m.release(1e-4);
            let last = m.marks.last().unwrap();
            assert_eq!(last.kind, kind);
            assert_eq!(last.anchor, Anchor::View(vec![[0.0, 0.0], [0.01, 0.005]]));
        }
        // A click with a two-point tool makes nothing.
        m.choose(Tool::Arrow);
        m.press([0.02, 0.0]);
        m.release(1e-4);
        assert_eq!(m.marks.len(), 4);
        // Text takes a click and a line typed.
        m.choose(Tool::Text);
        m.press([0.03, 0.01]);
        m.typing.as_mut().unwrap().1 = "  round this 2 mm\nand more".into();
        m.commit_text();
        let t = m.marks.last().unwrap();
        assert_eq!(
            (t.kind, t.text.as_deref()),
            (Kind::Text, Some("round this 2 mm"))
        );
        // Every mark reads back as the file holds it.
        let arg = m.marks_arg().unwrap();
        let file = json!({"part": arg.part, "list": arg.list});
        assert_eq!(
            ryucad::redline::Marks::from_json(&file, "/marks", false)
                .unwrap()
                .list,
            m.marks
        );
    }

    #[test]
    fn shift_snaps_lines_squares_boxes_and_circles() {
        let a = snap_angle([0.0, 0.0], [1.0, 0.1]);
        assert_eq!(a, [(1.01f64).sqrt(), 0.0]);
        let b = snap_angle([0.0, 0.0], [1.0, 0.95]);
        let half = (1.0f64 + 0.95 * 0.95).sqrt() / 2f64.sqrt();
        assert!((b[0] - half).abs() < 1e-12 && (b[1] - half).abs() < 1e-12);
        assert_eq!(square([1.0, 1.0], [3.0, 0.5]), [3.0, -1.0]);
        let mut m = Markup::new("p", camera());
        m.choose(Tool::Ellipse);
        m.press([0.0, 0.0]);
        m.drag([0.02, 0.01], true);
        m.release(1e-4);
        assert_eq!(
            m.marks[0].anchor,
            Anchor::View(vec![[0.0, 0.0], [0.02, 0.02]])
        );
    }

    #[test]
    fn the_pen_is_simplified_to_half_a_pixel() {
        let mut m = Markup::new("p", camera());
        m.choose(Tool::Pen);
        m.press([0.0, 0.0]);
        // A straight run with a wobble under half a pixel, and a corner.
        for i in 1..=100 {
            let x = f64::from(i) * 1e-4;
            m.drag([x, if i % 2 == 1 { 1e-6 } else { 0.0 }], false);
        }
        m.drag([0.01, 0.01], false);
        m.release(1e-4);
        assert_eq!(
            m.marks[0].anchor,
            Anchor::View(vec![[0.0, 0.0], [0.01, 0.0], [0.01, 0.01]])
        );
        // Never more points than a file holds.
        let many: Vec<[f64; 2]> = (0..5000)
            .map(|i| {
                let t = f64::from(i) * 0.01;
                [t, ryu_math::sin(t)]
            })
            .collect();
        assert!(simplified(&many, 0.0).len() <= ryucad::doc::redline::MAX_POINTS);
    }

    #[test]
    fn the_pen_on_faces_stops_where_it_leaves_its_solid() {
        let mut m = Markup::new("asm", camera());
        m.choose(Tool::PenFaces);
        let key = ("asm".to_owned(), "i1/base".to_owned());
        m.press_face(key.clone(), "asm/i1".into(), [0.0, 0.0, 0.0]);
        m.drag_face(Some((key.clone(), [0.01, 0.0, 0.0])));
        m.drag_face(Some((("asm".into(), "i2/base".into()), [0.02, 0.0, 0.0])));
        m.drag_face(Some((key, [0.03, 0.0, 0.0])));
        m.release(1e-4);
        assert_eq!(
            m.marks[0].anchor,
            Anchor::Face {
                on: "asm/i1".into(),
                points_m: vec![[0.0; 3], [0.01, 0.0, 0.0]],
            }
        );
    }

    #[test]
    fn undo_escape_clear_and_the_limit() {
        let mut m = Markup::new("p", camera());
        m.press([0.0, 0.0]);
        m.drag([0.01, 0.0], false);
        assert!(m.escape(), "the mark under way goes");
        assert!(!m.escape(), "then Esc is the tool's");
        for i in 0..ryucad::doc::redline::MAX_MARKS {
            let x = i as f64 * 1e-3;
            m.press([x, 0.0]);
            m.drag([x, 0.01], false);
            m.release(1e-4);
        }
        m.press([0.0, 0.02]);
        assert!(m.note.as_deref().unwrap().contains("64"));
        m.undo();
        assert_eq!(m.marks.len(), 63);
        m.clear();
        assert!(m.marks.is_empty());
    }

    #[test]
    fn post_sends_a_new_annotation_or_an_edit() {
        let mut m = Markup::new("p", camera());
        m.press([0.0, 0.0]);
        m.drag([0.01, 0.0], false);
        m.release(1e-4);
        assert!(m.request().is_none(), "words say what");
        m.comment = " move here ".into();
        let Some(Request::AnnotationCreate {
            comment,
            camera: c,
            marks,
            ..
        }) = m.request()
        else {
            panic!("a create");
        };
        assert_eq!(comment, "move here");
        assert!(c.is_some());
        let marks = marks.unwrap();
        assert_eq!(marks.part, "p");
        assert!(marks.list[0].get("touches").is_none(), "the daemon's");
        m.editing = Some("aaaaaaaaaa".into());
        m.clear();
        let Some(Request::AnnotationEdit {
            id,
            marks,
            no_marks,
            ..
        }) = m.request()
        else {
            panic!("an edit");
        };
        assert_eq!((id.as_str(), marks, no_marks), ("aaaaaaaaaa", None, true));
    }

    #[test]
    fn pixels_and_the_plane_agree() {
        let pr = Projector::new(&camera(), 2.0, [0.0; 3], 0.1);
        let size = [400.0, 200.0];
        // The middle pixel is the target; the plane's y is up the screen.
        let mid = plane_at(&pr, size, [200.0, 100.0]);
        assert!(mid[0].abs() < 1e-12 && mid[1].abs() < 1e-12);
        let up = plane_at(&pr, size, [200.0, 0.0]);
        assert!((up[1] - 0.05).abs() < 1e-12, "{up:?}");
        let plane = Plane::of(&camera());
        let back = pixel_of(&pr, size, &plane, [0.02, -0.01]).unwrap();
        let again = plane_at(&pr, size, back);
        assert!((again[0] - 0.02).abs() < 1e-12 && (again[1] + 0.01).abs() < 1e-12);
    }
}
