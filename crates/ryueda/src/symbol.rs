//! Symbols: `library/symbols/<id>.json` (kind `ryueda.symbol`). The whole
//! symbol, as drawn: its units, pins with type, side, place and length,
//! and its body's graphics. Coordinates are nanometres from the symbol's
//! origin, y up. There are no colours, widths or fonts: the look is the
//! style's (stage 2).

use crate::coord::METRE;
use crate::error::Code;
use crate::fault::Fault;
use crate::id::Id;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const KIND: &str = "ryueda.symbol";
pub const FORMAT: u64 = 1;

/// A point `[x, y]` in nanometres.
pub type Point = [i64; 2];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symbol {
    pub name: String,
    /// How many units the symbol has: 1 for most parts, one per gate or
    /// section for the rest.
    pub units: u32,
    /// By pin number (`1`, `A1`, `EP`).
    #[serde(default)]
    pub pins: BTreeMap<String, Pin>,
    /// By generated ID: `g1`, `g2`, … (one past the largest).
    #[serde(default)]
    pub graphics: BTreeMap<Id, Graphic>,
}

/// KiCad's set of electrical pin types, which ERC's matrix reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinType {
    Input,
    Output,
    Bidirectional,
    TriState,
    Passive,
    Free,
    Unspecified,
    PowerIn,
    PowerOut,
    OpenCollector,
    OpenEmitter,
    NoConnect,
}

/// Which way a pin points from the body to its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// How a pin's end is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinShape {
    #[default]
    Line,
    Inverted,
    Clock,
    InvertedClock,
    InputLow,
    ClockLow,
    OutputLow,
    EdgeClockHigh,
    NonLogic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: PinType,
    /// The unit it belongs to; `None` for a pin every unit has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<u32>,
    /// The connection point.
    pub at: Point,
    pub side: Side,
    pub length_nm: i64,
    #[serde(default)]
    pub shape: PinShape,
    /// Other names and types the pin can take, as MCUs list them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternates: Vec<Alternate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alternate {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: PinType,
    #[serde(default)]
    pub shape: PinShape,
}

/// What a closed shape is filled with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    #[default]
    None,
    /// The body's colour (KiCad's background fill).
    Body,
    /// The outline's colour.
    Outline,
}

/// One piece of the body's drawing, in a unit or every unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Graphic {
    Rect {
        start: Point,
        end: Point,
        #[serde(default)]
        fill: Fill,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<u32>,
    },
    Polyline {
        points: Vec<Point>,
        #[serde(default)]
        fill: Fill,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<u32>,
    },
    /// Through three points, start to end by way of `mid`: exact on
    /// integers.
    Arc {
        start: Point,
        mid: Point,
        end: Point,
        #[serde(default)]
        fill: Fill,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<u32>,
    },
    Circle {
        center: Point,
        radius_nm: i64,
        #[serde(default)]
        fill: Fill,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<u32>,
    },
    /// A cubic Bézier: start, two control points, end.
    Bezier {
        points: [Point; 4],
        #[serde(default)]
        fill: Fill,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<u32>,
    },
    Text {
        text: String,
        at: Point,
        size_nm: i64,
        #[serde(default)]
        rotation_mdeg: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<u32>,
    },
}

impl Graphic {
    pub fn unit(&self) -> Option<u32> {
        match self {
            Graphic::Rect { unit, .. }
            | Graphic::Polyline { unit, .. }
            | Graphic::Arc { unit, .. }
            | Graphic::Circle { unit, .. }
            | Graphic::Bezier { unit, .. }
            | Graphic::Text { unit, .. } => *unit,
        }
    }

    pub fn unit_mut(&mut self) -> &mut Option<u32> {
        match self {
            Graphic::Rect { unit, .. }
            | Graphic::Polyline { unit, .. }
            | Graphic::Arc { unit, .. }
            | Graphic::Circle { unit, .. }
            | Graphic::Bezier { unit, .. }
            | Graphic::Text { unit, .. } => unit,
        }
    }

    /// Every point it is drawn through (a circle's centre).
    pub fn points_mut(&mut self) -> Vec<&mut Point> {
        match self {
            Graphic::Rect { start, end, .. } => vec![start, end],
            Graphic::Polyline { points, .. } => points.iter_mut().collect(),
            Graphic::Arc {
                start, mid, end, ..
            } => vec![start, mid, end],
            Graphic::Circle { center, .. } => vec![center],
            Graphic::Bezier { points, .. } => points.iter_mut().collect(),
            Graphic::Text { at, .. } => vec![at],
        }
    }

    fn points(&self) -> Vec<Point> {
        let mut g = self.clone();
        g.points_mut().into_iter().map(|p| *p).collect()
    }
}

/// Whether `n` can name a pin: 1 to 32 characters, none of them white
/// space, a control character or `/` (which separates a reference's
/// steps).
pub fn pin_number_ok(n: &str) -> bool {
    !n.is_empty()
        && n.chars().count() <= 32
        && n.chars()
            .all(|c| !(c.is_whitespace() || c.is_control() || c == '/'))
}

/// Twice the signed area of the triangle `a b c`: zero when collinear.
pub(crate) fn cross(a: Point, b: Point, c: Point) -> i128 {
    let d = |p: Point, q: Point| [i128::from(q[0] - p[0]), i128::from(q[1] - p[1])];
    let (u, v) = (d(a, b), d(a, c));
    u[0] * v[1] - u[1] * v[0]
}

impl Symbol {
    /// What `library_*` checks find, each with its JSON pointer: units in
    /// range and each with a pin, pins well formed, graphics drawable,
    /// every value within a metre.
    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        let at = |p: String| move |f: Fault| f.with("pointer", p);
        if self.units < 1 {
            out.push(at("/units".into())(Fault::error(
                Code::LibraryUnitInvalid,
                "a symbol has at least one unit",
            )));
        }
        let in_range = |u: Option<u32>| u.is_none_or(|u| (1..=self.units).contains(&u));
        let far = |v: i64| v.unsigned_abs() > METRE as u64;
        for (n, p) in &self.pins {
            let ptr = format!("/pins/{}", pointer_escape(n));
            if !pin_number_ok(n) {
                out.push(at(ptr.clone())(Fault::error(
                    Code::LibraryPinInvalid,
                    format!("pin number {n:?} has white space, a control character or /, or is longer than 32"),
                )));
            }
            if !in_range(p.unit) {
                out.push(at(format!("{ptr}/unit"))(Fault::error(
                    Code::LibraryUnitInvalid,
                    format!(
                        "pin {n} is in unit {}, but the symbol has {}",
                        p.unit.unwrap_or(0),
                        self.units
                    ),
                )));
            }
            if p.length_nm < 0 {
                out.push(at(format!("{ptr}/length_nm"))(Fault::error(
                    Code::LibraryPinInvalid,
                    format!("pin {n} has a negative length"),
                )));
            }
            let mut names: Vec<&str> = p.alternates.iter().map(|a| a.name.as_str()).collect();
            names.sort_unstable();
            if names.windows(2).any(|w| w[0] == w[1]) {
                out.push(at(format!("{ptr}/alternates"))(Fault::error(
                    Code::LibraryPinInvalid,
                    format!("pin {n} lists an alternate name twice"),
                )));
            }
            if far(p.at[0]) || far(p.at[1]) || far(p.length_nm) {
                out.push(at(ptr.clone())(Fault::error(
                    Code::LibraryOutOfRange,
                    format!("pin {n} reaches past a metre from the origin"),
                )));
            }
        }
        // A drawing with no pins at all (a logo, a mounting hole) is a
        // symbol; a unit of several is a gate, and has a pin of its own.
        // One fault however many units lack a pin: a file may claim
        // billions of units, and each must not cost a fault.
        if self.units > 1 {
            let with: std::collections::BTreeSet<u32> = self
                .pins
                .values()
                .filter_map(|p| p.unit)
                .filter(|u| (1..=self.units).contains(u))
                .collect();
            let missing = self.units as usize - with.len();
            if missing > 0 {
                let first = (1..=self.units)
                    .find(|u| !with.contains(u))
                    .expect("a unit lacks a pin");
                out.push(at("/units".into())(Fault::error(
                    Code::LibraryUnitInvalid,
                    match missing {
                        1 => format!("unit {first} has no pin of its own"),
                        n => format!("{n} units have no pin of their own, unit {first} first"),
                    },
                )));
            }
        }
        for (id, g) in &self.graphics {
            let ptr = format!("/graphics/{id}");
            if !in_range(g.unit()) {
                out.push(at(format!("{ptr}/unit"))(Fault::error(
                    Code::LibraryUnitInvalid,
                    format!(
                        "{id} is in unit {}, but the symbol has {}",
                        g.unit().unwrap_or(0),
                        self.units
                    ),
                )));
            }
            if let Some(why) = undrawable(g) {
                out.push(at(ptr.clone())(Fault::error(
                    Code::LibraryGraphicInvalid,
                    format!("{id}: {why}"),
                )));
            }
            let size = match g {
                Graphic::Circle { radius_nm, .. } => *radius_nm,
                Graphic::Text { size_nm, .. } => *size_nm,
                _ => 0,
            };
            if g.points().iter().any(|p| far(p[0]) || far(p[1])) || far(size) {
                out.push(at(ptr)(Fault::error(
                    Code::LibraryOutOfRange,
                    format!("{id} reaches past a metre from the origin"),
                )));
            }
        }
        out
    }

    /// The next graphic ID: one past the largest `g<n>`.
    pub fn next_graphic(&self) -> Id {
        let n = self
            .graphics
            .keys()
            .filter_map(|id| id.as_str().strip_prefix('g')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        Id::parse(&format!("g{}", n + 1), "graphic").expect("g<n> is an ID")
    }
}

/// Why a graphic can't be drawn, if it can't.
fn undrawable(g: &Graphic) -> Option<&'static str> {
    match g {
        Graphic::Rect { start, end, .. } if start[0] == end[0] || start[1] == end[1] => {
            Some("a rectangle's corners differ in x and in y")
        }
        Graphic::Polyline { points, .. } if points.len() < 2 => {
            Some("a polyline has at least two points")
        }
        Graphic::Arc {
            start, mid, end, ..
        } if cross(*start, *mid, *end) == 0 => Some("an arc's three points can't be collinear"),
        Graphic::Circle { radius_nm, .. } if *radius_nm <= 0 => {
            Some("a circle's radius is positive")
        }
        Graphic::Text { text, size_nm, .. } if text.is_empty() || *size_nm <= 0 => {
            Some("a text has characters and a positive size")
        }
        _ => None,
    }
}

/// `s` as one step of a JSON pointer (`~` and `/` escaped).
pub fn pointer_escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc;
    use serde_json::json;

    /// A two-pin resistor, drawn as a box.
    pub fn resistor() -> Symbol {
        let v = json!({
            "kind": KIND, "format": 1, "name": "R", "units": 1,
            "pins": {
                "1": {"name": "~", "type": "passive", "at": [0, 3_810_000], "side": "up", "length_nm": 1_270_000},
                "2": {"name": "~", "type": "passive", "at": [0, -3_810_000], "side": "down", "length_nm": 1_270_000}
            },
            "graphics": {
                "g1": {"type": "rect", "start": [-1_016_000, 2_540_000], "end": [1_016_000, -2_540_000]}
            }
        });
        doc::from_value(v, "library/symbols/r.json", KIND, FORMAT).unwrap()
    }

    #[test]
    fn symbols_read_and_write_canonically() {
        let s = resistor();
        assert!(s.check().is_empty(), "{:?}", s.check());
        let text = doc::to_string(&s, KIND, FORMAT);
        let back: Symbol = doc::read(text.as_bytes(), "r.json", KIND, FORMAT).unwrap();
        assert_eq!(back, s);
        // Defaults are written out; absent options are not.
        assert!(text.contains("\"shape\": \"line\""));
        assert!(text.contains("\"fill\": \"none\""));
        assert!(!text.contains("\"unit\""));
        assert!(!text.contains("alternates"));
        assert_eq!(doc::to_string(&back, KIND, FORMAT), text);
    }

    #[test]
    fn shapes_are_refused_as_documents() {
        let bad = [
            json!({"name": "R", "units": 1, "colour": "red"}),
            json!({"name": "R", "units": 1, "pins": {"1": {"name": "", "type": "sideways", "at": [0, 0], "side": "up", "length_nm": 0}}}),
            json!({"name": "R", "units": 1, "graphics": {"g1": {"type": "rect", "start": [0.5, 0], "end": [1, 1]}}}),
            json!({"name": "R", "units": 1, "graphics": {"g1": {"type": "bezier", "points": [[0, 0], [1, 1]]}}}),
            json!({"name": "R", "units": 1, "graphics": {"g1": {"type": "rect", "start": [0, 0], "end": [1, 1], "width_nm": 5}}}),
        ];
        for mut v in bad {
            v["kind"] = json!(KIND);
            v["format"] = json!(1);
            let e = doc::from_value::<Symbol>(v.clone(), "s.json", KIND, FORMAT).unwrap_err();
            assert_eq!(e.code, Code::InvalidDocument, "{v}");
        }
    }

    fn codes(s: &Symbol) -> Vec<(Code, String)> {
        s.check()
            .into_iter()
            .map(|f| {
                (
                    f.error.code,
                    f.error.details["pointer"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn checks_find_units_pins_graphics_and_range() {
        let mut s = resistor();
        s.units = 2;
        s.pins.get_mut("1").unwrap().unit = Some(1);
        s.pins.get_mut("2").unwrap().unit = Some(3);
        assert_eq!(
            codes(&s),
            vec![
                (Code::LibraryUnitInvalid, "/pins/2/unit".to_owned()),
                (Code::LibraryUnitInvalid, "/units".to_owned()),
            ]
        );
        let mut s = resistor();
        let mut p = s.pins["1"].clone();
        p.length_nm = -1;
        p.alternates = vec![
            Alternate {
                name: "A".into(),
                ty: PinType::Input,
                shape: PinShape::Line,
            },
            Alternate {
                name: "A".into(),
                ty: PinType::Output,
                shape: PinShape::Line,
            },
        ];
        s.pins.insert("a/b".into(), p);
        s.pins.get_mut("2").unwrap().at = [0, METRE + 1];
        assert_eq!(
            codes(&s),
            vec![
                (Code::LibraryOutOfRange, "/pins/2".to_owned()),
                (Code::LibraryPinInvalid, "/pins/a~1b".to_owned()),
                (Code::LibraryPinInvalid, "/pins/a~1b/length_nm".to_owned()),
                (Code::LibraryPinInvalid, "/pins/a~1b/alternates".to_owned()),
            ]
        );
        let mut s = resistor();
        let g = |v| serde_json::from_value::<Graphic>(v).unwrap();
        let gid = |n: &str| Id::parse(n, "graphic").unwrap();
        s.graphics.insert(
            gid("g2"),
            g(json!({"type": "arc", "start": [0, 0], "mid": [1, 1], "end": [2, 2]})),
        );
        s.graphics.insert(
            gid("g3"),
            g(json!({"type": "circle", "center": [0, 0], "radius_nm": 0})),
        );
        s.graphics.insert(
            gid("g4"),
            g(json!({"type": "polyline", "points": [[0, 0]]})),
        );
        s.graphics.insert(
            gid("g5"),
            g(json!({"type": "text", "text": "", "at": [0, 0], "size_nm": 1})),
        );
        s.graphics.insert(
            gid("g6"),
            g(json!({"type": "rect", "start": [0, 0], "end": [0, 5]})),
        );
        s.graphics.insert(
            gid("g7"),
            g(json!({"type": "circle", "center": [METRE, 0], "radius_nm": 2 * METRE, "unit": 2})),
        );
        assert_eq!(
            codes(&s),
            vec![
                (Code::LibraryGraphicInvalid, "/graphics/g2".to_owned()),
                (Code::LibraryGraphicInvalid, "/graphics/g3".to_owned()),
                (Code::LibraryGraphicInvalid, "/graphics/g4".to_owned()),
                (Code::LibraryGraphicInvalid, "/graphics/g5".to_owned()),
                (Code::LibraryGraphicInvalid, "/graphics/g6".to_owned()),
                (Code::LibraryUnitInvalid, "/graphics/g7/unit".to_owned()),
                (Code::LibraryOutOfRange, "/graphics/g7".to_owned()),
            ]
        );
        assert_eq!(s.next_graphic().as_str(), "g8");
    }

    #[test]
    fn units_without_pins_are_one_fault_however_many() {
        let mut s = resistor();
        s.units = u32::MAX;
        s.pins.get_mut("1").unwrap().unit = Some(1);
        s.pins.get_mut("2").unwrap().unit = Some(3);
        let faults = s.check();
        assert_eq!(faults.len(), 1);
        assert_eq!(
            faults[0].error.message,
            format!(
                "{} units have no pin of their own, unit 2 first",
                u32::MAX - 2
            )
        );
    }

    #[test]
    fn a_drawing_with_no_pins_is_a_symbol() {
        let mut s = resistor();
        s.pins.clear();
        assert!(s.check().is_empty());
        s.units = 0;
        assert_eq!(
            codes(&s),
            vec![(Code::LibraryUnitInvalid, "/units".to_owned())]
        );
    }
}
