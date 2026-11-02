//! Circuits schematics draw one standard way, recognised in a block and
//! judged against that drawing on a laid out sheet: a buck converter (its
//! switch node, inductor, catch diode and output capacitors) and an op-amp
//! stage (its feedback element and its input element). Lint reports each
//! one drawn out of shape (`sch_topology`), saying what breaks the shape
//! and what the standard drawing is, so an agent can redraw it from the
//! message alone.

use crate::geom::{GRID, Point};
use crate::input::{Block, NetKind, PartKind, PinKind};
use crate::layout::Layout;
use crate::route;
use crate::shape::Shape;
use std::collections::{BTreeMap, BTreeSet};

/// A recognised circuit and how its drawing departs from the standard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judged {
    /// `buck_converter` or `opamp_stage`.
    pub kind: &'static str,
    /// Its parts, by key, the main one first.
    pub parts: Vec<String>,
    /// What it is, in words (`the buck converter around U1, switch node
    /// PH: U1.PH, L1, D1; output +3V3 through C8, C9`).
    pub what: String,
    /// What breaks the standard shape, each a short clause; empty when it
    /// is drawn in shape.
    pub problems: Vec<String>,
    /// The standard drawing, in a sentence.
    pub standard: String,
}

fn prefix(block: &Block, p: usize) -> &str {
    match &block.parts[p].kind {
        PartKind::Symbol { prefix } => prefix,
        _ => "",
    }
}

fn two_pin(block: &Block, p: usize, prefixes: &[&str]) -> bool {
    block.parts[p].pins.len() == 2 && prefixes.contains(&prefix(block, p))
}

/// An active part: a block, or a symbol of three pins or more that isn't a
/// connector.
fn active(block: &Block, p: usize) -> bool {
    match block.parts[p].kind {
        PartKind::Block => true,
        PartKind::Symbol { .. } => {
            block.parts[p].pins.len() >= 3 && !crate::cluster::is_connector(block, p)
        }
        PartKind::Port { .. } => false,
    }
}

/// A part as messages name it: its designator, else its key.
fn who(block: &Block, p: usize) -> String {
    let part = &block.parts[p];
    if part.refdes.is_empty() {
        part.key.clone()
    } else {
        part.refdes.clone()
    }
}

/// What one sheet's layout shows of the parts placed on it.
struct Sheet<'a> {
    block: &'a Block,
    layout: &'a Layout,
    sid: &'a str,
    shapes: &'a BTreeMap<usize, Shape>,
}

impl Sheet<'_> {
    fn on(&self, p: usize) -> bool {
        self.shapes.contains_key(&p)
    }

    /// Whether a two-pin part lies across the page (its pins point left
    /// and right).
    fn across(&self, p: usize) -> bool {
        self.shapes[&p].pins.iter().all(|x| x.side.horizontal())
    }

    fn middle(&self, p: usize) -> Point {
        self.shapes[&p].body.center()
    }

    fn end(&self, p: usize, k: usize) -> Point {
        self.shapes[&p].pins[k].end
    }

    /// Whether pins `(p, k)` and `(q, j)` of net `n` are joined by wires on
    /// this sheet.
    fn wired(&self, n: usize, (p, k): (usize, usize), (q, j): (usize, usize)) -> bool {
        let runs = self
            .layout
            .routes
            .get(&self.block.nets[n].key)
            .and_then(|r| r.get(self.sid))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let nodes: Vec<BTreeSet<route::Node>> = runs
            .iter()
            .map(|r| route::expand(r).into_iter().collect())
            .collect();
        let from = route::node_of(self.end(p, k));
        let to = route::node_of(self.end(q, j));
        let mut reached: BTreeSet<route::Node> = [from].into();
        let mut taken = vec![false; nodes.len()];
        loop {
            let mut grew = false;
            for (i, ns) in nodes.iter().enumerate() {
                if !taken[i] && ns.iter().any(|x| reached.contains(x)) {
                    taken[i] = true;
                    reached.extend(ns.iter().copied());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        reached.contains(&to)
    }
}

/// The pin of part `p` on net `n`.
fn pin_on(block: &Block, p: usize, n: usize) -> Option<usize> {
    block.parts[p].pins.iter().position(|x| x.net == Some(n))
}

/// Every recognised circuit with a part on sheet `sid`, judged there.
pub fn judge(
    block: &Block,
    layout: &Layout,
    sid: &str,
    shapes: &BTreeMap<usize, Shape>,
) -> Vec<Judged> {
    let sheet = Sheet {
        block,
        layout,
        sid,
        shapes,
    };
    let mut out = bucks(&sheet);
    out.extend(opamp_stages(&sheet));
    out
}

/// A switch pin's name: SW, LX, PH or SWITCH (SW1, SW_A too).
fn switch_pin(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("SW") || n == "LX" || n == "PH" || n.starts_with("LX")
}

/// Buck converters: an active part's switch pin, an inductor on its net
/// (the switch node), a diode on it too if there is one, and capacitors to
/// ground on the inductor's other net (the output).
fn bucks(s: &Sheet) -> Vec<Judged> {
    let block = s.block;
    let mut out = Vec::new();
    for (ic, part) in block.parts.iter().enumerate() {
        if !active(block, ic) || !s.on(ic) {
            continue;
        }
        for (k, pin) in part.pins.iter().enumerate() {
            let Some(sw) = pin.net else { continue };
            if !switch_pin(&pin.name) || block.nets[sw].kind != NetKind::Signal {
                continue;
            }
            let Some(l) = (0..block.parts.len())
                .find(|q| two_pin(block, *q, &["L"]) && pin_on(block, *q, sw).is_some())
            else {
                continue;
            };
            let lk = pin_on(block, l, sw).expect("on it");
            let lo = 1 - lk;
            let Some(vout) = block.parts[l].pins[lo].net else {
                continue;
            };
            let diode = (0..block.parts.len())
                .find(|q| two_pin(block, *q, &["D"]) && pin_on(block, *q, sw).is_some());
            let caps: Vec<usize> = (0..block.parts.len())
                .filter(|q| {
                    two_pin(block, *q, &["C"])
                        && pin_on(block, *q, vout).is_some()
                        && block.parts[*q]
                            .pins
                            .iter()
                            .any(|x| x.net.is_some_and(|n| block.nets[n].kind == NetKind::Ground))
                })
                .collect();
            let (ic_n, l_n) = (who(block, ic), who(block, l));
            let mut parts = vec![part.key.clone(), block.parts[l].key.clone()];
            let mut what = format!(
                "the buck converter around {ic_n} (switch node {}: {ic_n}.{}, {l_n}",
                block.nets[sw].name, pin.name
            );
            if let Some(d) = diode {
                what.push_str(&format!(", {}", who(block, d)));
                parts.push(block.parts[d].key.clone());
            }
            what.push_str(&format!("; output {}", block.nets[vout].name));
            if !caps.is_empty() {
                let names: Vec<String> = caps.iter().map(|c| who(block, *c)).collect();
                what.push_str(&format!(" through {}", names.join(", ")));
                parts.extend(caps.iter().map(|c| block.parts[*c].key.clone()));
            }
            what.push(')');
            let d_n = diode.map(|d| who(block, d));
            let standard = format!(
                "{l_n} lies across the page with {ic_n}'s {} pin wired straight to its left end{}; the output capacitors stand on {l_n}'s right end, down to ground; every part wired, none joined by a label",
                pin.name,
                d_n.as_ref()
                    .map(|d| format!(
                        "; {d} stands below the switch node, its cathode up, down to ground"
                    ))
                    .unwrap_or_default()
            );
            let mut problems = Vec::new();
            let mut all: Vec<usize> = vec![l];
            all.extend(diode);
            all.extend(caps.iter().copied());
            for q in &all {
                if !s.on(*q) {
                    problems.push(format!("{} is on another sheet", who(block, *q)));
                }
            }
            if s.on(l) {
                if !s.across(l) {
                    problems.push(format!("{l_n} stands upright"));
                }
                if s.middle(l)[0] < s.end(ic, k)[0] {
                    problems.push(format!("{l_n} is left of {ic_n}'s {} pin", pin.name));
                }
                if !s.wired(sw, (ic, k), (l, lk)) {
                    problems.push(format!(
                        "{ic_n}'s {} pin isn't wired to {l_n} (a label joins them)",
                        pin.name
                    ));
                }
            }
            if let Some(d) = diode.filter(|d| s.on(*d) && s.on(l)) {
                let dk = pin_on(block, d, sw).expect("on it");
                let d_n = who(block, d);
                if s.across(d) {
                    problems.push(format!("{d_n} lies across the page"));
                }
                if s.middle(d)[1] > s.end(l, lk)[1] - GRID {
                    problems.push(format!("{d_n} isn't below the switch node"));
                }
                if !s.wired(sw, (d, dk), (l, lk)) {
                    problems.push(format!("{d_n} is joined to the switch node by a label"));
                }
            }
            if s.on(l) {
                for c in caps.iter().copied().filter(|c| s.on(*c)) {
                    let ck = pin_on(block, c, vout).expect("on it");
                    let c_n = who(block, c);
                    if s.across(c) {
                        problems.push(format!("{c_n} lies across the page"));
                    }
                    if s.middle(c)[0] < s.middle(l)[0] {
                        problems.push(format!("{c_n} is left of {l_n}"));
                    }
                    if !s.wired(vout, (c, ck), (l, lo)) {
                        problems.push(format!("{c_n} isn't wired to {l_n}'s output end"));
                    }
                }
            }
            out.push(Judged {
                kind: "buck_converter",
                parts,
                what,
                problems,
                standard,
            });
        }
    }
    out
}

/// Whether a pin is an amplifier's output: an output pin, or one named
/// OUT.
fn out_pin(pin: &crate::input::Pin) -> bool {
    pin.kind == PinKind::Output || pin.name.to_ascii_uppercase().contains("OUT")
}

/// Whether a pin is an amplifier's inverting input.
fn inv_pin(pin: &crate::input::Pin) -> bool {
    let n = pin.name.to_ascii_uppercase();
    n.ends_with('-') || n.contains("IN-") || n.contains("INN")
}

/// Op-amp stages: an active part with an output and an inverting input,
/// the resistors and capacitors between them (feedback), and those from
/// elsewhere into the inverting input (input).
fn opamp_stages(s: &Sheet) -> Vec<Judged> {
    let block = s.block;
    let mut out = Vec::new();
    for (amp, part) in block.parts.iter().enumerate() {
        if !active(block, amp) || !s.on(amp) {
            continue;
        }
        let Some(ok) = part.pins.iter().position(out_pin) else {
            continue;
        };
        let Some(ik) = part.pins.iter().position(inv_pin) else {
            continue;
        };
        let (Some(on), Some(inn)) = (part.pins[ok].net, part.pins[ik].net) else {
            continue;
        };
        if on == inn {
            continue;
        }
        let passive = |q: &usize| two_pin(block, *q, &["R", "C"]);
        let feedback: Vec<usize> = (0..block.parts.len())
            .filter(|q| {
                passive(q) && pin_on(block, *q, on).is_some() && pin_on(block, *q, inn).is_some()
            })
            .collect();
        if feedback.is_empty() {
            continue;
        }
        let inputs: Vec<usize> = (0..block.parts.len())
            .filter(|q| {
                passive(q)
                    && !feedback.contains(q)
                    && pin_on(block, *q, inn).is_some()
                    && block.parts[*q].pins.iter().all(|x| {
                        x.net
                            .is_some_and(|n| n == inn || block.nets[n].kind == NetKind::Signal)
                    })
            })
            .collect();
        let a_n = who(block, amp);
        let names = |v: &[usize]| {
            v.iter()
                .map(|q| who(block, *q))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut parts = vec![part.key.clone()];
        parts.extend(feedback.iter().map(|q| block.parts[*q].key.clone()));
        parts.extend(inputs.iter().map(|q| block.parts[*q].key.clone()));
        let what = format!(
            "the op-amp stage around {a_n} (feedback {}{})",
            names(&feedback),
            if inputs.is_empty() {
                String::new()
            } else {
                format!("; input {}", names(&inputs))
            }
        );
        let standard = format!(
            "the feedback element{} {} across the page above {a_n}, wired from its output back to its inverting input{}; every part wired, none joined by a label",
            if feedback.len() > 1 { "s" } else { "" },
            if feedback.len() > 1 { "lie" } else { "lies" },
            if inputs.is_empty() {
                String::new()
            } else {
                format!(
                    "; the input element lies across the page left of the inverting input, wired to it"
                )
            }
        );
        let mut problems = Vec::new();
        let top = s.shapes[&amp].body.max[1];
        for q in feedback.iter().copied() {
            let q_n = who(block, q);
            if !s.on(q) {
                problems.push(format!("{q_n} is on another sheet"));
                continue;
            }
            if !s.across(q) {
                problems.push(format!("{q_n} stands upright"));
            }
            if s.middle(q)[1] < top {
                problems.push(format!("{q_n} isn't above {a_n}"));
            }
            let (qo, qi) = (
                pin_on(block, q, on).expect("on it"),
                pin_on(block, q, inn).expect("on it"),
            );
            if !s.wired(on, (q, qo), (amp, ok)) || !s.wired(inn, (q, qi), (amp, ik)) {
                problems.push(format!("{q_n} is joined to {a_n} by a label"));
            }
        }
        for q in inputs.iter().copied() {
            let q_n = who(block, q);
            if !s.on(q) {
                problems.push(format!("{q_n} is on another sheet"));
                continue;
            }
            if !s.across(q) {
                problems.push(format!("{q_n} stands upright"));
            }
            if s.middle(q)[0] > s.end(amp, ik)[0] {
                problems.push(format!("{q_n} isn't left of {a_n}'s inverting input"));
            }
            let qi = pin_on(block, q, inn).expect("on it");
            if !s.wired(inn, (q, qi), (amp, ik)) {
                problems.push(format!(
                    "{q_n} is joined to {a_n}'s inverting input by a label"
                ));
            }
        }
        out.push(Judged {
            kind: "opamp_stage",
            parts,
            what,
            problems,
            standard,
        });
    }
    out
}

#[cfg(test)]
mod tests;
