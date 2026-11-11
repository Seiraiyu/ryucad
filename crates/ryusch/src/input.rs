//! What the engine reads: a block as data. ryueda builds it from a
//! project; nothing here knows of files.

use crate::geom::{Nm, Point, Side};
use crate::layout::{Instruction, Layout};
use std::collections::BTreeMap;

/// A block's bus: nets the grammar may draw as one bundle (plan 2.C).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bus {
    /// `bus:<id>`: what a layout keys it by.
    pub key: String,
    pub name: String,
    /// Its member nets, by index into the block's nets, in its file's
    /// order.
    pub members: Vec<usize>,
}

/// One block to lay out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    /// What sheets' text calls it.
    pub name: String,
    /// The project's paper's area (the paper less what the PDF's border
    /// and title band take): the largest a sheet takes, and every sheet's when
    /// `papers` is empty.
    pub sheet: [Nm; 2],
    /// The papers a sheet may take, smallest first, the project's last;
    /// empty when paper fit is off (every sheet at `sheet`).
    pub papers: Vec<Paper>,
    pub parts: Vec<Part>,
    pub nets: Vec<Net>,
    /// The block's buses (plan 2.C).
    pub buses: Vec<Bus>,
    /// The lasting instructions, by ID.
    pub instructions: BTreeMap<String, Instruction>,
    /// Each note instruction's text as drawn, by its ID (plan 2.C:
    /// properties filled by ryueda); a note with no entry draws its
    /// instruction's text.
    pub notes: BTreeMap<String, String>,
    /// One-off moves: applied once, their results held as placements.
    pub moves: Vec<Instruction>,
    /// The layout the files hold, kept where it can be.
    pub previous: Option<Layout>,
    /// What to lay out from nothing, keeping the instructions.
    pub relayout: Relayout,
    /// Where parts a released instruction moved stood before it, by key:
    /// put back there where still free, before anything else is placed.
    pub restore: BTreeMap<String, crate::layout::Placed>,
}

/// A paper a sheet may take: its size's name, its orientation and its
/// drawing area (the paper less the PDF's border and title band).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paper {
    pub size: String,
    pub orientation: crate::layout::Orientation,
    pub area: [Nm; 2],
}

/// What a layout starts from nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Relayout {
    /// Nothing: keep what the previous layout holds.
    #[default]
    None,
    /// The whole block.
    Block,
    /// One sheet, by ID.
    Sheet(String),
    /// These parts (by key or subject: an instance's units, a stack's
    /// layers), placed again into free space near their neighbours, the
    /// rest kept: what a released instruction put somewhere.
    Parts(Vec<String>),
}

/// Something placed on a sheet: a symbol's unit, a child block's symbol,
/// or a port of the block.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// `<component>`, `<component>:<unit>`, `instance:<id>`, `port:<id>`.
    pub key: String,
    pub kind: PartKind,
    /// The first line of text beside it: a designator, a block's name.
    pub refdes: String,
    /// The second: a value ("" for none).
    pub value: String,
    /// What its component does, from its `function` property ("" for
    /// none): the title of the region it leads.
    pub function: String,
    /// How its designator and value are set (plan 2.E).
    pub text: PartText,
    /// The body, in the part's own frame.
    pub graphics: Vec<Graphic>,
    pub pins: Vec<Pin>,
}

/// How a part's designator and value are set (plan 2.E: the style's
/// `text_size` and `emphasis`, read as input).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartText {
    pub size: Nm,
    /// Its value in SemiBold too.
    pub emphasis: bool,
}

impl Default for PartText {
    fn default() -> PartText {
        PartText {
            size: crate::font::SIZE,
            emphasis: false,
        }
    }
}

impl Part {
    /// The font of its text field `field`: its designator SemiBold, its
    /// value SemiBold when emphasised, both at its text size.
    pub fn font(&self, field: &str) -> crate::font::Font {
        use crate::font::Weight;
        crate::font::Font {
            size: self.text.size,
            weight: if field == "refdes" || self.text.emphasis {
                Weight::SemiBold
            } else {
                Weight::Regular
            },
        }
    }

    /// Its designator and value as lines, empty or not.
    pub fn fields(&self) -> [(&'static str, crate::text::Line<'_>); 2] {
        [
            (
                "refdes",
                crate::text::Line {
                    text: &self.refdes,
                    font: self.font("refdes"),
                },
            ),
            (
                "value",
                crate::text::Line {
                    text: &self.value,
                    font: self.font("value"),
                },
            ),
        ]
    }

    /// The lines of text beside it: its designator and value, those it
    /// has; a port has none (its name is in its flag).
    pub fn lines(&self) -> Vec<(&'static str, crate::text::Line<'_>)> {
        if matches!(self.kind, PartKind::Port { .. }) {
            return Vec::new();
        }
        self.fields()
            .into_iter()
            .filter(|(_, l)| !l.text.is_empty())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartKind {
    /// A component's unit; `prefix` is its designator's letters.
    Symbol { prefix: String },
    /// A child block, drawn as a box with its ports as pins.
    Block,
    /// A port of this block: where a signal enters or leaves it.
    Port { flow: Flow },
}

/// Which way a port carries what it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Flow {
    In,
    Out,
    Bidirectional,
    Power,
    Passive,
}

/// KiCad's electrical pin types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinKind {
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

impl PinKind {
    /// Whether it drives the signal on its net.
    pub fn drives(self) -> bool {
        matches!(
            self,
            PinKind::Output | PinKind::TriState | PinKind::OpenCollector | PinKind::OpenEmitter
        )
    }

    /// Whether it takes the signal on its net.
    pub fn takes(self) -> bool {
        matches!(self, PinKind::Input)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub number: String,
    /// Drawn inside the body; "" for none.
    pub name: String,
    /// The connection point, in the part's frame.
    pub at: Point,
    /// Which way it points from the body to its end.
    pub side: Side,
    pub length: Nm,
    pub kind: PinKind,
    /// The net it is on, by index into the block's nets.
    pub net: Option<usize>,
    /// Marked not connected.
    pub nc: bool,
    /// Whether its number is drawn (not on two-pin parts).
    pub show_number: bool,
    /// On a stacked block symbol, the nets the layers behind the front one
    /// put it on, in layer order (`net` is the front layer's); empty on
    /// any other pin.
    pub layers: Vec<Option<usize>>,
}

impl Pin {
    /// Whether the layers of a stack put it on different nets: it is then
    /// drawn with one label naming each layer's net.
    pub fn stacked(&self) -> bool {
        self.layers.iter().any(|n| *n != self.net)
    }

    /// The net it is drawn on: none if marked not connected; on a stacked
    /// pin the first layer's net there is (its label names the rest).
    pub fn drawn_net(&self) -> Option<usize> {
        if self.nc {
            return None;
        }
        self.net
            .or_else(|| self.layers.iter().flatten().next().copied())
    }

    /// Every net it is on, the front layer's first, each once.
    pub fn nets(&self) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        if self.nc {
            return out;
        }
        for n in std::iter::once(&self.net).chain(&self.layers).flatten() {
            if !out.contains(n) {
                out.push(*n);
            }
        }
        out
    }
}

/// A net of the block, a rail it uses, or a port's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Net {
    /// `<net id>`, `rail:<id>` or `port:<id>`: what a layout keys it by.
    pub key: String,
    /// What its labels say.
    pub name: String,
    pub kind: NetKind,
    /// What the sheet writes beside its power ports or ground, in the
    /// small size: a rail's voltage where its name doesn't say it; ""
    /// for none (plan 2.C; 2.E's style decides what else is shown).
    pub intent: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetKind {
    Signal,
    /// A rail drawn by power ports pointing up.
    Supply,
    /// A rail drawn by ground symbols pointing down.
    Ground,
}

/// What a closed shape is filled with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fill {
    #[default]
    None,
    Body,
    Outline,
    /// A block port's flag: the ports' light tint (plan 2.C).
    Port,
}

/// A piece of a part's body, in its own frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Graphic {
    Rect {
        start: Point,
        end: Point,
        fill: Fill,
    },
    Polyline {
        points: Vec<Point>,
        fill: Fill,
    },
    /// Through three points.
    Arc {
        start: Point,
        mid: Point,
        end: Point,
    },
    Circle {
        center: Point,
        radius: Nm,
        fill: Fill,
    },
    /// A cubic Bézier.
    Bezier {
        points: [Point; 4],
    },
    Text {
        text: String,
        at: Point,
        size: Nm,
    },
}

impl Block {
    /// The paper a sheet has: its own where the series has it, else the
    /// project's (the series' last), else none (fit off).
    pub fn paper_of(&self, s: &crate::layout::Sheet) -> Option<&Paper> {
        let own = s.size.as_deref().and_then(|size| {
            self.papers
                .iter()
                .find(|p| p.size == size && s.orientation.is_none_or(|o| o == p.orientation))
        });
        own.or(self.papers.last())
    }

    /// A sheet's drawing area.
    pub fn area_of(&self, s: &crate::layout::Sheet) -> [Nm; 2] {
        self.paper_of(s).map_or(self.sheet, |p| p.area)
    }

    /// A sheet's drawing area, by its ID in `layout` (the project's paper
    /// for a sheet it doesn't have).
    pub fn area_in(&self, layout: &Layout, sid: &str) -> [Nm; 2] {
        layout
            .sheets
            .get(sid)
            .map_or(self.sheet, |s| self.area_of(s))
    }

    /// The next paper up the series from a sheet's, if any.
    pub fn grown(&self, s: &crate::layout::Sheet) -> Option<&Paper> {
        let now = self.paper_of(s)?;
        let i = self.papers.iter().position(|p| p == now)?;
        self.papers.get(i + 1)
    }

    /// The part keyed `key`.
    pub fn part(&self, key: &str) -> Option<usize> {
        self.parts.iter().position(|p| p.key == key)
    }

    /// The net keyed `key`.
    pub fn net(&self, key: &str) -> Option<usize> {
        self.nets.iter().position(|n| n.key == key)
    }

    /// What note `id` says as drawn: its entry in `notes`, else its
    /// instruction's text.
    pub fn note_text(&self, id: &str) -> Option<&str> {
        self.notes
            .get(id)
            .map(String::as_str)
            .or_else(|| match self.instructions.get(id) {
                Some(Instruction::Note { text, .. }) => Some(text.as_str()),
                _ => None,
            })
    }

    /// The bus keyed `key`.
    pub fn bus(&self, key: &str) -> Option<usize> {
        self.buses.iter().position(|b| b.key == key)
    }

    /// What a label on pin `k` of part `p` says: its net's name, or on a
    /// stacked pin each layer's net's name in layer order, each once.
    pub fn label_text(&self, p: usize, k: usize) -> String {
        let pin = &self.parts[p].pins[k];
        let names: Vec<&str> = pin
            .nets()
            .into_iter()
            .map(|n| self.nets[n].name.as_str())
            .collect();
        let mut seen: Vec<&str> = Vec::new();
        for n in names {
            if !seen.contains(&n) {
                seen.push(n);
            }
        }
        seen.join(", ")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_part_sets_its_designator_in_semibold_and_its_value_when_emphasised() {
        use super::PartText;
        use crate::font::{Font, Weight};
        let mut p = crate::testing::resistor("r1", "R1", "10k", [Some(0), Some(1)]);
        assert_eq!(p.font("refdes"), Font::DESIGNATOR);
        assert_eq!(p.font("value"), Font::REGULAR);
        p.text = PartText {
            size: 1_778_000,
            emphasis: true,
        };
        assert_eq!(p.font("value").weight, Weight::SemiBold);
        assert_eq!(p.font("refdes").size, 1_778_000);
        let names: Vec<&str> = p.lines().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["refdes", "value"]);
    }

    use super::*;
    use crate::layout::{Orientation, Sheet};

    #[test]
    fn a_sheet_takes_its_own_papers_area() {
        let a4 = Paper {
            size: "a4".into(),
            orientation: Orientation::Landscape,
            area: [277_000_000, 190_000_000],
        };
        let a3 = Paper {
            size: "a3".into(),
            orientation: Orientation::Landscape,
            area: [400_000_000, 277_000_000],
        };
        let block = Block {
            sheet: a3.area,
            papers: vec![a4.clone(), a3.clone()],
            ..Block::default()
        };
        let mut s = Sheet::new("Sheet 1".into(), [0, 0]);
        assert_eq!(block.area_of(&s), a3.area, "the project's paper by default");
        s.size = Some("a4".into());
        assert_eq!(block.area_of(&s), a4.area);
        assert_eq!(block.paper_of(&s), Some(&a4));
        s.size = Some("a0".into());
        assert_eq!(
            block.area_of(&s),
            a3.area,
            "a size not in the series reads as the project's"
        );
        assert_eq!(
            block.grown(&Sheet::new("x".into(), [0, 0])),
            None,
            "the largest grows no further"
        );
    }

    #[test]
    fn a_bus_is_found_by_its_key_and_lists_its_members() {
        let block = Block {
            buses: vec![Bus {
                key: "bus:b1".into(),
                name: "D".into(),
                members: vec![0, 1],
            }],
            ..Block::default()
        };
        assert_eq!(block.bus("bus:b1"), Some(0));
        assert_eq!(block.bus("b1"), None);
        assert_eq!(block.buses[0].members, vec![0, 1]);
    }
}
