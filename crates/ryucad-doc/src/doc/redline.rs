//! Redline marks (design 423): arrows, lines, ellipses, rectangles,
//! freehand strokes and text an annotation carries. A mark lies in its
//! annotation's view's plane (`in: "view"`, points `xy_m` in metres, the
//! plane through the view's target square to its line of sight, `x` right
//! and `y` up as the view sees them), or, for a stroke only, on the
//! model's faces (`in: "face"`, `points_m` in the frame of the part `on`
//! names). Each keeps what it touched when it was written (`touches`).
//! How a mark looks is the viewer's: no colour, width or fill is stored.
//! ryucad never reads a mark as an instruction.
//!
//! ```json
//! {"part": "rig", "list": [
//!   {"kind": "arrow", "in": "view", "xy_m": [[-0.02, 0.011], [0.009, -0.004]],
//!    "touches": [{"at": "tail", "ref": "rig/boss/base/3c1e…"}]},
//!   {"kind": "stroke", "in": "face", "on": "rig/plate", "points_m": [[…], …],
//!    "touches": [{"at": "under", "ref": "rig/plate/base/5f61…"}]},
//!   {"kind": "text", "in": "view", "xy_m": [[0.02, 0.015]], "text": "2 mm", "touches": []}]}
//! ```

use super::files::{Obj, invalid, number};
use super::id::Id;
use super::sketch::clean;
use crate::error::Result;
use serde_json::{Value, json};

/// At most this many marks an annotation.
pub const MAX_MARKS: usize = 64;
/// At most this many points a stroke.
pub const MAX_POINTS: usize = 2048;
/// A text mark's text: one line of 1 to this many characters.
pub const MAX_TEXT: usize = 120;
/// At most this many touches a mark.
pub const MAX_TOUCHES: usize = 16;
/// Every coordinate lies within ryukei's size box (metres).
pub const LIMIT_M: f64 = 500.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Line,
    Arrow,
    Ellipse,
    Rectangle,
    Stroke,
    Text,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Line,
        Kind::Arrow,
        Kind::Ellipse,
        Kind::Rectangle,
        Kind::Stroke,
        Kind::Text,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Line => "line",
            Kind::Arrow => "arrow",
            Kind::Ellipse => "ellipse",
            Kind::Rectangle => "rectangle",
            Kind::Stroke => "stroke",
            Kind::Text => "text",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Where on its mark a touch is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum At {
    /// A line's first and last points.
    Start,
    End,
    /// An arrow's tail and tip.
    Tail,
    Tip,
    /// Inside an ellipse, a rectangle or a closed stroke.
    Inside,
    /// Under an open stroke, or a stroke on the faces.
    Under,
}

impl At {
    pub fn as_str(self) -> &'static str {
        match self {
            At::Start => "start",
            At::End => "end",
            At::Tail => "tail",
            At::Tip => "tip",
            At::Inside => "inside",
            At::Under => "under",
        }
    }

    pub fn parse(s: &str) -> Option<At> {
        [At::Start, At::End, At::Tail, At::Tip, At::Inside, At::Under]
            .into_iter()
            .find(|a| a.as_str() == s)
    }
}

/// An entity a mark touched, its reference as pins write it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Touch {
    pub at: At,
    pub reference: String,
}

/// Where a mark's points are.
#[derive(Clone, Debug, PartialEq)]
pub enum Anchor {
    /// In the view's plane, metres from its target.
    View(Vec<[f64; 2]>),
    /// On the model's surface, in the frame of the part `on` names
    /// (`<asm>/<path>` or `<part>/<founder>`).
    Face { on: String, points_m: Vec<[f64; 3]> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub kind: Kind,
    pub anchor: Anchor,
    /// A text mark's text; `None` for every other kind.
    pub text: Option<String>,
    /// What it touched; `None` only in a request, for the daemon to work
    /// out.
    pub touches: Option<Vec<Touch>>,
}

impl Mark {
    /// An ellipse, a rectangle, or a stroke of three points or more whose
    /// ends are the same point.
    pub fn closed(&self) -> bool {
        match (self.kind, &self.anchor) {
            (Kind::Ellipse | Kind::Rectangle, _) => true,
            (Kind::Stroke, Anchor::View(p)) => p.len() > 2 && p.first() == p.last(),
            (Kind::Stroke, Anchor::Face { points_m, .. }) => {
                points_m.len() > 2 && points_m.first() == points_m.last()
            }
            _ => false,
        }
    }

    /// The `at`s its touches may have.
    pub fn ats(&self) -> &'static [At] {
        match self.kind {
            Kind::Line => &[At::Start, At::End],
            Kind::Arrow => &[At::Tail, At::Tip],
            Kind::Text => &[],
            _ if matches!(self.anchor, Anchor::Face { .. }) => &[At::Under],
            _ if self.closed() => &[At::Inside],
            _ => &[At::Under],
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"kind": self.kind.as_str()});
        match &self.anchor {
            Anchor::View(xy) => {
                v["in"] = json!("view");
                v["xy_m"] = json!(
                    xy.iter()
                        .map(|p| p.map(|x| number(clean(x))))
                        .collect::<Vec<_>>()
                );
            }
            Anchor::Face { on, points_m } => {
                v["in"] = json!("face");
                v["on"] = json!(on);
                v["points_m"] = json!(
                    points_m
                        .iter()
                        .map(|p| p.map(|x| number(clean(x))))
                        .collect::<Vec<_>>()
                );
            }
        }
        if let Some(t) = &self.text {
            v["text"] = json!(t);
        }
        if let Some(t) = &self.touches {
            v["touches"] = json!(
                t.iter()
                    .map(|t| json!({"at": t.at.as_str(), "ref": t.reference}))
                    .collect::<Vec<_>>()
            );
        }
        v
    }

    fn from_json(v: &Value, at: &str, part: &str, touches_required: bool) -> Result<Mark> {
        let mut o = Obj::new(v, at)?;
        let kind = o
            .take("kind")?
            .as_str()
            .and_then(Kind::parse)
            .ok_or_else(|| {
                invalid(
                    &format!("{at}/kind"),
                    "line, arrow, ellipse, rectangle, stroke or text",
                )
            })?;
        let points = |n: usize, key: &str, v: &Value| -> Result<Vec<Vec<f64>>> {
            let ptr = format!("{at}/{key}");
            let list = v
                .as_array()
                .ok_or_else(|| invalid(&ptr, "a list of points"))?;
            let (lo, hi) = match kind {
                Kind::Stroke => (2, MAX_POINTS),
                Kind::Text => (1, 1),
                _ => (2, 2),
            };
            if list.len() < lo || list.len() > hi {
                let want = if lo == hi {
                    format!("{lo} point(s)")
                } else {
                    format!("{lo} to {hi} points")
                };
                return Err(invalid(&ptr, &format!("a {} has {want}", kind.as_str())));
            }
            list.iter()
                .enumerate()
                .map(|(i, p)| {
                    let a = p
                        .as_array()
                        .filter(|a| a.len() == n)
                        .ok_or_else(|| invalid(&format!("{ptr}/{i}"), &format!("{n} numbers")))?;
                    a.iter()
                        .enumerate()
                        .map(|(k, x)| {
                            x.as_f64()
                                .filter(|x| x.is_finite() && x.abs() <= LIMIT_M)
                                .map(clean)
                                .ok_or_else(|| {
                                    invalid(
                                        &format!("{ptr}/{i}/{k}"),
                                        "a number of metres within the size box (500 m)",
                                    )
                                })
                        })
                        .collect()
                })
                .collect()
        };
        let anchor = match o.take("in")?.as_str() {
            Some("view") => {
                let p = points(2, "xy_m", &o.take("xy_m")?)?;
                Anchor::View(p.into_iter().map(|p| [p[0], p[1]]).collect())
            }
            Some("face") if kind == Kind::Stroke => {
                let on = o
                    .take("on")?
                    .as_str()
                    .filter(|s| {
                        s.split('/').next() == Some(part)
                            && s.split('/').count() >= 2
                            && !s.contains(char::is_whitespace)
                    })
                    .ok_or_else(|| {
                        invalid(
                            &format!("{at}/on"),
                            &format!("the occurrence or solid of {part} it was drawn on"),
                        )
                    })?
                    .to_owned();
                let p = points(3, "points_m", &o.take("points_m")?)?;
                Anchor::Face {
                    on,
                    points_m: p.into_iter().map(|p| [p[0], p[1], p[2]]).collect(),
                }
            }
            Some("face") => {
                return Err(invalid(
                    &format!("{at}/in"),
                    "only a stroke is drawn on faces",
                ));
            }
            _ => {
                return Err(invalid(
                    &format!("{at}/in"),
                    "expected \"view\" or \"face\"",
                ));
            }
        };
        let text = match (kind, o.take_opt("text")) {
            (Kind::Text, Some(Value::String(t)))
                if !t.trim().is_empty() && !t.contains('\n') && t.chars().count() <= MAX_TEXT =>
            {
                Some(t)
            }
            (Kind::Text, _) => {
                return Err(invalid(
                    &format!("{at}/text"),
                    "one line of 1 to 120 characters",
                ));
            }
            (_, None) => None,
            (_, Some(_)) => {
                return Err(invalid(&format!("{at}/text"), "only a text mark has text"));
            }
        };
        let mut mark = Mark {
            kind,
            anchor,
            text,
            touches: None,
        };
        mark.touches = match o.take_opt("touches") {
            None if touches_required => {
                return Err(invalid(
                    &format!("{at}/touches"),
                    "missing field \"touches\"",
                ));
            }
            None => None,
            Some(t) => {
                let ptr = format!("{at}/touches");
                let list = t
                    .as_array()
                    .ok_or_else(|| invalid(&ptr, "a list of touches"))?;
                if list.len() > MAX_TOUCHES || (mark.ats().is_empty() && !list.is_empty()) {
                    return Err(invalid(
                        &ptr,
                        &format!("at most {MAX_TOUCHES} touches, and none on a text mark"),
                    ));
                }
                let mut out = Vec::new();
                for (i, t) in list.iter().enumerate() {
                    let tp = format!("{ptr}/{i}");
                    let mut to = Obj::new(t, &tp)?;
                    let a = to
                        .take("at")?
                        .as_str()
                        .and_then(At::parse)
                        .filter(|a| mark.ats().contains(a))
                        .ok_or_else(|| {
                            let names: Vec<&str> = mark.ats().iter().map(|a| a.as_str()).collect();
                            invalid(&format!("{tp}/at"), &format!("one of {}", names.join(", ")))
                        })?;
                    let reference = to
                        .take("ref")?
                        .as_str()
                        .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
                        .ok_or_else(|| invalid(&format!("{tp}/ref"), "an entity reference"))?
                        .to_owned();
                    to.finish()?;
                    out.push(Touch { at: a, reference });
                }
                Some(out)
            }
        };
        o.finish()?;
        Ok(mark)
    }
}

/// An annotation's marks: the part or assembly they were drawn on (the
/// frame its `view` is in) and the marks in the order drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Marks {
    pub part: Id,
    pub list: Vec<Mark>,
}

impl Marks {
    pub fn to_json(&self) -> Value {
        json!({"part": self.part, "list": self.list.iter().map(Mark::to_json).collect::<Vec<_>>()})
    }

    /// Reads marks at the JSON pointer `at`: every mark's `touches` when
    /// `touches_required` (a file), else they may be left out (a
    /// request).
    pub fn from_json(v: &Value, at: &str, touches_required: bool) -> Result<Marks> {
        let mut o = Obj::new(v, at)?;
        let part = o
            .take("part")?
            .as_str()
            .and_then(|s| Id::parse(s, "part").ok())
            .ok_or_else(|| invalid(&format!("{at}/part"), "a part's ID"))?;
        let list_v = o.take("list")?;
        let list = list_v
            .as_array()
            .filter(|l| l.len() <= MAX_MARKS)
            .ok_or_else(|| invalid(&format!("{at}/list"), "a list of at most 64 marks"))?;
        let list = list
            .iter()
            .enumerate()
            .map(|(i, m)| {
                Mark::from_json(
                    m,
                    &format!("{at}/list/{i}"),
                    part.as_str(),
                    touches_required,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        o.finish()?;
        Ok(Marks { part, list })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use serde_json::json;

    fn sample() -> Value {
        json!({"part": "rig", "list": [
            {"kind": "arrow", "in": "view", "xy_m": [[-0.02, 0.011], [0.009, -0.004]],
             "touches": [{"at": "tail", "ref": "rig/boss/base/3c1e0000000000000000000000000000"},
                         {"at": "tip", "ref": "rig/plate/base/77a00000000000000000000000000000"}]},
            {"kind": "ellipse", "in": "view", "xy_m": [[0.004, 0.002], [0.016, 0.009]],
             "touches": [{"at": "inside", "ref": "rig/plate/base/1d2e0000000000000000000000000000"}]},
            {"kind": "rectangle", "in": "view", "xy_m": [[0, 0], [0.01, 0.01]], "touches": []},
            {"kind": "line", "in": "view", "xy_m": [[0, 0], [0.01, 0]],
             "touches": [{"at": "start", "ref": "rig/plate/base/1d2e0000000000000000000000000000"}]},
            {"kind": "stroke", "in": "face", "on": "rig/plate",
             "points_m": [[0, 0, 0.005], [0.01, 0, 0.005], [0.02, 0.001, 0.005]],
             "touches": [{"at": "under", "ref": "rig/plate/base/5f610000000000000000000000000000"}]},
            {"kind": "stroke", "in": "view", "xy_m": [[0, 0], [0.01, 0], [0.01, 0.01], [0, 0]],
             "touches": []},
            {"kind": "text", "in": "view", "xy_m": [[0.02, 0.015]], "text": "2 mm", "touches": []}
        ]})
    }

    #[test]
    fn marks_read_and_write_back_the_same() {
        let m = Marks::from_json(&sample(), "/marks", true).unwrap();
        assert_eq!(m.part.as_str(), "rig");
        assert_eq!(m.list.len(), 7);
        assert_eq!(m.list[0].kind, Kind::Arrow);
        assert_eq!(m.list[0].touches.as_ref().unwrap()[1].at, At::Tip);
        assert!(m.list[1].closed());
        assert!(!m.list[3].closed());
        assert!(m.list[5].closed(), "a stroke whose ends meet is closed");
        assert!(matches!(m.list[4].anchor, Anchor::Face { .. }));
        assert_eq!(m.list[6].text.as_deref(), Some("2 mm"));
        assert_eq!(Marks::from_json(&m.to_json(), "/marks", true).unwrap(), m);
        assert_eq!(m.to_json(), sample());
    }

    #[test]
    fn a_request_may_leave_touches_out_but_a_file_may_not() {
        let mut v = sample();
        v["list"][0].as_object_mut().unwrap().remove("touches");
        let m = Marks::from_json(&v, "/marks", false).unwrap();
        assert_eq!(m.list[0].touches, None);
        let e = Marks::from_json(&v, "/marks", true).unwrap_err();
        assert_eq!(e.details["pointer"], "/marks/list/0/touches");
    }

    #[test]
    fn faults_are_invalid_documents_at_their_pointer() {
        let bad = |edit: &dyn Fn(&mut Value), at: &str| {
            let mut v = sample();
            edit(&mut v);
            let e = Marks::from_json(&v, "/marks", true).unwrap_err();
            assert_eq!(e.code, Code::InvalidDocument, "{at}: {e:?}");
            assert_eq!(e.details["pointer"], at, "{e:?}");
        };
        bad(&|v| v["part"] = json!("Not A Part"), "/marks/part");
        bad(
            &|v| v["list"][0]["kind"] = json!("cloud"),
            "/marks/list/0/kind",
        );
        bad(&|v| v["list"][0]["in"] = json!("sheet"), "/marks/list/0/in");
        bad(
            &|v| v["list"][0]["xy_m"] = json!([[0, 0]]),
            "/marks/list/0/xy_m",
        );
        bad(
            &|v| v["list"][1]["xy_m"][0] = json!([0, 1e9]),
            "/marks/list/1/xy_m/0/1",
        );
        bad(
            &|v| v["list"][3]["xy_m"][0][0] = json!("x"),
            "/marks/list/3/xy_m/0/0",
        );
        bad(&|v| v["list"][2]["in"] = json!("face"), "/marks/list/2/in");
        bad(
            &|v| v["list"][4]["on"] = json!("other/plate"),
            "/marks/list/4/on",
        );
        bad(
            &|v| v["list"][4]["points_m"] = json!([[0, 0, 0]]),
            "/marks/list/4/points_m",
        );
        bad(&|v| v["list"][6]["text"] = json!(""), "/marks/list/6/text");
        bad(
            &|v| v["list"][6]["text"] = json!("two\nlines"),
            "/marks/list/6/text",
        );
        bad(
            &|v| v["list"][6]["text"] = json!("x".repeat(121)),
            "/marks/list/6/text",
        );
        bad(
            &|v| v["list"][0]["text"] = json!("no"),
            "/marks/list/0/text",
        );
        bad(
            &|v| v["list"][0]["touches"][0]["at"] = json!("inside"),
            "/marks/list/0/touches/0/at",
        );
        bad(
            &|v| v["list"][0]["touches"][0]["ref"] = json!("a b"),
            "/marks/list/0/touches/0/ref",
        );
        bad(
            &|v| v["list"][6]["touches"] = json!([{"at": "tail", "ref": "rig/x"}]),
            "/marks/list/6/touches",
        );
        bad(
            &|v| v["list"][1]["touches"] = json!(vec![json!({"at": "inside", "ref": "rig/x"}); 17]),
            "/marks/list/1/touches",
        );
        bad(
            &|v| v["list"] = json!(vec![v["list"][2].clone(); 65]),
            "/marks/list",
        );
        bad(
            &|v| v["list"][5]["xy_m"] = json!(vec![json!([0, 0]); 2049]),
            "/marks/list/5/xy_m",
        );
        bad(&|v| v["list"][0]["extra"] = json!(1), "/marks/list/0/extra");
    }
}
