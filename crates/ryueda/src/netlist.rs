//! The flattened netlist of a design: every component at every instance
//! path, with its reference designator after channel rules and
//! overrides, and every net of every block occurrence joined through
//! ports to what its parent connects them to, and every rail. Derived
//! data, never written into the project.
//!
//! A design reads as its files are or not at all: a fault in any file it
//! reads (its blocks', the rails') refuses it, naming them. What the
//! library makes of a component (its pins' types, its pads) is read as
//! far as it resolves; ERC judges the rest.
//!
//! References: an occurrence is `<design>:<instance>/…/<component>`, a
//! pin `…/<component>/<pin>`; a flattened net `<design>:<instance>/…/<net>`
//! (or `…/port:<port>` for an edge no net holds) and a rail
//! `<design>:rail:<rail>`. A net's name is its highest node's: a rail's
//! name, or the instance path and the net's (or port's) name.

use crate::conn::{Direction, Node};
use crate::design::BlockFiles;
use crate::error::{Code, Error, Result};
use crate::fault::{self, Fault};
use crate::id::Id;
use crate::libcomp::LibRef;
use crate::library::ObjKind;
use crate::project::{Project, SELF, ref_text};
use crate::symbol::PinType;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::BTreeMap;

/// A block where a design uses it.
struct BlockOcc<'a> {
    /// Instance IDs from the root.
    path: Vec<Id>,
    files: &'a BlockFiles,
    /// The occurrence holding it, and the instance there.
    parent: Option<(usize, &'a crate::conn::Instance)>,
}

/// A block occurrence still to walk: its path, block and parent.
type Walk<'a> = (Vec<Id>, Id, Option<(usize, &'a crate::conn::Instance)>);

/// What a flattened net is, by its highest node.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Rail(Id),
    Net(usize, Id),
    Port(usize, Id),
}

/// A pin of a component occurrence.
#[derive(Clone, Debug, Serialize)]
pub struct PinOcc {
    pub number: String,
    /// From the symbol, when it resolves and has the pin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<PinType>,
    /// The pads it connects to on the component's footprint.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pads: Vec<String>,
    /// The flattened net's index, when on one.
    #[serde(skip)]
    pub net: Option<usize>,
    /// Marked not connected.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub nc: bool,
    /// Whether the symbol has the pin (`false` when the component names a
    /// pin its symbol lost, or the symbol doesn't resolve).
    #[serde(skip)]
    pub in_symbol: bool,
}

/// A component at an instance path.
#[derive(Clone, Debug, Serialize)]
pub struct Occurrence {
    pub reference: String,
    pub block: Id,
    pub component: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refdes: Option<String>,
    /// Why a format gave it none, when one couldn't.
    #[serde(skip)]
    pub refdes_error: Option<Error>,
    /// `lib/<alias>/libcomp/<id>`.
    pub of: String,
    /// `lib/<alias>/footprint/<id>`, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub pins: Vec<PinOcc>,
    /// Why the library doesn't give the component's pins, if it doesn't.
    #[serde(skip)]
    pub library: Option<Error>,
    /// The faults of its library component (pin maps and footprints).
    #[serde(skip)]
    pub libcomp_faults: Vec<Fault>,
    /// Whether it names a footprint its library component doesn't list.
    #[serde(skip)]
    pub footprint_unlisted: bool,
    #[serde(skip)]
    pub file: String,
}

/// A port a flattened net passes through.
#[derive(Clone, Debug, Serialize)]
pub struct PortAt {
    /// `<design>:<path>/port:<port>`.
    pub port: String,
    pub direction: Direction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voltage: Option<String>,
    /// Whether it is an edge of the design: a port of the root block.
    pub edge: bool,
}

/// A flattened net.
#[derive(Clone, Debug, Serialize)]
pub struct FlatNet {
    pub id: String,
    pub name: String,
    /// The rail, for a rail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rail: Option<Id>,
    /// Its pins, as `(occurrence index, pin index)`, sorted by reference.
    #[serde(skip)]
    pub members: Vec<(usize, usize)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<PortAt>,
    /// The voltages its net or rail and its ports declare, with where.
    #[serde(skip)]
    pub voltages: Vec<(String, String)>,
    /// The block net it is, with its pair, for a block net:
    /// `(block, net)`.
    #[serde(skip)]
    pub net: Option<(Id, Id)>,
}

/// A design's netlist.
#[derive(Clone, Debug)]
pub struct Netlist {
    pub design: Id,
    pub root: Id,
    pub occurrences: Vec<Occurrence>,
    pub nets: Vec<FlatNet>,
}

/// A design's file, sound, or why not.
pub fn design_root(p: &Project, design: &Id) -> Result<Id> {
    let d = p.conn.designs.get(design).ok_or_else(|| {
        Error::new(Code::NotFound, format!("there is no design {design}")).with("design", design)
    })?;
    let item = d
        .item
        .as_ref()
        .map_err(|e| e.clone().with("design", design))?;
    Ok(item.root.clone())
}

/// Every error in the files design `design` reads: its own, its blocks'
/// and the rails'.
pub fn faults(p: &Project, design: &Id) -> Vec<Fault> {
    let mut out: Vec<Fault> = Vec::new();
    if let Some(d) = p.conn.designs.get(design) {
        if let Err(e) = &d.item {
            out.push(Fault {
                severity: fault::Severity::Error,
                error: e.clone(),
            });
        }
        out.extend(d.faults.iter().cloned());
        if let Ok(d) = &d.item {
            for b in p.conn.reached(&d.root) {
                if let Some(files) = p.conn.blocks.get(b) {
                    out.extend(files.faults());
                }
            }
        }
    }
    for (id, r) in &p.conn.rails {
        if let Err(e) = &r.item {
            out.push(Fault {
                severity: fault::Severity::Error,
                error: e
                    .clone()
                    .with("path", crate::design::top_path(crate::conn::RAILS, id)),
            });
        }
        out.extend(r.faults.iter().cloned());
    }
    out.retain(Fault::is_error);
    out
}

/// An instance's place among the instances of its block in the block
/// holding it, in ID order: its index (from 1) and their count. A block
/// placed several times there is a stack, each instance a layer.
pub fn stack_of(parent: &BlockFiles, instance: &Id) -> (u64, u64) {
    let of = parent
        .instances
        .get(instance)
        .and_then(|i| i.item.as_ref().ok())
        .map(|i| &i.of);
    let mut index = 0;
    let mut count = 0;
    for (id, i) in &parent.instances {
        if i.item.as_ref().is_ok_and(|i| Some(&i.of) == of) {
            count += 1;
            if id == instance {
                index = count;
            }
        }
    }
    (index, count)
}

/// The levels whose properties a component at block occurrence `o`
/// reads, innermost first: its own parameters, then each instance and
/// its block out to the root block, then the project.
fn levels<'a>(
    p: &'a Project,
    occs: &[BlockOcc<'a>],
    o: usize,
    c: &'a crate::conn::Component,
) -> Vec<&'a crate::props::Properties> {
    let mut out = vec![&c.parameters];
    let mut q = Some(o);
    while let Some(at) = q {
        let occ = &occs[at];
        if let Some((_, inst)) = occ.parent {
            out.push(&inst.properties);
        }
        if let Ok(b) = &occ.files.block.item {
            out.push(&b.properties);
        }
        q = occ.parent.map(|(up, _)| up);
    }
    out.push(&p.manifest.properties);
    out
}

/// A component's designator under the formats and overrides of the
/// instances above it, innermost first: each instance's override for it
/// replaces what its format would give, and outer formats still apply.
/// Why it has none, when a format can't give one.
#[allow(clippy::too_many_arguments)]
fn refdes(
    p: &Project,
    design: &Id,
    occs: &[BlockOcc],
    o: usize,
    cid: &Id,
    c: &crate::conn::Component,
    value: Option<&str>,
) -> std::result::Result<Option<String>, Error> {
    let mut r = c.refdes.clone();
    let path = &occs[o].path;
    let levels = levels(p, occs, o, c);
    let design_name = p
        .conn
        .designs
        .get(design)
        .and_then(|d| d.item.as_ref().ok())
        .map_or_else(|| design.to_string(), |d| d.name.clone());
    let mut q = o;
    while let Some((up, inst)) = occs[q].parent {
        let depth = occs[q].path.len();
        let below: Vec<&str> = path[depth..]
            .iter()
            .map(Id::as_str)
            .chain(std::iter::once(cid.as_str()))
            .collect();
        if let Some(over) = inst.overrides.get(&below.join("/")) {
            r = Some(over.clone());
        } else if let (Some(f), Some(x)) = (&inst.refdes, &r) {
            let mut built_in = std::collections::BTreeMap::new();
            crate::props::designator(x, &mut built_in);
            built_in.insert("design", design_name.clone());
            if let Some(v) = value {
                built_in.insert("value", v.to_owned());
            }
            if let Ok(b) = &occs[q].files.block.item {
                built_in.insert("block", b.name.clone());
            }
            let iid = &occs[q].path[depth - 1];
            let (index, count) = stack_of(occs[up].files, iid);
            built_in.insert("stack_index", index.to_string());
            built_in.insert("stack_letter", crate::props::letter(index));
            built_in.insert("stack_count", count.to_string());
            built_in.insert("stack_path", stack_path(occs, q));
            let lookup = crate::props::Levels {
                levels: levels.clone(),
                built_in,
            };
            let text = crate::props::Template::parse(&f.0)?.render(&|n| lookup.get(n))?;
            if !crate::conn::refdes_ok(&text) {
                return Err(Error::new(
                    Code::RefdesFormatInvalid,
                    format!("{} gives {text:?} for {x}, not a reference designator", f.0),
                )
                .with("format", &f.0));
            }
            r = Some(text);
        }
        q = up;
    }
    Ok(r)
}

/// The layers from the root down to block occurrence `q`: each stacked
/// level's letter (a level of one instance has none), joined by `/`.
fn stack_path(occs: &[BlockOcc], q: usize) -> String {
    let mut letters = Vec::new();
    let mut at = q;
    while let Some((up, _)) = occs[at].parent {
        let iid = occs[at]
            .path
            .last()
            .expect("a placed occurrence has a path");
        let (index, count) = stack_of(occs[up].files, iid);
        if count > 1 {
            letters.push(crate::props::letter(index));
        }
        at = up;
    }
    letters.reverse();
    letters.join("/")
}

/// `<design>:` and the instance path, each segment followed by `/`.
fn prefix(design: &Id, path: &[Id]) -> String {
    let mut s = format!("{design}:");
    for i in path {
        s.push_str(i.as_str());
        s.push('/');
    }
    s
}

/// The names of a path's instances, joined, with a trailing `/`.
fn name_prefix(path: &[Id]) -> String {
    path.iter().map(|i| format!("{i}/")).collect()
}

/// Flattens design `design`. Refused (with the first fault's code and
/// every fault in the details) when any file it reads has an error.
pub fn flatten(p: &Project, design: &Id) -> Result<Netlist> {
    let root = design_root(p, design)?;
    let errors = faults(p, design);
    if !errors.is_empty() {
        let first = &errors[0].error;
        return Err(Error::new(
            first.code,
            format!(
                "design {design} reads {} file fault(s); `ryueda check` lists them: {}",
                errors.len(),
                first.message
            ),
        )
        .with("design", design)
        .with("faults", fault::list(&errors)));
    }
    // Every block occurrence, depth first, instances in ID order.
    let mut occs: Vec<BlockOcc> = Vec::new();
    let mut stack: Vec<Walk> = vec![(Vec::new(), root.clone(), None)];
    while let Some((path, block, parent)) = stack.pop() {
        let files = p.conn.block(&block)?;
        let at = occs.len();
        if path.len() > 64 {
            return Err(Error::new(
                Code::BlockCycle,
                format!("design {design} nests blocks more than 64 deep"),
            ));
        }
        let children: Vec<Walk> = files
            .instances
            .iter()
            .rev()
            .filter_map(|(iid, i)| {
                let i = i.item.as_ref().ok()?;
                let mut path: Vec<Id> = path.clone();
                path.push(iid.clone());
                Some((path, i.of.clone(), Some((at, i))))
            })
            .collect();
        occs.push(BlockOcc {
            path,
            files,
            parent,
        });
        stack.extend(children);
    }
    // Each node's highest node: a port climbs to what its instance maps
    // it to, unless that is nothing or `nc`.
    let key = |o: usize, n: &Node| -> Option<Key> {
        let (mut o, mut n) = (o, n.clone());
        loop {
            return match n {
                Node::Nc => None,
                Node::Rail(r) => Some(Key::Rail(r)),
                Node::Net(id) => Some(Key::Net(o, id)),
                Node::Port(port) => match occs[o].parent {
                    Some((up, inst)) => match inst.ports.get(&port) {
                        Some(Node::Nc) | None => Some(Key::Port(o, port)),
                        Some(m) => {
                            (o, n) = (up, m.clone());
                            continue;
                        }
                    },
                    None => Some(Key::Port(o, port)),
                },
            };
        }
    };
    // Components, by block occurrence in parallel, in order.
    let comps: Vec<Vec<(Occurrence, Vec<Option<Key>>)>> = occs
        .par_iter()
        .enumerate()
        .map(|(o, occ)| {
            occ.files
                .components
                .iter()
                .filter_map(|(cid, c)| Some((cid, c.item.as_ref().ok()?)))
                .map(|(cid, c)| {
                    let (occurrence, keys) = component(p, design, &occs, o, cid, c, &key);
                    (occurrence, keys)
                })
                .collect()
        })
        .collect();
    let mut occurrences = Vec::new();
    let mut pin_keys: Vec<Vec<Option<Key>>> = Vec::new();
    for list in comps {
        for (occ, keys) in list {
            occurrences.push(occ);
            pin_keys.push(keys);
        }
    }
    // The flattened nets: every rail, every block net of every
    // occurrence, and every edge port pins reach.
    let mut nets: BTreeMap<Key, FlatNet> = BTreeMap::new();
    for (rid, r) in &p.conn.rails {
        let Ok(r) = &r.item else { continue };
        nets.insert(
            Key::Rail(rid.clone()),
            FlatNet {
                id: format!("{design}:rail:{rid}"),
                name: r.name.clone(),
                rail: Some(rid.clone()),
                members: Vec::new(),
                ports: Vec::new(),
                voltages: r
                    .voltage
                    .iter()
                    .map(|v| (v.clone(), format!("rail/{rid}")))
                    .collect(),
                net: None,
            },
        );
    }
    for (o, occ) in occs.iter().enumerate() {
        for (nid, n) in &occ.files.nets {
            let Ok(n) = &n.item else { continue };
            nets.insert(
                Key::Net(o, nid.clone()),
                FlatNet {
                    id: format!("{}{nid}", prefix(design, &occ.path)),
                    name: format!("{}{}", name_prefix(&occ.path), n.name),
                    rail: None,
                    members: Vec::new(),
                    ports: Vec::new(),
                    voltages: n
                        .voltage
                        .iter()
                        .map(|v| (v.clone(), format!("{}/{nid}", occ.files.id)))
                        .collect(),
                    net: Some((occ.files.id.clone(), nid.clone())),
                },
            );
        }
    }
    for (c, keys) in pin_keys.iter().enumerate() {
        for (i, k) in keys.iter().enumerate() {
            let Some(k) = k else { continue };
            let net = nets.entry(k.clone()).or_insert_with(|| {
                let Key::Port(o, port) = k else {
                    unreachable!("rails and nets are listed")
                };
                let occ = &occs[*o];
                let name = occ
                    .files
                    .block
                    .item
                    .as_ref()
                    .ok()
                    .and_then(|b| b.ports.get(port))
                    .map_or(port.to_string(), |x| x.name.clone());
                FlatNet {
                    id: format!("{}port:{port}", prefix(design, &occ.path)),
                    name: format!("{}{name}", name_prefix(&occ.path)),
                    rail: None,
                    members: Vec::new(),
                    ports: Vec::new(),
                    voltages: Vec::new(),
                    net: None,
                }
            });
            net.members.push((c, i));
        }
    }
    // Each port a net passes through: every port of every occurrence
    // that reaches one.
    for (o, occ) in occs.iter().enumerate() {
        let Ok(block) = &occ.files.block.item else {
            continue;
        };
        for (pid, port) in &block.ports {
            let Some(k) = key(o, &Node::Port(pid.clone())) else {
                continue;
            };
            let Some(net) = nets.get_mut(&k) else {
                continue;
            };
            let at = format!("{}port:{pid}", prefix(design, &occ.path));
            if let Some(v) = &port.voltage {
                net.voltages.push((v.clone(), at.clone()));
            }
            net.ports.push(PortAt {
                port: at,
                direction: port.direction,
                voltage: port.voltage.clone(),
                edge: o == 0,
            });
        }
    }
    let mut nets: Vec<(Key, FlatNet)> = nets.into_iter().collect();
    nets.sort_by(|a, b| a.1.name.cmp(&b.1.name).then_with(|| a.1.id.cmp(&b.1.id)));
    let index: BTreeMap<&Key, usize> = nets.iter().enumerate().map(|(i, (k, _))| (k, i)).collect();
    for (c, keys) in pin_keys.iter().enumerate() {
        for (i, k) in keys.iter().enumerate() {
            occurrences[c].pins[i].net = k.as_ref().map(|k| index[k]);
        }
    }
    let mut nets: Vec<FlatNet> = nets.into_iter().map(|(_, n)| n).collect();
    for n in &mut nets {
        n.members.sort_by(|a, b| {
            let r = |(c, i): (usize, usize)| {
                (
                    occurrences[c].reference.as_str(),
                    occurrences[c].pins[i].number.as_str(),
                )
            };
            r(*a).cmp(&r(*b))
        });
        n.ports.sort_by(|a, b| a.port.cmp(&b.port));
        n.voltages.sort();
    }
    Ok(Netlist {
        design: design.clone(),
        root,
        occurrences,
        nets,
    })
}

/// A component at block occurrence `o`: its occurrence, and each of its
/// pins' highest node.
fn component(
    p: &Project,
    design: &Id,
    occs: &[BlockOcc],
    o: usize,
    cid: &Id,
    c: &crate::conn::Component,
    key: &(dyn Fn(usize, &Node) -> Option<Key> + Sync),
) -> (Occurrence, Vec<Option<Key>>) {
    let alias = c.of.library.as_ref().map_or(SELF, Id::as_str);
    let lc = p.libcomp(SELF, &c.of).ok();
    let symbol = lc.map(|lc| p.symbol(alias, &lc.symbol));
    let libcomp_faults = p
        .library(alias)
        .ok()
        .and_then(|l| l.components.get(&c.of.id))
        .map(|l| l.faults.clone())
        .unwrap_or_default();
    // The footprint taken: the one named, else the library component's
    // first.
    let uses = lc.map(|lc| lc.footprints.as_slice()).unwrap_or_default();
    let chosen = match &c.footprint {
        Some(f) => uses.iter().find(|u| &u.footprint == f),
        None => uses.first(),
    };
    let fp_text = |f: &LibRef| {
        let a = f.library.as_ref().map_or(alias, Id::as_str);
        ref_text(a, ObjKind::Footprint, &f.id)
    };
    let mut pins: Vec<PinOcc> = Vec::new();
    let mut keys = Vec::new();
    let symbol_pins = symbol
        .as_ref()
        .and_then(|s| s.as_ref().ok())
        .map(|s| &s.pins);
    let mut numbers: Vec<&String> = symbol_pins.map(|s| s.keys().collect()).unwrap_or_default();
    for n in c.pins.keys() {
        if !numbers.contains(&n) {
            numbers.push(n);
        }
    }
    numbers.sort();
    for n in numbers {
        let sp = symbol_pins.and_then(|s| s.get(n));
        let node = c.pins.get(n);
        keys.push(node.and_then(|x| key(o, x)));
        pins.push(PinOcc {
            number: n.clone(),
            name: sp.map(|x| x.name.clone()),
            ty: sp.map(|x| x.ty),
            pads: chosen
                .map(|u| u.pads_of(n).into_iter().map(str::to_owned).collect())
                .unwrap_or_default(),
            net: None,
            nc: node == Some(&Node::Nc),
            in_symbol: sp.is_some(),
        });
    }
    let library = match (lc, symbol) {
        (None, _) => Some(Error::new(
            Code::NotFound,
            "the library component doesn't resolve",
        )),
        (_, Some(Err(e))) => Some(e),
        _ => None,
    };
    let occ = &occs[o];
    let reference = format!("{}{cid}", prefix(design, &occ.path));
    let value = c
        .value
        .clone()
        .or_else(|| lc.map(|l| l.value.clone()).filter(|v| !v.is_empty()));
    let (refdes, refdes_error) = match refdes(p, design, occs, o, cid, c, value.as_deref()) {
        Ok(r) => (r, None),
        Err(e) => (None, Some(e)),
    };
    let occurrence = Occurrence {
        reference,
        block: occ.files.id.clone(),
        component: cid.clone(),
        refdes,
        refdes_error,
        of: ref_text(alias, ObjKind::Libcomp, &c.of.id),
        footprint: chosen.map(|u| fp_text(&u.footprint)),
        value,
        pins,
        library,
        libcomp_faults,
        footprint_unlisted: c.footprint.is_some() && chosen.is_none(),
        file: crate::design::part_path(&occ.files.id, crate::design::Part::Component, cid),
    };
    (occurrence, keys)
}

impl Netlist {
    /// A pin's reference.
    pub fn pin_ref(&self, c: usize, i: usize) -> String {
        format!(
            "{}/{}",
            self.occurrences[c].reference, self.occurrences[c].pins[i].number
        )
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        let occurrences: Vec<serde_json::Value> = self
            .occurrences
            .iter()
            .map(|o| {
                let mut v = json!(o);
                for (i, pin) in o.pins.iter().enumerate() {
                    if let Some(n) = pin.net {
                        v["pins"][i]["net"] = json!(self.nets[n].id);
                    }
                }
                v
            })
            .collect();
        let nets: Vec<serde_json::Value> = self
            .nets
            .iter()
            .map(|n| {
                let mut v = json!(n);
                v["members"] = n
                    .members
                    .iter()
                    .map(|&(c, i)| {
                        let o = &self.occurrences[c];
                        json!({
                            "pin": self.pin_ref(c, i),
                            "refdes": o.refdes,
                            "number": o.pins[i].number,
                            "pads": o.pins[i].pads,
                        })
                    })
                    .collect();
                v
            })
            .collect();
        json!({
            "design": self.design,
            "root": self.root,
            "occurrences": occurrences,
            "nets": nets,
        })
    }
}

/// `netlist`: a design's flattened netlist (the project's one design
/// when none is named).
pub fn netlist(p: &Project, design: Option<&str>) -> Result<serde_json::Value> {
    let id = one_design(p, design)?;
    Ok(flatten(p, &id)?.to_json())
}

/// The design a request names (by ID or name), or the project's only one.
pub fn one_design(p: &Project, design: Option<&str>) -> Result<Id> {
    match design {
        Some(text) => {
            let by_id = p.conn.designs.keys().find(|k| k.as_str() == text);
            let by_name = || {
                let mut hits = p
                    .conn
                    .designs
                    .iter()
                    .filter(|(_, d)| d.item.as_ref().is_ok_and(|d| d.name == text));
                let first = hits.next().map(|(id, _)| id);
                first.filter(|_| hits.next().is_none())
            };
            by_id.or_else(by_name).cloned().ok_or_else(|| {
                Error::new(Code::NotFound, format!("there is no design {text}"))
                    .with("design", text)
            })
        }
        None => match p.conn.designs.keys().collect::<Vec<_>>().as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(Error::new(
                Code::NotFound,
                "the project has no design; `ryueda design create` makes one",
            )),
            many => Err(Error::new(
                Code::InvalidParam,
                format!("the project has {} designs: name one", many.len()),
            )
            .with("designs", many)),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::{design_in, library_in, manifest};
    use crate::source::write_atomic;
    use serde_json::json;

    /// A project holding the test library and design, with `extra` files.
    pub fn project(extra: &[(&str, serde_json::Value)]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        for (path, v) in extra {
            write_atomic(&board.join(path), crate::canonical::to_string(v)).unwrap();
        }
        let p = Project::load(&board).unwrap();
        (dir, p)
    }

    fn id(s: &str) -> Id {
        Id::parse(s, "test").unwrap()
    }

    #[test]
    fn a_design_flattens_through_its_instances() {
        let (_dir, p) = project(&[]);
        let n = flatten(&p, &id("d")).unwrap();
        let refs: Vec<(&str, Option<&str>)> = n
            .occurrences
            .iter()
            .map(|o| (o.reference.as_str(), o.refdes.as_deref()))
            .collect();
        assert_eq!(
            refs,
            [
                ("d:r1", Some("R1")),
                ("d:ch_a/u1", Some("U1_A")),
                ("d:ch_b/u1", Some("U1_B")),
            ]
        );
        let names: Vec<(&str, &str)> = n
            .nets
            .iter()
            .map(|x| (x.name.as_str(), x.id.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("3V3", "d:rail:v3v3"),
                ("GND", "d:rail:gnd"),
                ("SDA", "d:sda")
            ]
        );
        let v = n.to_json();
        let sda = &v["nets"][2];
        let pins: Vec<&serde_json::Value> = sda["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| &m["pin"])
            .collect();
        assert_eq!(
            pins,
            [
                &json!("d:ch_a/u1/1"),
                &json!("d:ch_b/u1/1"),
                &json!("d:r1/1")
            ]
        );
        assert_eq!(sda["members"][2]["pads"], json!(["1"]));
        assert_eq!(sda["ports"][0]["port"], "d:ch_a/port:sda");
        assert_eq!(sda["ports"][0]["edge"], false);
        assert_eq!(v["occurrences"][0]["pins"][1]["net"], "d:rail:v3v3");
        assert_eq!(
            v["occurrences"][0]["footprint"],
            "lib/self/footprint/r_0402_1005metric"
        );
        assert_eq!(v["occurrences"][0]["value"], "100k");
        assert_eq!(
            n.nets[0].voltages,
            [("3.3 V".to_owned(), "rail/v3v3".to_owned())]
        );
    }

    #[test]
    fn channel_rules_compose_outward_and_overrides_replace() {
        let mut ch_a = json!({"kind": "ryueda.instance", "format": 1, "of": "s", "ports": {"sda": "sda"},
            "refdes": {"rule": "offset", "value": 100}, "overrides": {"u1": "U7"}});
        let (_dir, p) = project(&[("blocks/top/instances/ch_a.json", ch_a.clone())]);
        let n = flatten(&p, &id("d")).unwrap();
        assert_eq!(n.occurrences[1].refdes.as_deref(), Some("U7"));
        ch_a["overrides"] = json!({});
        let (_dir, p) = project(&[("blocks/top/instances/ch_a.json", ch_a)]);
        let n = flatten(&p, &id("d")).unwrap();
        assert_eq!(n.occurrences[1].refdes.as_deref(), Some("U101"));
        // Nested: the inner rule first, then the outer, as phase 0 gave.
        let (_dir, p) = project(&[
            (
                "blocks/top/instances/ch_a.json",
                json!({"kind": "ryueda.instance", "format": 1, "of": "m",
                    "refdes": {"rule": "suffix", "value": "_A"}}),
            ),
            (
                "blocks/m/block.json",
                json!({"kind": "ryueda.block", "format": 1, "name": "Mid"}),
            ),
            (
                "blocks/m/instances/x.json",
                json!({"kind": "ryueda.instance", "format": 1, "of": "s",
                    "refdes": {"rule": "offset", "value": 100}}),
            ),
        ]);
        let n = flatten(&p, &id("d")).unwrap();
        let r = |reference: &str| {
            n.occurrences
                .iter()
                .find(|o| o.reference == reference)
                .and_then(|o| o.refdes.clone())
        };
        assert_eq!(r("d:ch_a/x/u1").as_deref(), Some("U101_A"));
    }

    #[test]
    fn formats_read_properties_from_every_level_and_stack_layers() {
        let inst = |of: &str, refdes: &str| json!({"kind": "ryueda.instance", "format": 2, "of": of, "refdes": refdes});
        let (_dir, p) = project(&[
            (
                "blocks/top/instances/ch_a.json",
                inst("s", "{prefix}{number + base * stack_index}{rest}"),
            ),
            (
                "blocks/s/block.json",
                json!({"kind": "ryueda.block", "format": 2, "name": "Sensor",
                    "ports": {"sda": {"name": "SDA", "direction": "bidirectional"}},
                    "properties": {"base": "100"}}),
            ),
        ]);
        let n = flatten(&p, &id("d")).unwrap();
        let r = |reference: &str| {
            let o = n
                .occurrences
                .iter()
                .find(|o| o.reference == reference)
                .unwrap();
            (o.refdes.clone(), o.refdes_error.as_ref().map(|e| e.code))
        };
        assert_eq!(r("d:ch_a/u1"), (Some("U101".into()), None));
        assert_eq!(r("d:ch_b/u1"), (Some("U1_B".into()), None));
        // A format naming no property leaves no designator, and says why.
        let (_dir, p) = project(&[(
            "blocks/top/instances/ch_b.json",
            inst("s", "{refdes}_{stack_letter}{missing}"),
        )]);
        let n = flatten(&p, &id("d")).unwrap();
        let o = n
            .occurrences
            .iter()
            .find(|o| o.reference == "d:ch_b/u1")
            .unwrap();
        assert_eq!(
            (o.refdes.as_deref(), o.refdes_error.as_ref().map(|e| e.code)),
            (None, Some(Code::PropertyUnknown))
        );
        let (_dir, p) = project(&[(
            "blocks/top/instances/ch_b.json",
            inst("s", "{refdes}_{stack_letter}{stack_count}"),
        )]);
        let n = flatten(&p, &id("d")).unwrap();
        let o = n
            .occurrences
            .iter()
            .find(|o| o.reference == "d:ch_b/u1")
            .unwrap();
        assert_eq!(o.refdes.as_deref(), Some("U1_B2"));
    }

    #[test]
    fn edges_and_unmapped_ports_are_nets_of_their_own() {
        // The root has an edge port pins reach; a sensor's port left
        // unmapped is its own net.
        let top = json!({"kind": "ryueda.block", "format": 1, "name": "Top",
            "ports": {"vin": {"name": "VIN", "direction": "power", "voltage": "5 V"}}});
        let r1 = json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_100k"}, "refdes": "R1",
            "pins": {"1": {"port": "vin"}, "2": "nc"}});
        let ch_b = json!({"kind": "ryueda.instance", "format": 1, "of": "s"});
        let (_dir, p) = project(&[
            ("blocks/top/block.json", top),
            ("blocks/top/components/r1.json", r1),
            ("blocks/top/instances/ch_b.json", ch_b),
        ]);
        let n = flatten(&p, &id("d")).unwrap();
        let ids: Vec<&str> = n.nets.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "d:rail:v3v3",
                "d:rail:gnd",
                "d:sda",
                "d:port:vin",
                "d:ch_b/port:sda"
            ]
        );
        let vin = &n.nets[3];
        assert_eq!((vin.name.as_str(), vin.ports[0].edge), ("VIN", true));
        assert_eq!(vin.voltages, [("5 V".to_owned(), "d:port:vin".to_owned())]);
        assert_eq!(n.nets[4].name, "ch_b/SDA");
        assert!(n.occurrences[0].pins[1].nc);
    }

    #[test]
    fn a_design_reading_a_broken_file_is_refused() {
        let bad = json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_100k"}, "pins": {"1": "gone"}});
        let (_dir, p) = project(&[("blocks/s/components/u2.json", bad)]);
        let e = flatten(&p, &id("d")).unwrap_err();
        assert_eq!(e.code, Code::NetNotFound);
        assert_eq!(
            e.details["faults"][0]["details"]["path"],
            "blocks/s/components/u2.json"
        );
        assert_eq!(flatten(&p, &id("nope")).unwrap_err().code, Code::NotFound);
        assert_eq!(one_design(&p, None).unwrap(), id("d"));
        assert_eq!(one_design(&p, Some("Breakout")).unwrap(), id("d"));
    }

    #[test]
    fn one_thread_and_eight_flatten_alike() {
        let (_dir, p) = project(&[]);
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| flatten(&p, &id("d")).unwrap().to_json())
        };
        assert_eq!(run(1), run(8));
    }
}
