//! Drawing templates: a sheet's border, zones and title block. ryucad
//! compiles in its own (`asme-a` to `asme-e`, `iso-a4` to `iso-a0`), and a
//! project may add its own at `templates/<id>.json` beside its manifest
//! (design 417).
//!
//! ```json
//! {
//!   "format": 1,
//!   "kind": "ryucad.drawing_template",
//!   "size": "ansi_b",
//!   "area": [[10, 10], [421.8, 269.4]],
//!   "keep_out": [[[251.8, 10], [421.8, 60]]],
//!   "lines": [[[10, 10], [421.8, 10], [421.8, 269.4], [10, 269.4], [10, 10]]],
//!   "texts": [{"at": [255, 50], "size": 3.5, "text": "DWG NO {number}"}]
//! }
//! ```
//!
//! Lengths are plain numbers in sheet millimetres, y up from the sheet's
//! lower left corner: a template is geometry, not intent. `area` is where
//! views go; each `keep_out` box inside it (the title block) holds none.
//! Lines are thick polylines (the border, the title block's frame); `thin`
//! ones its cells and zones. A text is one line of Inter at `size`, its
//! `{name}` fields filled when the drawing is built (`{number}`,
//! `{name}`, `{material}`, `{revision}`, `{scale}`, `{sheet}`,
//! `{sheets}`, `{standard}`, `{date}`, any PDM property, any of the
//! drawing's `fields`; a name with no value is empty), `{{` and `}}`
//! braces.

use super::drawing::{Orientation, Size, field_name_ok};
use super::files::{Obj, header, invalid};
use super::id::Id;
use crate::error::Result;
use serde_json::{Value, json};

pub const KIND: &str = "ryucad.drawing_template";

/// The newest format this build reads.
pub const FORMAT: u64 = 1;

/// The project's templates' folder, beside its manifest.
pub const TEMPLATES: &str = "templates";

/// A template's ID: a part ID.
pub fn check_id(text: &str) -> Result<Id> {
    Id::parse(text, "template")
}

/// The IDs of the templates ryucad compiles in.
pub const BUILTIN: [&str; 10] = [
    "asme-a", "asme-b", "asme-c", "asme-d", "asme-e", "iso-a0", "iso-a1", "iso-a2", "iso-a3",
    "iso-a4",
];

/// Whether `id` is a template ryucad compiles in: a project template of
/// that ID is never read.
pub fn builtin(id: &str) -> bool {
    BUILTIN.contains(&id)
}

/// Where project template `id` lives, relative to the project's root.
pub fn template_file(id: &str) -> String {
    format!("{TEMPLATES}/{id}.json")
}

/// One piece of a text: literal, or a field's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Literal(String),
    Field(String),
}

/// A text's pieces, or why it doesn't read.
pub fn parse_fields(text: &str) -> core::result::Result<Vec<Part>, String> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                lit.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                lit.push('}');
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => name.push(c),
                        None => return Err(format!("{text:?} opens a field it doesn't close")),
                    }
                }
                if !field_name_ok(&name) {
                    return Err(format!("{{{name}}} isn't a field's name"));
                }
                if !lit.is_empty() {
                    out.push(Part::Literal(std::mem::take(&mut lit)));
                }
                out.push(Part::Field(name));
            }
            '}' => return Err(format!("{text:?} closes a field it didn't open")),
            c => lit.push(c),
        }
    }
    if !lit.is_empty() {
        out.push(Part::Literal(lit));
    }
    Ok(out)
}

/// The text `pieces` fill with `value` (none: empty).
pub fn fill(pieces: &[Part], value: &dyn Fn(&str) -> Option<String>) -> String {
    pieces
        .iter()
        .map(|p| match p {
            Part::Literal(s) => s.clone(),
            Part::Field(n) => value(n).unwrap_or_default(),
        })
        .collect()
}

/// Where a text's point is along it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    #[default]
    Start,
    Middle,
    End,
}

/// A text of a template.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub at: [f64; 2],
    pub size: f64,
    pub anchor: Anchor,
    pub text: String,
    pub pieces: Vec<Part>,
}

/// A template, compiled in or read from a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub size: Size,
    pub orientation: Orientation,
    /// Where views go.
    pub area: [[f64; 2]; 2],
    /// Boxes in the area no view may cover (the title block).
    pub keep_out: Vec<[[f64; 2]; 2]>,
    /// Thick polylines.
    pub lines: Vec<Vec<[f64; 2]>>,
    /// Thin polylines.
    pub thin: Vec<Vec<[f64; 2]>>,
    pub texts: Vec<Text>,
}

fn num(v: &Value, at: &str) -> Result<f64> {
    v.as_f64()
        .filter(|x| x.is_finite() && x.abs() <= 10_000.0)
        .ok_or_else(|| invalid(at, "a length in millimetres, a number"))
}

fn point(v: &Value, at: &str) -> Result<[f64; 2]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(|| invalid(at, "a point is two numbers"))?;
    Ok([
        num(&a[0], &format!("{at}/0"))?,
        num(&a[1], &format!("{at}/1"))?,
    ])
}

fn rect(v: &Value, at: &str) -> Result<[[f64; 2]; 2]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(|| invalid(at, "a box is its lower left and upper right corners"))?;
    let (lo, hi) = (
        point(&a[0], &format!("{at}/0"))?,
        point(&a[1], &format!("{at}/1"))?,
    );
    if lo[0] >= hi[0] || lo[1] >= hi[1] {
        return Err(invalid(
            at,
            "a box's upper right corner is above and right of its lower left",
        ));
    }
    Ok([lo, hi])
}

fn polylines(v: Option<Value>, at: &str) -> Result<Vec<Vec<[f64; 2]>>> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let lines = v
        .as_array()
        .ok_or_else(|| invalid(at, "a list of polylines"))?;
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let at = format!("{at}/{i}");
            let pts = l
                .as_array()
                .filter(|p| p.len() >= 2)
                .ok_or_else(|| invalid(&at, "a polyline is two points or more"))?;
            pts.iter()
                .enumerate()
                .map(|(j, p)| point(p, &format!("{at}/{j}")))
                .collect()
        })
        .collect()
}

fn pts(lines: &[Vec<[f64; 2]>]) -> Value {
    json!(lines)
}

impl Template {
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "area": self.area,
            "format": FORMAT,
            "kind": KIND,
            "size": self.size.as_str(),
            "texts": self.texts.iter().map(|t| {
                let mut x = json!({"at": t.at, "size": t.size, "text": t.text});
                match t.anchor {
                    Anchor::Start => {}
                    Anchor::Middle => x["anchor"] = json!("middle"),
                    Anchor::End => x["anchor"] = json!("end"),
                }
                x
            }).collect::<Vec<_>>(),
        });
        if self.orientation != Orientation::Landscape {
            v["orientation"] = json!(self.orientation.as_str());
        }
        if !self.keep_out.is_empty() {
            v["keep_out"] = json!(self.keep_out);
        }
        if !self.lines.is_empty() {
            v["lines"] = pts(&self.lines);
        }
        if !self.thin.is_empty() {
            v["thin"] = pts(&self.thin);
        }
        v
    }

    pub fn from_json(v: &Value) -> Result<Template> {
        let mut o = header(v, KIND, FORMAT)?;
        let size = o
            .take("size")?
            .as_str()
            .and_then(Size::parse)
            .ok_or_else(|| {
                invalid(
                    "/size",
                    "expected one of iso_a0 to iso_a4, ansi_a to ansi_e",
                )
            })?;
        let orientation = match o.take_opt("orientation") {
            None => Orientation::Landscape,
            Some(x) => x
                .as_str()
                .and_then(Orientation::parse)
                .ok_or_else(|| invalid("/orientation", "expected one of landscape, portrait"))?,
        };
        let area = rect(&o.take("area")?, "/area")?;
        let paper = size.paper(orientation);
        if area[0][0] < 0.0 || area[0][1] < 0.0 || area[1][0] > paper[0] || area[1][1] > paper[1] {
            return Err(invalid("/area", "the drawing area lies on the sheet"));
        }
        let keep_out = match o.take_opt("keep_out") {
            None => Vec::new(),
            Some(k) => k
                .as_array()
                .ok_or_else(|| invalid("/keep_out", "a list of boxes"))?
                .iter()
                .enumerate()
                .map(|(i, b)| rect(b, &format!("/keep_out/{i}")))
                .collect::<Result<_>>()?,
        };
        let lines = polylines(o.take_opt("lines"), "/lines")?;
        let thin = polylines(o.take_opt("thin"), "/thin")?;
        let texts = match o.take_opt("texts") {
            None => Vec::new(),
            Some(t) => {
                let list = t
                    .as_array()
                    .ok_or_else(|| invalid("/texts", "a list of texts"))?;
                let mut out = Vec::with_capacity(list.len());
                for (i, t) in list.iter().enumerate() {
                    let at_ = format!("/texts/{i}");
                    let mut x = Obj::new(t, &at_)?;
                    let at = point(&x.take("at")?, &format!("{at_}/at"))?;
                    let size = num(&x.take("size")?, &format!("{at_}/size"))?;
                    if size <= 0.0 {
                        return Err(invalid(
                            &format!("{at_}/size"),
                            "a text's size is more than 0",
                        ));
                    }
                    let text = x
                        .take("text")?
                        .as_str()
                        .filter(|s| !s.contains('\n'))
                        .map(str::to_owned)
                        .ok_or_else(|| invalid(&format!("{at_}/text"), "one line of text"))?;
                    let pieces =
                        parse_fields(&text).map_err(|m| invalid(&format!("{at_}/text"), &m))?;
                    let anchor = match x.take_opt("anchor") {
                        None => Anchor::Start,
                        Some(a) => match a.as_str() {
                            Some("start") => Anchor::Start,
                            Some("middle") => Anchor::Middle,
                            Some("end") => Anchor::End,
                            _ => {
                                return Err(invalid(
                                    &format!("{at_}/anchor"),
                                    "expected one of start, middle, end",
                                ));
                            }
                        },
                    };
                    x.finish()?;
                    out.push(Text {
                        at,
                        size,
                        anchor,
                        text,
                        pieces,
                    });
                }
                out
            }
        };
        o.finish()?;
        Ok(Template {
            size,
            orientation,
            area,
            keep_out,
            lines,
            thin,
            texts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    #[test]
    fn fields_parse_and_fill() {
        let p = parse_fields("DWG {number} {{x}} rev {revision}").unwrap();
        assert_eq!(p.len(), 4);
        let v = |n: &str| (n == "number").then(|| "BRK-1".to_owned());
        assert_eq!(fill(&p, &v), "DWG BRK-1 {x} rev ");
        assert!(parse_fields("{Number}").is_err());
        assert!(parse_fields("{open").is_err());
        assert!(parse_fields("close}").is_err());
    }

    #[test]
    fn a_template_reads_writes_and_refuses_what_leaves_its_sheet() {
        let v = json!({
            "format": 1, "kind": KIND, "size": "iso_a4", "orientation": "portrait",
            "area": [[20, 10], [200, 287]],
            "keep_out": [[[20, 10], [200, 50]]],
            "lines": [[[20, 10], [200, 10], [200, 287]]],
            "texts": [{"at": [25, 20], "size": 3.5, "text": "{name}", "anchor": "start"}]
        });
        let t = Template::from_json(&v).unwrap();
        assert_eq!(t.orientation, Orientation::Portrait);
        assert_eq!(Template::from_json(&t.to_json()).unwrap(), t);
        let mut bad = v.clone();
        bad["area"] = json!([[20, 10], [300, 287]]);
        let e = Template::from_json(&bad).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidDocument, Some("/area"))
        );
        let mut bad = v;
        bad["texts"][0]["text"] = json!("{Bad}");
        assert!(Template::from_json(&bad).is_err());
    }
}
