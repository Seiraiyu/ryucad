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
pub const LAYOUT_FORMAT: u64 = 1;
/// What the PDF's border takes on every side of the paper.
pub const BORDER: Nm = 10_000_000;

/// The path of a block's layout file.
pub fn layout_path(block: &Id) -> String {
    crate::source::join(
        &crate::source::join(crate::conn::BLOCKS, block.as_str()),
        LAYOUT,
    )
}

/// Reads a layout file.
pub fn read_layout(bytes: &[u8], path: &str) -> Result<Layout> {
    doc::read(bytes, path, LAYOUT_KIND, LAYOUT_FORMAT).map_err(|e| {
        let code = if e.code == Code::FormatTooNew {
            e.code
        } else {
            Code::LayoutInvalid
        };
        Error::new(code, e.message.clone()).with("path", path)
    })
}

/// A layout file's canonical text.
pub fn layout_text(l: &Layout) -> String {
    doc::to_string(l, LAYOUT_KIND, LAYOUT_FORMAT)
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
pub struct Built {
    pub input: input::Block,
    pub of: Vec<Of>,
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
            }
        })
        .collect();
    (g, pins)
}

/// Builds block `b` of project `p` for ryusch, with no previous layout
/// and no instructions.
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
        nets.push(input::Net {
            key,
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
        });
    }
    let net_of = |n: Option<&Node>| n.and_then(node_key).and_then(|k| index.get(&k).copied());
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
        let refdes = c.refdes.clone().unwrap_or_else(|| format!("{prefix}?"));
        match lib.map(|(_, s)| s) {
            Some(sym) => {
                // Pins every unit has go on the unit with the power pins,
                // else the first.
                let home = sym
                    .pins
                    .values()
                    .find(|x| matches!(x.ty, PinType::PowerIn | PinType::PowerOut))
                    .and_then(|x| x.unit)
                    .unwrap_or(1);
                for unit in 1..=sym.units {
                    let key = if sym.units == 1 {
                        cid.to_string()
                    } else {
                        format!("{cid}:{unit}")
                    };
                    let pins: Vec<Pin> = sym
                        .pins
                        .iter()
                        .filter(|(_, x)| x.unit == Some(unit) || (x.unit.is_none() && unit == home))
                        .map(|(num, x)| {
                            let node = c.pins.get(num);
                            let shown = !(x.name.is_empty() || x.name == "~" || x.name == *num);
                            Pin {
                                number: num.clone(),
                                name: if shown { x.name.clone() } else { String::new() },
                                at: x.at,
                                side: side(x.side),
                                length: x.length_nm,
                                kind: pin_kind(x.ty),
                                net: net_of(node),
                                nc: node == Some(&Node::Nc),
                                show_number: sym.pins.len() > 2,
                            }
                        })
                        .collect();
                    let graphics = sym
                        .graphics
                        .values()
                        .filter(|g| g.unit().is_none_or(|u| u == unit))
                        .map(graphic)
                        .collect();
                    let label = if sym.units == 1 {
                        refdes.clone()
                    } else {
                        format!("{refdes}{}", crate::props::letter(u64::from(unit)))
                    };
                    parts.push(Part {
                        key,
                        kind: PartKind::Symbol {
                            prefix: prefix.clone(),
                        },
                        refdes: label,
                        value: value.clone(),
                        graphics,
                        pins,
                    });
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
    for (iid, i) in &b.instances {
        let Ok(i) = &i.item else { continue };
        let child = p
            .conn
            .blocks
            .get(&i.of)
            .and_then(|c| c.block.item.as_ref().ok());
        let name = child.map_or_else(|| i.of.to_string(), |c| c.name.clone());
        let ports: Vec<(String, String, Flow, Option<usize>)> = child
            .map(|c| {
                c.ports
                    .iter()
                    .map(|(pid, port)| {
                        (
                            pid.to_string(),
                            port.name.clone(),
                            flow(port.direction),
                            net_of(i.ports.get(pid)),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut part = ryusch::shape::block_symbol(&format!("instance:{iid}"), &name, &ports);
        part.value = iid.to_string();
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
    Ok(Built {
        input: input::Block {
            name: block.name.clone(),
            sheet: [w - 2 * BORDER, h - 2 * BORDER],
            parts,
            nets,
            ..input::Block::default()
        },
        of,
    })
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
    let mut built = build(p, b)?;
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
        let broken = ryusch::rules::check(&built.input, l);
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
    let outcome = ryusch::engine::lay_out(&built.input).map_err(|r| refusal(&b.id, r))?;
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
    fn blocks_build_with_units_instances_ports_and_rails() {
        let (_dir, p) = project(&[]);
        let top = p.conn.block(&Id::parse("top", "b").unwrap()).unwrap();
        let built = build(&p, top).unwrap();
        let keys: Vec<&str> = built.input.parts.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["r1", "instance:ch_a", "instance:ch_b"]);
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
}
