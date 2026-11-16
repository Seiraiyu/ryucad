//! A board's copper: a `ryu_copper` layout built from the board step of a
//! `ryu_board` board, its connectivity, its netlist placed and compared,
//! and what `nets`, `measure`, `clearance`, merged renders and `check` say
//! of it.
//!
//! Items are the copper layers' features whose shapes build, keyed by
//! their board layer and index (so keys order a layer's items as drawn).
//! Holes are the drill layers' round pads and round-pen lines, over the
//! copper layers between their layer's span. Points are the CAD
//! netlist's, else the components' pins. What has no shape is left out
//! and listed.

use std::collections::BTreeMap;

use ryu_board::geom::Point;
use ryu_board::model::{AttrValue, Board, Context, FeatureKind, Side, Symbol, ToolKind};
use ryu_board::symbol::StandardSymbol;
use ryu_copper::layout::{Hole, HoleShape, Item, Layer, Layout, NetPoint, PointSide};

/// ODB++'s name for the net of what is on none.
pub const NO_NET: &str = "$NONE$";

/// What an item, hole or netlist point of a board's layout is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// A feature by its board layer and index: copper items and holes.
    Feature { layer: u32, index: u32 },
    /// A component's pin, a netlist point when the job has no CAD netlist.
    Pin { component: u32, pin: u32 },
    /// A point of the CAD netlist, by net and its place among its points.
    Cad { net: u32, point: u32 },
}

/// Where the netlist's points came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Cad,
    Pins,
    None,
}

/// A board step's layout, and what was left out of it.
pub struct Built {
    pub step: usize,
    /// The copper layers by board layer index, in stack order: layout
    /// layer `i` is board layer `copper[i]`.
    pub copper: Vec<usize>,
    pub layout: Layout<Key>,
    /// Copper features whose shape failed, and drill features that are
    /// neither round pads nor round-pen lines, or span no copper.
    pub left_out: Vec<Key>,
    /// Holes with no `.drill` attribute, plated or not by their layer's
    /// tools.
    pub plating_from_tools: usize,
    pub source: Source,
}

fn point_side(s: Option<Side>) -> Option<PointSide> {
    match s {
        Some(Side::Top) => Some(PointSide::Top),
        Some(Side::Bottom) => Some(PointSide::Bottom),
        _ => None,
    }
}

/// Plated round holes at a back-drill's centre whose span it crosses lose
/// their plating over its span.
pub fn strip_backdrilled(holes: &mut BTreeMap<Key, Hole>, back: &[(Key, Point, (usize, usize))]) {
    for (bk, at, sp) in back {
        for (k, h) in holes.iter_mut() {
            if k == bk || !h.plated {
                continue;
            }
            if let HoleShape::Round { at: c, .. } = h.shape
                && c == *at
                && h.span.0 <= sp.1
                && sp.0 <= h.span.1
            {
                h.backdrilled.push(*sp);
            }
        }
    }
    for h in holes.values_mut() {
        h.backdrilled.sort_unstable();
        h.backdrilled.dedup();
    }
}

/// The layout of a board's board step.
pub fn layout(b: &Board) -> Built {
    let step = b.board_step;
    let s = &b.steps[step];
    let mut copper: Vec<usize> = (0..b.layers.len())
        .filter(|&i| b.layers[i].kind.is_copper() && b.layers[i].context == Context::Board)
        .collect();
    copper.sort_by_key(|&i| b.layers[i].row);
    b.build_shapes(step, &copper);
    let net_name = |n: Option<u32>| -> Option<String> {
        let name = &s.nets.get(n? as usize)?.name;
        (name != NO_NET).then(|| name.clone())
    };
    let mut layout = Layout {
        layers: copper
            .iter()
            .map(|&i| Layer {
                name: b.layers[i].name.clone(),
                side: b.layers[i].side.unwrap_or(Side::Inner),
                copper: true,
            })
            .collect(),
        ..Layout::default()
    };
    let mut left_out = Vec::new();
    for (li, &bl) in copper.iter().enumerate() {
        let features = s.layers[bl].features();
        for (index, shape) in b.layer_shapes(step, bl).into_iter().enumerate() {
            let key = Key::Feature {
                layer: bl as u32,
                index: index as u32,
            };
            match shape {
                Ok(shape) if !shape.parts.is_empty() => {
                    layout.items.insert(
                        key,
                        Item {
                            layer: li,
                            shape: shape.clone(),
                            net: net_name(features[index].net),
                        },
                    );
                }
                Ok(_) => {}
                Err(_) => left_out.push(key),
            }
        }
    }
    let mut plating_from_tools = 0;
    let mut back: Vec<(Key, Point, (usize, usize))> = Vec::new();
    for d in &b.drills {
        let bl = d.layer;
        let span = match b.layers[bl].span {
            Some(sp) => {
                let (lo, hi) = (sp.start.min(sp.end), sp.start.max(sp.end));
                let inside: Vec<usize> = (0..copper.len())
                    .filter(|&i| (lo..=hi).contains(&copper[i]))
                    .collect();
                inside.first().zip(inside.last()).map(|(a, z)| (*a, *z))
            }
            None => (!copper.is_empty()).then(|| (0, copper.len() - 1)),
        };
        let features = s.layers[bl].features();
        let symbols = s.layers[bl].symbols();
        let round = |sym: u32| match symbols.get(sym as usize) {
            Some(Symbol::Standard(StandardSymbol::Round { d })) => Some(*d),
            _ => None,
        };
        let all_unplated =
            !d.tools.is_empty() && d.tools.iter().all(|t| t.kind == ToolKind::NonPlated);
        for (index, f) in features.iter().enumerate() {
            let key = Key::Feature {
                layer: bl as u32,
                index: index as u32,
            };
            let shape = match &f.kind {
                FeatureKind::Pad { symbol, place } => {
                    round(*symbol).map(|d| HoleShape::Round { at: place.at, d })
                }
                FeatureKind::Line { a, b, symbol } => {
                    round(*symbol).map(|d| HoleShape::Slot { a: *a, b: *b, d })
                }
                _ => None,
            };
            let (Some(shape), Some(span)) = (shape, span) else {
                left_out.push(key);
                continue;
            };
            let plated = if d.back_drill {
                false
            } else {
                match f.attributes.get(".drill") {
                    Some(AttrValue::Text(t)) if t == "non_plated" => false,
                    Some(AttrValue::Text(t)) if t == "plated" || t == "via" => true,
                    _ => {
                        plating_from_tools += 1;
                        !all_unplated
                    }
                }
            };
            if d.back_drill
                && let HoleShape::Round { at, .. } = shape
            {
                back.push((key, at, span));
            }
            layout.holes.insert(
                key,
                Hole {
                    shape,
                    span,
                    plated,
                    backdrilled: Vec::new(),
                },
            );
        }
    }
    strip_backdrilled(&mut layout.holes, &back);
    let mut source = Source::Cad;
    for (ni, n) in s.nets.iter().enumerate() {
        if n.name == NO_NET {
            continue;
        }
        for (pi, c) in n.cad.iter().enumerate() {
            layout.points.push(NetPoint {
                net: n.name.clone(),
                at: c.at,
                side: point_side(c.side),
                key: Key::Cad {
                    net: ni as u32,
                    point: pi as u32,
                },
            });
        }
    }
    if layout.points.is_empty() {
        source = Source::Pins;
        for (ci, c) in s.components.iter().enumerate() {
            for (pi, p) in c.pins.iter().enumerate() {
                if let Some(net) = net_name(p.net) {
                    layout.points.push(NetPoint {
                        net,
                        at: p.at,
                        side: point_side(Some(c.side)),
                        key: Key::Pin {
                            component: ci as u32,
                            pin: pi as u32,
                        },
                    });
                }
            }
        }
    }
    if layout.points.is_empty() {
        source = Source::None;
    }
    Built {
        step,
        copper,
        layout,
        left_out,
        plating_from_tools,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    pub(crate) fn read(name: &str) -> Board {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus")
            .join(name);
        crate::odb::read_path(&path, &Default::default()).unwrap()
    }

    #[test]
    fn minimal_clean_s_layout() {
        let b = read("minimal-clean.tgz");
        let built = layout(&b);
        let names: Vec<&str> = built
            .layout
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(names, ["top", "bottom"]);
        assert_eq!(built.source, Source::Cad);
        // The CAD netlist's 23 points less the ten of $NONE$.
        assert_eq!(built.layout.points.len(), 13);
        assert!(built.layout.points.iter().all(|p| p.net != NO_NET));
        let s = &b.steps[b.board_step];
        let features: usize = built
            .copper
            .iter()
            .map(|&l| s.layers[l].features().len())
            .sum();
        assert!(!built.layout.items.is_empty());
        assert!(built.layout.items.len() + built.left_out.len() <= features);
        assert!(built.layout.items.values().all(|it| it.layer < 2));
        // Keys order each layer's items as drawn.
        let keys: Vec<Key> = built.layout.items.keys().copied().collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        // Its plated drill layer runs from top to bottom.
        assert!(!built.layout.holes.is_empty());
        assert!(built.layout.holes.values().all(|h| h.span == (0, 1)));
        assert!(built.layout.holes.values().any(|h| h.plated));
    }

    #[test]
    fn back_drills_take_the_plating_they_cross() {
        let key = |i| Key::Feature { layer: 9, index: i };
        let round = |x| HoleShape::Round {
            at: Point::new(x, 0),
            d: 300_000,
        };
        let mut holes = BTreeMap::from([
            (
                key(0),
                Hole {
                    shape: round(0),
                    span: (0, 3),
                    plated: true,
                    backdrilled: vec![],
                },
            ),
            (
                key(1),
                Hole {
                    shape: round(5),
                    span: (0, 3),
                    plated: true,
                    backdrilled: vec![],
                },
            ),
            (
                key(2),
                Hole {
                    shape: round(0),
                    span: (2, 3),
                    plated: false,
                    backdrilled: vec![],
                },
            ),
        ]);
        strip_backdrilled(&mut holes, &[(key(2), Point::new(0, 0), (2, 3))]);
        assert_eq!(holes[&key(0)].backdrilled, [(2, 3)]);
        assert!(holes[&key(1)].backdrilled.is_empty());
        assert!(holes[&key(2)].backdrilled.is_empty());
    }
}
