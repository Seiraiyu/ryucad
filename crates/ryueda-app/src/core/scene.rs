//! A sheet as the daemon's `scene` answers it (design 519, plan 3.B),
//! read into what the canvas draws and picks over. Coordinates become
//! millimetres (the answer's nanometres over 10⁶, the unit
//! `ryu_app::canvas` works in), y up; colours become egui's. Nothing here
//! lays out or paints a schematic: the daemon did.

use eframe::egui::Color32;
use serde_json::Value;
use std::collections::BTreeMap;

pub use ryu_app::canvas::P;

/// Nanometres a millimetre.
pub const NM: f64 = 1e6;

/// Where a text's `at` is along its width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// One drawn thing, in millimetres.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Polyline {
        points: Vec<P>,
        stroke: Color32,
        fill: Option<Color32>,
        width: f64,
        dash: bool,
    },
    Circle {
        center: P,
        radius: f64,
        stroke: Option<Color32>,
        fill: Option<Color32>,
        width: f64,
    },
    Arc {
        start: P,
        mid: P,
        end: P,
        stroke: Color32,
        width: f64,
    },
    Bezier {
        points: Vec<P>,
        stroke: Color32,
        width: f64,
    },
    /// A region's frame (dashed unless `frame` is false) and its title's tab.
    Frame {
        rect: [P; 2],
        title_tab: Option<[P; 2]>,
        stroke: Color32,
        paper: Color32,
        width: f64,
        frame: bool,
    },
    Bus {
        points: Vec<P>,
        stroke: Color32,
        width: f64,
    },
    /// `width` is the width SVG stretches the text to (`textLength`).
    Text {
        text: String,
        at: P,
        size: f64,
        anchor: Anchor,
        vertical: bool,
        colour: Color32,
        bold: bool,
        width: f64,
    },
}

/// An item with its owner's index and the PDF layer it is in, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub owner: Option<usize>,
    pub layer: Option<String>,
    pub shape: Shape,
}

/// An owner: what its items draw, by stable reference.
#[derive(Clone, Debug, PartialEq)]
pub struct Owner {
    pub reference: String,
    /// `component`, `net`, `label`, `port`, `rail`, `bus`, `region`,
    /// `note` or `instance`.
    pub kind: String,
    /// A label's net.
    pub net: Option<String>,
    pub boxed: Option<[P; 2]>,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub reference: String,
    pub owner: Option<usize>,
    /// Where wires meet it.
    pub at: P,
    /// Where it meets the body.
    pub base: P,
    pub number: String,
    pub name: String,
    pub kind: String,
    pub net: Option<String>,
    pub nc: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paper {
    pub size: P,
    pub rect: [P; 2],
    pub colour: Color32,
    pub canvas: Color32,
}

/// One sheet of a block, as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub block: String,
    pub design: Option<String>,
    /// The layer drawn (`stack_path`), if any, and the block's layers.
    pub layer: Option<String>,
    pub layers: Vec<String>,
    pub sheet: String,
    pub name: String,
    /// The block's sheets in canvas order, `(id, name)`.
    pub sheets: Vec<(String, String)>,
    pub theme: String,
    pub revision: String,
    pub revisions: BTreeMap<String, Value>,
    pub paper: Paper,
    pub score: Option<u64>,
    pub stale: bool,
    /// The block's faults, when it is drawn from its last good layout.
    pub invalid: Option<Value>,
    pub findings: Vec<Finding>,
    pub pins: Vec<Pin>,
    pub items: Vec<Item>,
    pub owners: Vec<Owner>,
    index: BTreeMap<String, usize>,
}

/// `#rrggbb`.
pub fn colour(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Color32::from_rgb(b(0)?, b(2)?, b(4)?))
}

fn mm(v: &Value) -> Option<f64> {
    Some(v.as_i64()? as f64 / NM)
}

fn point(v: &Value) -> Option<P> {
    Some([mm(&v[0])?, mm(&v[1])?])
}

fn points(v: &Value) -> Option<Vec<P>> {
    v.as_array()?.iter().map(point).collect()
}

fn rect(v: &Value) -> Option<[P; 2]> {
    Some([point(&v[0])?, point(&v[1])?])
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn shape(v: &Value) -> Option<Shape> {
    let stroke = || colour(v["stroke"].as_str()?);
    let fill = || v["fill"].as_str().and_then(colour);
    let width = || mm(&v["width"]);
    Some(match v["type"].as_str()? {
        "polyline" => Shape::Polyline {
            points: points(&v["points"])?,
            stroke: stroke()?,
            fill: fill(),
            width: width()?,
            dash: v["dash"].as_bool().unwrap_or(false),
        },
        "circle" => Shape::Circle {
            center: point(&v["center"])?,
            radius: mm(&v["radius"])?,
            stroke: v["stroke"].as_str().and_then(colour),
            fill: fill(),
            width: width()?,
        },
        "arc" => Shape::Arc {
            start: point(&v["start"])?,
            mid: point(&v["mid"])?,
            end: point(&v["end"])?,
            stroke: stroke()?,
            width: width()?,
        },
        "bezier" => Shape::Bezier {
            points: points(&v["points"])?,
            stroke: stroke()?,
            width: width()?,
        },
        "bus" => Shape::Bus {
            points: points(&v["points"])?,
            stroke: stroke()?,
            width: width()?,
        },
        "frame" => Shape::Frame {
            rect: rect(&v["rect"])?,
            title_tab: rect(&v["title_tab"]),
            stroke: stroke()?,
            paper: colour(v["paper"].as_str()?)?,
            width: width()?,
            frame: v["frame"].as_bool().unwrap_or(true),
        },
        "text" => Shape::Text {
            text: text(&v["text"])?,
            at: point(&v["at"])?,
            size: mm(&v["size"])?,
            anchor: match v["anchor"].as_str()? {
                "middle" => Anchor::Middle,
                "end" => Anchor::End,
                _ => Anchor::Start,
            },
            vertical: v["vertical"].as_bool().unwrap_or(false),
            colour: colour(v["colour"].as_str()?)?,
            bold: v["bold"].as_bool().unwrap_or(false),
            width: mm(&v["width"])?,
        },
        _ => return None,
    })
}

impl Scene {
    /// A `scene` answer; `None` if it isn't one.
    pub fn from_json(v: &Value) -> Option<Scene> {
        let mut items = Vec::new();
        let mut layer: Option<String> = None;
        for it in v["items"].as_array()? {
            match it["type"].as_str()? {
                "begin" => layer = text(&it["layer"]),
                "end" => layer = None,
                _ => items.push(Item {
                    owner: it["owner"].as_u64().map(|n| n as usize),
                    layer: layer.clone(),
                    shape: shape(it)?,
                }),
            }
        }
        let owners: Vec<Owner> = v["owners"]
            .as_array()?
            .iter()
            .map(|o| {
                Some(Owner {
                    reference: text(&o["ref"])?,
                    kind: text(&o["kind"])?,
                    net: text(&o["net"]),
                    boxed: rect(&o["box"]),
                    hash: text(&o["hash"])?,
                })
            })
            .collect::<Option<_>>()?;
        let index = owners
            .iter()
            .enumerate()
            .map(|(i, o)| (o.reference.clone(), i))
            .collect();
        let pins = v["pins"]
            .as_array()?
            .iter()
            .map(|p| {
                Some(Pin {
                    reference: text(&p["ref"])?,
                    owner: p["owner"].as_u64().map(|n| n as usize),
                    at: point(&p["at"])?,
                    base: point(&p["base"])?,
                    number: text(&p["number"])?,
                    name: text(&p["name"]).unwrap_or_default(),
                    kind: text(&p["type"]).unwrap_or_default(),
                    net: text(&p["net"]),
                    nc: p["nc"].as_bool().unwrap_or(false),
                })
            })
            .collect::<Option<_>>()?;
        let findings = v["findings"]
            .as_array()?
            .iter()
            .map(|f| Finding {
                code: text(&f["code"]).unwrap_or_default(),
                severity: text(&f["severity"]).unwrap_or_default(),
                message: text(&f["message"]).unwrap_or_default(),
                refs: ryu_app::people::strings(&f["refs"]),
            })
            .collect();
        let p = &v["paper"];
        Some(Scene {
            block: text(&v["block"])?,
            design: text(&v["design"]),
            layer: text(&v["layer"]["stack_path"]).filter(|s| !s.is_empty()),
            layers: v["layers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| text(&l["stack_path"]))
                .collect(),
            sheet: text(&v["sheet"])?,
            name: text(&v["name"]).unwrap_or_default(),
            sheets: v["sheets"]
                .as_array()?
                .iter()
                .filter_map(|s| Some((text(&s["sheet"])?, text(&s["name"]).unwrap_or_default())))
                .collect(),
            theme: text(&v["theme"]).unwrap_or_default(),
            revision: text(&v["revision"]).unwrap_or_default(),
            revisions: v["revisions"]
                .as_object()
                .map(|m| m.clone().into_iter().collect())
                .unwrap_or_default(),
            paper: Paper {
                size: point(&p["size"])?,
                rect: rect(&p["rect"])?,
                colour: colour(p["colour"].as_str()?)?,
                canvas: colour(p["canvas"].as_str()?)?,
            },
            score: v["score"].as_u64(),
            stale: v["stale"].as_bool().unwrap_or(false),
            invalid: v.get("invalid").cloned().filter(|x| !x.is_null()),
            findings,
            pins,
            items,
            owners,
            index,
        })
    }

    /// The owner with this reference, by index.
    pub fn owner(&self, reference: &str) -> Option<usize> {
        self.index.get(reference).copied()
    }

    /// Whether a `project.changed`'s `changed` keys touch what this scene
    /// is drawn from (design 519: re-ask only the sheets whose block's
    /// revision moved).
    pub fn moved_by(&self, changed: &Value) -> bool {
        changed
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|k| self.revisions.contains_key(k))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A small scene as `ryueda scene` answers it: a resistor with two
    /// pins, a net's wire and label, a region frame in the frames layer.
    pub(crate) fn small() -> Value {
        json!({
            "block": "top", "design": "d", "layer": null, "layers": [],
            "sheet": "s1", "name": "Power", "sheets": [{"sheet": "s1", "name": "Power"}, {"sheet": "s2", "name": "IO"}],
            "theme": "light", "revision": "r0", "revisions": {"block/top": "a", "layout/top": "b", "style": "c"},
            "paper": {"size": [297000000, 210000000], "rect": [[-10000000, -20000000], [287000000, 190000000]],
                      "colour": "#ffffff", "canvas": "#808080"},
            "score": 912, "findings": [{"code": "sch_crossing", "severity": "warning", "message": "m", "refs": ["top/r1"]}],
            "pins": [
                {"ref": "top/r1/1", "owner": 0, "at": [0, 0], "base": [2540000, 0], "number": "1", "name": "~", "type": "passive", "net": "top/sda", "nc": false},
                {"ref": "top/r1/2", "owner": 0, "at": [10160000, 0], "base": [7620000, 0], "number": "2", "name": "~", "type": "passive", "net": null, "nc": false}
            ],
            "items": [
                {"owner": null, "type": "polyline", "points": [[-10000000, -20000000], [287000000, -20000000]], "stroke": "#000000", "fill": null, "width": 250000, "dash": false},
                {"owner": 0, "type": "polyline", "points": [[2540000, -1000000], [7620000, -1000000], [7620000, 1000000], [2540000, 1000000], [2540000, -1000000]], "stroke": "#202020", "fill": "#ffffe0", "width": 250000, "dash": false},
                {"owner": 0, "type": "text", "text": "R1", "at": [5080000, 2000000], "size": 1270000, "anchor": "middle", "vertical": false, "colour": "#202020", "bold": false, "width": 1600000},
                {"owner": 1, "type": "polyline", "points": [[-5080000, 0], [0, 0]], "stroke": "#008000", "fill": null, "width": 250000, "dash": false},
                {"owner": 1, "type": "circle", "center": [0, 0], "radius": 500000, "stroke": null, "fill": "#008000", "width": 0},
                {"owner": 2, "type": "text", "text": "SDA", "at": [-5080000, 500000], "size": 1270000, "anchor": "end", "vertical": false, "colour": "#008000", "bold": false, "width": 2400000},
                {"owner": null, "type": "begin", "layer": "frames"},
                {"owner": 3, "type": "frame", "rect": [[-20000000, -10000000], [20000000, 10000000]], "title_tab": [[-20000000, 10000000], [-5000000, 13000000]], "stroke": "#a0a0a0", "paper": "#ffffff", "width": 250000, "frame": true},
                {"owner": null, "type": "end"},
                {"owner": 0, "type": "arc", "start": [0, 0], "mid": [1, 1], "end": [2, 0], "stroke": "#202020", "width": 1},
                {"owner": 0, "type": "bezier", "points": [[0, 0], [1, 2], [2, 0]], "stroke": "#202020", "width": 1}
            ],
            "owners": [
                {"ref": "top/r1", "kind": "component", "net": null, "box": [[2400000, -1200000], [7800000, 2700000]], "hash": "h-r1"},
                {"ref": "top/sda", "kind": "net", "net": null, "box": [[-5200000, -600000], [600000, 600000]], "hash": "h-sda"},
                {"ref": "top/label:sda@1", "kind": "label", "net": "top/sda", "box": [[-7500000, 400000], [-5000000, 1800000]], "hash": "h-l"},
                {"ref": "top/region:power", "kind": "region", "net": null, "box": [[-20100000, -10100000], [20100000, 13100000]], "hash": "h-reg"}
            ]
        })
    }

    #[test]
    fn a_scene_reads_in_millimetres_with_owners_layers_and_pins() {
        let s = Scene::from_json(&small()).unwrap();
        assert_eq!(
            (s.block.as_str(), s.sheet.as_str(), s.name.as_str()),
            ("top", "s1", "Power")
        );
        assert_eq!(s.sheets.len(), 2);
        assert_eq!(s.paper.size, [297.0, 210.0]);
        assert_eq!(s.paper.rect, [[-10.0, -20.0], [287.0, 190.0]]);
        assert_eq!(s.items.len(), 9, "begin and end become layers, not items");
        assert_eq!(s.items[0].owner, None);
        let Shape::Polyline {
            points,
            fill,
            width,
            ..
        } = &s.items[1].shape
        else {
            panic!()
        };
        assert_eq!(points[1], [7.62, -1.0]);
        assert!(fill.is_some());
        assert!((width - 0.25).abs() < 1e-12);
        assert_eq!(s.items[6].layer.as_deref(), Some("frames"));
        assert_eq!(s.items[7].layer, None);
        assert_eq!(s.owner("top/sda"), Some(1));
        assert_eq!(s.owners[2].net.as_deref(), Some("top/sda"));
        assert_eq!(s.owners[0].boxed.unwrap()[1], [7.8, 2.7]);
        assert_eq!(s.pins[0].net.as_deref(), Some("top/sda"));
        assert_eq!(s.findings[0].refs, ["top/r1"]);
        assert_eq!(s.score, Some(912));
        let Shape::Text { anchor, width, .. } = &s.items[2].shape else {
            panic!()
        };
        assert_eq!(*anchor, Anchor::Middle);
        assert!((width - 1.6).abs() < 1e-12);
    }

    #[test]
    fn a_change_moves_a_scene_only_through_its_revisions() {
        let s = Scene::from_json(&small()).unwrap();
        assert!(s.moved_by(&json!(["layout/top"])));
        assert!(s.moved_by(&json!(["style", "block/other"])));
        assert!(!s.moved_by(&json!(["block/other", "lib/self"])));
        assert!(!s.moved_by(&json!([])));
    }

    #[test]
    fn colours_parse_and_bad_scenes_are_refused() {
        assert_eq!(colour("#ff8000"), Some(Color32::from_rgb(255, 128, 0)));
        assert_eq!(colour("ff8000"), None);
        assert!(Scene::from_json(&json!({"items": "no"})).is_none());
    }
}
