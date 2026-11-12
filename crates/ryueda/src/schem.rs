//! A block's schematic: `blocks/<id>/layout.json` (kind `ryueda.layout`,
//! format 1), and the block as `ryusch` reads it, built from the project:
//! each component's units with their pins, each child instance as a block
//! symbol, each port of the block, and their nets in keys the layout file
//! holds (`<net>`, `rail:<id>`, `port:<id>`).

use crate::conn::{Direction, Node};
use crate::design::BlockFiles;
use crate::doc;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::{Project, SELF};
use crate::symbol::{self, PinType};
use ryusch::geom::{GRID, Nm, Side};
use ryusch::input::{self, Flow, Graphic, NetKind, Part, PartKind, Pin, PinKind};
use ryusch::layout::Layout;
use std::collections::BTreeMap;

/// A block's layout file, in its folder.
pub const LAYOUT: &str = "layout.json";
pub const LAYOUT_KIND: &str = "ryueda.layout";
/// The newest layout format this build reads: 5 holds a region titled by
/// its ID (plan 3.A); 4 holds off-sheet
/// connectors, drawn buses and notes (plan 2.C); 3 holds regions, with each
/// region's parts placed relative to its origin, sheets' own papers and
/// the region and paper instructions (plan 2.A); 2 a lasting pin-level
/// `align` (plan 1.D); every other layout is written at 1. A layout of
/// format 1 or 2 reads as one with no regions, at the project's paper.
pub const LAYOUT_FORMAT: u64 = 5;
/// What the PDF's border takes on every side of the paper.
pub const BORDER: Nm = 10_000_000;
/// The band at the foot of every page that holds the title block (plan
/// 2.E): the drawing area stops above it.
pub const TITLE_BAND: Nm = 20_000_000;

/// The path of a block's layout file.
pub fn layout_path(block: &Id) -> String {
    crate::source::join(
        &crate::source::join(crate::conn::BLOCKS, block.as_str()),
        LAYOUT,
    )
}

/// Reads a layout file, into sheet coordinates.
pub fn read_layout(bytes: &[u8], path: &str) -> Result<Layout> {
    doc::read::<Layout>(bytes, path, LAYOUT_KIND, LAYOUT_FORMAT)
        .map(|l| l.absolute())
        .map_err(|e| {
            let code = if matches!(e.code, Code::FormatTooNew | Code::MergeConflict) {
                e.code
            } else {
                Code::LayoutInvalid
            };
            Error::new(code, e.message.clone()).with("path", path)
        })
}

/// A layout file's canonical text, at the lowest format that holds it,
/// each region's parts placed relative to its origin.
pub fn layout_text(l: &Layout) -> String {
    doc::to_string(&l.relative(), LAYOUT_KIND, l.format())
}

/// What a part's layout key names: its component for a unit
/// (`<component>:<n>`), else the key itself.
pub(crate) fn part_of(key: &str) -> &str {
    const KEEP: [&str; 6] = ["instance:", "port:", "rail:", "bus:", "region:", "note:"];
    if KEEP.iter().any(|p| key.starts_with(p)) {
        return key;
    }
    match key.split_once(':') {
        Some((c, u)) if !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit()) => c,
        _ => key,
    }
}

/// The reference path of what layout key `key` draws in block `block`
/// (designs 518 and 519): `<block>/<component>` for a component's part (a
/// unit's too), `<block>/instance:<id>`, `<block>/port:<id>`,
/// `<block>/<net>`, `<block>/rail:<id>`, `<block>/bus:<id>`,
/// `<block>/region:<id>`, `<block>/note:<id>`, and a label
/// (`label:<net>@<part>/<pin>`, `ryusch::paint`'s owner) as
/// `<block>/label:<net>@<component>/<pin>`. Diff entries, compare renders'
/// `data-ref` and stage 3's scene all name things so.
pub fn ref_path(block: &Id, key: &str) -> String {
    if let Some(rest) = key.strip_prefix("label:")
        && let Some((net, on)) = rest.rsplit_once('@')
    {
        let (part, pin) = on.rsplit_once('/').unwrap_or((on, ""));
        return format!("{block}/label:{net}@{}/{pin}", part_of(part));
    }
    format!("{block}/{}", part_of(key))
}

fn pin_kind(t: PinType) -> PinKind {
    match t {
        PinType::Input => PinKind::Input,
        PinType::Output => PinKind::Output,
        PinType::Bidirectional => PinKind::Bidirectional,
        PinType::TriState => PinKind::TriState,
        PinType::Passive => PinKind::Passive,
        PinType::Free => PinKind::Free,
        PinType::Unspecified => PinKind::Unspecified,
        PinType::PowerIn => PinKind::PowerIn,
        PinType::PowerOut => PinKind::PowerOut,
        PinType::OpenCollector => PinKind::OpenCollector,
        PinType::OpenEmitter => PinKind::OpenEmitter,
        PinType::NoConnect => PinKind::NoConnect,
    }
}

fn side(s: symbol::Side) -> Side {
    match s {
        symbol::Side::Left => Side::Left,
        symbol::Side::Right => Side::Right,
        symbol::Side::Up => Side::Up,
        symbol::Side::Down => Side::Down,
    }
}

fn flow(d: Direction) -> Flow {
    match d {
        Direction::In => Flow::In,
        Direction::Out => Flow::Out,
        Direction::Bidirectional => Flow::Bidirectional,
        Direction::Power => Flow::Power,
        Direction::Passive => Flow::Passive,
    }
}

fn fill(f: symbol::Fill) -> input::Fill {
    match f {
        symbol::Fill::None => input::Fill::None,
        symbol::Fill::Body => input::Fill::Body,
        symbol::Fill::Outline => input::Fill::Outline,
    }
}

/// A symbol graphic as ryusch draws it.
fn graphic(g: &symbol::Graphic) -> Graphic {
    match g {
        symbol::Graphic::Rect {
            start,
            end,
            fill: f,
            ..
        } => Graphic::Rect {
            start: *start,
            end: *end,
            fill: fill(*f),
        },
        symbol::Graphic::Polyline {
            points, fill: f, ..
        } => Graphic::Polyline {
            points: points.clone(),
            fill: fill(*f),
        },
        symbol::Graphic::Arc {
            start, mid, end, ..
        } => Graphic::Arc {
            start: *start,
            mid: *mid,
            end: *end,
        },
        symbol::Graphic::Circle {
            center,
            radius_nm,
            fill: f,
            ..
        } => Graphic::Circle {
            center: *center,
            radius: *radius_nm,
            fill: fill(*f),
        },
        symbol::Graphic::Bezier { points, .. } => Graphic::Bezier { points: *points },
        symbol::Graphic::Text {
            text, at, size_nm, ..
        } => Graphic::Text {
            text: text.clone(),
            at: *at,
            size: *size_nm,
        },
    }
}

/// The net key of what a pin or port names.
fn node_key(n: &Node) -> Option<String> {
    match n {
        Node::Net(id) => Some(id.to_string()),
        Node::Rail(id) => Some(format!("rail:{id}")),
        Node::Port(id) => Some(format!("port:{id}")),
        Node::Nc => None,
    }
}

/// Whether a rail is ground: its voltage 0 V, or its name a ground's.
/// Whether a rail's name already says its voltage (plan 2.C): a ground's
/// name for 0 V (`is_ground`'s names), else a number in it written `3V3`,
/// `3.3V` or `5V` (after a `+`, `_`, anything not a letter or digit, or the
/// start) equal to it.
fn name_says_voltage(name: &str, voltage: &str) -> bool {
    let q = |t: &str| ryueda_parts::param::quantity(t, crate::units::Dim::VOLTAGE);
    let Some(want) = q(voltage) else {
        return false;
    };
    let upper = name.to_ascii_uppercase();
    if q("0 V") == Some(want)
        && ["GND", "VSS", "AGND", "DGND", "PGND", "0V"]
            .iter()
            .any(|g| upper == *g || upper.starts_with(&format!("{g}_")))
    {
        return true;
    }
    let b = upper.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
            i += 1;
        }
        if i < b.len() && b[i] == b'V' {
            let whole = &upper[start..i];
            i += 1;
            let frac = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let text = if whole.contains('.') || frac == i {
                format!("{whole} V")
            } else {
                format!("{whole}.{} V", &upper[frac..i])
            };
            if q(&text) == Some(want) {
                return true;
            }
        }
    }
    false
}

fn is_ground(r: &crate::conn::Rail) -> bool {
    let name = r.name.to_ascii_uppercase();
    let zero = r
        .voltage
        .as_deref()
        .and_then(|v| crate::conn::exact_value(v, crate::units::Dim::VOLTAGE))
        == Some((0, 0));
    zero || ["GND", "VSS", "AGND", "DGND", "PGND", "0V"]
        .iter()
        .any(|g| name == *g || name.starts_with(&format!("{g}_")))
}

/// What a part of the block stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Of {
    /// A component's unit (1 for a symbol of one unit).
    Unit {
        component: Id,
        unit: u32,
    },
    Instance(Id),
    Port(Id),
}

/// The block as ryusch reads it, and what each part stands for.
#[derive(Clone)]
pub struct Built {
    pub input: input::Block,
    pub of: Vec<Of>,
    /// Each stack by its front instance: its layers' instances, in ID
    /// order, the front first.
    pub stacks: BTreeMap<Id, Vec<Id>>,
}

impl Built {
    /// The front instance of the stack `instance` is a layer of, or itself.
    pub fn front<'a>(&'a self, instance: &'a Id) -> &'a Id {
        self.stacks
            .iter()
            .find(|(_, list)| list.contains(instance))
            .map_or(instance, |(front, _)| front)
    }

    /// Gives each unit the designator `refdes` gives its component (a
    /// unit of several, with its letter): what a layer of the block shows.
    pub fn designators(&mut self, refdes: &BTreeMap<Id, String>) {
        for (part, of) in self.input.parts.iter_mut().zip(&self.of) {
            if let Of::Unit { component, unit } = of
                && let Some(r) = refdes.get(component)
            {
                part.refdes = if part.key.contains(':') {
                    format!("{r}{}", crate::props::letter(u64::from(*unit)))
                } else {
                    r.clone()
                };
            }
        }
    }
}

/// A block where a design draws it: an occurrence, the instance path from
/// the design's root (none for the root itself), and each of its
/// components' designators there. Every layer of a stack is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub design: Id,
    pub path: Vec<Id>,
    /// The stacked levels' letters from the root (`B`, `A/C`), as the
    /// built-in property `stack_path` reads.
    pub stack_path: String,
    pub refdes: BTreeMap<Id, String>,
}

impl Layer {
    /// Its instance path as text: `ch_b`, `front/ch_a`, `` for the root.
    pub fn path_text(&self) -> String {
        self.path
            .iter()
            .map(Id::as_str)
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Whether `text` names it: its path, its stack path, or either after
    /// `<design>:`.
    pub fn named(&self, text: &str) -> bool {
        let (d, rest) = match text.split_once(':') {
            Some((d, r)) => (Some(d), r),
            None => (None, text),
        };
        d.is_none_or(|d| d == self.design.as_str())
            && (rest == self.path_text()
                || (!self.stack_path.is_empty() && rest == self.stack_path))
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "design": self.design,
            "path": self.path_text(),
            "stack_path": self.stack_path,
        })
    }
}

/// Every occurrence of block `block` in the project's designs, each
/// design's depth first with instances in ID order, designs in ID order.
/// A design that doesn't flatten still gives its occurrences, with no
/// designators.
pub fn layers(p: &Project, block: &Id) -> Vec<Layer> {
    fn walk(
        p: &Project,
        at: &Id,
        path: &mut Vec<Id>,
        stack: &str,
        want: &Id,
        found: &mut Vec<(Vec<Id>, String)>,
    ) {
        // A block holding itself is a fault the files report; stop there.
        if path.len() > 64 {
            return;
        }
        if at == want {
            found.push((path.clone(), stack.to_owned()));
        }
        let Ok(b) = p.conn.block(at) else { return };
        for (iid, i) in &b.instances {
            let Ok(i) = &i.item else { continue };
            let (index, count) = crate::netlist::stack_of(b, iid);
            let s = if count > 1 {
                let l = crate::props::letter(index);
                if stack.is_empty() {
                    l
                } else {
                    format!("{stack}/{l}")
                }
            } else {
                stack.to_owned()
            };
            path.push(iid.clone());
            walk(p, &i.of, path, &s, want, found);
            path.pop();
        }
    }
    let mut out = Vec::new();
    for did in p.conn.designs.keys() {
        let Ok(root) = crate::netlist::design_root(p, did) else {
            continue;
        };
        let mut found = Vec::new();
        walk(p, &root, &mut Vec::new(), "", block, &mut found);
        if found.is_empty() {
            continue;
        }
        let refdes: BTreeMap<String, String> = crate::netlist::flatten(p, did)
            .map(|n| {
                n.occurrences
                    .into_iter()
                    .filter_map(|o| Some((o.reference, o.refdes?)))
                    .collect()
            })
            .unwrap_or_default();
        let comps: Vec<&Id> = p
            .conn
            .blocks
            .get(block)
            .map(|b| b.components.keys().collect())
            .unwrap_or_default();
        for (path, stack_path) in found {
            let mut prefix = format!("{did}:");
            for step in &path {
                prefix.push_str(step.as_str());
                prefix.push('/');
            }
            let mine = comps
                .iter()
                .filter_map(|c| Some(((*c).clone(), refdes.get(&format!("{prefix}{c}"))?.clone())))
                .collect();
            out.push(Layer {
                design: did.clone(),
                path,
                stack_path,
                refdes: mine,
            });
        }
    }
    out
}

/// Each component's widest designator across `layers`, by the font's
/// measure (ties to the later in text order): the text a layout reserves
/// room for, so every layer's drawing is the same.
fn widest(layers: &[Layer]) -> BTreeMap<Id, String> {
    let mut out: BTreeMap<Id, String> = BTreeMap::new();
    for l in layers {
        for (c, r) in &l.refdes {
            let w = |t: &str| {
                (
                    ryusch::font::width_in(t, ryusch::font::Font::DESIGNATOR),
                    t.to_owned(),
                )
            };
            if out.get(c).is_none_or(|old| w(r) > w(old)) {
                out.insert(c.clone(), r.clone());
            }
        }
    }
    out
}

/// A component whose symbol doesn't resolve: a box with its pins down
/// the left, so it still draws; `net` gives each pin's net and whether
/// it is marked not connected.
fn placeholder(
    numbers: &[String],
    net: &dyn Fn(&str) -> (Option<usize>, bool),
) -> (Vec<Graphic>, Vec<Pin>) {
    let n = numbers.len().max(1) as Nm;
    let g = vec![Graphic::Rect {
        start: [0, GRID],
        end: [4 * GRID, -2 * GRID * n + GRID],
        fill: input::Fill::None,
    }];
    let pins = numbers
        .iter()
        .enumerate()
        .map(|(i, num)| {
            let (net, nc) = net(num);
            Pin {
                number: num.clone(),
                name: String::new(),
                at: [-2 * GRID, -2 * GRID * i as Nm],
                side: Side::Left,
                length: 2 * GRID,
                kind: PinKind::Unspecified,
                net,
                nc,
                show_number: true,
                layers: Vec::new(),
            }
        })
        .collect();
    (g, pins)
}

/// The paper series a project's sheets take, smallest first, each paper's
/// drawing area the paper less the PDF's border and the title band (plans
/// 2.A and 2.E): none with fit off.
pub fn papers(sheet: &crate::project::Sheet) -> Vec<input::Paper> {
    sheet
        .series()
        .into_iter()
        .map(|size| {
            let s = crate::project::Sheet {
                size,
                ..sheet.clone()
            };
            let (w, h) = s.size_nm();
            input::Paper {
                size: size.as_str().into(),
                orientation: match s.orientation {
                    crate::project::Orientation::Landscape => {
                        ryusch::layout::Orientation::Landscape
                    }
                    crate::project::Orientation::Portrait => ryusch::layout::Orientation::Portrait,
                },
                area: [w - 2 * BORDER, h - 2 * BORDER - TITLE_BAND],
            }
        })
        .collect()
}

/// Builds block `b` of project `p` for ryusch, with no previous layout
/// and no instructions.
/// One unit of a library symbol as ryusch draws it, joined to nothing: its
/// own pins and the common ones on the unit with the power pins (else the
/// first), names KiCad leaves generic not drawn, and its graphics.
/// `build` joins the pins to their nets; the symbol lint measures it.
pub(crate) fn unit_part(sym: &crate::symbol::Symbol, unit: u32, key: String, prefix: &str) -> Part {
    let home = crate::symlint::home(sym);
    let pins = sym
        .pins
        .iter()
        .filter(|(_, x)| x.unit == Some(unit) || (x.unit.is_none() && unit == home))
        .map(|(num, x)| {
            // KiCad's generic names (`~`, `Pin_3`) say nothing.
            let shown = !(x.name.is_empty()
                || x.name == "~"
                || x.name == *num
                || x.name == format!("Pin_{num}"));
            Pin {
                number: num.clone(),
                name: if shown { x.name.clone() } else { String::new() },
                at: x.at,
                side: side(x.side),
                length: x.length_nm,
                kind: pin_kind(x.ty),
                net: None,
                nc: false,
                show_number: sym.pins.len() > 2,
                layers: Vec::new(),
            }
        })
        .collect();
    let graphics = sym
        .graphics
        .values()
        .filter(|g| g.unit().is_none_or(|u| u == unit))
        .map(graphic)
        .collect();
    Part {
        key,
        kind: PartKind::Symbol {
            prefix: prefix.to_owned(),
        },
        refdes: String::new(),
        value: String::new(),
        function: String::new(),
        text: ryusch::input::PartText::default(),
        graphics,
        pins,
    }
}

pub fn build(p: &Project, b: &BlockFiles) -> Result<Built> {
    let block = b
        .block
        .item
        .as_ref()
        .map_err(|e| e.clone().with("block", &b.id))?;
    let (w, h) = p.manifest.schematic.sheet.size_nm();
    // The nets: the block's, then the rails its pins and instances use,
    // then its ports.
    let mut nets: Vec<input::Net> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (id, n) in &b.nets {
        let name = n
            .item
            .as_ref()
            .map_or_else(|_| id.to_string(), |n| n.name.clone());
        index.insert(id.to_string(), nets.len());
        nets.push(input::Net {
            key: id.to_string(),
            name,
            kind: NetKind::Signal,
            intent: String::new(),
        });
    }
    let mut rails: Vec<Id> = Vec::new();
    for c in b.components.values().filter_map(|c| c.item.as_ref().ok()) {
        rails.extend(c.pins.values().filter_map(|n| match n {
            Node::Rail(r) => Some(r.clone()),
            _ => None,
        }));
    }
    for i in b.instances.values().filter_map(|i| i.item.as_ref().ok()) {
        rails.extend(i.ports.values().filter_map(|n| match n {
            Node::Rail(r) => Some(r.clone()),
            _ => None,
        }));
    }
    rails.sort();
    rails.dedup();
    for r in rails {
        let Some(rail) = p.conn.rails.get(&r).and_then(|x| x.item.as_ref().ok()) else {
            continue;
        };
        let key = format!("rail:{r}");
        index.insert(key.clone(), nets.len());
        // A rail's voltage under its power port, where its name doesn't
        // say it already (plan 2.C).
        let intent = rail
            .voltage
            .as_deref()
            .filter(|v| !name_says_voltage(&rail.name, v))
            .map(String::from)
            .unwrap_or_default();
        nets.push(input::Net {
            key,
            intent,
            name: rail.name.clone(),
            kind: if is_ground(rail) {
                NetKind::Ground
            } else {
                NetKind::Supply
            },
        });
    }
    for (id, port) in &block.ports {
        let key = format!("port:{id}");
        index.insert(key.clone(), nets.len());
        nets.push(input::Net {
            key,
            name: port.name.clone(),
            kind: NetKind::Signal,
            intent: String::new(),
        });
    }
    // The block's buses (plan 2.C), their members by index in the file's
    // order; a member that isn't one of the block's nets is left out.
    let buses: Vec<input::Bus> = block
        .buses
        .iter()
        .map(|(id, bus)| input::Bus {
            key: format!("bus:{id}"),
            name: bus.name.clone(),
            members: bus
                .nets
                .iter()
                .filter_map(|n| index.get(n.as_str()).copied())
                .collect(),
        })
        .collect();
    let net_of = |n: Option<&Node>| n.and_then(node_key).and_then(|k| index.get(&k).copied());
    let all_layers = layers(p, &b.id);
    let room_for = widest(&all_layers);
    // What a component does, for its region's title: its `function`
    // parameter, else the first that its first layer's levels give
    // (each instance from the innermost, the block, the project).
    let mut outer: Vec<&crate::props::Properties> = Vec::new();
    if let Some(first) = all_layers.first() {
        for iid in first.path.iter().rev() {
            if let Some(inst) = p
                .conn
                .blocks
                .values()
                .find_map(|bf| bf.instances.get(iid))
                .and_then(|i| i.item.as_ref().ok())
            {
                outer.push(&inst.properties);
            }
        }
    }
    if let Ok(own) = &b.block.item {
        outer.push(&own.properties);
    }
    outer.push(&p.manifest.properties);
    let function_of = |c: &crate::conn::Component| -> String {
        std::iter::once(&c.parameters)
            .chain(outer.iter().copied())
            .find_map(|l| l.get("function").cloned())
            .unwrap_or_default()
    };
    let mut parts = Vec::new();
    let mut of = Vec::new();
    for (cid, c) in &b.components {
        let Ok(c) = &c.item else { continue };
        let lib = crate::partcmd::placed(p, &c.of).ok();
        let value = c
            .value
            .clone()
            .or_else(|| lib.map(|(lc, _)| lc.value.clone()))
            .unwrap_or_default();
        let prefix = lib.map_or_else(
            || {
                crate::conn::refdes_parts(c.refdes.as_deref().unwrap_or("U"))
                    .0
                    .to_owned()
            },
            |(lc, _)| lc.prefix.clone(),
        );
        let refdes = room_for
            .get(cid)
            .cloned()
            .or_else(|| c.refdes.clone())
            .unwrap_or_else(|| format!("{prefix}?"));
        match lib.map(|(_, s)| s) {
            Some(sym) => {
                for unit in 1..=sym.units {
                    let key = if sym.units == 1 {
                        cid.to_string()
                    } else {
                        format!("{cid}:{unit}")
                    };
                    let mut part = unit_part(sym, unit, key, &prefix);
                    for pin in &mut part.pins {
                        let node = c.pins.get(&pin.number);
                        pin.net = net_of(node);
                        pin.nc = node == Some(&Node::Nc);
                    }
                    part.refdes = if sym.units == 1 {
                        refdes.clone()
                    } else {
                        format!("{refdes}{}", crate::props::letter(u64::from(unit)))
                    };
                    part.value = value.clone();
                    part.function = function_of(c);
                    parts.push(part);
                    of.push(Of::Unit {
                        component: cid.clone(),
                        unit,
                    });
                }
            }
            None => {
                let numbers: Vec<String> = c.pins.keys().cloned().collect();
                let (graphics, pins) = placeholder(&numbers, &|num| {
                    let node = c.pins.get(num);
                    (net_of(node), node == Some(&Node::Nc))
                });
                parts.push(Part {
                    key: cid.to_string(),
                    kind: PartKind::Symbol { prefix },
                    refdes,
                    function: function_of(c),
                    text: ryusch::input::PartText::default(),
                    value,
                    graphics,
                    pins,
                });
                of.push(Of::Unit {
                    component: cid.clone(),
                    unit: 1,
                });
            }
        }
    }
    // Stacks: instances of one block whose ports connect alike (each port
    // to the same rail, or left unconnected, in every one of them, else to
    // some signal) are drawn as one symbol, their layers in ID order.
    let pattern = |i: &crate::conn::Instance| -> Vec<String> {
        let child = p
            .conn
            .blocks
            .get(&i.of)
            .and_then(|c| c.block.item.as_ref().ok());
        child.map_or_else(Vec::new, |c| {
            c.ports
                .keys()
                .map(|pid| match i.ports.get(pid) {
                    Some(Node::Rail(r)) => format!("rail:{r}"),
                    Some(Node::Nc) => "nc".to_owned(),
                    Some(_) => "signal".to_owned(),
                    None => "open".to_owned(),
                })
                .collect()
        })
    };
    let mut groups: BTreeMap<(Id, Vec<String>), Vec<Id>> = BTreeMap::new();
    for (iid, i) in &b.instances {
        if let Ok(i) = &i.item {
            groups
                .entry((i.of.clone(), pattern(i)))
                .or_default()
                .push(iid.clone());
        }
    }
    let stacks: BTreeMap<Id, Vec<Id>> = groups
        .into_values()
        .filter(|list| list.len() > 1)
        .map(|list| (list[0].clone(), list))
        .collect();
    for (iid, i) in &b.instances {
        let Ok(i) = &i.item else { continue };
        let layers: Vec<&crate::conn::Instance> = match stacks.get(iid) {
            Some(list) => list
                .iter()
                .filter_map(|m| b.instances.get(m).and_then(|x| x.item.as_ref().ok()))
                .collect(),
            None if stacks.values().any(|l| l.contains(iid)) => continue,
            None => vec![i],
        };
        let child = p
            .conn
            .blocks
            .get(&i.of)
            .and_then(|c| c.block.item.as_ref().ok());
        let name = child.map_or_else(|| i.of.to_string(), |c| c.name.clone());
        let ports: Vec<(String, String, Flow, Vec<Option<usize>>)> = child
            .map(|c| {
                c.ports
                    .iter()
                    .map(|(pid, port)| {
                        (
                            pid.to_string(),
                            port.name.clone(),
                            flow(port.direction),
                            layers.iter().map(|x| net_of(x.ports.get(pid))).collect(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut part = ryusch::shape::stack_symbol(&format!("instance:{iid}"), &name, &ports);
        part.value = match stacks.get(iid) {
            Some(list) => list.iter().map(Id::as_str).collect::<Vec<_>>().join(", "),
            None => iid.to_string(),
        };
        parts.push(part);
        of.push(Of::Instance(iid.clone()));
    }
    for (pid, port) in &block.ports {
        let key = format!("port:{pid}");
        parts.push(ryusch::shape::port_symbol(
            &key,
            &port.name,
            flow(port.direction),
            index.get(&key).copied(),
        ));
        of.push(Of::Port(pid.clone()));
    }
    let mut built = Built {
        input: input::Block {
            name: block.name.clone(),
            sheet: [w - 2 * BORDER, h - 2 * BORDER - TITLE_BAND],
            papers: papers(&p.manifest.schematic.sheet),
            parts,
            nets,
            buses,
            ..input::Block::default()
        },
        of,
        stacks,
    };
    style_inputs(p, b, &mut built);
    Ok(built)
}

/// The geometry the style moves (plan 2.E), as engine input: each
/// component's text size and emphasis, and each net's and rail's intent
/// where the style shows it (a rail's voltage, a net's class, voltage,
/// current and impedance). Resolved with no sheet: rules naming a sheet
/// or a region can't set these.
fn style_inputs(p: &Project, b: &BlockFiles, built: &mut Built) {
    use crate::style::Kind;
    let layers = crate::cascade::Layers::of(p);
    for o in crate::cascade::objects(p, b, built) {
        let r = layers.resolve(&o, None).props;
        match o.kind {
            Kind::Component => {
                let text = ryusch::input::PartText {
                    size: r.text_size.unwrap_or_default().nm(),
                    emphasis: r.emphasis.unwrap_or(false),
                };
                for part in &mut built.input.parts {
                    if part.key == o.key || part.key.split(':').next() == Some(o.key.as_str()) {
                        part.text = text;
                    }
                }
            }
            Kind::Rail if r.visible.intent == Some(false) => {
                if let Some(n) = built.input.nets.iter_mut().find(|n| n.key == o.key) {
                    n.intent.clear();
                }
            }
            Kind::Net if r.visible.intent == Some(true) => {
                let Some(Ok(net)) = Id::parse(&o.key, "net")
                    .ok()
                    .and_then(|id| b.nets.get(&id))
                    .map(|n| n.item.as_ref())
                else {
                    continue;
                };
                let class = net.class.as_ref().map(|c| {
                    p.conn
                        .classes
                        .get(c)
                        .and_then(|x| x.item.as_ref().ok())
                        .map_or_else(|| c.to_string(), |x| x.name.clone())
                });
                let text: Vec<String> = class
                    .into_iter()
                    .chain(net.voltage.clone())
                    .chain(net.current.clone())
                    .chain(net.impedance.clone())
                    .collect();
                if let Some(n) = built.input.nets.iter_mut().find(|n| n.key == o.key) {
                    n.intent = text.join(" · ");
                }
            }
            _ => {}
        }
    }
}

/// A block laid out as its files stand: over its layout file where that
/// reads and keeps every hard rule, from nothing where it doesn't; and
/// whether the file was stale (laying out over it changed it) or invalid.
pub struct Laid {
    pub built: Built,
    pub outcome: ryusch::engine::Outcome,
    pub stale: bool,
    pub invalid: Option<Error>,
}

/// Lays out block `b` with `instructions` (the file's when `None`), moves
/// and what to lay out from nothing.
pub fn lay(
    p: &Project,
    b: &BlockFiles,
    instructions: Option<&BTreeMap<String, ryusch::layout::Instruction>>,
    moves: &[ryusch::layout::Instruction],
    relayout: ryusch::input::Relayout,
) -> Result<Laid> {
    lay_restoring(p, b, instructions, moves, relayout, BTreeMap::new())
}

/// `lay`, putting parts a released instruction moved back where they
/// stood before it (`restore`, by key) where still free.
pub fn lay_restoring(
    p: &Project,
    b: &BlockFiles,
    instructions: Option<&BTreeMap<String, ryusch::layout::Instruction>>,
    moves: &[ryusch::layout::Instruction],
    relayout: ryusch::input::Relayout,
    restore: BTreeMap<String, ryusch::layout::Placed>,
) -> Result<Laid> {
    let mut built = build(p, b)?;
    built.input.restore = restore;
    let stored = match &b.layout {
        Some(Ok(l)) => Some(l.clone()),
        _ => None,
    };
    let mut invalid = match &b.layout {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    };
    if let Some(l) = &stored {
        built.input.instructions = l.instructions.clone();
        // A pin the file doesn't draw joined is connectivity the file
        // hasn't caught up with (stale), not a broken file.
        let broken: Vec<_> = ryusch::rules::check(&built.input, l)
            .into_iter()
            .filter(|v| v.rule != "connected")
            .collect();
        if let Some(v) = broken.first() {
            invalid = Some(
                Error::new(
                    Code::LayoutInvalid,
                    format!(
                        "block {}'s layout breaks a rule ({}): {}; laid out again",
                        b.id, v.rule, v.message
                    ),
                )
                .with("block", &b.id)
                .with("rule", v.rule),
            );
        } else {
            built.input.previous = Some(l.clone());
        }
    }
    if let Some(i) = instructions {
        built.input.instructions = i.clone();
    }
    built.input.moves = moves.to_vec();
    built.input.relayout = relayout;
    let unknown = fill_notes(p, b, &mut built.input);
    let mut outcome = ryusch::engine::lay_out(&built.input).map_err(|r| refusal(&b.id, r))?;
    outcome.notes.extend(unknown);
    let stale = invalid.is_none()
        && instructions.is_none()
        && moves.is_empty()
        && stored.as_ref().is_some_and(|l| *l != outcome.layout);
    Ok(Laid {
        built,
        outcome,
        stale,
        invalid,
    })
}

/// The property levels text in block `b` reads (its first layer's
/// instances, the block, the project), with `built_in` read first.
pub fn levels<'a>(
    p: &'a Project,
    b: &'a BlockFiles,
    built_in: BTreeMap<&'static str, String>,
) -> crate::props::Levels<'a> {
    let mut outer: Vec<&crate::props::Properties> = Vec::new();
    if let Some(first) = layers(p, &b.id).first() {
        for iid in first.path.iter().rev() {
            if let Some(inst) = p
                .conn
                .blocks
                .values()
                .find_map(|bf| bf.instances.get(iid))
                .and_then(|i| i.item.as_ref().ok())
            {
                outer.push(&inst.properties);
            }
        }
    }
    if let Ok(own) = b.block.item.as_ref() {
        outer.push(&own.properties);
    }
    outer.push(&p.manifest.properties);
    crate::props::Levels {
        levels: outer,
        built_in,
    }
}

/// Fills each note instruction's properties (plan 2.C): `{name}` from the
/// levels a designator format reads for the block's first layer (each
/// instance from the innermost, the block, the project) and the built-in
/// `block` (the block's name). A note naming a property no level sets, or
/// that doesn't read as a template, keeps its text as written, with the
/// note `note_property_unknown`.
pub fn fill_notes(
    p: &Project,
    b: &BlockFiles,
    input: &mut input::Block,
) -> Vec<ryusch::engine::Note> {
    let levels = levels(p, b, BTreeMap::from([("block", input.name.clone())]));
    let mut notes = Vec::new();
    input.notes.clear();
    for (id, ins) in &input.instructions {
        let ryusch::layout::Instruction::Note { text, .. } = ins else {
            continue;
        };
        let filled = crate::props::Template::parse(text).and_then(|t| t.render(&|n| levels.get(n)));
        match filled {
            Ok(t) => {
                input.notes.insert(id.clone(), t);
            }
            Err(e) => {
                let property = e.details.get("property").cloned().unwrap_or_default();
                notes.push(ryusch::engine::Note {
                    code: "note_property_unknown".into(),
                    message: format!("note {id} keeps its text as written: {}", e.message),
                    details: serde_json::json!({"instruction": id, "property": property}),
                });
            }
        }
    }
    notes
}

/// The most fixes a block's findings have checked (the findings with the
/// most points first): each is a layout of the block.
pub const FIX_CHECKS: usize = 24;

/// A fix withheld for its cost (plan 3.A): the batch, and why.
pub type Withheld = (Vec<ryusch::layout::Instruction>, String);

/// `laid`'s lint with every fix tried first (`ryusch::lint::try_fix`), in
/// parallel: a fix that doesn't hold is dropped; one that holds but costs
/// the block points is dropped and listed, per finding, as withheld;
/// findings past `FIX_CHECKS` fixes say `fixes_checked: false` and keep
/// theirs.
pub fn checked_findings(
    laid: &Laid,
    prefs: &ryusch::lint::Prefs,
) -> (Vec<ryusch::lint::Finding>, Vec<bool>, Vec<Vec<Withheld>>) {
    use rayon::prelude::*;
    let (mut found, _) = ryusch::lint::lint_with(&laid.built.input, &laid.outcome, prefs);
    let before = found.clone();
    let mut order: Vec<usize> = (0..found.len())
        .filter(|i| !found[*i].fixes.is_empty())
        .collect();
    order.sort_by_key(|i| (std::cmp::Reverse(found[*i].points), *i));
    let mut budget = FIX_CHECKS;
    let mut checked = vec![true; found.len()];
    let mut asks: Vec<(usize, usize)> = Vec::new();
    for i in order {
        let n = found[i].fixes.len();
        if n > budget {
            checked[i] = false;
            continue;
        }
        budget -= n;
        asks.extend((0..n).map(|j| (i, j)));
    }
    let verdicts: Vec<Result<(), String>> = asks
        .par_iter()
        .map(|(i, j)| {
            ryusch::lint::try_fix(
                &laid.built.input,
                &laid.outcome,
                &found[*i],
                &found[*i].fixes[*j],
                prefs,
                &before,
            )
        })
        .collect();
    let mut withheld: Vec<Vec<Withheld>> = vec![Vec::new(); found.len()];
    let mut keep: std::collections::BTreeMap<usize, Vec<bool>> = Default::default();
    for ((i, j), v) in asks.iter().zip(verdicts) {
        if let Err(why) = &v
            && why.starts_with("it costs more")
        {
            withheld[*i].push((found[*i].fixes[*j].clone(), why.clone()));
        }
        keep.entry(*i).or_default().push(v.is_ok());
    }
    for (i, oks) in keep {
        let mut it = oks.into_iter();
        found[i].fixes.retain(|_| it.next().unwrap_or(false));
    }
    (found, checked, withheld)
}

/// ryusch's refusal as ryueda's error.
pub fn refusal(block: &Id, r: ryusch::instr::Refusal) -> Error {
    let code = match r.code {
        "layout_rule" => Code::LayoutRule,
        _ => Code::InstructionConflict,
    };
    let mut e = Error::new(code, format!("block {block}: {}", r.message)).with("block", block);
    if let serde_json::Value::Object(o) = r.details {
        for (k, v) in o {
            e = e.with(&k, v);
        }
    }
    e
}

/// The library alias a component's library component is read from.
pub fn alias(c: &crate::conn::Component) -> &str {
    c.of.library.as_ref().map_or(SELF, Id::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::tests::project;

    #[test]
    fn layout_keys_have_reference_paths() {
        let b = Id::parse("breakout", "test").unwrap();
        for (key, path) in [
            ("r1", "breakout/r1"),
            ("u1:2", "breakout/u1"),
            ("instance:s1", "breakout/instance:s1"),
            ("port:sda", "breakout/port:sda"),
            ("rail:3", "breakout/rail:3"),
            ("bus:q", "breakout/bus:q"),
            ("region:r2", "breakout/region:r2"),
            ("note:i4", "breakout/note:i4"),
            ("sda", "breakout/sda"),
            ("label:sda@u1:2/5", "breakout/label:sda@u1/5"),
            ("label:rail:gnd@r1/2", "breakout/label:rail:gnd@r1/2"),
        ] {
            assert_eq!(ref_path(&b, key), path, "{key}");
        }
    }

    #[test]
    fn a_layout_file_places_region_parts_relative_to_their_region() {
        use ryusch::layout::{Placed, Region, Sheet};
        let mut l = Layout::default();
        l.sheets
            .insert("s1".into(), Sheet::new("Sheet 1".into(), [0, 0]));
        l.placed.insert(
            "u1".into(),
            Placed {
                sheet: "s1".into(),
                at: [30_000_000, 40_000_000],
                rotation: 0,
                mirror: false,
                held: false,
            },
        );
        l.regions.insert(
            "r1".into(),
            Region {
                sheet: "s1".into(),
                at: [10_000_000, 20_000_000],
                frame: ryusch::geom::Rect::new([0, 0], [50_000_000, 60_000_000]),
                title: "U1".into(),
                named: false,
                parts: vec!["u1".into()],
            },
        );
        let text = layout_text(&l);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["format"], 3);
        assert_eq!(
            v["placed"]["u1"]["at"],
            serde_json::json!([20_000_000, 20_000_000])
        );
        assert_eq!(read_layout(text.as_bytes(), "layout.json").unwrap(), l);
        let mut old = l.clone();
        old.regions.clear();
        let v: serde_json::Value = serde_json::from_str(&layout_text(&old)).unwrap();
        assert_eq!(v["format"], 1, "a layout without regions keeps format 1");
    }

    #[test]
    fn blocks_build_with_units_instances_ports_and_rails() {
        let (_dir, p) = project(&[]);
        let top = p.conn.block(&Id::parse("top", "b").unwrap()).unwrap();
        let built = build(&p, top).unwrap();
        let keys: Vec<&str> = built.input.parts.iter().map(|x| x.key.as_str()).collect();
        // ch_a and ch_b place block s alike: one stack, ch_a in front.
        assert_eq!(keys, ["r1", "instance:ch_a"]);
        let ch_a = Id::parse("ch_a", "i").unwrap();
        let ch_b = Id::parse("ch_b", "i").unwrap();
        assert_eq!(built.stacks[&ch_a], [ch_a.clone(), ch_b.clone()]);
        assert_eq!(built.front(&ch_b), &ch_a);
        let stack = &built.input.parts[1];
        assert_eq!(stack.value, "ch_a, ch_b");
        assert!(stack.pins.iter().all(|x| x.layers.len() == 1));
        // Block s is drawn twice by design d: its layers, each with its
        // own designators.
        let s_id = Id::parse("s", "b").unwrap();
        let layers = layers(&p, &s_id);
        let named: Vec<(String, &str)> = layers
            .iter()
            .map(|l| (l.path_text(), l.stack_path.as_str()))
            .collect();
        assert_eq!(named, [("ch_a".to_owned(), "A"), ("ch_b".to_owned(), "B")]);
        assert_eq!(layers[1].refdes[&Id::parse("u1", "c").unwrap()], "U1_B");
        assert!(layers[1].named("B") && layers[1].named("d:ch_b") && !layers[1].named("ch_a"));
        let nets: Vec<(&str, NetKind)> = built
            .input
            .nets
            .iter()
            .map(|n| (n.key.as_str(), n.kind))
            .collect();
        assert_eq!(
            nets,
            [("sda", NetKind::Signal), ("rail:v3v3", NetKind::Supply)]
        );
        let s = p.conn.block(&Id::parse("s", "b").unwrap()).unwrap();
        let built = build(&p, s).unwrap();
        assert!(
            built
                .input
                .nets
                .iter()
                .any(|n| n.key == "rail:gnd" && n.kind == NetKind::Ground)
        );
        assert!(built.input.parts.iter().any(|x| x.key == "port:sda"));
        let laid = lay(&p, s, None, &[], ryusch::input::Relayout::None).unwrap();
        assert!(!laid.stale && laid.invalid.is_none());
        assert_eq!(
            ryusch::rules::check(&laid.built.input, &laid.outcome.layout),
            []
        );
        // A layout file reads back as it was written.
        let text = layout_text(&laid.outcome.layout);
        assert_eq!(
            read_layout(text.as_bytes(), "x").unwrap(),
            laid.outcome.layout
        );
        assert_eq!(
            read_layout(b"{}", "x").unwrap_err().code,
            Code::LayoutInvalid
        );
    }

    #[test]
    fn a_blocks_buses_reach_the_engine() {
        use serde_json::json;
        let (_dir, p) = project(&[
            (
                "blocks/top/block.json",
                json!({"format": 1, "kind": crate::conn::BLOCK_KIND, "name": "Top",
                    "buses": {"b1": {"name": "D", "nets": ["sda", "scl"]}}}),
            ),
            (
                "blocks/top/nets/scl.json",
                json!({"format": 1, "kind": crate::conn::NET_KIND, "name": "SCL"}),
            ),
        ]);
        let top = p.conn.block(&Id::parse("top", "b").unwrap()).unwrap();
        let built = build(&p, top).unwrap();
        let input = &built.input;
        assert_eq!(input.buses.len(), 1);
        let bus = &input.buses[0];
        assert_eq!(bus.key, "bus:b1");
        assert_eq!(bus.name, "D");
        let keys: Vec<&str> = bus
            .members
            .iter()
            .map(|n| input.nets[*n].key.as_str())
            .collect();
        assert_eq!(keys, ["sda", "scl"], "in the file's order");
    }

    #[test]
    fn a_rails_name_that_says_its_voltage_draws_none() {
        for (name, v) in [
            ("3V3", "3.3 V"),
            ("+3V3", "3.3 V"),
            ("+3.3V", "3.3 V"),
            ("VCC_5V", "5 V"),
            ("+12V", "12 V"),
            ("GND", "0 V"),
        ] {
            assert!(name_says_voltage(name, v), "{name} says {v}");
        }
        for (name, v) in [
            ("VDD", "3.3 V"),
            ("3V3", "5 V"),
            ("VBAT", "3.7 V"),
            ("AGND", "0.5 V"),
        ] {
            assert!(!name_says_voltage(name, v), "{name} doesn't say {v}");
        }
    }

    #[test]
    fn notes_take_their_blocks_properties() {
        use ryusch::layout::Instruction;
        use serde_json::json;
        let (_dir, p) = project(&[(
            "blocks/top/block.json",
            json!({"format": 2, "kind": crate::conn::BLOCK_KIND, "name": "Top",
                "properties": {"rev": "B"}}),
        )]);
        let top = p.conn.block(&Id::parse("top", "b").unwrap()).unwrap();
        let mut input = build(&p, top).unwrap().input;
        input.instructions.insert(
            "i1".into(),
            Instruction::Note {
                on: "s1".into(),
                text: "Rev {rev} of {block}".into(),
            },
        );
        input.instructions.insert(
            "i2".into(),
            Instruction::Note {
                on: "s1".into(),
                text: "{nothing}".into(),
            },
        );
        let notes = fill_notes(&p, top, &mut input);
        assert_eq!(input.notes["i1"], "Rev B of Top");
        assert!(!input.notes.contains_key("i2"));
        assert_eq!(input.note_text("i2"), Some("{nothing}"));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].code, "note_property_unknown");
        assert_eq!(notes[0].details["property"], "nothing");
    }

    /// The test project with SDA at 3.3 V and `style` as its style.json.
    fn styled_project(style: &str) -> (tempfile::TempDir, Project) {
        let (dir, _) = project(&[]);
        let board = dir.path().join("board");
        crate::source::write_atomic(
            &board.join("blocks/top/nets/sda.json"),
            r#"{"format": 1, "kind": "ryueda.net", "name": "SDA", "voltage": "3.3 V"}"#,
        )
        .unwrap();
        crate::source::write_atomic(&board.join(crate::style::FILE), style).unwrap();
        let p = Project::load(&board).unwrap();
        (dir, p)
    }

    #[test]
    fn the_style_sets_text_sizes_emphasis_and_intent() {
        let (dir, p) = styled_project(
            r#"{"kind": "ryueda.style", "format": 1,
                "rules": [{"select": {"kind": "net"}, "set": {"visible": {"intent": true}}},
                          {"select": {"component_class": "resistor"}, "set": {"text_size": "large", "emphasis": true}}]}"#,
        );
        let b = &p.conn.blocks[&Id::parse("top", "block").unwrap()];
        let built = build(&p, b).unwrap();
        let r = built.input.parts.iter().find(|x| x.refdes == "R1").unwrap();
        assert_eq!(r.text.size, 1_778_000);
        assert!(r.text.emphasis);
        let sda = built.input.nets.iter().find(|n| n.name == "SDA").unwrap();
        assert_eq!(sda.intent, "3.3 V");
        drop(dir);
    }

    #[test]
    fn a_papers_drawing_area_leaves_the_title_band() {
        let sheet = crate::project::Sheet::default();
        let a4 = &papers(&sheet)[0];
        assert_eq!(a4.size, "a4");
        assert_eq!(
            a4.area,
            [277_000_000, 170_000_000],
            "297 × 210 less 10 mm a side and the 20 mm band"
        );
        let a3 = &papers(&sheet)[1];
        assert_eq!(a3.area, [400_000_000, 257_000_000]);
    }

    /// A layout made before the title band whose drawing reaches into it
    /// breaks `inside`: read as layout_invalid, laid out again (plan 2.E).
    #[test]
    fn a_layout_reaching_into_the_title_band_is_laid_out_again() {
        let (dir, p) = project(&[]);
        let board = dir.path().join("board");
        let top = Id::parse("top", "block").unwrap();
        let laid = lay(
            &p,
            &p.conn.blocks[&top],
            None,
            &[],
            ryusch::input::Relayout::None,
        )
        .unwrap();
        let mut v: serde_json::Value =
            serde_json::from_str(&layout_text(&laid.outcome.layout)).unwrap();
        let key = v["placed"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        // The old A4 area's top (190 mm) less two steps: inside before the
        // band, past the 170 mm the band leaves.
        v["placed"][&key]["at"][1] = serde_json::json!(190_000_000 - 2 * GRID);
        crate::source::write_atomic(
            &board.join(layout_path(&top)),
            crate::canonical::to_string(&v),
        )
        .unwrap();
        let p = Project::load(&board).unwrap();
        let again = lay(
            &p,
            &p.conn.blocks[&top],
            None,
            &[],
            ryusch::input::Relayout::None,
        )
        .unwrap();
        let e = again.invalid.expect("the file breaks a rule");
        assert_eq!(e.code, Code::LayoutInvalid, "{e:?}");
        assert_eq!(
            ryusch::rules::check(&again.built.input, &again.outcome.layout),
            []
        );
    }
}
