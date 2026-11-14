//! The Drawings tile's model (design 417, The app): the shown part's
//! drawings from `drawing.list`, a chosen sheet from `drawing.get --items`
//! (each item's exact geometry, the entity each curve draws, each view's
//! and annotation's items, and the file as written), a 2D camera over the
//! sheet, curves cut into polylines at the screen's chord, picking, drags
//! and the dimension form. Nothing here talks to the daemon: the
//! workspace sends what these make and hands back what comes.
//!
//! Humans don't draft: the tile adds no view or dimension. A drag moves a
//! view or an annotation (`drawing.edit`'s `move`), the form sets a
//! dimension's text, tolerance and precision (`set`), each one write.

use crate::core::edit::{Choice, Field};
use ryu_app::canvas::{arc_steps, bezier_at, inside, segment_distance};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A point on the sheet, in millimetres, y up; and the 2D camera over a
/// sheet (`ryu_app::canvas`', ryueda 3.C).
pub use ryu_app::canvas::{Camera, P};

/// One of the part's drawings, as `drawing.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub drawing: String,
    pub name: String,
    /// Its sheets: key and name.
    pub sheets: Vec<(String, String)>,
    /// Why its file doesn't read.
    pub error: Option<String>,
}

/// The part's drawings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct List {
    pub part: String,
    pub entries: Vec<Entry>,
    /// The part's revision the list was read at.
    pub revision: Option<String>,
}

impl List {
    /// From a `drawing.list` answer for `part`.
    pub fn from_answer(part: &str, v: &Value) -> List {
        let entries = v["drawings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["part"] == part)
            .map(|d| Entry {
                drawing: s(&d["drawing"]),
                name: d["name"].as_str().unwrap_or("").to_owned(),
                sheets: d["sheets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|x| (s(&x["sheet"]), s(&x["name"])))
                    .collect(),
                error: d["error"]["message"].as_str().map(str::to_owned),
            })
            .collect();
        List {
            part: part.to_owned(),
            entries,
            revision: v["revisions"][part].as_str().map(str::to_owned),
        }
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_owned()
}

fn point(v: &Value) -> Option<P> {
    Some([v[0].as_f64()?, v[1].as_f64()?])
}

/// One piece of a path, as `drawing.get` writes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Line(P),
    Arc {
        centre: P,
        radius: f64,
        from: f64,
        sweep: f64,
    },
    Ellipse {
        centre: P,
        a: P,
        b: P,
        from: f64,
        sweep: f64,
    },
    Bezier {
        points: Vec<P>,
        weights: Option<Vec<f64>>,
    },
}

/// Where a text's point is along it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// One item of the sheet.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Path {
        start: P,
        segments: Vec<Segment>,
        closed: bool,
        /// Stroke colour, width (mm) and dash pattern (mm), if stroked.
        stroke: Option<([u8; 3], f64, Vec<f64>)>,
        fill: Option<[u8; 3]>,
    },
    Text {
        text: String,
        at: P,
        size: f64,
        angle: f64,
        anchor: Anchor,
        colour: [u8; 3],
    },
}

fn colour(v: &Value) -> [u8; 3] {
    let t = v.as_str().unwrap_or("#000000").trim_start_matches('#');
    let h = |i: usize| u8::from_str_radix(t.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    [h(0), h(2), h(4)]
}

fn segment(v: &Value) -> Option<Segment> {
    if let Some(p) = point(&v["line"]) {
        return Some(Segment::Line(p));
    }
    if let Some(a) = v.get("arc") {
        return Some(Segment::Arc {
            centre: point(&a["centre"])?,
            radius: a["radius"].as_f64()?,
            from: a["from"].as_f64()?,
            sweep: a["sweep"].as_f64()?,
        });
    }
    if let Some(e) = v.get("ellipse") {
        return Some(Segment::Ellipse {
            centre: point(&e["centre"])?,
            a: point(&e["a"])?,
            b: point(&e["b"])?,
            from: e["from"].as_f64()?,
            sweep: e["sweep"].as_f64()?,
        });
    }
    let b = v.get("bezier")?;
    let points: Vec<P> = b["points"].as_array()?.iter().filter_map(point).collect();
    let weights = b["weights"]
        .as_array()
        .map(|w| w.iter().filter_map(Value::as_f64).collect());
    Some(Segment::Bezier { points, weights })
}

fn item(v: &Value) -> Option<Item> {
    if let Some(p) = v.get("path") {
        let stroke = v.get("stroke").map(|st| {
            let dash = st["dash"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_f64)
                .collect();
            (
                colour(&st["colour"]),
                st["width"].as_f64().unwrap_or(0.25),
                dash,
            )
        });
        return Some(Item::Path {
            start: point(&p["start"])?,
            segments: p["segments"]
                .as_array()?
                .iter()
                .filter_map(segment)
                .collect(),
            closed: p["closed"] == true,
            stroke,
            fill: v.get("fill").map(colour),
        });
    }
    let text = v["text"].as_str()?.to_owned();
    Some(Item::Text {
        text,
        at: point(&v["at"])?,
        size: v["size"].as_f64()?,
        angle: v["angle"].as_f64().unwrap_or(0.0),
        anchor: match v["anchor"].as_str() {
            Some("middle") => Anchor::Middle,
            Some("end") => Anchor::End,
            _ => Anchor::Start,
        },
        colour: colour(&v["colour"]),
    })
}

/// A view's frame on the sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewFrame {
    pub view: String,
    pub kind: String,
    pub at: P,
    pub bounds: [P; 2],
    pub items: std::ops::Range<usize>,
    /// The part (or assembly) it draws.
    pub of: String,
    pub error: Option<String>,
}

/// An annotation as built.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub annotation: String,
    pub kind: String,
    pub view: Option<String>,
    /// What it reads, as one line.
    pub text: String,
    /// Its place as written (a view's annotation relative to its view's
    /// centre), and a dimension's offset and the way it is measured.
    pub at: P,
    pub offset: Option<f64>,
    pub normal: Option<P>,
    pub items: std::ops::Range<usize>,
    /// Its entities' references.
    pub entities: Vec<String>,
    pub error: Option<String>,
}

impl Note {
    /// Whether it is a dimension (its form edits text, tolerance and
    /// precision).
    pub fn is_dimension(&self) -> bool {
        [
            "linear",
            "angular",
            "radius",
            "diameter",
            "ordinate",
            "chamfer",
            "hole_callout",
        ]
        .contains(&self.kind.as_str())
    }
}

/// One finding of the drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub sheet: Option<String>,
    pub view: Option<String>,
    pub annotation: Option<String>,
}

/// The chosen sheet, as `drawing.get --items` built it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub part: String,
    pub drawing: String,
    pub sheet: String,
    pub name: String,
    /// The part's revision it was built at: what a write is guarded by.
    pub revision: Option<String>,
    pub paper: P,
    pub items: Vec<Item>,
    /// The model entity each view's curve draws, by item.
    pub entities: BTreeMap<usize, String>,
    pub views: Vec<ViewFrame>,
    pub notes: Vec<Note>,
    pub findings: Vec<Finding>,
    pub score: Option<i64>,
    /// The drawing as written.
    pub file: Value,
}

fn range(v: &Value) -> std::ops::Range<usize> {
    let a = v[0].as_u64().unwrap_or(0) as usize;
    let b = v[1].as_u64().unwrap_or(0) as usize;
    a..b.max(a)
}

impl Sheet {
    /// From a `drawing.get --items --sheet` answer; `None` if it holds no
    /// sheet.
    pub fn from_answer(v: &Value) -> Option<Sheet> {
        let j = v["sheets"].get(0)?;
        let sheet = s(&j["sheet"]);
        let raw = j["items"].as_array().cloned().unwrap_or_default();
        let mut items = Vec::new();
        let mut entities = BTreeMap::new();
        for r in &raw {
            // An item this build can't read still takes its place, so
            // the ranges stay the daemon's.
            let it = item(r).unwrap_or(Item::Text {
                text: String::new(),
                at: [0.0, 0.0],
                size: 0.0,
                angle: 0.0,
                anchor: Anchor::Start,
                colour: [0, 0, 0],
            });
            if let Some(e) = r["entity"].as_str() {
                entities.insert(items.len(), e.to_owned());
            }
            items.push(it);
        }
        let views = j["views"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| ViewFrame {
                view: s(&w["view"]),
                kind: s(&w["type"]),
                at: point(&w["at_mm"]).unwrap_or([0.0, 0.0]),
                bounds: [
                    point(&w["box_mm"][0]).unwrap_or([0.0, 0.0]),
                    point(&w["box_mm"][1]).unwrap_or([0.0, 0.0]),
                ],
                items: range(&w["items"]),
                of: s(&w["of"]),
                error: w["error"]["message"].as_str().map(str::to_owned),
            })
            .collect();
        let notes = j["annotations"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| Note {
                annotation: s(&a["annotation"]),
                kind: s(&a["type"]),
                view: a["view"].as_str().map(str::to_owned),
                text: s(&a["text"]),
                at: point(&a["at_mm"]).unwrap_or([0.0, 0.0]),
                offset: a["offset_mm"].as_f64(),
                normal: point(&a["normal"]),
                items: range(&a["items"]),
                entities: a["entities"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e["reference"].as_str().map(str::to_owned))
                    .collect(),
                error: a["error"]["message"].as_str().map(str::to_owned),
            })
            .collect();
        let findings = v["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| f["sheet"].as_str().is_none_or(|k| k == sheet))
            .map(|f| Finding {
                code: s(&f["code"]),
                severity: s(&f["severity"]),
                message: s(&f["message"]),
                sheet: f["sheet"].as_str().map(str::to_owned),
                view: f["view"].as_str().map(str::to_owned),
                annotation: f["annotation"].as_str().map(str::to_owned),
            })
            .collect();
        Some(Sheet {
            part: s(&v["part"]),
            drawing: s(&v["drawing"]),
            name: s(&j["name"]),
            revision: v["revision"].as_str().map(str::to_owned),
            paper: point(&j["paper_mm"]).unwrap_or([279.4, 215.9]),
            score: v["scores"][&sheet].as_i64(),
            sheet,
            items,
            entities,
            views,
            notes,
            findings,
            file: v["file"].clone(),
        })
    }

    pub fn note(&self, key: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.annotation == key)
    }

    pub fn view(&self, key: &str) -> Option<&ViewFrame> {
        self.views.iter().find(|v| v.view == key)
    }

    /// The box round items `r`, if any draws.
    pub fn bounds_of(&self, r: std::ops::Range<usize>) -> Option<[P; 2]> {
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for it in &self.items[r.start.min(self.items.len())..r.end.min(self.items.len())] {
            for q in outline(it, 0.05) {
                lo = [lo[0].min(q[0]), lo[1].min(q[1])];
                hi = [hi[0].max(q[0]), hi[1].max(q[1])];
            }
        }
        (lo[0] <= hi[0]).then_some([lo, hi])
    }
}

/// A path's points, each curve cut so no chord strays more than `chord`
/// (sheet mm) from it; a text's corners.
pub fn outline(it: &Item, chord: f64) -> Vec<P> {
    match it {
        Item::Path {
            start, segments, ..
        } => {
            let mut out = vec![*start];
            let mut at = *start;
            for s in segments {
                match s {
                    Segment::Line(p) => out.push(*p),
                    Segment::Arc {
                        centre,
                        radius,
                        from,
                        sweep,
                    } => {
                        let n = arc_steps(*radius, *sweep, chord);
                        for i in 1..=n {
                            let t = from + sweep * i as f64 / n as f64;
                            let (s, c) = ryu_math::sin_cos(t);
                            out.push([centre[0] + radius * c, centre[1] + radius * s]);
                        }
                    }
                    Segment::Ellipse {
                        centre,
                        a,
                        b,
                        from,
                        sweep,
                    } => {
                        let r = ryu_math::hypot(a[0], a[1]).max(ryu_math::hypot(b[0], b[1]));
                        let n = arc_steps(r, *sweep, chord);
                        for i in 1..=n {
                            let t = from + sweep * i as f64 / n as f64;
                            let (s, c) = ryu_math::sin_cos(t);
                            out.push([
                                centre[0] + a[0] * c + b[0] * s,
                                centre[1] + a[1] * c + b[1] * s,
                            ]);
                        }
                    }
                    Segment::Bezier { points, weights } => {
                        let mut ctrl = vec![at];
                        ctrl.extend(points.iter().copied());
                        let w = weights.clone().unwrap_or_else(|| vec![1.0; ctrl.len()]);
                        let len: f64 = ctrl
                            .windows(2)
                            .map(|p| ryu_math::hypot(p[1][0] - p[0][0], p[1][1] - p[0][1]))
                            .sum();
                        let n = ((len / chord.max(1e-9)).sqrt().ceil() as usize * 2).clamp(2, 256);
                        for i in 1..=n {
                            out.push(bezier_at(&ctrl, &w, i as f64 / n as f64));
                        }
                    }
                }
                at = *out.last().unwrap_or(&at);
            }
            out
        }
        Item::Text {
            text,
            at,
            size,
            angle,
            anchor,
            ..
        } => {
            // Inter's advance is about 0.55 em a character: picking
            // and boxes only.
            let w = 0.55 * size * text.chars().count() as f64;
            let x0 = match anchor {
                Anchor::Start => 0.0,
                Anchor::Middle => -w / 2.0,
                Anchor::End => -w,
            };
            let (s, c) = ryu_math::sin_cos(*angle);
            [
                [x0, -0.25 * size],
                [x0 + w, -0.25 * size],
                [x0 + w, 0.75 * size],
                [x0, 0.75 * size],
            ]
            .map(|q| [at[0] + c * q[0] - s * q[1], at[1] + s * q[0] + c * q[1]])
            .to_vec()
        }
    }
}

/// What a click on the sheet chose.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// A model entity a view draws (`<part>/<solid>/<name>`), in that
    /// view.
    Entity {
        entity: String,
        view: String,
    },
    Annotation(String),
    View(String),
}

/// What lies at sheet point `p` within `reach` mm: an annotation first
/// (the last drawn on top), then the nearest curve naming an entity, then
/// the view whose box holds it.
pub fn pick(sheet: &Sheet, p: P, reach: f64) -> Option<Pick> {
    for n in sheet.notes.iter().rev() {
        if let Some(b) = sheet.bounds_of(n.items.clone())
            && inside(p, b, reach)
        {
            return Some(Pick::Annotation(n.annotation.clone()));
        }
    }
    let mut best: Option<(f64, Pick)> = None;
    for v in &sheet.views {
        if !inside(p, v.bounds, reach) {
            continue;
        }
        for i in v.items.clone() {
            let (Some(e), Some(it)) = (sheet.entities.get(&i), sheet.items.get(i)) else {
                continue;
            };
            let pts = outline(it, reach / 4.0);
            let d = pts
                .windows(2)
                .map(|w| segment_distance(p, w[0], w[1]))
                .fold(f64::INFINITY, f64::min);
            if d <= reach && best.as_ref().is_none_or(|(b, _)| d < *b) {
                best = Some((
                    d,
                    Pick::Entity {
                        entity: e.clone(),
                        view: v.view.clone(),
                    },
                ));
            }
        }
    }
    if let Some((_, p)) = best {
        return Some(p);
    }
    sheet
        .views
        .iter()
        .find(|v| inside(p, v.bounds, 0.0))
        .map(|v| Pick::View(v.view.clone()))
}

/// A length on the sheet as a drawing writes it: millimetres to a
/// thousandth.
pub fn mm(x: f64) -> String {
    let r = (x * 1000.0).round() / 1000.0;
    format!("{} mm", if r == 0.0 { 0.0 } else { r })
}

/// The `drawing.edit` ops a drag of what `pick` chose by `d` sheet mm
/// writes, or why it writes none.
pub fn drag_ops(sheet: &Sheet, pick: &Pick, d: P) -> Result<Value, &'static str> {
    if d[0] == 0.0 && d[1] == 0.0 {
        return Err("nothing moved");
    }
    match pick {
        Pick::View(key) | Pick::Entity { view: key, .. } => {
            let v = sheet.view(key).ok_or("the view is gone")?;
            let written = &sheet.file["views"][key];
            match written["from"].as_str() {
                // Placed along its parent: only its distance moves.
                Some(parent)
                    if written.get("at").is_none()
                        && ["projected", "section", "auxiliary"].contains(&v.kind.as_str()) =>
                {
                    let p = sheet.view(parent).ok_or("its parent isn't on this sheet")?;
                    let rel = [v.at[0] - p.at[0], v.at[1] - p.at[1]];
                    let len = ryu_math::hypot(rel[0], rel[1]);
                    if len == 0.0 {
                        return Err("it stands on its parent");
                    }
                    let dir = [rel[0] / len, rel[1] / len];
                    let along = d[0] * dir[0] + d[1] * dir[1];
                    Ok(json!([{"op": "move", "id": key, "offset": mm(len + along)}]))
                }
                _ => Ok(json!([{"op": "move", "id": key,
                    "at": [mm(v.at[0] + d[0]), mm(v.at[1] + d[1])]}])),
            }
        }
        Pick::Annotation(key) => {
            let n = sheet.note(key).ok_or("the annotation is gone")?;
            let mut ops = vec![json!({"op": "move", "id": key,
                "at": [mm(n.at[0] + d[0]), mm(n.at[1] + d[1])]})];
            if let (Some(o), Some(nm)) = (n.offset, n.normal) {
                ops.push(json!({"op": "move", "id": key,
                    "offset": mm(o + d[0] * nm[0] + d[1] * nm[1])}));
            }
            Ok(json!(ops))
        }
    }
}

/// The tolerance forms the form offers, as the file names them.
pub const TOLERANCES: [&str; 7] = [
    "none",
    "symmetric",
    "deviation",
    "limits",
    "fit",
    "basic",
    "reference",
];

/// A dimension's form: its text, tolerance and precision, as written.
#[derive(Clone, Debug, PartialEq)]
pub struct DimensionForm {
    pub annotation: String,
    pub text: Field,
    pub tolerance: Choice,
    /// The tolerance's value: `0.1 mm` (symmetric), `+0.1 mm, -0.05 mm`
    /// (deviation or limits, upper first) or `H7/g6` (fit).
    pub value: Field,
    pub precision: Field,
}

fn quantity_text(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

impl DimensionForm {
    /// From the drawing as written; `None` for an annotation that isn't
    /// a dimension or callout.
    pub fn from_file(file: &Value, key: &str, kind: &str) -> Option<DimensionForm> {
        let note = Note {
            annotation: key.to_owned(),
            kind: kind.to_owned(),
            view: None,
            text: String::new(),
            at: [0.0; 2],
            offset: None,
            normal: None,
            items: 0..0,
            entities: Vec::new(),
            error: None,
        };
        if !note.is_dimension() {
            return None;
        }
        let a = &file["annotations"][key];
        let t = &a["tolerance"];
        let (form, value) = match t.as_object().and_then(|m| m.iter().next()) {
            None => ("none", String::new()),
            Some((k, x)) => (
                k.as_str(),
                match x {
                    Value::Array(pair) => pair
                        .iter()
                        .map(quantity_text)
                        .collect::<Vec<_>>()
                        .join(", "),
                    Value::Bool(_) => String::new(),
                    other => quantity_text(other),
                },
            ),
        };
        let form = TOLERANCES
            .iter()
            .find(|f| **f == form)
            .copied()
            .unwrap_or("none");
        Some(DimensionForm {
            annotation: key.to_owned(),
            text: Field::new("text", a["text"].as_str().unwrap_or("").to_owned()),
            tolerance: Choice::new("tolerance", &TOLERANCES, form.to_owned()),
            value: Field::new("tolerance value", value),
            precision: Field::new(
                "precision",
                a["precision"]
                    .as_u64()
                    .map(|p| p.to_string())
                    .unwrap_or_default(),
            ),
        })
    }

    pub fn dirty(&self) -> bool {
        self.text.dirty() || self.tolerance.dirty() || self.value.dirty() || self.precision.dirty()
    }

    pub fn revert(&mut self) {
        self.text.text.clone_from(&self.text.read);
        self.tolerance.chosen.clone_from(&self.tolerance.read);
        self.value.text.clone_from(&self.value.read);
        self.precision.text.clone_from(&self.precision.read);
    }

    /// The tolerance as the file writes it, or why what was typed isn't
    /// one.
    fn tolerance_json(&self) -> Result<Value, String> {
        let v = self.value.text.trim();
        let two = || -> Result<Value, String> {
            let parts: Vec<&str> = v
                .split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .collect();
            match parts.as_slice() {
                [u, l] => Ok(json!([u, l])),
                _ => Err("give the upper and the lower, a comma between".into()),
            }
        };
        Ok(match self.tolerance.chosen.as_str() {
            "none" => Value::Null,
            "basic" => json!({"basic": true}),
            "reference" => json!({"reference": true}),
            "symmetric" if !v.is_empty() => json!({"symmetric": v}),
            "fit" if !v.is_empty() => json!({"fit": v}),
            "deviation" => json!({"deviation": two()?}),
            "limits" => json!({"limits": two()?}),
            _ => return Err("a symmetric tolerance or a fit needs its value".into()),
        })
    }

    /// The `drawing.edit` op that writes what was typed, or why it can't.
    pub fn op(&self) -> Result<Value, String> {
        let mut fields = serde_json::Map::new();
        if self.text.dirty() {
            let t = self.text.text.trim();
            fields.insert(
                "text".into(),
                if t.is_empty() { Value::Null } else { json!(t) },
            );
        }
        if self.tolerance.dirty() || self.value.dirty() {
            fields.insert("tolerance".into(), self.tolerance_json()?);
        }
        if self.precision.dirty() {
            let p = self.precision.text.trim();
            fields.insert(
                "precision".into(),
                if p.is_empty() {
                    Value::Null
                } else {
                    json!(
                        p.parse::<u64>()
                            .map_err(|_| "precision is a whole number of decimals".to_owned())?
                    )
                },
            );
        }
        Ok(json!({"op": "set", "id": self.annotation, "fields": fields}))
    }
}

/// What the Drawings tile shows and holds between frames.
#[derive(Clone, Debug, Default)]
pub struct Tile {
    /// Whether it has been opened: nothing is read before.
    pub open: bool,
    /// The part whose drawings it lists.
    pub part: Option<String>,
    pub list: Option<List>,
    /// The drawing and sheet chosen.
    pub chosen: Option<(String, String)>,
    /// A drawing asked for by its tree row: its first sheet is chosen
    /// once the list has it.
    pub want: Option<String>,
    pub sheet: Option<Sheet>,
    /// A `drawing.get` on its way.
    pub loading: bool,
    /// Where the tile looks; `None` fits the sheet on the next frame.
    pub camera: Option<Camera>,
    /// What was last clicked.
    pub selected: Option<Pick>,
    /// The chosen dimension's form.
    pub form: Option<DimensionForm>,
    /// A write on its way.
    pub saving: bool,
    /// The last refusal or failure.
    pub error: Option<ryucad::error::Error>,
    /// A line said under the sheet (why a drag wrote nothing, a conflict).
    pub note: Option<String>,
    /// Every part's drawings, for the tree: ID, name, and whether its
    /// file doesn't read (`drawing.list` of the project).
    pub drawing_rows: BTreeMap<String, Vec<(String, String, bool)>>,
}

impl Tile {
    /// The chosen drawing and sheet, else the list's first.
    pub fn first_choice(&self) -> Option<(String, String)> {
        if let Some(c) = &self.chosen {
            return Some(c.clone());
        }
        let want = self.want.as_deref();
        self.list
            .as_ref()?
            .entries
            .iter()
            .filter(|e| want.is_none_or(|w| e.drawing == w))
            .find_map(|e| {
                e.sheets
                    .first()
                    .map(|(k, _)| (e.drawing.clone(), k.clone()))
            })
    }

    /// Chooses what a click picked: an annotation that is a dimension
    /// opens its form.
    pub fn choose(&mut self, pick: Option<Pick>) {
        self.form = match (&pick, &self.sheet) {
            (Some(Pick::Annotation(k)), Some(s)) => s
                .note(k)
                .and_then(|n| DimensionForm::from_file(&s.file, k, &n.kind)),
            _ => None,
        };
        self.selected = pick;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> Value {
        json!({
            "part": "p", "drawing": "d", "revision": "r1", "scores": {"s1": 97},
            "findings": [
                {"code": "dwg_untoleranced", "severity": "warning", "sheet": "s1",
                 "annotation": "d1", "message": "d1 has no tolerance"},
                {"code": "x", "severity": "warning", "sheet": "s2", "message": "elsewhere"}
            ],
            "file": {"views": {"v1": {"type": "base", "sheet": "s1", "at": ["100 mm", "80 mm"]},
                               "v2": {"type": "projected", "from": "v1", "side": "right", "sheet": "s1"}},
                     "annotations": {"d1": {"type": "linear", "view": "v1", "text": "<> TYP",
                                            "tolerance": {"deviation": ["0.1 mm", "-0.05 mm"]},
                                            "precision": 3}}},
            "sheets": [{
                "sheet": "s1", "name": "Sheet 1", "paper_mm": [431.8, 279.4],
                "views": [
                    {"view": "v1", "type": "base", "at_mm": [100.0, 80.0],
                     "box_mm": [[80.0, 70.0], [120.0, 90.0]], "items": [0, 2], "of": "p"},
                    {"view": "v2", "type": "projected", "at_mm": [160.0, 80.0],
                     "box_mm": [[150.0, 70.0], [170.0, 90.0]], "items": [2, 3], "of": "p"}
                ],
                "annotations": [
                    {"annotation": "d1", "type": "linear", "view": "v1", "text": "40.000 TYP",
                     "at_mm": [0.0, 20.0], "offset_mm": 12.0, "normal": [0.0, 1.0],
                     "items": [3, 4], "entities": [{"reference": "p/b/aa"}]}
                ],
                "items": [
                    {"path": {"start": [80.0, 70.0], "segments": [{"line": [120.0, 70.0]}], "closed": false},
                     "stroke": {"colour": "#000000", "width": 0.6, "dash": []}, "entity": "p/b/aa"},
                    {"path": {"start": [110.0, 80.0],
                              "segments": [{"arc": {"centre": [100.0, 80.0], "radius": 10.0,
                                                    "from": 0.0, "sweep": std::f64::consts::TAU}}],
                              "closed": true},
                     "stroke": {"colour": "#000000", "width": 0.6, "dash": []}, "entity": "p/b/bb"},
                    {"path": {"start": [150.0, 70.0], "segments": [{"line": [170.0, 70.0]}], "closed": false},
                     "stroke": {"colour": "#000000", "width": 0.6, "dash": []}, "entity": "p/b/cc"},
                    {"text": "40.000 TYP", "at": [100.0, 100.0], "size": 3.0, "angle": 0.0,
                     "anchor": "middle", "colour": "#000000"}
                ]
            }]
        })
    }

    #[test]
    fn a_list_keeps_its_parts_drawings_and_says_what_doesnt_read() {
        let v = json!({"drawings": [
            {"part": "p", "drawing": "d", "name": "Block",
             "sheets": [{"sheet": "s1", "name": "Sheet 1", "size": "ansi_b"}]},
            {"part": "p", "drawing": "e", "error": {"code": "invalid_document", "message": "bad"}},
            {"part": "q", "drawing": "z", "name": "Other", "sheets": []}
        ], "revisions": {"p": "r1"}});
        let l = List::from_answer("p", &v);
        assert_eq!(l.entries.len(), 2);
        assert_eq!(
            l.entries[0].sheets,
            [("s1".to_owned(), "Sheet 1".to_owned())]
        );
        assert_eq!(l.entries[1].error.as_deref(), Some("bad"));
        assert_eq!(l.revision.as_deref(), Some("r1"));
    }

    #[test]
    fn a_sheet_reads_its_items_entities_frames_and_its_findings() {
        let s = Sheet::from_answer(&answer()).unwrap();
        assert_eq!(s.items.len(), 4);
        assert_eq!(s.entities.len(), 3);
        assert_eq!(s.views[0].items, 0..2);
        assert_eq!(s.notes[0].normal, Some([0.0, 1.0]));
        // Only this sheet's findings, and its score.
        assert_eq!(s.findings.len(), 1);
        assert_eq!(s.score, Some(97));
        assert_eq!(s.revision.as_deref(), Some("r1"));
    }

    #[test]
    fn curves_are_cut_within_the_chord() {
        let s = Sheet::from_answer(&answer()).unwrap();
        for chord in [1.0, 0.1, 0.01] {
            let pts = outline(&s.items[1], chord);
            // Every chord's middle lies within `chord` of the circle.
            for w in pts.windows(2) {
                let m = [(w[0][0] + w[1][0]) / 2.0, (w[0][1] + w[1][1]) / 2.0];
                let r = ryu_math::hypot(m[0] - 100.0, m[1] - 80.0);
                assert!(10.0 - r <= chord + 1e-12, "{chord}: {r}");
            }
            // And it closes where it started.
            let (a, b) = (pts[0], *pts.last().unwrap());
            assert!(ryu_math::hypot(a[0] - b[0], a[1] - b[1]) < 1e-9);
        }
        // A rational quadratic quarter circle stays on its circle.
        let q = Item::Path {
            start: [1.0, 0.0],
            segments: vec![Segment::Bezier {
                points: vec![[1.0, 1.0], [0.0, 1.0]],
                weights: Some(vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0]),
            }],
            closed: false,
            stroke: None,
            fill: None,
        };
        for p in outline(&q, 0.001) {
            assert!((ryu_math::hypot(p[0], p[1]) - 1.0).abs() < 1e-12, "{p:?}");
        }
    }

    #[test]
    fn the_camera_fits_the_sheet_and_zooms_about_the_pointer() {
        let size = [800.0, 600.0];
        let mut c = Camera::fit([431.8, 279.4], size);
        let q = c.to_screen([0.0, 0.0], size);
        assert!(q[0] >= 0.0 && q[1] <= 600.0);
        let back = c.to_sheet(c.to_screen([12.0, 34.0], size), size);
        assert!((back[0] - 12.0).abs() < 1e-9 && (back[1] - 34.0).abs() < 1e-9);
        let at = [200.0, 100.0];
        let before = c.to_sheet(at, size);
        c.zoom_at(2.0, at, size);
        let after = c.to_sheet(at, size);
        assert!((before[0] - after[0]).abs() < 1e-9 && (before[1] - after[1]).abs() < 1e-9);
        let (z, x) = (c.zoom, c.centre[0]);
        c.pan([10.0, 0.0]);
        assert!((c.centre[0] - (x - 10.0 / z)).abs() < 1e-12);
        assert!((c.chord() - 0.25 / z).abs() < 1e-12);
    }

    #[test]
    fn picking_takes_an_annotation_then_the_nearest_curve_then_a_view() {
        let s = Sheet::from_answer(&answer()).unwrap();
        assert_eq!(
            pick(&s, [100.0, 100.5], 0.5),
            Some(Pick::Annotation("d1".into()))
        );
        assert_eq!(
            pick(&s, [84.0, 70.2], 0.5),
            Some(Pick::Entity {
                entity: "p/b/aa".into(),
                view: "v1".into()
            })
        );
        // On the circle, not the edge below it.
        assert_eq!(
            pick(&s, [100.0, 89.9], 0.5),
            Some(Pick::Entity {
                entity: "p/b/bb".into(),
                view: "v1".into()
            })
        );
        assert_eq!(pick(&s, [85.0, 85.0], 0.5), Some(Pick::View("v1".into())));
        assert_eq!(pick(&s, [10.0, 10.0], 0.5), None);
    }

    #[test]
    fn a_drag_moves_a_views_place_a_projected_views_distance_and_a_dimension() {
        let s = Sheet::from_answer(&answer()).unwrap();
        assert_eq!(
            drag_ops(&s, &Pick::View("v1".into()), [5.0, -2.5]).unwrap(),
            json!([{"op": "move", "id": "v1", "at": ["105 mm", "77.5 mm"]}])
        );
        // v2 stands 60 mm right of v1: a drag right and up takes it 4 mm
        // further along, never off its row.
        assert_eq!(
            drag_ops(&s, &Pick::View("v2".into()), [4.0, 9.0]).unwrap(),
            json!([{"op": "move", "id": "v2", "offset": "64 mm"}])
        );
        // A dimension's text, and its line by the drag along its normal.
        assert_eq!(
            drag_ops(&s, &Pick::Annotation("d1".into()), [1.0, 3.0]).unwrap(),
            json!([{"op": "move", "id": "d1", "at": ["1 mm", "23 mm"]},
                   {"op": "move", "id": "d1", "offset": "15 mm"}])
        );
        assert!(drag_ops(&s, &Pick::View("v1".into()), [0.0, 0.0]).is_err());
    }

    #[test]
    fn the_dimension_form_reads_the_file_and_sets_what_changed() {
        let s = Sheet::from_answer(&answer()).unwrap();
        let mut f = DimensionForm::from_file(&s.file, "d1", "linear").unwrap();
        assert_eq!(f.text.read, "<> TYP");
        assert_eq!(f.tolerance.read, "deviation");
        assert_eq!(f.value.read, "0.1 mm, -0.05 mm");
        assert_eq!(f.precision.read, "3");
        assert!(!f.dirty());
        f.tolerance.chosen = "fit".into();
        f.value.text = "H7".into();
        assert_eq!(
            f.op().unwrap(),
            json!({"op": "set", "id": "d1", "fields": {"tolerance": {"fit": "H7"}}})
        );
        f.precision.text = "two".into();
        assert!(f.op().is_err());
        f.revert();
        assert!(!f.dirty());
        f.tolerance.chosen = "none".into();
        f.text.text = String::new();
        assert_eq!(
            f.op().unwrap(),
            json!({"op": "set", "id": "d1", "fields": {"text": null, "tolerance": null}})
        );
        // Notes have no such form.
        assert!(DimensionForm::from_file(&s.file, "n1", "note").is_none());
    }
}
