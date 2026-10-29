//! ERC: a fixed set of checks with stable codes over each design's
//! flattened netlist. Each finding names what it is about by reference
//! and has a stable ID, a hash of its code, design and references, so a
//! waiver survives unrelated edits. The pin-type matrix is KiCad's,
//! built in; severities and waivers are project data (`erc.json`).

use crate::conn::{Direction, exact_value};
use crate::error::{Code, Result};
use crate::fault::{self, Fault, Severity};
use crate::id::Id;
use crate::netlist::{FlatNet, Netlist, flatten};
use crate::project::{Project, SELF};
use crate::symbol::PinType;
use crate::units::Dim;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Every ERC code with its default severity, in the order output lists
/// them.
pub const CODES: [(Code, Severity); 15] = [
    (Code::PinConflict, Severity::Error),
    (Code::PowerUndriven, Severity::Error),
    (Code::PinUnconnected, Severity::Warning),
    (Code::NcConnected, Severity::Error),
    (Code::NetSinglePin, Severity::Warning),
    (Code::InputUndriven, Severity::Warning),
    (Code::RefdesDuplicate, Severity::Error),
    (Code::RefdesMissing, Severity::Error),
    (Code::FootprintMissing, Severity::Error),
    (Code::PinWithoutPad, Severity::Error),
    (Code::PinNotFound, Severity::Error),
    (Code::PairUnmatched, Severity::Error),
    (Code::IntentConflict, Severity::Warning),
    (Code::LibraryFault, Severity::Error),
    (Code::WaiverUnused, Severity::Warning),
];

pub const KIND: &str = "ryueda.erc";
pub const FORMAT: u64 = 1;

/// A code's severity as `erc.json` sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Error,
    Warning,
    /// Not checked.
    Off,
}

/// Why a finding is accepted, and who said so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    pub reason: String,
    pub by: String,
}

/// `erc.json` (kind `ryueda.erc`): severities by code and waivers by
/// finding ID. The pin-type matrix is built in, never data.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub severity: BTreeMap<String, Level>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub waivers: BTreeMap<String, Waiver>,
}

/// The ERC code a name in `erc.json` names.
pub fn code_named(name: &str) -> Option<Code> {
    CODES
        .iter()
        .map(|(c, _)| *c)
        .find(|c| json!(c).as_str() == Some(name))
}

impl Config {
    /// Severities name ERC codes; waivers name finding IDs (16 hex
    /// digits) with a reason.
    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        for name in self.severity.keys() {
            if code_named(name).is_none() {
                out.push(
                    Fault::error(Code::InvalidDocument, format!("{name} isn't an ERC code"))
                        .with("pointer", format!("/severity/{name}")),
                );
            }
        }
        for (id, w) in &self.waivers {
            let hex = id.len() == 16
                && id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
            if !hex || w.reason.trim().is_empty() {
                out.push(
                    Fault::error(
                        Code::InvalidDocument,
                        format!("waiver {id} needs a finding's ID (16 hex digits) and a reason"),
                    )
                    .with("pointer", format!("/waivers/{id}")),
                );
            }
        }
        out
    }

    /// `findings` as configured: severities set, codes turned off left
    /// out, waivers marked.
    pub fn apply(&self, findings: Vec<Finding>) -> Vec<Finding> {
        findings
            .into_iter()
            .filter_map(|mut f| {
                let name = json!(f.code).as_str().unwrap_or_default().to_owned();
                match self.severity.get(&name) {
                    Some(Level::Off) => return None,
                    Some(Level::Error) => f.severity = Severity::Error,
                    Some(Level::Warning) => f.severity = Severity::Warning,
                    None => {}
                }
                f.waived = self.waivers.get(&f.id).map(|w| json!(w));
                Some(f)
            })
            .collect()
    }
}

/// What the matrix says of two pin types on one net.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Ok,
    Warn,
    Err,
}

/// The matrix's order of pin types (KiCad's): `no_connect` is
/// `nc_connected`'s, not the matrix's.
const ORDER: [PinType; 11] = [
    PinType::Input,
    PinType::Output,
    PinType::Bidirectional,
    PinType::TriState,
    PinType::Passive,
    PinType::Free,
    PinType::Unspecified,
    PinType::PowerIn,
    PinType::PowerOut,
    PinType::OpenCollector,
    PinType::OpenEmitter,
];

/// KiCad's default pin-to-pin matrix, rows and columns in [`ORDER`].
const MATRIX: [[Verdict; 11]; 11] = {
    use Verdict::{Err as E, Ok as O, Warn as W};
    [
        // I  O  Bi 3S Pas Free UnS PwrI PwrO OC OE
        [O, O, O, O, O, O, W, O, O, O, O], // input
        [O, E, O, W, O, O, W, O, E, E, E], // output
        [O, O, O, O, O, O, W, O, W, O, W], // bidirectional
        [O, W, O, O, O, O, W, W, E, W, W], // tri-state
        [O, O, O, O, O, O, W, O, O, O, O], // passive
        [O, O, O, O, O, O, O, O, O, O, O], // free
        [W, W, W, W, W, O, W, W, W, W, W], // unspecified
        [O, O, O, W, O, O, W, O, O, O, O], // power input
        [O, E, W, E, O, O, W, O, E, E, E], // power output
        [O, E, O, W, O, O, W, O, E, O, O], // open collector
        [O, E, W, W, O, O, W, O, E, O, O], // open emitter
    ]
};

fn verdict(a: PinType, b: PinType) -> Verdict {
    let at = |t| ORDER.iter().position(|x| *x == t);
    match (at(a), at(b)) {
        (Some(i), Some(j)) => MATRIX[i][j],
        _ => Verdict::Ok,
    }
}

fn type_name(t: PinType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// One finding.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub id: String,
    pub code: Code,
    pub severity: Severity,
    pub design: Id,
    pub refs: Vec<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
    /// The waiver's reason and author, when `erc.json` waives it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waived: Option<Value>,
}

/// A finding's stable ID: 16 hex digits of BLAKE3 over its code, design
/// and sorted references.
pub fn finding_id(code: Code, design: &Id, refs: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ryueda.finding.1\0");
    h.update(json!(code).as_str().unwrap_or_default().as_bytes());
    h.update(b"\0");
    h.update(design.as_str().as_bytes());
    let mut sorted: Vec<&String> = refs.iter().collect();
    sorted.sort();
    for r in sorted {
        h.update(b"\0");
        h.update(r.as_bytes());
    }
    h.finalize().to_hex()[..16].to_owned()
}

/// Findings as they are made, before severities and waivers.
struct Found<'a> {
    design: &'a Id,
    out: Vec<Finding>,
}

impl Found<'_> {
    fn add(
        &mut self,
        code: Code,
        severity: Option<Severity>,
        refs: Vec<String>,
        message: String,
        details: Value,
    ) {
        let default = CODES
            .iter()
            .find(|(c, _)| *c == code)
            .map_or(Severity::Error, |(_, s)| *s);
        self.out.push(Finding {
            id: finding_id(code, self.design, &refs),
            code,
            severity: severity.unwrap_or(default),
            design: self.design.clone(),
            refs,
            message,
            details,
            waived: None,
        });
    }
}

/// Whether a net reaches an edge of the design: a port of its root
/// block, driven or read from off the board.
fn at_edge(n: &FlatNet) -> bool {
    n.ports.iter().any(|p| p.edge)
}

/// ERC over one design's netlist, with default severities, sorted.
pub fn check_netlist(p: &Project, n: &Netlist) -> Vec<Finding> {
    let mut f = Found {
        design: &n.design,
        out: Vec::new(),
    };
    for net in &n.nets {
        net_checks(&mut f, n, net);
    }
    pin_checks(&mut f, n);
    refdes_checks(&mut f, n);
    library_checks(&mut f, p, n);
    pair_checks(&mut f, p, n);
    let mut out = f.out;
    out.sort_by(|a, b| {
        let at = |c: Code| CODES.iter().position(|(x, _)| *x == c);
        at(a.code)
            .cmp(&at(b.code))
            .then_with(|| a.refs.cmp(&b.refs))
    });
    out
}

fn net_checks(f: &mut Found, n: &Netlist, net: &FlatNet) {
    let types: Vec<(PinType, String)> = net
        .members
        .iter()
        .filter_map(|&(c, i)| Some((n.occurrences[c].pins[i].ty?, n.pin_ref(c, i))))
        .collect();
    // pin_conflict: each pair of types the matrix refuses, once per net.
    let mut by_type: BTreeMap<usize, Vec<&String>> = BTreeMap::new();
    for (t, r) in &types {
        if let Some(i) = ORDER.iter().position(|x| x == t) {
            by_type.entry(i).or_default().push(r);
        }
    }
    let present: Vec<usize> = by_type.keys().copied().collect();
    for (k, &a) in present.iter().enumerate() {
        for &b in &present[k..] {
            if a == b && by_type[&a].len() < 2 {
                continue;
            }
            let v = verdict(ORDER[a], ORDER[b]);
            if v == Verdict::Ok {
                continue;
            }
            let mut refs = vec![net.id.clone()];
            refs.extend(by_type[&a].iter().map(|r| (*r).clone()));
            if a != b {
                refs.extend(by_type[&b].iter().map(|r| (*r).clone()));
            }
            let (ta, tb) = (type_name(ORDER[a]), type_name(ORDER[b]));
            f.add(
                Code::PinConflict,
                Some(if v == Verdict::Err {
                    Severity::Error
                } else {
                    Severity::Warning
                }),
                refs,
                format!("net {} joins {ta} and {tb} pins", net.name),
                json!({"net": net.name, "types": [ta, tb]}),
            );
        }
    }
    let has = |t: PinType| types.iter().any(|(x, _)| *x == t);
    let powered = has(PinType::PowerOut)
        || net.rail.is_some()
        || net.ports.iter().any(|p| p.direction == Direction::Power);
    if has(PinType::PowerIn) && !powered {
        let mut refs = vec![net.id.clone()];
        refs.extend(
            types
                .iter()
                .filter(|(t, _)| *t == PinType::PowerIn)
                .map(|(_, r)| r.clone()),
        );
        f.add(
            Code::PowerUndriven,
            None,
            refs,
            format!(
                "net {} has power inputs and nothing to drive them: no power output, rail or power port",
                net.name
            ),
            json!({"net": net.name}),
        );
    }
    if net.members.len() == 1 && !at_edge(net) {
        let (c, i) = net.members[0];
        f.add(
            Code::NetSinglePin,
            None,
            vec![net.id.clone(), n.pin_ref(c, i)],
            format!("net {} has one pin", net.name),
            json!({"net": net.name}),
        );
    }
    if !types.is_empty()
        && types.len() == net.members.len()
        && types.iter().all(|(t, _)| *t == PinType::Input)
        && !at_edge(net)
    {
        let mut refs = vec![net.id.clone()];
        refs.extend(types.iter().map(|(_, r)| r.clone()));
        f.add(
            Code::InputUndriven,
            None,
            refs,
            format!("net {} has only input pins: nothing drives it", net.name),
            json!({"net": net.name}),
        );
    }
    // intent_conflict: what the net or rail and the ports it passes
    // through declare, exactly.
    let values: BTreeSet<(i128, i32)> = net
        .voltages
        .iter()
        .filter_map(|(v, _)| exact_value(v, Dim::VOLTAGE))
        .collect();
    if values.len() > 1 {
        let voltages: Vec<Value> = net
            .voltages
            .iter()
            .map(|(v, at)| json!({"voltage": v, "at": at}))
            .collect();
        f.add(
            Code::IntentConflict,
            None,
            vec![net.id.clone()],
            format!("net {} joins different voltages", net.name),
            json!({"net": net.name, "voltages": voltages}),
        );
    }
}

fn pin_checks(f: &mut Found, n: &Netlist) {
    for (c, o) in n.occurrences.iter().enumerate() {
        for (i, pin) in o.pins.iter().enumerate() {
            let r = n.pin_ref(c, i);
            if !pin.in_symbol {
                if o.library.is_none() && (pin.net.is_some() || pin.nc) {
                    f.add(
                        Code::PinNotFound,
                        None,
                        vec![r],
                        format!(
                            "{} names pin {}, which its symbol doesn't have",
                            o.reference, pin.number
                        ),
                        json!({"pin": pin.number}),
                    );
                }
                continue;
            }
            let no_connect = pin.ty == Some(PinType::NoConnect);
            match pin.net {
                None if !pin.nc && !no_connect => f.add(
                    Code::PinUnconnected,
                    None,
                    vec![r],
                    format!(
                        "pin {} of {} is on no net and not marked not connected",
                        pin.number,
                        o.refdes.as_deref().unwrap_or(&o.reference)
                    ),
                    json!({"pin": pin.number}),
                ),
                Some(net) if no_connect => f.add(
                    Code::NcConnected,
                    None,
                    vec![r, n.nets[net].id.clone()],
                    format!(
                        "pin {} of {} connects to nothing by design, but is on net {}",
                        pin.number,
                        o.refdes.as_deref().unwrap_or(&o.reference),
                        n.nets[net].name
                    ),
                    json!({"pin": pin.number, "net": n.nets[net].name}),
                ),
                _ => {}
            }
        }
    }
}

fn refdes_checks(f: &mut Found, n: &Netlist) {
    let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for o in &n.occurrences {
        match &o.refdes {
            Some(r) => by.entry(r).or_default().push(&o.reference),
            None => f.add(
                Code::RefdesMissing,
                None,
                vec![o.reference.clone()],
                format!("{} has no reference designator", o.reference),
                Value::Null,
            ),
        }
        if o.footprint.is_none() || o.footprint_unlisted {
            f.add(
                Code::FootprintMissing,
                None,
                vec![o.reference.clone()],
                match o.footprint_unlisted {
                    true => format!("{} names a footprint {} doesn't list", o.reference, o.of),
                    false => format!(
                        "{}'s library component {} has no footprint",
                        o.reference, o.of
                    ),
                },
                json!({"libcomp": o.of}),
            );
        }
    }
    for (r, list) in by {
        if list.len() > 1 {
            f.add(
                Code::RefdesDuplicate,
                None,
                list.iter().map(|s| (*s).to_owned()).collect(),
                format!("{} occurrences are all {r}", list.len()),
                json!({"refdes": r}),
            );
        }
    }
}

/// The error faults of what a placed library component reads: its own
/// (but its pin maps', which are `pin_without_pad`), its symbol's and
/// its footprints'.
fn library_errors(p: &Project, of: &str) -> (Vec<Fault>, Vec<Fault>) {
    let mut pads = Vec::new();
    let mut other = Vec::new();
    let Some((alias, id)) = of
        .strip_prefix("lib/")
        .and_then(|s| s.split_once("/libcomp/"))
    else {
        return (pads, other);
    };
    let Ok(lib) = p.library(alias) else {
        return (pads, other);
    };
    let Some(l) = Id::parse(id, "libcomp")
        .ok()
        .and_then(|i| lib.components.get(&i))
    else {
        return (pads, other);
    };
    for x in l.faults.iter().filter(|x| x.is_error()) {
        match x.error.code {
            Code::PinWithoutPad => pads.push(x.clone()),
            _ => other.push(x.clone()),
        }
    }
    if let Ok(c) = &l.item {
        let alias = if alias == SELF { SELF } else { alias };
        let symbol_faults = c
            .symbol
            .library
            .is_none()
            .then(|| lib.symbols.get(&c.symbol.id))
            .flatten()
            .map(|s| s.faults.clone())
            .unwrap_or_default();
        other.extend(symbol_faults.into_iter().filter(Fault::is_error));
        for u in &c.footprints {
            let fp_lib = match &u.footprint.library {
                None => p.library(alias).ok(),
                Some(a) => p.library(a.as_str()).ok(),
            };
            if let Some(fl) = fp_lib.and_then(|l| l.footprints.get(&u.footprint.id)) {
                other.extend(fl.faults.iter().filter(|x| x.is_error()).cloned());
            }
        }
    }
    (pads, other)
}

fn library_checks(f: &mut Found, p: &Project, n: &Netlist) {
    let mut by: BTreeMap<&str, Vec<&crate::netlist::Occurrence>> = BTreeMap::new();
    for o in &n.occurrences {
        by.entry(&o.of).or_default().push(o);
    }
    for (of, list) in by {
        let mut refs = vec![of.to_owned()];
        refs.extend(list.iter().map(|o| o.reference.clone()));
        let (pads, mut other) = library_errors(p, of);
        if let Some(e) = &list[0].library {
            other.insert(
                0,
                Fault {
                    severity: Severity::Error,
                    error: e.clone(),
                },
            );
        }
        if !pads.is_empty() {
            f.add(
                Code::PinWithoutPad,
                None,
                refs.clone(),
                format!("{of} maps a pin to no pad: {}", pads[0].error.message),
                json!({"faults": fault::list(&pads)}),
            );
        }
        if !other.is_empty() {
            f.add(
                Code::LibraryFault,
                None,
                refs,
                format!(
                    "{of} fails its library's checks: {}",
                    other[0].error.message
                ),
                json!({"faults": fault::list(&other)}),
            );
        }
    }
}

fn pair_checks(f: &mut Found, p: &Project, n: &Netlist) {
    let ids: BTreeMap<&str, &FlatNet> = n.nets.iter().map(|x| (x.id.as_str(), x)).collect();
    for net in &n.nets {
        let Some((block, nid)) = &net.net else {
            continue;
        };
        let Some(files) = p.conn.blocks.get(block) else {
            continue;
        };
        let pair_of = |id: &Id| {
            files
                .nets
                .get(id)
                .and_then(|l| l.item.as_ref().ok())
                .and_then(|x| x.pair.clone())
        };
        let Some(pair) = pair_of(nid) else { continue };
        let base = &net.id[..net.id.len() - nid.as_str().len()];
        let other = format!("{base}{}", pair.with);
        let back = pair_of(&pair.with);
        let matched = back
            .as_ref()
            .is_some_and(|b| &b.with == nid && b.polarity != pair.polarity);
        // A pair that matches, or one already reported from its other half.
        if matched || (back.as_ref().is_some_and(|b| &b.with == nid) && other < net.id) {
            continue;
        }
        let other_name = ids
            .get(other.as_str())
            .map_or(other.clone(), |x| x.name.clone());
        f.add(
            Code::PairUnmatched,
            None,
            vec![net.id.clone(), other.clone()],
            match back {
                Some(b) if &b.with == nid => format!(
                    "nets {} and {other_name} are a pair of one polarity",
                    net.name
                ),
                _ => format!(
                    "net {} pairs with {other_name}, which doesn't pair with it",
                    net.name
                ),
            },
            json!({"net": net.name, "with": other_name}),
        );
    }
}

/// ERC over design `design`, as `erc.json` configures it: its findings,
/// or why its netlist is refused.
pub fn check_design(p: &Project, design: &Id) -> Result<Vec<Finding>> {
    let found = check_netlist(p, &flatten(p, design)?);
    Ok(config(p).apply(found))
}

/// `erc.json` as read; none when it doesn't read (a fault of the project).
pub fn config(p: &Project) -> Config {
    p.erc.item.clone().unwrap_or_default()
}

/// Waivers whose findings no design has, as findings of their own: judged
/// only when every design's netlist reads.
fn unused(p: &Project, seen: &BTreeSet<String>) -> Vec<Value> {
    let c = config(p);
    let level = c.severity.get("waiver_unused").copied();
    if level == Some(Level::Off) {
        return Vec::new();
    }
    c.waivers
        .iter()
        .filter(|(id, _)| !seen.contains(*id))
        .map(|(id, w)| {
            json!({
                "id": id,
                "code": Code::WaiverUnused,
                "severity": if level == Some(Level::Error) { Severity::Error } else { Severity::Warning },
                "refs": [format!("waiver/{id}")],
                "message": format!("waiver {id} ({}) waives a finding no design has", w.reason),
            })
        })
        .collect()
}

/// The answer for findings: the list and how many errors and warnings.
pub fn report(design: &Id, findings: &[Finding]) -> Value {
    let count = |s: Severity| {
        findings
            .iter()
            .filter(|x| x.severity == s && x.waived.is_none())
            .count()
    };
    json!({
        "design": design,
        "findings": findings,
        "errors": count(Severity::Error),
        "warnings": count(Severity::Warning),
        "waived": findings.iter().filter(|x| x.waived.is_some()).count(),
    })
}

/// `erc`: every design's findings (or the one named), each design whose
/// netlist is refused with its error.
pub fn erc(p: &Project, design: Option<&str>) -> Result<Value> {
    let designs: Vec<Id> = match design {
        Some(d) => vec![crate::netlist::one_design(p, Some(d))?],
        None => p.conn.designs.keys().cloned().collect(),
    };
    let mut list = Vec::new();
    let (mut errors, mut warnings) = (0, 0);
    let mut seen = BTreeSet::new();
    let mut all_read = design.is_none();
    for d in &designs {
        match check_design(p, d) {
            Ok(found) => {
                seen.extend(found.iter().map(|f| f.id.clone()));
                let r = report(d, &found);
                errors += r["errors"].as_u64().unwrap_or(0);
                warnings += r["warnings"].as_u64().unwrap_or(0);
                list.push(r);
            }
            Err(e) => {
                all_read = false;
                errors += 1;
                list.push(json!({"design": d, "error": e}));
            }
        }
    }
    let project = if all_read {
        unused(p, &seen)
    } else {
        Vec::new()
    };
    for f in &project {
        match f["severity"].as_str() {
            Some("error") => errors += 1,
            _ => warnings += 1,
        }
    }
    Ok(json!({"designs": list, "findings": project, "errors": errors, "warnings": warnings}))
}

/// Every finding ID of every design now, or why a design can't say.
fn finding_ids(p: &Project) -> Result<BTreeMap<String, Finding>> {
    let mut out = BTreeMap::new();
    for d in p.conn.designs.keys() {
        let found = check_netlist(p, &flatten(p, d)?);
        out.extend(found.into_iter().map(|f| (f.id.clone(), f)));
    }
    Ok(out)
}

/// Writes `c` as `erc.json`, or deletes it when it holds nothing.
fn save(session: &mut crate::session::Session, c: &Config) -> Result<()> {
    let mut w = crate::write::Writes::default();
    if c == &Config::default() {
        w.delete(crate::project::ERC.to_owned());
    } else {
        w.0.push((
            crate::project::ERC.to_owned(),
            Some(crate::doc::to_string(c, KIND, FORMAT)),
        ));
    }
    crate::write::commit(session, "erc.json", w)
}

/// `erc.waive`: accepts a finding a design has now, with a reason, or
/// with `remove` withdraws a waiver.
pub fn waive(
    session: &mut crate::session::Session,
    finding: &str,
    reason: Option<&str>,
    remove: bool,
    by: &str,
    expected: Option<&str>,
) -> Result<Value> {
    use crate::error::Error;
    let p = session.project();
    crate::server::check_revision("the project", &p.revision, expected)?;
    p.erc.item.as_ref().map_err(|e| e.clone())?;
    let mut c = config(p);
    if remove {
        if c.waivers.remove(finding).is_none() {
            return Err(
                Error::new(Code::NotFound, format!("nothing waives finding {finding}"))
                    .with("finding", finding),
            );
        }
    } else {
        let reason = reason.filter(|r| !r.trim().is_empty()).ok_or_else(|| {
            Error::new(Code::InvalidParam, "a waiver says why").with("param", "reason")
        })?;
        let found = finding_ids(p)?;
        let f = found.get(finding).ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("no design has finding {finding} now"),
            )
            .with("finding", finding)
        })?;
        let _ = f;
        c.waivers.insert(
            finding.to_owned(),
            Waiver {
                reason: reason.to_owned(),
                by: by.to_owned(),
            },
        );
    }
    save(session, &c)?;
    Ok(json!({"erc": config(session.project()), "revision": session.project().revision}))
}

/// `erc.configure`: each code's severity, `error`, `warning` or `off`, or
/// `null` for its default.
pub fn configure(
    session: &mut crate::session::Session,
    severity: &serde_json::Map<String, Value>,
    expected: Option<&str>,
) -> Result<Value> {
    use crate::error::Error;
    let p = session.project();
    crate::server::check_revision("the project", &p.revision, expected)?;
    p.erc.item.as_ref().map_err(|e| e.clone())?;
    let mut c = config(p);
    for (name, v) in severity {
        if code_named(name).is_none() {
            return Err(
                Error::new(Code::InvalidParam, format!("{name} isn't an ERC code"))
                    .with("param", format!("severity/{name}"))
                    .with("codes", CODES.iter().map(|(c, _)| *c).collect::<Vec<_>>()),
            );
        }
        match v {
            Value::Null => {
                c.severity.remove(name);
            }
            v => {
                let level: Level = serde_json::from_value(v.clone()).map_err(|_| {
                    Error::new(
                        Code::InvalidParam,
                        format!("{name}'s severity is error, warning or off, not {v}"),
                    )
                    .with("param", format!("severity/{name}"))
                })?;
                c.severity.insert(name.clone(), level);
            }
        }
    }
    save(session, &c)?;
    Ok(json!({"erc": config(session.project()), "revision": session.project().revision}))
}

/// `check`: the project loaded from nothing, every fault of every file
/// it holds or names, and every design's ERC. Fails with `check_failed`
/// (the report in its details) on any error a waiver doesn't accept.
pub fn check(root: &std::path::Path) -> Result<Value> {
    let p = Project::load(root)?;
    let faults = p.faults();
    let erc = erc(&p, None)?;
    let errors =
        faults.iter().filter(|f| f.is_error()).count() as u64 + erc["errors"].as_u64().unwrap_or(0);
    let warnings = faults.iter().filter(|f| !f.is_error()).count() as u64
        + erc["warnings"].as_u64().unwrap_or(0);
    let report = json!({
        "faults": fault::list(&faults),
        "erc": erc,
        "errors": errors,
        "warnings": warnings,
    });
    if errors > 0 {
        return Err(crate::error::Error::new(
            Code::CheckFailed,
            format!("check found {errors} error(s) and {warnings} warning(s)"),
        )
        .with("report", report));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::tests::project;

    fn id(s: &str) -> Id {
        Id::parse(s, "test").unwrap()
    }

    #[test]
    fn the_matrix_is_symmetric_and_kicads() {
        for a in ORDER {
            for b in ORDER {
                assert_eq!(verdict(a, b), verdict(b, a), "{a:?} {b:?}");
            }
        }
        assert_eq!(verdict(PinType::Output, PinType::Output), Verdict::Err);
        assert_eq!(verdict(PinType::PowerOut, PinType::PowerOut), Verdict::Err);
        assert_eq!(verdict(PinType::Output, PinType::PowerOut), Verdict::Err);
        assert_eq!(
            verdict(PinType::OpenCollector, PinType::OpenEmitter),
            Verdict::Ok
        );
        assert_eq!(verdict(PinType::Unspecified, PinType::Input), Verdict::Warn);
        assert_eq!(verdict(PinType::Free, PinType::Unspecified), Verdict::Ok);
        assert_eq!(verdict(PinType::NoConnect, PinType::Output), Verdict::Ok);
    }

    #[test]
    fn finding_ids_ignore_order_but_not_code_or_design() {
        let a = finding_id(Code::NetSinglePin, &id("d"), &["x".into(), "y".into()]);
        assert_eq!(
            a,
            finding_id(Code::NetSinglePin, &id("d"), &["y".into(), "x".into()])
        );
        assert_ne!(
            a,
            finding_id(Code::InputUndriven, &id("d"), &["x".into(), "y".into()])
        );
        assert_ne!(
            a,
            finding_id(Code::NetSinglePin, &id("e"), &["x".into(), "y".into()])
        );
        assert_eq!(a.len(), 16);
    }

    fn codes(found: &[Finding]) -> Vec<(Code, Vec<&str>)> {
        found
            .iter()
            .map(|f| (f.code, f.refs.iter().map(String::as_str).collect()))
            .collect()
    }

    #[test]
    fn the_test_design_passes_but_for_its_resistors_open_ends() {
        let (_dir, p) = project(&[]);
        let found = check_design(&p, &id("d")).unwrap();
        // 3V3 holds only R1's second pin; GND only the sensors' resistors.
        assert_eq!(
            codes(&found),
            [(Code::NetSinglePin, vec!["d:rail:v3v3", "d:r1/2"])]
        );
        assert_eq!(found[0].severity, Severity::Warning);
    }

    /// A symbol of one pin of each type, numbered by its type's name, a
    /// component on it with no footprint, and `pins` to connect.
    fn typed(pins: Value, extra: &[(&str, Value)]) -> Vec<Finding> {
        let types = [
            "input",
            "output",
            "bidirectional",
            "tri_state",
            "passive",
            "free",
            "unspecified",
            "power_in",
            "power_out",
            "open_collector",
            "open_emitter",
            "no_connect",
        ];
        let symbol_pins: serde_json::Map<String, Value> = types
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    (*t).to_owned(),
                    json!({"name": t, "type": t, "at": [0, i64::try_from(i).unwrap() * 2_540_000], "side": "left", "length_nm": 2_540_000}),
                )
            })
            .collect();
        let mut files = vec![
            (
                "library/symbols/all.json",
                json!({"kind": "ryueda.symbol", "format": 1, "name": "All", "units": 1, "pins": symbol_pins}),
            ),
            (
                "library/components/all.json",
                json!({"kind": "ryueda.libcomp", "format": 1, "symbol": {"id": "all"}, "prefix": "U"}),
            ),
            (
                "blocks/top/components/u9.json",
                json!({"kind": "ryueda.component", "format": 1, "of": {"id": "all"}, "refdes": "U9", "pins": pins}),
            ),
        ];
        files.extend(extra.iter().cloned());
        let (_dir, p) = project(&files);
        check_design(&p, &id("d")).unwrap()
    }

    fn net(name: &str) -> (String, Value) {
        (
            format!("blocks/top/nets/{name}.json"),
            json!({"kind": "ryueda.net", "format": 1, "name": name.to_uppercase()}),
        )
    }

    fn of(found: &[Finding], code: Code) -> Vec<Vec<&str>> {
        found
            .iter()
            .filter(|f| f.code == code)
            .map(|f| f.refs.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn every_check_fires_and_holds_its_peace() {
        let (a, b, c) = (net("a"), net("b"), net("c"));
        let extra: Vec<(&str, Value)> = [&a, &b, &c]
            .iter()
            .map(|(p, v)| (p.as_str(), v.clone()))
            .collect();
        let found = typed(
            json!({
                "output": "a", "power_out": "a",
                "power_in": "b", "input": "b",
                "no_connect": "c", "passive": "c",
                "open_collector": {"rail": "gnd"}, "open_emitter": {"rail": "gnd"},
                "bidirectional": "nc", "free": "nc", "tri_state": "nc"
            }),
            &extra,
        );
        assert_eq!(
            of(&found, Code::PinConflict),
            [vec!["d:a", "d:u9/output", "d:u9/power_out"]]
        );
        assert_eq!(
            of(&found, Code::PowerUndriven),
            [vec!["d:b", "d:u9/power_in"]]
        );
        assert_eq!(
            of(&found, Code::NcConnected),
            [vec!["d:u9/no_connect", "d:c"]]
        );
        assert_eq!(of(&found, Code::PinUnconnected), [vec!["d:u9/unspecified"]]);
        assert_eq!(of(&found, Code::FootprintMissing), [vec!["d:u9"]]);
        // Open collector and emitter share GND; input with power input is
        // fine; a net of an input and a power input isn't all inputs.
        assert!(of(&found, Code::InputUndriven).is_empty());
        // A net of inputs alone, and the power input driven by a rail.
        let found = typed(
            json!({"input": "a", "power_in": {"rail": "v3v3"}}),
            &[(a.0.as_str(), a.1.clone())],
        );
        assert_eq!(of(&found, Code::InputUndriven), [vec!["d:a", "d:u9/input"]]);
        assert_eq!(of(&found, Code::NetSinglePin), [vec!["d:a", "d:u9/input"]]);
        assert!(of(&found, Code::PowerUndriven).is_empty());
        // Unspecified pins warn with what they meet.
        let found = typed(
            json!({"unspecified": "a", "input": "a"}),
            &[(a.0.as_str(), a.1.clone())],
        );
        let conflict: Vec<&Finding> = found
            .iter()
            .filter(|f| f.code == Code::PinConflict)
            .collect();
        assert_eq!(conflict[0].severity, Severity::Warning);
    }

    #[test]
    fn refdes_footprints_and_pins_the_symbol_lost() {
        let dup = json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_100k"}, "refdes": "R1", "pins": {"1": "sda", "2": "sda", "3": "sda"}});
        let none = json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_100k"}, "pins": {"1": "nc", "2": "nc"}});
        let other = json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_100k"}, "refdes": "R3",
            "footprint": {"id": "elsewhere"}, "pins": {"1": "nc", "2": "nc"}});
        let (_dir, p) = project(&[
            ("blocks/top/components/r2.json", dup),
            ("blocks/top/components/r0.json", none),
            ("blocks/top/components/r3.json", other),
        ]);
        let found = check_design(&p, &id("d")).unwrap();
        assert_eq!(of(&found, Code::RefdesDuplicate), [vec!["d:r1", "d:r2"]]);
        assert_eq!(of(&found, Code::RefdesMissing), [vec!["d:r0"]]);
        assert_eq!(of(&found, Code::FootprintMissing), [vec!["d:r3"]]);
        assert_eq!(of(&found, Code::PinNotFound), [vec!["d:r2/3"]]);
    }

    #[test]
    fn pairs_must_match_and_intent_must_agree() {
        let p_net = json!({"kind": "ryueda.net", "format": 1, "name": "D_P", "pair": {"with": "dn", "polarity": "p"}});
        let n_net = json!({"kind": "ryueda.net", "format": 1, "name": "D_N", "pair": {"with": "dp", "polarity": "p"}});
        let lone = json!({"kind": "ryueda.net", "format": 1, "name": "X", "pair": {"with": "dp", "polarity": "n"}});
        // The sensor block expects 5 V on SDA; the top's SDA says 3.3 V.
        let s = json!({"kind": "ryueda.block", "format": 1, "name": "Sensor",
            "ports": {"sda": {"name": "SDA", "direction": "bidirectional", "voltage": "5 V"}}});
        let sda = json!({"kind": "ryueda.net", "format": 1, "name": "SDA", "voltage": "3300 mV"});
        let (_dir, p) = project(&[
            ("blocks/top/nets/dp.json", p_net),
            ("blocks/top/nets/dn.json", n_net),
            ("blocks/top/nets/x.json", lone),
            ("blocks/s/block.json", s.clone()),
            ("blocks/top/nets/sda.json", sda),
        ]);
        let found = check_design(&p, &id("d")).unwrap();
        assert_eq!(
            of(&found, Code::PairUnmatched),
            [vec!["d:dn", "d:dp"], vec!["d:x", "d:dp"]]
        );
        let intent: Vec<&Finding> = found
            .iter()
            .filter(|f| f.code == Code::IntentConflict)
            .collect();
        assert_eq!(intent.len(), 1);
        assert_eq!(intent[0].details["voltages"][0]["at"], "top/sda");
        assert_eq!(intent[0].details["voltages"][1]["at"], "d:ch_a/port:sda");
        // 3.3 V and 3300 mV agree.
        let s = json!({"kind": "ryueda.block", "format": 1, "name": "Sensor",
            "ports": {"sda": {"name": "SDA", "direction": "bidirectional", "voltage": "3.3 V"}}});
        let sda = json!({"kind": "ryueda.net", "format": 1, "name": "SDA", "voltage": "3300 mV"});
        let (_dir, p) = project(&[
            ("blocks/s/block.json", s),
            ("blocks/top/nets/sda.json", sda),
        ]);
        assert!(of(&check_design(&p, &id("d")).unwrap(), Code::IntentConflict).is_empty());
    }

    #[test]
    fn faulty_library_objects_a_design_places_are_findings() {
        // r_100k's footprint loses its pad 2: pin 2 has no pad; a pad
        // made zero-size is the footprint's own fault.
        let (_dir, p) = project(&[]);
        let fp = p
            .library(SELF)
            .unwrap()
            .footprints
            .values()
            .next()
            .unwrap()
            .item
            .clone()
            .unwrap();
        let mut v = serde_json::to_value(&fp).unwrap();
        v["kind"] = json!("ryueda.footprint");
        v["format"] = json!(1);
        let pads = v["pads"].as_object_mut().unwrap();
        let two = pads
            .iter()
            .find(|(_, x)| x["number"] == "2")
            .unwrap()
            .0
            .clone();
        pads.remove(&two);
        let one = pads.keys().next().unwrap().clone();
        v["pads"][&one]["size"] = json!([0, 0]);
        let (_dir, p) = project(&[("library/footprints/r_0402_1005metric.json", v)]);
        let found = check_design(&p, &id("d")).unwrap();
        let refs = vec!["lib/self/libcomp/r_100k", "d:r1", "d:ch_a/u1", "d:ch_b/u1"];
        assert_eq!(of(&found, Code::PinWithoutPad), std::slice::from_ref(&refs));
        assert_eq!(of(&found, Code::LibraryFault), [refs]);
    }

    #[test]
    fn edges_are_driven_from_off_the_board() {
        let top = json!({"kind": "ryueda.block", "format": 1, "name": "Top",
            "ports": {"rx": {"name": "RX", "direction": "in"}}});
        let r1 = json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_100k"}, "refdes": "R1",
            "pins": {"1": {"port": "rx"}, "2": {"rail": "v3v3"}}});
        let (_dir, p) = project(&[
            ("blocks/top/block.json", top),
            ("blocks/top/components/r1.json", r1),
        ]);
        let found = check_design(&p, &id("d")).unwrap();
        assert!(!found.iter().any(|f| f.refs[0] == "d:port:rx"), "{found:?}");
    }

    #[test]
    fn severities_and_waivers_are_data() {
        let (_dir, p) = project(&[]);
        let found = check_design(&p, &id("d")).unwrap();
        let single = found[0].id.clone();
        let erc = json!({"kind": KIND, "format": 1,
            "severity": {"net_single_pin": "error", "refdes_missing": "off"},
            "waivers": {single.clone(): {"reason": "a test point", "by": "agent"}, "0123456789abcdef": {"reason": "gone", "by": "agent"}}});
        let (_dir, p) = project(&[("erc.json", erc)]);
        let found = check_design(&p, &id("d")).unwrap();
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(
            found[0].waived,
            Some(json!({"reason": "a test point", "by": "agent"}))
        );
        let all = erc_all(&p);
        assert_eq!((&all["errors"], &all["warnings"]), (&json!(0), &json!(1)));
        assert_eq!(all["findings"][0]["code"], "waiver_unused");
        assert_eq!(
            all["findings"][0]["refs"],
            json!(["waiver/0123456789abcdef"])
        );
        assert_eq!(all["designs"][0]["waived"], 1);
        // A bad file is the project's fault, and ERC reads defaults.
        let bad = json!({"kind": KIND, "format": 1, "severity": {"nope": "off"}});
        let (_dir, p) = project(&[("erc.json", bad)]);
        let faults = p.faults();
        assert_eq!(faults[0].error.details["pointer"], "/severity/nope");
    }

    fn erc_all(p: &Project) -> Value {
        erc(p, None).unwrap()
    }

    #[test]
    fn one_thread_and_eight_find_alike() {
        let (_dir, p) = project(&[]);
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| erc(&p, None).unwrap())
        };
        assert_eq!(run(1), run(8));
    }
}
