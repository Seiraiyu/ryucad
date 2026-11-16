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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::error::{Code, Error, Result};
use ryu_board::geom::{Nm, Point};
use ryu_board::model::{AttrValue, Board, Context, FeatureKind, Ref, Side, Symbol, ToolKind};
use ryu_board::symbol::StandardSymbol;
use ryu_copper::connect::{Copper, Gap, Island, IslandId, Node, Placement, PointContact};
use ryu_copper::layout::{Hole, HoleShape, Item, Layer, Layout, NetPoint, PointSide};
use ryu_copper::measure::BodyOptions;
use ryu_copper::netlist::{self, Report};
use ryu_copper::poly::{self, FlattenOptions, Polygons};

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

/// A board's copper, as the session keeps it.
pub struct CopperBoard {
    pub board: Arc<Board>,
    /// The layout's layers, holes and points; its items live in `copper`.
    pub built: Built,
    pub copper: Copper<Key>,
    /// Where each point landed, in the points' order.
    pub placed: Vec<Placement<Key>>,
    pub report: Report<Key>,
    /// The netlist's nets on each island.
    pub nets: BTreeMap<IslandId<Key>, BTreeSet<String>>,
    points: BTreeMap<Key, usize>,
    contacts: Mutex<BTreeMap<IslandId<Key>, Contacts>>,
    merged: Mutex<BTreeMap<usize, Arc<Polygons>>>,
    gaps: Mutex<BTreeMap<(usize, Nm), Gaps>>,
}

/// An island's point contacts, as cached.
type Contacts = Arc<Vec<PointContact<Key>>>;
/// A layer's gaps under a distance, as cached.
type Gaps = Arc<Vec<Gap<Key>>>;

fn not_found(what: &str, text: &str) -> Error {
    Error::new(Code::NotFound, format!("no {what} {text} on this board")).with(what, text)
}

impl CopperBoard {
    /// Builds a board's copper: its layout, connectivity and compare.
    pub fn new(board: Arc<Board>) -> CopperBoard {
        let mut built = layout(&board);
        let copper = Copper::new(&built.layout, BodyOptions::default());
        built.layout.items.clear();
        let placed = netlist::placements(&copper, &built.layout.points);
        let report = netlist::compare_placed(&copper, &built.layout.points, &placed);
        let mut nets: BTreeMap<IslandId<Key>, BTreeSet<String>> = BTreeMap::new();
        for (p, at) in built.layout.points.iter().zip(&placed) {
            if let Placement::On(id) = at {
                nets.entry(*id).or_default().insert(p.net.clone());
            }
        }
        let points = built
            .layout
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| (p.key, i))
            .collect();
        CopperBoard {
            board,
            built,
            copper,
            placed,
            report,
            nets,
            points,
            contacts: Mutex::default(),
            merged: Mutex::default(),
            gaps: Mutex::default(),
        }
    }

    pub fn step_name(&self) -> &str {
        &self.board.steps[self.built.step].name
    }

    /// A layout layer's board layer name.
    pub fn layer_name(&self, layer: usize) -> &str {
        &self.board.layers[self.built.copper[layer]].name
    }

    /// The copper layer named `name`: `not_found` for no layer,
    /// `invalid_param` for one that isn't copper.
    pub fn layer_of(&self, name: &str) -> Result<usize> {
        let bl = self
            .board
            .layer(name)
            .ok_or_else(|| not_found("layer", name))?;
        self.built
            .copper
            .iter()
            .position(|&c| c == bl)
            .ok_or_else(|| {
                Error::new(Code::InvalidParam, format!("{name} isn't a copper layer"))
                    .with("layer", name)
            })
    }

    /// A key's reference, prefixed by the board's ID in a project. A CAD
    /// point's is its net's (see [`Self::point_json`]).
    pub fn reference(&self, prefix: Option<&str>, key: Key) -> String {
        let s = &self.board.steps[self.built.step];
        let step = s.name.clone();
        let r = match key {
            Key::Feature { layer, index } => Ref::Feature {
                step,
                layer: self.board.layers[layer as usize].name.clone(),
                index: index as usize,
            },
            Key::Pin { component, pin } => {
                let c = &s.components[component as usize];
                Ref::Pin {
                    step,
                    refdes: c.refdes.clone(),
                    pin: c.pins[pin as usize].name.clone(),
                }
            }
            Key::Cad { net, .. } => Ref::Net {
                step,
                name: s.nets[net as usize].name.clone(),
            },
        };
        match prefix {
            Some(p) => format!("{p}/{r}"),
            None => r.to_string(),
        }
    }

    /// A piece's (or an island's) name: its item's reference, with `#n`
    /// for any piece but the first.
    pub fn island_name(&self, prefix: Option<&str>, id: &(Key, u32)) -> String {
        let r = self.reference(prefix, id.0);
        if id.1 == 0 {
            r
        } else {
            format!("{r}#{}", id.1)
        }
    }

    /// A node's name: a piece's, or a hole's reference.
    pub fn node_name(&self, prefix: Option<&str>, n: &Node<Key>) -> String {
        match n {
            Node::Piece(k, i) => self.island_name(prefix, &(*k, *i)),
            Node::Hole(k) => self.reference(prefix, *k),
        }
    }

    /// A netlist point as JSON: its reference (a CAD point's net, with
    /// `cad_point`), its net and where it is.
    pub fn point_json(&self, prefix: Option<&str>, key: Key) -> serde_json::Value {
        let mut v = serde_json::json!({"ref": self.reference(prefix, key)});
        if let Key::Cad { point, .. } = key {
            v["cad_point"] = serde_json::json!(point);
        }
        if let Some(&i) = self.points.get(&key) {
            let p = &self.built.layout.points[i];
            v["net"] = serde_json::json!(p.net);
            v["at"] = crate::inspect::point(p.at);
        }
        v
    }

    /// The key a feature or pin reference names.
    pub fn key_of(&self, prefix: Option<&str>, text: &str, what: &str) -> Result<Key> {
        let s = &self.board.steps[self.built.step];
        let wrong_step = |step: &str| step != s.name;
        match crate::inspect::parse_reference(prefix, text, what)? {
            Ref::Feature { step, layer, index } if !wrong_step(&step) => {
                let l = self
                    .board
                    .layer(&layer)
                    .ok_or_else(|| not_found("layer", &layer))?;
                if index >= s.layers[l].features().len() {
                    return Err(not_found("feature", text));
                }
                Ok(Key::Feature {
                    layer: l as u32,
                    index: index as u32,
                })
            }
            Ref::Pin { step, refdes, pin } if !wrong_step(&step) => {
                let c = s
                    .components
                    .iter()
                    .position(|c| c.refdes == refdes)
                    .ok_or_else(|| not_found("pin", text))?;
                let p = s.components[c]
                    .pins
                    .iter()
                    .position(|p| p.name == pin)
                    .ok_or_else(|| not_found("pin", text))?;
                Ok(Key::Pin {
                    component: c as u32,
                    pin: p as u32,
                })
            }
            _ => Err(not_found(what, text)),
        }
    }

    /// The island an island's name (as [`Self::island_name`] gives it)
    /// names.
    pub fn island_of_name(&self, prefix: Option<&str>, text: &str) -> Result<IslandId<Key>> {
        let (r, piece) = match text.rsplit_once('#') {
            Some((r, n)) if n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty() => {
                (r, n.parse::<u32>().map_err(|_| not_found("island", text))?)
            }
            _ => (text, 0),
        };
        let key = self.key_of(prefix, r, "island")?;
        let id = self
            .copper
            .island_of(&Node::Piece(key, piece))
            .ok_or_else(|| not_found("island", text))?;
        if id != (key, piece) {
            return Err(Error::new(
                Code::NotFound,
                format!(
                    "{text} is on island {}, not one",
                    self.island_name(prefix, &id)
                ),
            )
            .with("island", self.island_name(prefix, &id)));
        }
        Ok(id)
    }

    /// An island's point contacts, found once.
    pub fn point_contacts(&self, island: &Island<Key>) -> Arc<Vec<PointContact<Key>>> {
        if let Some(c) = self.contacts.lock().unwrap().get(&island.id) {
            return Arc::clone(c);
        }
        let found = Arc::new(self.copper.point_contacts(island));
        self.contacts
            .lock()
            .unwrap()
            .insert(island.id, Arc::clone(&found));
        found
    }

    /// A copper layer merged (its items in drawing order less its holes),
    /// found once.
    pub fn merged(&self, layer: usize) -> Arc<Polygons> {
        if let Some(m) = self.merged.lock().unwrap().get(&layer) {
            return Arc::clone(m);
        }
        let fo = FlattenOptions::new(self.copper.options().bound());
        let parts: Vec<_> = self
            .copper
            .items()
            .filter(|(_, it)| it.layer == layer)
            .flat_map(|(_, it)| poly::flatten_shape(&it.shape, &fo))
            .collect();
        let holes: Vec<_> = self
            .built
            .layout
            .holes
            .values()
            .filter(|h| h.span.0 <= layer && layer <= h.span.1)
            .map(|h| poly::flatten_hole(&h.shape, &fo))
            .collect();
        let m = Arc::new(poly::merge_layer(&parts, &holes));
        self.merged.lock().unwrap().insert(layer, Arc::clone(&m));
        m
    }

    /// The gaps under `below` on a copper layer, found once per distance
    /// (the last sixteen asked kept).
    pub fn gaps(&self, layer: usize, below: Nm) -> Arc<Vec<Gap<Key>>> {
        if let Some(g) = self.gaps.lock().unwrap().get(&(layer, below)) {
            return Arc::clone(g);
        }
        let found = Arc::new(self.copper.clearance(layer, below));
        let mut kept = self.gaps.lock().unwrap();
        if kept.len() >= 16 {
            kept.clear();
        }
        kept.insert((layer, below), Arc::clone(&found));
        found
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
    fn minimal_clean_s_copper() {
        let cb = CopperBoard::new(Arc::new(read("minimal-clean.tgz")));
        assert!(cb.report.judged);
        assert!(!cb.copper.islands().is_empty());
        for k in cb.copper.items().map(|(k, _)| *k).take(20) {
            let text = cb.reference(Some("mc"), k);
            assert!(text.starts_with("mc/pcb/"), "{text}");
            assert_eq!(cb.key_of(Some("mc"), &text, "a").unwrap(), k);
        }
        for i in cb.copper.islands() {
            let name = cb.island_name(None, &i.id);
            assert_eq!(cb.island_of_name(None, &name).unwrap(), i.id);
        }
        // Every island holding points says which nets.
        let on: usize = cb.nets.values().map(BTreeSet::len).sum();
        assert!(on >= 3, "{:?}", cb.nets);
        assert_eq!(
            cb.key_of(None, "pcb/comp/NOPE/1", "a").unwrap_err().code,
            crate::error::Code::NotFound
        );
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
