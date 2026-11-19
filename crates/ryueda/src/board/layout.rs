//! A board's copper as `ryu-copper` takes it (design 732): two copper
//! layers; pads, tracks and vias as items claiming their nets; every
//! drill as a hole; one netlist point per pad with a net, so the copper
//! is compared with the design's own netlist.

use super::shapes::{BoardShapes, HoleOf};
use ryu_board::geom::Point;
use ryu_board::model::Side;
use ryu_copper::layout::{Hole, HoleShape, Item, Layer, Layout, NetPoint, PointSide};
use std::collections::BTreeMap;

/// What the copper engine keys a board's items and holes by. Pads by
/// occurrence and pad name, so two occurrences sharing a designator never
/// collide.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    Pad { occ: String, pad: String, layer: u8 },
    Track(String),
    Via(String, u8),
    PadDrill { occ: String, pad: String },
    ViaDrill(String),
    Hole(String),
}

/// A board's layout and the names its keys print by.
#[derive(Clone, Debug)]
pub struct Built {
    pub board: String,
    pub layout: Layout<Key>,
    /// Each pad's reference (`<board>:<refdes>/<pad>`) by occurrence and
    /// pad name.
    pub pads: BTreeMap<(String, String), String>,
    /// The copper layers' names, by index (`copper.top`, `copper.in1`,
    /// …, `copper.bottom`).
    pub layers: Vec<String>,
}

impl Built {
    /// The reference a key prints as: a pad's (its drill's too),
    /// `<board>:track/<id>`, `<board>:via/<id>`, `<board>:hole/<id>`.
    pub fn reference(&self, k: &Key) -> String {
        let b = &self.board;
        match k {
            Key::Pad { occ, pad, .. } | Key::PadDrill { occ, pad } => self
                .pads
                .get(&(occ.clone(), pad.clone()))
                .cloned()
                .unwrap_or_else(|| format!("{b}:{occ}/{pad}")),
            Key::Track(id) => format!("{b}:track/{id}"),
            Key::Via(id, _) | Key::ViaDrill(id) => format!("{b}:via/{id}"),
            Key::Hole(id) => format!("{b}:hole/{id}"),
        }
    }

    pub fn layer_name(&self, l: usize) -> &str {
        self.layers.get(l).map_or("copper", String::as_str)
    }
}

fn centre(h: &HoleShape) -> Point {
    match *h {
        HoleShape::Round { at, .. } => at,
        HoleShape::Slot { a, b, .. } => Point::new(a.x + (b.x - a.x) / 2, a.y + (b.y - a.y) / 2),
    }
}

/// The layout of board `board`'s shapes.
pub fn build(board: &str, s: &BoardShapes) -> Built {
    let n = s.n();
    let names = super::stackup::names(n);
    let layers = names
        .iter()
        .enumerate()
        .map(|(i, name)| Layer {
            name: name.clone(),
            side: if i == 0 {
                Side::Top
            } else if i + 1 == n {
                Side::Bottom
            } else {
                Side::Inner
            },
            copper: true,
        })
        .collect();
    let mut layout = Layout {
        layers,
        ..Layout::default()
    };
    let mut pads = BTreeMap::new();
    for pd in &s.pads {
        pads.insert(
            (pd.occurrence.clone(), pd.pad.clone()),
            pd.reference.clone(),
        );
        for layer in 0..n as u8 {
            if pd.on(usize::from(layer), n) {
                layout.items.insert(
                    Key::Pad {
                        occ: pd.occurrence.clone(),
                        pad: pd.pad.clone(),
                        layer,
                    },
                    Item {
                        layer: usize::from(layer),
                        shape: pd.copper.clone(),
                        net: pd.net.clone(),
                    },
                );
            }
        }
    }
    for t in &s.tracks {
        let Some(layer) = t.layer.index(n) else {
            continue;
        };
        layout.items.insert(
            Key::Track(t.id.to_string()),
            Item {
                layer,
                shape: t.shape.clone(),
                net: Some(t.net.clone()),
            },
        );
    }
    for v in &s.vias {
        for layer in v.from as u8..=v.to.min(n - 1) as u8 {
            layout.items.insert(
                Key::Via(v.id.to_string(), layer),
                Item {
                    layer: usize::from(layer),
                    shape: v.shape.clone(),
                    net: Some(v.net.clone()),
                },
            );
        }
    }
    let mut drills: BTreeMap<(String, String), HoleShape> = BTreeMap::new();
    for h in &s.holes {
        let key = match &h.of {
            HoleOf::Pad { occ, pad } => {
                if h.plated {
                    drills.insert((occ.clone(), pad.clone()), h.shape);
                }
                Key::PadDrill {
                    occ: occ.clone(),
                    pad: pad.clone(),
                }
            }
            HoleOf::Via(id) => Key::ViaDrill(id.to_string()),
            HoleOf::Hole(id) => Key::Hole(id.to_string()),
        };
        layout.holes.insert(
            key,
            Hole {
                shape: h.shape,
                span: (h.span.0, h.span.1.min(n - 1)),
                plated: h.plated,
                backdrilled: Vec::new(),
            },
        );
    }
    for pd in &s.pads {
        let Some(net) = &pd.net else { continue };
        let at_drill = drills.get(&(pd.occurrence.clone(), pd.pad.clone()));
        let (key, at, side) = match at_drill {
            Some(h) => (
                Key::PadDrill {
                    occ: pd.occurrence.clone(),
                    pad: pd.pad.clone(),
                },
                centre(h),
                None,
            ),
            None if pd.top => (
                Key::Pad {
                    occ: pd.occurrence.clone(),
                    pad: pd.pad.clone(),
                    layer: 0,
                },
                pd.at,
                Some(PointSide::Top),
            ),
            None if pd.bottom => (
                Key::Pad {
                    occ: pd.occurrence.clone(),
                    pad: pd.pad.clone(),
                    layer: (n - 1) as u8,
                },
                pd.at,
                Some(PointSide::Bottom),
            ),
            None => continue,
        };
        layout.points.push(NetPoint {
            net: net.clone(),
            at,
            side,
            key,
        });
    }
    Built {
        board: board.to_owned(),
        layout,
        pads,
        layers: names,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::file::CopperLayer;
    use crate::board::testing::{MM, UM, board};

    #[test]
    fn four_layers_blind_vias_and_inner_tracks() {
        let mut s = board(20 * MM, 10 * MM);
        s.through(
            "j1",
            "1",
            (10 * MM, 5 * MM),
            1700 * UM,
            1000 * UM,
            Some("a"),
        )
        .smd("r1", "1", (2 * MM, 5 * MM), (600 * UM, 600 * UM), Some("a"))
        .layers(4)
        .track(
            "t1",
            "a",
            CopperLayer::Inner(1),
            250 * UM,
            &[(2 * MM, 5 * MM), (10 * MM, 5 * MM)],
        )
        .blind("v1", "a", (2 * MM, 5 * MM), 300 * UM, 600 * UM, (0, 1));
        let b = build("b", &s);
        let l = &b.layout;
        assert_eq!(
            l.layers.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["copper.top", "copper.in1", "copper.in2", "copper.bottom"]
        );
        assert_eq!(l.layers[1].side, Side::Inner);
        assert_eq!(l.items[&Key::Track("t1".into())].layer, 1);
        // The through pad on all four, the blind via on its two, the surface pad on top.
        let count = |f: &dyn Fn(&Key) -> bool| l.items.keys().filter(|k| f(k)).count();
        assert_eq!(
            count(&|k| matches!(k, Key::Pad { occ, .. } if occ == "j1")),
            4
        );
        assert_eq!(count(&|k| matches!(k, Key::Via(..))), 2);
        assert_eq!(
            count(&|k| matches!(k, Key::Pad { occ, .. } if occ == "r1")),
            1
        );
        assert_eq!(l.holes[&Key::ViaDrill("v1".into())].span, (0, 1));
        assert_eq!(
            l.holes[&Key::PadDrill {
                occ: "j1".into(),
                pad: "1".into()
            }]
                .span,
            (0, 3)
        );
        assert_eq!(b.layer_name(2), "copper.in2");
        // The blind via and the inner track join the surface pad to the through pad.
        let c = crate::board::connect::connect("b", &s);
        assert!(c.report.opens.is_empty(), "{:?}", c.report.opens);
    }

    #[test]
    fn pads_tracks_vias_and_drills_are_keyed_and_pads_are_points() {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (2 * MM, 5 * MM), (600 * UM, 600 * UM), Some("a"))
            .smd("r1", "2", (4 * MM, 5 * MM), (600 * UM, 600 * UM), None)
            .through(
                "j1",
                "1",
                (10 * MM, 5 * MM),
                1700 * UM,
                1000 * UM,
                Some("a"),
            )
            .track(
                "t1",
                "a",
                CopperLayer::Top,
                250 * UM,
                &[(2 * MM, 5 * MM), (10 * MM, 5 * MM)],
            )
            .via("v1", "a", (15 * MM, 5 * MM), 300 * UM, 600 * UM)
            .hole("h1", (18 * MM, 8 * MM), 3200 * UM);
        let b = build("b", &s);
        let l = &b.layout;
        assert_eq!(l.layers.len(), 2);
        let keys: Vec<String> = l.items.keys().map(|k| b.reference(k)).collect();
        assert_eq!(
            keys,
            [
                "b:j1/1",
                "b:j1/1",
                "b:r1/1",
                "b:r1/2",
                "b:track/t1",
                "b:via/v1",
                "b:via/v1"
            ]
        );
        assert_eq!(l.items[&Key::Track("t1".into())].layer, 0);
        assert_eq!(l.items[&Key::Track("t1".into())].net.as_deref(), Some("a"));
        assert_eq!(
            l.items[&Key::Pad {
                occ: "r1".into(),
                pad: "2".into(),
                layer: 0
            }]
                .net,
            None
        );
        assert_eq!(l.holes.len(), 3);
        assert!(l.holes[&Key::ViaDrill("v1".into())].plated);
        assert!(!l.holes[&Key::Hole("h1".into())].plated);
        // Points: the surface pad from the top, the through pad through its drill.
        assert_eq!(l.points.len(), 2);
        let smd = l
            .points
            .iter()
            .find(|p| matches!(p.key, Key::Pad { .. }))
            .unwrap();
        assert_eq!(smd.side, Some(PointSide::Top));
        let th = l
            .points
            .iter()
            .find(|p| matches!(p.key, Key::PadDrill { .. }))
            .unwrap();
        assert_eq!(th.side, None);
        assert_eq!(b.reference(&th.key), "b:j1/1");
        assert_eq!(b.layer_name(1), "copper.bottom");
    }
}
