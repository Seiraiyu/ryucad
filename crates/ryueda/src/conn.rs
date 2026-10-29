//! Connectivity's files: a design's blocks, what they hold, and the
//! project-wide rails, net classes and designs.
//!
//! - `designs/<id>.json` (kind `ryueda.design`): a name and a root block.
//! - `blocks/<id>/block.json` (kind `ryueda.block`): a functional block's
//!   name, description, ports and buses. Inside its folder:
//!   `components/<id>.json` (`ryueda.component`), `nets/<id>.json`
//!   (`ryueda.net`) and `instances/<id>.json` (`ryueda.instance`).
//! - `rails/<id>.json` (`ryueda.rail`): project-wide nets.
//! - `classes/<id>.json` (`ryueda.class`): net classes.
//!
//! A pin names its net in its component's file, so a net is the set of
//! pins naming it and has no list of its own to conflict on. What one file
//! alone can be checked for is here; what needs others (a net of the
//! block, a cycle) is `design`'s.

use crate::error::Code;
use crate::fault::Fault;
use crate::id::Id;
use crate::libcomp::LibRef;
use crate::props::Properties;
use crate::symbol::{pin_number_ok, pointer_escape};
use crate::units::{Dim, Quantity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DESIGNS: &str = "designs";
pub const BLOCKS: &str = "blocks";
pub const RAILS: &str = "rails";
pub const CLASSES: &str = "classes";
/// A block's own file, in its folder.
pub const BLOCK_FILE: &str = "block.json";
pub const COMPONENTS: &str = "components";
pub const NETS: &str = "nets";
pub const INSTANCES: &str = "instances";

pub const DESIGN_KIND: &str = "ryueda.design";
pub const BLOCK_KIND: &str = "ryueda.block";
pub const COMPONENT_KIND: &str = "ryueda.component";
pub const NET_KIND: &str = "ryueda.net";
pub const INSTANCE_KIND: &str = "ryueda.instance";
pub const RAIL_KIND: &str = "ryueda.rail";
pub const CLASS_KIND: &str = "ryueda.class";
/// Connectivity files are at format 1 but for blocks and instances with
/// properties or a designator format, which are format 2.
pub const FORMAT: u64 = 1;
/// The newest format of blocks and instances.
pub const FORMAT_2: u64 = 2;

/// The newest format of a file of `kind` this build reads.
pub fn newest(kind: &str) -> u64 {
    match kind {
        BLOCK_KIND | INSTANCE_KIND => FORMAT_2,
        _ => FORMAT,
    }
}

/// The format a file of `kind` is written at: 2 for a block or an
/// instance holding properties or a designator format, else 1, so files
/// phase 0 could write stay readable by it.
pub fn format_of(kind: &str, v: &serde_json::Value) -> u64 {
    let newer = match kind {
        BLOCK_KIND => v.get("properties").is_some(),
        INSTANCE_KIND => v.get("properties").is_some() || v.get("refdes").is_some(),
        _ => false,
    };
    if newer { FORMAT_2 } else { FORMAT }
}

/// A design: what a board (stage 5) is made from. Flattening starts at
/// its root block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Design {
    pub name: String,
    pub root: Id,
}

/// Which way a port carries what it carries, for the grammar to read
/// flow by and ERC to know what drives a net.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    In,
    Out,
    Bidirectional,
    Power,
    Passive,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    pub name: String,
    pub direction: Direction,
    /// What the block expects on it (`"3.3 V"`): ERC's `intent_conflict`
    /// compares it with the net or rail its instance connects it to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<String>,
}

/// Nets the grammar draws as one: membership only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bus {
    pub name: String,
    pub nets: Vec<Id>,
}

/// A functional block: one per sheet from stage 2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ports: BTreeMap<Id, Port>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buses: BTreeMap<Id, Bus>,
    /// Defaults for every placement of the block (format 2).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: Properties,
}

/// What a pin, or an instance's port, connects to in its block: a net of
/// the block (its ID), a rail, a port of the block, or nothing, marked so
/// (`"nc"`). In files: `"<net>"`, `{"rail": id}`, `{"port": id}`, `"nc"`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "NodeRepr", into = "NodeRepr")]
pub enum Node {
    Net(Id),
    Rail(Id),
    Port(Id),
    Nc,
}

/// How a node is written.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum NodeRepr {
    Text(String),
    Rail(RailRepr),
    Port(PortRepr),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RailRepr {
    rail: Id,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortRepr {
    port: Id,
}

/// The text a file writes for a pin marked not connected. No net may
/// take it as its ID.
pub const NC: &str = "nc";

impl TryFrom<NodeRepr> for Node {
    type Error = String;

    fn try_from(r: NodeRepr) -> Result<Node, String> {
        match r {
            NodeRepr::Text(t) if t == NC => Ok(Node::Nc),
            NodeRepr::Text(t) => Id::parse(&t, "net").map(Node::Net).map_err(|e| e.message),
            NodeRepr::Rail(r) => Ok(Node::Rail(r.rail)),
            NodeRepr::Port(p) => Ok(Node::Port(p.port)),
        }
    }
}

impl From<Node> for NodeRepr {
    fn from(n: Node) -> NodeRepr {
        match n {
            Node::Net(id) => NodeRepr::Text(id.to_string()),
            Node::Rail(rail) => NodeRepr::Rail(RailRepr { rail }),
            Node::Port(port) => NodeRepr::Port(PortRepr { port }),
            Node::Nc => NodeRepr::Text(NC.to_owned()),
        }
    }
}

/// A component placed in a design: a library component, its reference
/// designator and what each of its pins connects to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    /// A library component, from this project's point of view: no
    /// `library` for its own.
    pub of: LibRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refdes: Option<String>,
    /// One of the library component's footprints, as it lists it; its
    /// first when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprint: Option<LibRef>,
    /// Over the library component's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Merged over the library component's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
    /// By pin number. A pin left out is unconnected.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pins: BTreeMap<String, Node>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    P,
    N,
}

/// One half of a differential pair: the other net, and which half this
/// is. Both nets name each other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub with: Id,
    pub polarity: Polarity,
}

/// A net of a block: its name (unique among the block's nets and ports)
/// and its intent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Net {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<Id>,
    /// Quantities, as written: `"3.3 V"`, `"500 mA"`, `"90 Ohm"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<Pair>,
}

/// A project-wide net (`GND`, `3V3`): any component in any block connects
/// to it by ID, so power crosses the hierarchy without a port each.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rail {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<String>,
}

/// A net class: a label intent names. Its rules arrive in stage 7.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Class {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Phase 0's rule for an instance's designators (format 1): read as the
/// format that gives the same designators, and taken from the CLI's
/// `--suffix` and `--offset`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", content = "value", rename_all = "snake_case")]
pub enum Channel {
    Suffix(String),
    Offset(u32),
}

impl Channel {
    /// The format giving what the rule gave: `R5` with suffix `_A` is
    /// `R5_A`; with offset 100, `R105`; the rest of a designator kept.
    pub fn format(&self) -> String {
        match self {
            Channel::Suffix(s) => format!("{{refdes}}{s}"),
            Channel::Offset(n) => format!("{{prefix}}{{number + {n}}}{{rest}}"),
        }
    }
}

/// An instance's designator format, as written (a preset expanded); a
/// format 1 file's rule reads as its format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RefdesRepr", into = "String")]
pub struct RefdesFormat(pub String);

#[derive(Deserialize)]
#[serde(untagged)]
enum RefdesRepr {
    Text(String),
    Rule(Channel),
}

impl TryFrom<RefdesRepr> for RefdesFormat {
    type Error = String;

    fn try_from(r: RefdesRepr) -> Result<RefdesFormat, String> {
        Ok(RefdesFormat(match r {
            RefdesRepr::Text(t) => t,
            RefdesRepr::Rule(c) => c.format(),
        }))
    }
}

impl From<RefdesFormat> for String {
    fn from(f: RefdesFormat) -> String {
        f.0
    }
}

/// A block used inside another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    /// The block placed.
    pub of: Id,
    /// Each of its ports' node, in this block's terms. A port left out
    /// connects to nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ports: BTreeMap<Id, Node>,
    /// How its occurrences' designators are named, from the innermost
    /// instance outward (format 2; see `props`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refdes: Option<RefdesFormat>,
    /// This placement's own properties (format 2).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: Properties,
    /// By occurrence path below this instance (`<component>`, or
    /// `<instance>/…/<component>`): the reference designator this instance
    /// gives it, in place of its rule's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, String>,
}

/// Whether `name` can name a net, a port, a rail, a bus or a class: one
/// to 128 characters, none of them space, a control or `/` (flattened
/// names join names with `/`).
pub fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 128
        && name
            .chars()
            .all(|c| !(c.is_whitespace() || c.is_control() || c == '/'))
}

/// Whether `text` is a reference designator: a letter, then letters,
/// digits and `_`, at most 32 in all.
pub fn refdes_ok(text: &str) -> bool {
    let b = text.as_bytes();
    b.first().is_some_and(u8::is_ascii_alphabetic)
        && b.len() <= 32
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// A reference designator's parts: its leading letters, the number after
/// them (0 when it has none) and the rest (`R5_A` is `R`, 5, `_A`).
pub fn refdes_parts(text: &str) -> (&str, u64, &str) {
    let letters = text
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (prefix, rest) = text.split_at(letters);
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (number, rest) = rest.split_at(digits);
    // More digits than a u64 holds read as none, so they stay in the rest.
    match number.parse() {
        Ok(n) => (prefix, n, rest),
        Err(_) => (prefix, 0, &text[letters..]),
    }
}

fn bad(code: Code, ptr: String, msg: String) -> Fault {
    Fault::error(code, msg).with("pointer", ptr)
}

fn name_fault(name: &str, ptr: &str) -> Option<Fault> {
    (!name_ok(name)).then(|| {
        bad(
            Code::InvalidDocument,
            ptr.to_owned(),
            format!("{name:?} can't be a name: 1 to 128 characters, no spaces or /"),
        )
    })
}

/// The dimension each intent field holds.
pub const INTENT: [(&str, Dim); 3] = [
    ("voltage", Dim::VOLTAGE),
    ("current", Dim::CURRENT),
    ("impedance", Dim::RESISTANCE),
];

/// What a net's or rail's intent fields find: each a quantity of its
/// dimension.
fn intent_faults(fields: [&Option<String>; 3]) -> Vec<Fault> {
    let mut out = Vec::new();
    for ((field, dim), text) in INTENT.into_iter().zip(fields) {
        if let Some(t) = text
            && let Err(e) = Quantity::electrical(t, dim)
        {
            out.push(bad(Code::InvalidQuantity, format!("/{field}"), e.message));
        }
    }
    out
}

/// An electrical quantity's exact value, `(mantissa, power of ten)` with
/// no trailing zeros in the mantissa, so `3.3 V` and `3300 mV` compare
/// equal with no floating point. `None` for text that isn't a quantity of
/// `dim`, or holds more than 27 significant digits.
pub fn exact_value(text: &str, dim: Dim) -> Option<(i128, i32)> {
    let canonical = Quantity::electrical(text, dim).ok()?.text();
    let (number, unit) = canonical.split_once(' ')?;
    let (_, _, symbol) = Dim::ELECTRICAL.iter().find(|(_, d, _)| *d == dim)?;
    let prefix = match unit.strip_suffix(symbol)? {
        "p" => -12,
        "n" => -9,
        "u" => -6,
        "m" => -3,
        "" => 0,
        "k" => 3,
        "M" => 6,
        "G" => 9,
        _ => return None,
    };
    let (digits, exp) = match number.split_once(['e', 'E']) {
        Some((d, e)) => (d, e.parse::<i32>().ok()?),
        None => (number, 0),
    };
    let (negative, digits) = match digits.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, digits.strip_prefix('+').unwrap_or(digits)),
    };
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let all = format!("{whole}{frac}");
    let all = all.trim_start_matches('0');
    if all.len() > 27 {
        return None;
    }
    let mut mantissa: i128 = if all.is_empty() { 0 } else { all.parse().ok()? };
    let mut exp = exp - i32::try_from(frac.len()).ok()? + prefix;
    if mantissa == 0 {
        return Some((0, 0));
    }
    while mantissa % 10 == 0 {
        mantissa /= 10;
        exp += 1;
    }
    Some((if negative { -mantissa } else { mantissa }, exp))
}

impl Design {
    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        if self.name.trim().is_empty() {
            out.push(bad(
                Code::InvalidDocument,
                "/name".into(),
                "a design has a name".into(),
            ));
        }
        out
    }
}

impl Block {
    /// Names well formed; ports and buses each with a name.
    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        if self.name.trim().is_empty() {
            out.push(bad(
                Code::InvalidDocument,
                "/name".into(),
                "a block has a name".into(),
            ));
        }
        for (id, p) in &self.ports {
            out.extend(name_fault(&p.name, &format!("/ports/{id}/name")));
            if let Some(v) = &p.voltage
                && let Err(e) = Quantity::electrical(v, Dim::VOLTAGE)
            {
                out.push(bad(
                    Code::InvalidQuantity,
                    format!("/ports/{id}/voltage"),
                    e.message,
                ));
            }
        }
        for (id, b) in &self.buses {
            out.extend(name_fault(&b.name, &format!("/buses/{id}/name")));
        }
        out.extend(crate::props::faults(&self.properties, "/properties"));
        out
    }

    /// The port called `name`, by ID or by name.
    pub fn port(&self, name: &str) -> Option<&Id> {
        self.ports
            .keys()
            .find(|id| id.as_str() == name)
            .or_else(|| {
                self.ports
                    .iter()
                    .find(|(_, p)| p.name == name)
                    .map(|(id, _)| id)
            })
    }
}

impl Component {
    /// Pin numbers that can name pins, a reference designator that is one,
    /// and no net called `nc` (the mark's own text).
    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        if let Some(r) = &self.refdes
            && !refdes_ok(r)
        {
            out.push(bad(
                Code::InvalidDocument,
                "/refdes".into(),
                format!("{r:?} isn't a reference designator: a letter, then letters, digits and _"),
            ));
        }
        for n in self.pins.keys() {
            if !pin_number_ok(n) {
                out.push(bad(
                    Code::InvalidDocument,
                    format!("/pins/{}", pointer_escape(n)),
                    format!("{n:?} can't number a pin"),
                ));
            }
        }
        out
    }
}

impl Net {
    pub fn check(&self) -> Vec<Fault> {
        let mut out: Vec<Fault> = name_fault(&self.name, "/name").into_iter().collect();
        out.extend(intent_faults([
            &self.voltage,
            &self.current,
            &self.impedance,
        ]));
        out
    }
}

impl Rail {
    pub fn check(&self) -> Vec<Fault> {
        let mut out: Vec<Fault> = name_fault(&self.name, "/name").into_iter().collect();
        out.extend(intent_faults([
            &self.voltage,
            &self.current,
            &self.impedance,
        ]));
        out
    }
}

impl Class {
    pub fn check(&self) -> Vec<Fault> {
        name_fault(&self.name, "/name").into_iter().collect()
    }
}

impl Instance {
    /// Overrides keyed by occurrence paths of IDs, each a reference
    /// designator; a suffix that keeps it one.
    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        if let Some(RefdesFormat(f)) = &self.refdes
            && let Err(e) = crate::props::Template::parse(f)
        {
            out.push(bad(Code::RefdesFormatInvalid, "/refdes".into(), e.message));
        }
        out.extend(crate::props::faults(&self.properties, "/properties"));
        for (path, r) in &self.overrides {
            let ptr = format!("/overrides/{}", pointer_escape(path));
            if path.split('/').any(|s| Id::parse(s, "occurrence").is_err()) {
                out.push(bad(
                    Code::InvalidDocument,
                    ptr.clone(),
                    format!("{path:?} isn't an occurrence path: IDs joined by /"),
                ));
            }
            if !refdes_ok(r) {
                out.push(bad(
                    Code::InvalidDocument,
                    ptr,
                    format!("{r:?} isn't a reference designator"),
                ));
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc;
    use serde_json::json;

    fn id(s: &str) -> Id {
        Id::parse(s, "test").unwrap()
    }

    #[test]
    fn nodes_read_as_written() {
        let pins: BTreeMap<String, Node> = serde_json::from_value(json!({
            "1": "sda", "2": {"rail": "gnd"}, "3": {"port": "p1"}, "4": "nc"
        }))
        .unwrap();
        assert_eq!(pins["1"], Node::Net(id("sda")));
        assert_eq!(pins["2"], Node::Rail(id("gnd")));
        assert_eq!(pins["3"], Node::Port(id("p1")));
        assert_eq!(pins["4"], Node::Nc);
        assert_eq!(
            serde_json::to_value(&pins).unwrap(),
            json!({"1": "sda", "2": {"rail": "gnd"}, "3": {"port": "p1"}, "4": "nc"})
        );
        for bad in [json!("SDA"), json!({"rial": "gnd"}), json!(3)] {
            assert!(
                serde_json::from_value::<Node>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn files_read_and_write_canonically() {
        let c: Component = doc::from_value(
            json!({"kind": COMPONENT_KIND, "format": 1, "of": {"id": "r_100k"}, "refdes": "R1",
                   "pins": {"1": "n1", "2": {"rail": "gnd"}}}),
            "c.json",
            COMPONENT_KIND,
            FORMAT,
        )
        .unwrap();
        let text = doc::to_string(&c, COMPONENT_KIND, FORMAT);
        assert_eq!(
            doc::read::<Component>(text.as_bytes(), "c.json", COMPONENT_KIND, FORMAT).unwrap(),
            c
        );
        assert!(!text.contains("parameters"));
        let i: Instance = doc::from_value(
            json!({"kind": INSTANCE_KIND, "format": 1, "of": "sensor",
                   "ports": {"sda": "n1", "vdd": {"rail": "v3v3"}},
                   "refdes": {"rule": "offset", "value": 100},
                   "overrides": {"r1": "R99"}}),
            "i.json",
            INSTANCE_KIND,
            FORMAT,
        )
        .unwrap();
        // Phase 0's rule reads as the format giving the same designators,
        // and is written as that format, at format 2.
        assert_eq!(
            i.refdes,
            Some(RefdesFormat("{prefix}{number + 100}{rest}".into()))
        );
        let v = serde_json::to_value(&i).unwrap();
        assert_eq!(format_of(INSTANCE_KIND, &v), FORMAT_2);
        assert_eq!(v["refdes"], json!("{prefix}{number + 100}{rest}"));
        assert!(i.check().is_empty());
        let plain = Instance {
            refdes: None,
            overrides: BTreeMap::new(),
            ..i.clone()
        };
        assert_eq!(
            format_of(INSTANCE_KIND, &serde_json::to_value(&plain).unwrap()),
            FORMAT
        );
        let newer = doc::to_string(&i, INSTANCE_KIND, FORMAT_2);
        assert_eq!(
            doc::read::<Instance>(newer.as_bytes(), "i.json", INSTANCE_KIND, FORMAT)
                .unwrap_err()
                .code,
            Code::FormatTooNew
        );
        assert_eq!(
            doc::read::<Instance>(
                newer.as_bytes(),
                "i.json",
                INSTANCE_KIND,
                newest(INSTANCE_KIND)
            )
            .unwrap(),
            i
        );
        let e = doc::from_value::<Net>(
            json!({"kind": NET_KIND, "format": 1, "name": "x", "colour": "red"}),
            "n.json",
            NET_KIND,
            FORMAT,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
    }

    #[test]
    fn checks_find_bad_names_quantities_and_refdes() {
        let net = Net {
            name: "SDA 1".into(),
            class: None,
            voltage: Some("3.3 A".into()),
            current: Some("20 mA".into()),
            impedance: Some("90 Ohm".into()),
            pair: None,
        };
        let found: Vec<(Code, String)> = net
            .check()
            .into_iter()
            .map(|f| {
                (
                    f.error.code,
                    f.error.details["pointer"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (Code::InvalidDocument, "/name".to_owned()),
                (Code::InvalidQuantity, "/voltage".to_owned()),
            ]
        );
        let mut c = Component {
            of: LibRef::local(id("r")),
            refdes: Some("5R".into()),
            footprint: None,
            value: None,
            parameters: BTreeMap::new(),
            pins: BTreeMap::new(),
        };
        assert_eq!(c.check().len(), 1);
        c.refdes = Some("R5_A".into());
        assert!(c.check().is_empty());
        let i = Instance {
            of: id("b"),
            ports: BTreeMap::new(),
            refdes: Some(RefdesFormat("{refdes}{".into())),
            properties: [("Bad".to_owned(), "1".to_owned())].into(),
            overrides: [("x/Y".to_owned(), "R1".to_owned())].into(),
        };
        let codes: Vec<Code> = i.check().into_iter().map(|f| f.error.code).collect();
        assert_eq!(
            codes,
            [
                Code::RefdesFormatInvalid,
                Code::InvalidDocument,
                Code::InvalidDocument
            ]
        );
    }

    #[test]
    fn channel_rules_name_occurrences() {
        let apply = |c: Channel, r: &str| {
            let mut b = std::collections::BTreeMap::new();
            crate::props::designator(r, &mut b);
            crate::props::Template::parse(&c.format())
                .unwrap()
                .render(&|n| b.get(n).cloned())
                .unwrap()
        };
        assert_eq!(apply(Channel::Suffix("_A".into()), "R5"), "R5_A");
        assert_eq!(apply(Channel::Suffix("_B".into()), "R5_A"), "R5_A_B");
        assert_eq!(apply(Channel::Offset(100), "R5"), "R105");
        assert_eq!(apply(Channel::Offset(100), "R5_A"), "R105_A");
        assert_eq!(apply(Channel::Offset(10), "TP"), "TP10");
        assert_eq!(refdes_parts("U12"), ("U", 12, ""));
        assert_eq!(
            refdes_parts("R99999999999999999999x"),
            ("R", 0, "99999999999999999999x")
        );
        assert!(refdes_ok("SW1") && !refdes_ok("1R") && !refdes_ok("R-1") && !refdes_ok(""));
    }

    #[test]
    fn intent_values_compare_exactly() {
        let v = |t| exact_value(t, Dim::VOLTAGE);
        assert_eq!(v("3.3 V"), Some((33, -1)));
        assert_eq!(v("3300 mV"), v("3.3 V"));
        assert_eq!(v("3.30V"), v("3.3 V"));
        assert_eq!(v("0.0033 kV"), v("3.3 V"));
        assert_eq!(v("-5 V"), Some((-5, 0)));
        assert_eq!(v("0 V"), v("0 mV"));
        assert_ne!(v("3.3 V"), v("3.31 V"));
        assert_eq!(v("3.3 A"), None);
        assert_eq!(exact_value("90 Ohm", Dim::RESISTANCE), Some((9, 1)));
    }
}
