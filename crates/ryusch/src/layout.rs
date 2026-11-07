//! A block's layout: what `blocks/<id>/layout.json` holds, committed
//! like a lockfile. Every number is an integer, so a write and a read
//! give the same bits. Keys are the files' IDs: a part by its key
//! (`<component>`, `<component>:<unit>`, `instance:<id>`, `port:<id>`), a
//! net by its key (`<net>`, `rail:<id>`, `port:<id>`), a sheet by its ID.

use crate::geom::{Nm, Point, Rect, Side};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    /// Each sheet's name and place on the block's canvas.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sheets: BTreeMap<String, Sheet>,
    /// The sheets' regions, by ID (`r1`, `r2`, …): what each holds and
    /// where its frame is (format 3).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub regions: BTreeMap<String, Region>,
    /// The lasting instructions, by ID (`i1`, `i2`, … one past the
    /// largest).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub instructions: BTreeMap<String, Instruction>,
    /// Every part's place, by part key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placed: BTreeMap<String, Placed>,
    /// Each net drawn by labels or power ports: its labels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, Vec<Label>>,
    /// Each part's text, by field (`refdes`, `value`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub text: BTreeMap<String, BTreeMap<String, Point>>,
    /// Each net's wires, by sheet: runs of points.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<String, BTreeMap<String, Vec<Vec<Point>>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sheet {
    pub name: String,
    /// Its lower left corner on the block's canvas.
    pub at: Point,
    /// Its paper (`a4`, `ansi_b`), where it isn't the project's (format 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// Its paper's orientation, where it isn't the project's (format 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Orientation>,
    /// Its paper pinned by a `paper` instruction: never fitted again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

impl Sheet {
    /// A sheet on the project's paper.
    pub fn new(name: String, at: Point) -> Sheet {
        Sheet {
            name,
            at,
            size: None,
            orientation: None,
            pinned: false,
        }
    }
}

/// Which way up a sheet's paper is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Landscape,
    Portrait,
}

/// A titled rectangle on a sheet holding one function's parts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub sheet: String,
    /// Its origin on the sheet: set when it is made, moved only when the
    /// region moves whole. Its parts' places are relative to it in the
    /// file.
    pub at: Point,
    /// Its frame, relative to `at`: what it holds (parts, their text and
    /// labels), grown by its margin and title band.
    pub frame: Rect,
    pub title: String,
    /// Named by the agent: never proposed again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub named: bool,
    /// Its parts' keys, in key order.
    pub parts: Vec<String>,
}

impl Region {
    /// Its frame on the sheet.
    pub fn frame_on_sheet(&self) -> Rect {
        self.frame.shift(self.at)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placed {
    pub sheet: String,
    /// The part's origin, on the sheet.
    pub at: Point,
    /// Degrees counterclockwise: 0, 90, 180 or 270.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: u16,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mirror: bool,
    /// Put here by a one-off move: kept where it is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

/// How a label draws its net.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    /// A net label: the net's name in a flag.
    Net,
    /// A supply: a bar and the rail's name, pointing up.
    Power,
    /// Ground: the ground symbol, pointing down.
    Ground,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    pub sheet: String,
    /// Where it connects: a pin's end, or the end of its stub.
    pub at: Point,
    /// Which way it points from there.
    pub side: Side,
    pub kind: LabelKind,
    /// The pin it serves: `<part key>/<pin number>`.
    pub on: String,
    /// What it says: its net's name when it was placed. A rename is told
    /// by it, so labels it doesn't touch keep their places first.
    pub text: String,
}

/// Which way an instruction lines parts up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Row,
    Column,
}

/// How a net is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetStyle {
    Wire,
    Label,
    /// Power ports (a rail's).
    Port,
}

/// The connectivity patterns drawn in canonical shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdiomKind {
    /// A capacitor between a pin's supply and ground.
    Decoupling,
    PullUp,
    PullDown,
    /// Two resistors in series between rails or a signal and a rail, their
    /// middle read by a pin.
    Divider,
    /// A crystal or resonator with its load capacitors.
    Crystal,
    /// An LED and its resistor.
    LedResistor,
    RcFilter,
    LcFilter,
    /// A resistor from an amplifier's output back to its inverting input.
    Feedback,
    /// A resistor in series with a driving pin.
    SeriesTermination,
    /// A connector on its sheet's edge.
    Connector,
    /// A buck converter's power stage: the inductor across the page from
    /// the switch pin, the catch diode standing under the switch node, the
    /// output capacitors standing on the inductor's far end.
    Buck,
    /// An op-amp stage: its feedback elements across the page above the
    /// amplifier, its input element across the page into the inverting
    /// input.
    OpampStage,
}

impl IdiomKind {
    pub const ALL: [IdiomKind; 13] = [
        IdiomKind::Decoupling,
        IdiomKind::PullUp,
        IdiomKind::PullDown,
        IdiomKind::Divider,
        IdiomKind::Crystal,
        IdiomKind::LedResistor,
        IdiomKind::RcFilter,
        IdiomKind::LcFilter,
        IdiomKind::Feedback,
        IdiomKind::SeriesTermination,
        IdiomKind::Connector,
        IdiomKind::Buck,
        IdiomKind::OpampStage,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            IdiomKind::Decoupling => "decoupling",
            IdiomKind::PullUp => "pull_up",
            IdiomKind::PullDown => "pull_down",
            IdiomKind::Divider => "divider",
            IdiomKind::Crystal => "crystal",
            IdiomKind::LedResistor => "led_resistor",
            IdiomKind::RcFilter => "rc_filter",
            IdiomKind::LcFilter => "lc_filter",
            IdiomKind::Feedback => "feedback",
            IdiomKind::SeriesTermination => "series_termination",
            IdiomKind::Connector => "connector",
            IdiomKind::Buck => "buck",
            IdiomKind::OpampStage => "opamp_stage",
        }
    }
}

/// An idiom an instruction asks for, or `none`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdiomChoice {
    None,
    #[serde(untagged)]
    Kind(IdiomKind),
}

/// What an agent says about the drawing. Subjects are part keys (a
/// component's key names all its units); nets are net keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Instruction {
    /// Put the parts on the sheet so named, made if new.
    Sheet { parts: Vec<String>, sheet: String },
    /// The parts to that side of `of`.
    Beside {
        parts: Vec<String>,
        of: String,
        side: Side,
    },
    /// The parts close to a pin (`<part key>/<pin>`) or a part.
    Near { parts: Vec<String>, to: String },
    /// The parts lined up: by their bodies, or where a subject is a pin
    /// (`<part key>/<number>`), by those pins' ends.
    Align { parts: Vec<String>, axis: Axis },
    /// The parts kept together as one cluster.
    Group { parts: Vec<String> },
    /// The parts on that edge of their sheet.
    Edge { parts: Vec<String>, side: Side },
    /// The parts' rotation and mirror.
    Orient {
        parts: Vec<String>,
        rotation: u16,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        mirror: bool,
    },
    /// How a net is drawn.
    Net { net: String, style: NetStyle },
    /// The parts drawn as an idiom, or never as one.
    Idiom {
        parts: Vec<String>,
        kind: IdiomChoice,
    },
    /// The parts may run against the default flow.
    Flow { parts: Vec<String> },
    /// The parts nudged by whole grid steps from where they stand: a move
    /// only (lasting, it would nudge them again on every layout).
    Shift {
        parts: Vec<String>,
        side: Side,
        steps: u16,
    },
    /// The parts drawn as one region so titled; the agent's, never
    /// proposed again.
    Region { title: String, parts: Vec<String> },
    /// The parts in no region the agent named: proposed again.
    RegionNone { parts: Vec<String> },
    /// A sheet's paper, pinned: never fitted again.
    Paper {
        sheet: String,
        size: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        orientation: Option<Orientation>,
    },
}

impl Instruction {
    /// The parts it names (a net's instruction names none).
    pub fn parts(&self) -> &[String] {
        match self {
            Instruction::Sheet { parts, .. }
            | Instruction::Beside { parts, .. }
            | Instruction::Near { parts, .. }
            | Instruction::Align { parts, .. }
            | Instruction::Group { parts }
            | Instruction::Edge { parts, .. }
            | Instruction::Orient { parts, .. }
            | Instruction::Idiom { parts, .. }
            | Instruction::Flow { parts }
            | Instruction::Shift { parts, .. }
            | Instruction::Region { parts, .. }
            | Instruction::RegionNone { parts } => parts,
            Instruction::Net { .. } | Instruction::Paper { .. } => &[],
        }
    }

    /// Everything it names: its parts, and the part, pin or net it is
    /// relative to.
    pub fn subjects(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.parts().iter().map(String::as_str).collect();
        match self {
            Instruction::Beside { of, .. } => out.push(of),
            Instruction::Near { to, .. } => out.push(to),
            Instruction::Net { net, .. } => out.push(net),
            Instruction::Paper { sheet, .. } => out.push(sheet),
            _ => {}
        }
        out
    }

    /// Its verb, as `place` takes it.
    pub fn verb(&self) -> &'static str {
        match self {
            Instruction::Sheet { .. } => "sheet",
            Instruction::Beside { .. } => "beside",
            Instruction::Near { .. } => "near",
            Instruction::Align { .. } => "align",
            Instruction::Group { .. } => "group",
            Instruction::Edge { .. } => "edge",
            Instruction::Orient { .. } => "orient",
            Instruction::Net { .. } => "net",
            Instruction::Idiom { .. } => "idiom",
            Instruction::Flow { .. } => "flow",
            Instruction::Shift { .. } => "shift",
            Instruction::Region { .. } | Instruction::RegionNone { .. } => "region",
            Instruction::Paper { .. } => "paper",
        }
    }
}

impl Layout {
    /// Whether a lasting instruction names a pin (a pin-level `align`),
    /// which an older reader would take for a part that isn't there.
    pub fn names_pins(&self) -> bool {
        self.instructions
            .values()
            .any(|i| matches!(i, Instruction::Align { parts, .. } if parts.iter().any(|p| p.contains('/'))))
    }

    /// The lowest layout format that holds it: 3 for regions, a sheet's
    /// own paper or an instruction format 3 brings; 2 for a lasting
    /// pin-level `align`; else 1.
    pub fn format(&self) -> u64 {
        let three = !self.regions.is_empty()
            || self
                .sheets
                .values()
                .any(|s| s.size.is_some() || s.orientation.is_some() || s.pinned)
            || self.instructions.values().any(|i| {
                matches!(
                    i,
                    Instruction::Region { .. }
                        | Instruction::RegionNone { .. }
                        | Instruction::Paper { .. }
                )
            });
        if three {
            3
        } else if self.names_pins() {
            2
        } else {
            1
        }
    }

    /// The region each part is in, by part key.
    pub fn region_of(&self) -> BTreeMap<&str, &str> {
        self.regions
            .iter()
            .flat_map(|(id, r)| r.parts.iter().map(move |k| (k.as_str(), id.as_str())))
            .collect()
    }

    /// The layout as its file holds it: each part in a region placed
    /// relative to the region's origin.
    pub fn relative(&self) -> Layout {
        self.shifted(-1)
    }

    /// The layout in sheet coordinates, from its file's form.
    pub fn absolute(&self) -> Layout {
        self.shifted(1)
    }

    fn shifted(&self, sign: Nm) -> Layout {
        let mut out = self.clone();
        for r in self.regions.values() {
            for k in &r.parts {
                if let Some(p) = out.placed.get_mut(k) {
                    p.at = [p.at[0] + sign * r.at[0], p.at[1] + sign * r.at[1]];
                }
            }
        }
        out
    }
}

/// The next instruction ID: one past the largest `i<n>`.
pub fn next_id(ids: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let n = ids
        .into_iter()
        .filter_map(|id| id.as_ref().strip_prefix('i')?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("i{}", n + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn instructions_read_and_write_by_op() {
        let i: Instruction = serde_json::from_value(
            json!({"op": "beside", "parts": ["c1"], "of": "u1", "side": "left"}),
        )
        .unwrap();
        assert_eq!(i.subjects(), ["c1", "u1"]);
        let idiom: Instruction =
            serde_json::from_value(json!({"op": "idiom", "parts": ["r1"], "kind": "none"}))
                .unwrap();
        assert_eq!(
            idiom,
            Instruction::Idiom {
                parts: vec!["r1".into()],
                kind: IdiomChoice::None
            }
        );
        let pull: Instruction =
            serde_json::from_value(json!({"op": "idiom", "parts": ["r1"], "kind": "pull_up"}))
                .unwrap();
        assert_eq!(
            serde_json::to_value(&pull).unwrap(),
            json!({"op": "idiom", "parts": ["r1"], "kind": "pull_up"})
        );
        assert!(
            serde_json::from_value::<Instruction>(json!({"op": "sheet", "parts": [], "x": 1}))
                .is_err()
        );
        assert_eq!(next_id(["i1", "i9", "x"]), "i10");
        assert_eq!(next_id(Vec::<String>::new()), "i1");
    }

    #[test]
    fn regions_place_their_parts_relative_to_their_origin() {
        let mut l = Layout::default();
        l.sheets
            .insert("s1".into(), Sheet::new("Sheet 1".into(), [0, 0]));
        let placed = |at| Placed {
            sheet: "s1".into(),
            at,
            rotation: 0,
            mirror: false,
            held: false,
        };
        l.placed.insert("u1".into(), placed([30, 40]));
        l.placed.insert("port:a".into(), placed([5, 5]));
        l.regions.insert(
            "r1".into(),
            Region {
                sheet: "s1".into(),
                at: [10, 20],
                frame: crate::geom::Rect::new([0, 0], [50, 60]),
                title: "U1 LAN8710A".into(),
                named: false,
                parts: vec!["u1".into()],
            },
        );
        let rel = l.relative();
        assert_eq!(rel.placed["u1"].at, [20, 20]);
        assert_eq!(rel.placed["port:a"].at, [5, 5], "a port is in no region");
        assert_eq!(rel.absolute(), l);
        assert_eq!(l.format(), 3);
        let mut plain = l.clone();
        plain.regions.clear();
        assert_eq!(plain.format(), 1);
        let paper: Instruction = serde_json::from_value(
            json!({"op": "paper", "sheet": "s2", "size": "a3", "orientation": "portrait"}),
        )
        .unwrap();
        assert_eq!(paper.subjects(), ["s2"]);
        plain.instructions.insert("i1".into(), paper);
        assert_eq!(plain.format(), 3);
        let none: Instruction =
            serde_json::from_value(json!({"op": "region_none", "parts": ["r1"]})).unwrap();
        assert_eq!(none.verb(), "region");
        let mut sized = Layout::default();
        let mut s = Sheet::new("Sheet 1".into(), [0, 0]);
        s.size = Some("a4".into());
        sized.sheets.insert("s1".into(), s);
        assert_eq!(sized.format(), 3);
        assert_eq!(
            serde_json::to_value(&sized.sheets["s1"]).unwrap(),
            json!({"name": "Sheet 1", "at": [0, 0], "size": "a4"})
        );
    }

    #[test]
    fn layouts_leave_out_what_is_empty() {
        let mut l = Layout::default();
        assert_eq!(serde_json::to_value(&l).unwrap(), json!({}));
        l.placed.insert(
            "r1".into(),
            Placed {
                sheet: "s1".into(),
                at: [0, 0],
                rotation: 0,
                mirror: false,
                held: false,
            },
        );
        assert_eq!(
            serde_json::to_value(&l).unwrap(),
            json!({"placed": {"r1": {"sheet": "s1", "at": [0, 0]}}})
        );
    }
}
