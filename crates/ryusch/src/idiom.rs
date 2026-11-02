//! Idioms: connectivity patterns drawn in their canonical shapes. Each is
//! matched deterministically (parts in key order, kinds in a fixed order,
//! a part in one idiom at most; where a part could serve several, the
//! first in that order takes it and the idiom says so) and becomes part of
//! a rigid cluster: decoupling capacitors on the line out of the pin they
//! serve, pull-ups standing above it, pull-downs hanging below, a crystal
//! beside its pins with its load capacitors, and so on.

use crate::cluster::{Cluster, is_connector};
use crate::geom::{GRID, Nm, Place, Point, Rect, Side, snap};
use crate::input::{Block, Flow, NetKind, PartKind, PinKind};
use crate::layout::{IdiomChoice, IdiomKind};
use crate::shape::Shape;
use std::collections::{BTreeMap, BTreeSet};

/// A matched idiom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Idiom {
    pub kind: IdiomKind,
    /// Its own parts, in the order the kind lists them.
    pub parts: Vec<usize>,
    /// The part and pin it serves, if it serves one.
    pub anchor: Option<(usize, usize)>,
    /// Why the match took this anchor over another, when it had a choice.
    pub note: Option<String>,
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

fn is_resistor(block: &Block, p: usize) -> bool {
    two_pin(block, p, &["R"])
}

fn is_capacitor(block: &Block, p: usize) -> bool {
    two_pin(block, p, &["C"])
}

fn is_inductor(block: &Block, p: usize) -> bool {
    two_pin(block, p, &["L", "FB"])
}

fn is_led(block: &Block, p: usize) -> bool {
    two_pin(block, p, &["D", "LED"])
}

fn is_crystal(block: &Block, p: usize) -> bool {
    matches!(prefix(block, p), "Y" | "X") && (2..=4).contains(&block.parts[p].pins.len())
}

/// A part that does something: a symbol with three pins or more that
/// isn't a connector, or a child block.
fn is_active(block: &Block, p: usize) -> bool {
    match block.parts[p].kind {
        PartKind::Block => true,
        PartKind::Symbol { .. } => block.parts[p].pins.len() >= 3 && !is_connector(block, p),
        PartKind::Port { .. } => false,
    }
}

/// The nets of a two-pin part's pins.
fn ends(block: &Block, p: usize) -> [Option<usize>; 2] {
    let pins = &block.parts[p].pins;
    [pins[0].net, pins[1].net]
}

fn kind_of(block: &Block, n: Option<usize>) -> Option<NetKind> {
    n.map(|n| block.nets[n].kind)
}

/// Every pin on each net, by net: `(part, pin)` in part key order.
fn pins_by_net(block: &Block) -> BTreeMap<usize, Vec<(usize, usize)>> {
    let mut out: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    let mut order: Vec<usize> = (0..block.parts.len()).collect();
    order.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
    for p in order {
        for (k, pin) in block.parts[p].pins.iter().enumerate() {
            if let Some(n) = pin.net
                && !pin.nc
            {
                out.entry(n).or_default().push((p, k));
            }
        }
    }
    out
}

/// What may join an idiom: parts no instruction keeps out, and parts an
/// instruction names only for this kind.
fn allowed(choices: &BTreeMap<usize, IdiomChoice>, p: usize, kind: IdiomKind) -> bool {
    match choices.get(&p) {
        None => true,
        Some(IdiomChoice::None) => false,
        Some(IdiomChoice::Kind(k)) => *k == kind,
    }
}

/// The active pin a passive on net `n` serves: the first active part's
/// pin that isn't power, by part key; and how many it chose among.
fn served(
    block: &Block,
    by_net: &BTreeMap<usize, Vec<(usize, usize)>>,
    n: usize,
    not: &[usize],
) -> Option<((usize, usize), usize)> {
    let list: Vec<(usize, usize)> = by_net
        .get(&n)?
        .iter()
        .copied()
        .filter(|(p, _)| !not.contains(p) && is_active(block, *p))
        .collect();
    let first = *list.first()?;
    Some((first, list.len()))
}

/// Finds every idiom, in kind order then part key order.
pub fn find(block: &Block, choices: &BTreeMap<usize, IdiomChoice>) -> Vec<Idiom> {
    let by_net = pins_by_net(block);
    let mut order: Vec<usize> = (0..block.parts.len()).collect();
    order.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
    let mut used: BTreeSet<usize> = BTreeSet::new();
    let mut out = Vec::new();
    let note = |count: usize, block: &Block, (p, k): (usize, usize)| {
        (count > 1).then(|| {
            format!(
                "{count} parts could take it; the first by key, {}/{}, did",
                block.parts[p].key, block.parts[p].pins[k].number
            )
        })
    };
    let ground = |n: Option<usize>| kind_of(block, n) == Some(NetKind::Ground);
    let supply = |n: Option<usize>| kind_of(block, n) == Some(NetKind::Supply);
    let signal = |n: Option<usize>| kind_of(block, n) == Some(NetKind::Signal);
    // Buck converters' power stages: an active part's switch pin, the
    // inductor on its net, the catch diode on it if there is one, and the
    // capacitors from the inductor's far end to ground.
    for &ic in &order {
        if !is_active(block, ic) {
            continue;
        }
        for (k, pin) in block.parts[ic].pins.iter().enumerate() {
            let Some(sw) = pin.net else { continue };
            if !crate::topology::switch_pin(&pin.name) || !signal(Some(sw)) {
                continue;
            }
            let on_sw = |q: usize| ends(block, q).contains(&Some(sw));
            let Some(l) = order.iter().copied().find(|q| {
                !used.contains(q)
                    && two_pin(block, *q, &["L"])
                    && on_sw(*q)
                    && allowed(choices, *q, IdiomKind::Buck)
            }) else {
                continue;
            };
            let e = ends(block, l);
            let Some(vout) = (if e[0] == Some(sw) { e[1] } else { e[0] }) else {
                continue;
            };
            let mut parts = vec![l];
            parts.extend(order.iter().copied().find(|q| {
                !used.contains(q)
                    && two_pin(block, *q, &["D"])
                    && on_sw(*q)
                    && allowed(choices, *q, IdiomKind::Buck)
            }));
            parts.extend(order.iter().copied().filter(|c| {
                !used.contains(c)
                    && is_capacitor(block, *c)
                    && allowed(choices, *c, IdiomKind::Buck)
                    && {
                        let ce = ends(block, *c);
                        (ce[0] == Some(vout) && ground(ce[1]))
                            || (ce[1] == Some(vout) && ground(ce[0]))
                    }
            }));
            used.extend(parts.iter().copied());
            out.push(Idiom {
                kind: IdiomKind::Buck,
                parts,
                anchor: Some((ic, k)),
                note: None,
            });
        }
    }
    // Op-amp stages: an amplifier's feedback elements (resistors and
    // capacitors from its output to its inverting input) and its input
    // elements (from a signal into the inverting input).
    for &amp in &order {
        if !is_active(block, amp) {
            continue;
        }
        let pins = &block.parts[amp].pins;
        let (Some(ok), Some(ik)) = (
            pins.iter().position(crate::topology::out_pin),
            pins.iter().position(crate::topology::inv_pin),
        ) else {
            continue;
        };
        let (Some(on), Some(inn)) = (pins[ok].net, pins[ik].net) else {
            continue;
        };
        if on == inn {
            continue;
        }
        let free = |q: &usize| {
            !used.contains(q)
                && two_pin(block, *q, &["R", "C"])
                && allowed(choices, *q, IdiomKind::OpampStage)
        };
        let feedback: Vec<usize> = order
            .iter()
            .copied()
            .filter(|q| {
                free(q)
                    && ends(block, *q).contains(&Some(on))
                    && ends(block, *q).contains(&Some(inn))
            })
            .collect();
        if feedback.is_empty() {
            continue;
        }
        let inputs: Vec<usize> = order
            .iter()
            .copied()
            .filter(|q| {
                free(q)
                    && !feedback.contains(q)
                    && ends(block, *q).contains(&Some(inn))
                    && ends(block, *q)
                        .iter()
                        .all(|n| *n == Some(inn) || signal(*n))
            })
            .take(1)
            .collect();
        let mut parts = feedback;
        parts.extend(inputs);
        used.extend(parts.iter().copied());
        out.push(Idiom {
            kind: IdiomKind::OpampStage,
            parts,
            anchor: Some((amp, ik)),
            note: None,
        });
    }
    // Crystals with their load capacitors.
    for &y in &order {
        if !is_crystal(block, y) || !allowed(choices, y, IdiomKind::Crystal) {
            continue;
        }
        let nets: Vec<usize> = block.parts[y]
            .pins
            .iter()
            .filter_map(|p| p.net)
            .filter(|n| block.nets[*n].kind == NetKind::Signal)
            .collect();
        if nets.len() != 2 {
            continue;
        }
        let mut parts = vec![y];
        for n in &nets {
            let cap = order.iter().copied().find(|c| {
                !used.contains(c)
                    && !parts.contains(c)
                    && is_capacitor(block, *c)
                    && allowed(choices, *c, IdiomKind::Crystal)
                    && {
                        let e = ends(block, *c);
                        (e[0] == Some(*n) && ground(e[1])) || (e[1] == Some(*n) && ground(e[0]))
                    }
            });
            parts.extend(cap);
        }
        let anchor = served(block, &by_net, nets[0], &parts);
        used.extend(parts.iter().copied());
        out.push(Idiom {
            kind: IdiomKind::Crystal,
            note: anchor.and_then(|(a, c)| note(c, block, a)),
            anchor: anchor.map(|(a, _)| a),
            parts,
        });
    }
    // Dividers: two resistors meeting at a net read by an active pin, the
    // top to a supply or signal, the bottom to ground.
    for &top in &order {
        if used.contains(&top)
            || !is_resistor(block, top)
            || !allowed(choices, top, IdiomKind::Divider)
        {
            continue;
        }
        let te = ends(block, top);
        for (mid_i, far_i) in [(1, 0), (0, 1)] {
            let (Some(mid), far) = (te[mid_i], te[far_i]) else {
                continue;
            };
            if !signal(Some(mid)) || ground(far) || far.is_none() {
                continue;
            }
            let bottom = order.iter().copied().find(|b| {
                *b != top
                    && !used.contains(b)
                    && is_resistor(block, *b)
                    && allowed(choices, *b, IdiomKind::Divider)
                    && {
                        let e = ends(block, *b);
                        (e[0] == Some(mid) && ground(e[1])) || (e[1] == Some(mid) && ground(e[0]))
                    }
            });
            if let Some(bottom) = bottom
                && let Some((a, count)) = served(block, &by_net, mid, &[top, bottom])
            {
                used.extend([top, bottom]);
                out.push(Idiom {
                    kind: IdiomKind::Divider,
                    parts: vec![top, bottom],
                    anchor: Some(a),
                    note: note(count, block, a),
                });
                break;
            }
        }
    }
    // Feedback: a resistor from an amplifier's output to its inverting
    // input.
    for &r in &order {
        if used.contains(&r) || !is_resistor(block, r) || !allowed(choices, r, IdiomKind::Feedback)
        {
            continue;
        }
        let e = ends(block, r);
        let (Some(a), Some(b)) = (e[0], e[1]) else {
            continue;
        };
        let amp = order.iter().copied().find(|q| {
            is_active(block, *q) && {
                let pins = &block.parts[*q].pins;
                let on = |n: usize, f: &dyn Fn(&str) -> bool| {
                    pins.iter().any(|p| p.net == Some(n) && f(&p.name))
                };
                let out = |s: &str| s.contains("OUT") || s == "~";
                let inv = |s: &str| s.ends_with('-') || s.contains("IN-") || s.contains("INN");
                (on(a, &out) && on(b, &inv)) || (on(b, &out) && on(a, &inv))
            }
        });
        if let Some(q) = amp {
            let k = block.parts[q]
                .pins
                .iter()
                .position(|p| p.net == Some(a) || p.net == Some(b))
                .expect("found on it");
            used.insert(r);
            out.push(Idiom {
                kind: IdiomKind::Feedback,
                parts: vec![r],
                anchor: Some((q, k)),
                note: None,
            });
        }
    }
    // Decoupling: a capacitor between a supply and ground, by the active
    // part using that supply with the fewest so far.
    let mut served_by: BTreeMap<usize, usize> = BTreeMap::new();
    let mut pin_served: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for &c in &order {
        if used.contains(&c)
            || !is_capacitor(block, c)
            || !allowed(choices, c, IdiomKind::Decoupling)
        {
            continue;
        }
        let e = ends(block, c);
        let s = if supply(e[0]) && ground(e[1]) {
            e[0]
        } else if supply(e[1]) && ground(e[0]) {
            e[1]
        } else {
            continue;
        };
        let Some(s) = s else { continue };
        let users: Vec<(usize, usize)> = by_net
            .get(&s)
            .map(|v| {
                v.iter()
                    .copied()
                    .filter(|(p, k)| {
                        is_active(block, *p)
                            && matches!(
                                block.parts[*p].pins[*k].kind,
                                PinKind::PowerIn
                                    | PinKind::PowerOut
                                    | PinKind::Passive
                                    | PinKind::Bidirectional
                            )
                    })
                    .collect()
            })
            .unwrap_or_default();
        // The part with the fewest so far, and of its pins on the supply
        // the one with the fewest, so each pin gets its own capacitor
        // before any gets a second.
        let Some(&best) = users.iter().min_by_key(|(p, k)| {
            (
                served_by.get(p).copied().unwrap_or(0),
                block.parts[*p].key.clone(),
                pin_served.get(&(*p, *k)).copied().unwrap_or(0),
                *k,
            )
        }) else {
            continue;
        };
        *served_by.entry(best.0).or_default() += 1;
        *pin_served.entry(best).or_default() += 1;
        used.insert(c);
        let distinct: BTreeSet<usize> = users.iter().map(|(p, _)| *p).collect();
        out.push(Idiom {
            kind: IdiomKind::Decoupling,
            parts: vec![c],
            anchor: Some(best),
            note: (distinct.len() > 1).then(|| {
                format!(
                    "{} parts use its supply; it serves {}, which had the fewest",
                    distinct.len(),
                    block.parts[best.0].key
                )
            }),
        });
    }
    // Pull-ups and pull-downs.
    for &r in &order {
        if used.contains(&r) || !is_resistor(block, r) {
            continue;
        }
        let e = ends(block, r);
        for (rail_i, sig_i) in [(0, 1), (1, 0)] {
            let kind = if supply(e[rail_i]) {
                IdiomKind::PullUp
            } else if ground(e[rail_i]) {
                IdiomKind::PullDown
            } else {
                continue;
            };
            if !signal(e[sig_i]) || !allowed(choices, r, kind) {
                continue;
            }
            if let Some((a, count)) = served(block, &by_net, e[sig_i].expect("signal"), &[r]) {
                used.insert(r);
                // A jumper strapping the same pin to the same rail (two
                // pins, a connector's prefix) stands beside the resistor:
                // drawn on the sheet's edge, the pin's wire would run
                // across everything beside the pin to reach it.
                let mut parts = vec![r];
                for &j in &order {
                    if !used.contains(&j)
                        && is_connector(block, j)
                        && block.parts[j].pins.len() == 2
                        && allowed(choices, j, kind)
                    {
                        let je = ends(block, j);
                        if (je[0] == e[0] && je[1] == e[1]) || (je[0] == e[1] && je[1] == e[0]) {
                            used.insert(j);
                            parts.push(j);
                        }
                    }
                }
                out.push(Idiom {
                    kind,
                    parts,
                    anchor: Some(a),
                    note: note(count, block, a),
                });
                break;
            }
        }
    }
    // An LED and its resistor: in series through a net only they share.
    for &d in &order {
        if used.contains(&d) || !is_led(block, d) || !allowed(choices, d, IdiomKind::LedResistor) {
            continue;
        }
        for n in ends(block, d).into_iter().flatten() {
            let on = by_net.get(&n).cloned().unwrap_or_default();
            if on.len() != 2 {
                continue;
            }
            let other = on.iter().map(|(p, _)| *p).find(|p| *p != d);
            if let Some(r) = other
                && !used.contains(&r)
                && is_resistor(block, r)
                && allowed(choices, r, IdiomKind::LedResistor)
            {
                used.extend([d, r]);
                out.push(Idiom {
                    kind: IdiomKind::LedResistor,
                    parts: vec![r, d],
                    anchor: None,
                    note: None,
                });
                break;
            }
        }
    }
    // RC and LC filters: a series part into a net whose capacitor goes to
    // ground, the net read beyond.
    for &s in &order {
        let kind = if is_resistor(block, s) {
            IdiomKind::RcFilter
        } else if is_inductor(block, s) {
            IdiomKind::LcFilter
        } else {
            continue;
        };
        if used.contains(&s) || !allowed(choices, s, kind) {
            continue;
        }
        let e = ends(block, s);
        for out_i in [1, 0] {
            let (Some(o), Some(i)) = (e[out_i], e[1 - out_i]) else {
                continue;
            };
            if !signal(Some(o)) || !signal(Some(i)) {
                continue;
            }
            let cap = order.iter().copied().find(|c| {
                !used.contains(c) && is_capacitor(block, *c) && allowed(choices, *c, kind) && {
                    let ce = ends(block, *c);
                    (ce[0] == Some(o) && ground(ce[1])) || (ce[1] == Some(o) && ground(ce[0]))
                }
            });
            let read = by_net.get(&o).is_some_and(|v| v.len() >= 3);
            if let Some(c) = cap
                && read
            {
                used.extend([s, c]);
                out.push(Idiom {
                    kind,
                    parts: vec![s, c],
                    anchor: None,
                    note: None,
                });
                break;
            }
        }
    }
    // Series terminations: a resistor on a net it shares only with one
    // driving pin.
    for &r in &order {
        if used.contains(&r)
            || !is_resistor(block, r)
            || !allowed(choices, r, IdiomKind::SeriesTermination)
        {
            continue;
        }
        for n in ends(block, r).into_iter().flatten() {
            let on = by_net.get(&n).cloned().unwrap_or_default();
            if on.len() != 2 || !signal(Some(n)) {
                continue;
            }
            let (q, k) = *on.iter().find(|(p, _)| *p != r).expect("two");
            let kind = block.parts[q].pins[k].kind;
            if is_active(block, q) && (kind.drives() || kind == PinKind::Bidirectional) {
                used.insert(r);
                out.push(Idiom {
                    kind: IdiomKind::SeriesTermination,
                    parts: vec![r],
                    anchor: Some((q, k)),
                    note: None,
                });
                break;
            }
        }
    }
    // Connectors, on their sheet's edge.
    for &j in &order {
        if !used.contains(&j) && is_connector(block, j) && allowed(choices, j, IdiomKind::Connector)
        {
            used.insert(j);
            out.push(Idiom {
                kind: IdiomKind::Connector,
                parts: vec![j],
                anchor: None,
                note: None,
            });
        }
    }
    out
}

/// The turn that points pin `k` of a part `want`.
fn turn_for(block: &Block, p: usize, k: usize, want: Side) -> (u16, bool) {
    let side = block.parts[p].pins[k].side;
    let mut r = 0u16;
    let mut s = side;
    while s != want {
        s = s.turned();
        r += 90;
    }
    (r, false)
}

/// The turn (and mirror) pointing pin `k` of part `p` `want` with it the
/// uppermost of the part's pins, else as `turn_for` turns it.
fn signal_on_top(block: &Block, p: usize, k: usize, want: Side) -> (u16, bool) {
    for mirror in [false, true] {
        for rot in [0, 90, 180, 270] {
            let sh = Shape::of(&block.parts[p], Place::new([0, 0], rot, mirror));
            if sh.pins[k].side == want && sh.pins.iter().all(|q| q.end[1] <= sh.pins[k].end[1]) {
                return (rot, mirror);
            }
        }
    }
    turn_for(block, p, k, want)
}

/// The place putting pin `k` of part `p`, turned so, at `at`.
fn put(block: &Block, p: usize, (rot, mirror): (u16, bool), k: usize, at: Point) -> Place {
    let place = Place::new([0, 0], rot, mirror);
    let e = Shape::of(&block.parts[p], place).pins[k].end;
    Place::new([at[0] - e[0], at[1] - e[1]], rot, mirror)
}

fn pin_on(block: &Block, p: usize, n: Option<usize>) -> usize {
    block.parts[p]
        .pins
        .iter()
        .position(|x| x.net == n && n.is_some())
        .unwrap_or(0)
}

/// `p` moved `by` (nanometres) towards `s`.
fn step(p: Point, s: Side, by: Nm) -> Point {
    let d = s.step();
    [p[0] + d[0] / GRID * by, p[1] + d[1] / GRID * by]
}

/// Makes the clusters: each idiom's parts joined to the cluster of the
/// part it serves, in their canonical shapes, else a cluster of their
/// own; every other part alone. `singles` holds every part's own cluster.
pub fn clusters(block: &Block, idioms: &[Idiom], singles: Vec<Cluster>) -> Vec<Cluster> {
    let mut of: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, c) in singles.iter().enumerate() {
        of.insert(c.first(), i);
    }
    let mut out: Vec<Option<Cluster>> = singles.into_iter().map(Some).collect();
    // Per anchor: how many of each kind joined so far, by pin side.
    let mut count: BTreeMap<(usize, IdiomKind, Side), Nm> = BTreeMap::new();
    let mut on_pin: BTreeMap<(usize, usize), Nm> = BTreeMap::new();
    // Decoupling capacitors are placed last, so each can stand past what
    // the other idioms put beside the pins below its own.
    let decoupling: BTreeSet<usize> = idioms
        .iter()
        .filter(|i| i.kind == IdiomKind::Decoupling)
        .flat_map(|i| i.parts.iter().copied())
        .collect();
    let later = |i: &&Idiom| i.kind == IdiomKind::Decoupling;
    for idiom in idioms
        .iter()
        .filter(|i| !later(i))
        .chain(idioms.iter().filter(later))
    {
        let ground = |n: Option<usize>| kind_of(block, n) == Some(NetKind::Ground);
        match idiom.anchor {
            Some((a, k)) if of.contains_key(&a) => {
                let ci = of[&a];
                let Some(cluster) = out[ci].clone() else {
                    continue;
                };
                let (_, rel) = cluster
                    .members
                    .iter()
                    .find(|(p, _)| *p == a)
                    .copied()
                    .expect("member");
                let shape = Shape::of(&block.parts[a], rel);
                let body = shape.body;
                let pin = shape.pins[k];
                let side = pin.side;
                let mut members = cluster.members.clone();
                let n = count.entry((a, idiom.kind, side)).or_default();
                let i = *n;
                *n += 1;
                let mut adds: Vec<(usize, Place)> = Vec::new();
                match idiom.kind {
                    IdiomKind::Decoupling => {
                        let c = idiom.parts[0];
                        let e = ends(block, c);
                        let top = if ground(e[0]) { 1 } else { 0 };
                        // Counted per pin: a pin's second capacitor stands
                        // further out along its line.
                        let n = on_pin.entry((a, k)).or_default();
                        let i = *n;
                        *n += 1;
                        // On the line out of the pin it serves: standing
                        // under that line a few steps out (beside the
                        // part, past its body, from a pin pointing up or
                        // down), so a wire runs from the pin to its top and
                        // the rail's port sits on that wire.
                        let at = if side.horizontal() {
                            // Past every other idiom's part beside a pin
                            // below this one on its side, so that pin's wire
                            // is taken off before the capacitors.
                            let reach = members
                                .iter()
                                .filter(|(q, _)| *q != a && !decoupling.contains(q))
                                .map(|(q, pl)| Cluster::single(*q, *pl).room(block, [0, 0]))
                                .filter(|r| r.min[1] < pin.end[1])
                                .filter_map(|r| match side {
                                    Side::Left if r.max[0] < pin.end[0] => {
                                        Some(pin.end[0] - r.min[0])
                                    }
                                    Side::Right if r.min[0] > pin.end[0] => {
                                        Some(r.max[0] - pin.end[0])
                                    }
                                    _ => None,
                                })
                                .max();
                            let first = reach.map_or(3, |r| (r + GRID - 1) / GRID + 3).max(3);
                            let at = step(pin.end, side, (first + 3 * i) * GRID);
                            [at[0], at[1] - 2 * GRID]
                        } else {
                            let x = snap(body.max[0]) + (3 + 3 * i) * GRID;
                            let y = match side {
                                Side::Up => pin.end[1] + GRID,
                                _ => pin.end[1] - 3 * GRID,
                            };
                            [x, y]
                        };
                        adds.push((c, put(block, c, turn_for(block, c, top, Side::Up), top, at)));
                    }
                    IdiomKind::PullUp | IdiomKind::PullDown => {
                        let r = idiom.parts[0];
                        let e = ends(block, r);
                        let rail = if kind_of(block, e[0]) == Some(NetKind::Signal) {
                            1
                        } else {
                            0
                        };
                        let up = idiom.kind == IdiomKind::PullUp;
                        let (dir, rail_side) = if up {
                            (Side::Up, Side::Up)
                        } else {
                            (Side::Down, Side::Down)
                        };
                        let turn = turn_for(block, r, rail, rail_side);
                        let sig = 1 - rail;
                        // Out from the pin, one column per resistor, the
                        // signal end two steps past the body's top or foot.
                        let edge = snap(if up {
                            shape.extent.max[1]
                        } else {
                            shape.extent.min[1]
                        });
                        let y = edge + dir.step()[1] / GRID * 3 * GRID;
                        let out_x = if side.horizontal() {
                            pin.end[0] + side.step()[0] / GRID * (5 + 3 * i) * GRID
                        } else {
                            pin.end[0] + (2 + 3 * i) * GRID
                        };
                        let rplace = put(block, r, turn, sig, [out_x, y]);
                        adds.push((r, rplace));
                        // Jumpers strapping the same pin to the same rail,
                        // pins towards the part, the signal pin uppermost:
                        // from a pin pointing sideways on that pin's own
                        // line past the resistor's room, so the pin's wire
                        // runs straight into them and the resistor hangs
                        // off it; else a column further out each, level
                        // with the resistor's signal end.
                        let up_grid = |v: Nm| (v + GRID - 1) / GRID * GRID;
                        let mut past = if side.horizontal() {
                            let rr = Cluster::single(r, rplace).room(block, [0, 0]);
                            match side {
                                Side::Left => pin.end[0] - rr.min[0],
                                _ => rr.max[0] - pin.end[0],
                            }
                        } else {
                            0
                        };
                        for (m, &j) in idiom.parts[1..].iter().enumerate() {
                            let jk = pin_on(block, j, e[sig]);
                            if side.horizontal() {
                                let jturn = signal_on_top(block, j, jk, side.opposite());
                                let rj = Cluster::single(j, put(block, j, jturn, jk, [0, 0]))
                                    .room(block, [0, 0]);
                                // How far its room reaches back towards the
                                // part from its pin, and on past it.
                                let (back, on) = match side {
                                    Side::Left => (rj.max[0], -rj.min[0]),
                                    _ => (-rj.min[0], rj.max[0]),
                                };
                                let by = up_grid(past + back.max(0)) + 2 * GRID;
                                past = by + on.max(0);
                                adds.push((j, put(block, j, jturn, jk, step(pin.end, side, by))));
                            } else {
                                let at = step([out_x, y], Side::Right, 3 * (m as Nm + 1) * GRID);
                                adds.push((
                                    j,
                                    put(block, j, signal_on_top(block, j, jk, Side::Left), jk, at),
                                ));
                            }
                        }
                        *count.entry((a, idiom.kind, side)).or_default() +=
                            idiom.parts.len() as Nm - 1;
                    }
                    IdiomKind::SeriesTermination => {
                        let r = idiom.parts[0];
                        let near = pin_on(block, r, block.parts[a].pins[k].net);
                        let turn = turn_for(block, r, near, side.opposite());
                        adds.push((r, put(block, r, turn, near, step(pin.end, side, 2 * GRID))));
                    }
                    IdiomKind::Feedback => {
                        let r = idiom.parts[0];
                        let turn = turn_for(block, r, 0, Side::Left);
                        let mid = snap((body.min[0] + body.max[0]) / 2);
                        let at = [mid - 3 * GRID, snap(body.max[1]) + 4 * GRID + i * 4 * GRID];
                        adds.push((r, put(block, r, turn, 0, at)));
                    }
                    IdiomKind::Divider => {
                        let (top, bottom) = (idiom.parts[0], idiom.parts[1]);
                        let mid = block.parts[a].pins[k].net;
                        let tk = pin_on(block, top, mid);
                        let bk = pin_on(block, bottom, mid);
                        let x = step(pin.end, side, (4 + 4 * i) * GRID)[0];
                        let y = pin.end[1];
                        adds.push((
                            top,
                            put(
                                block,
                                top,
                                turn_for(block, top, tk, Side::Down),
                                tk,
                                [x, y + GRID],
                            ),
                        ));
                        adds.push((
                            bottom,
                            put(
                                block,
                                bottom,
                                turn_for(block, bottom, bk, Side::Up),
                                bk,
                                [x, y - GRID],
                            ),
                        ));
                    }
                    IdiomKind::Crystal => {
                        let y = idiom.parts[0];
                        let out_x = step(pin.end, side, 6 * GRID)[0];
                        let p1 = block.parts[y]
                            .pins
                            .iter()
                            .position(|p| p.net.is_some())
                            .unwrap_or(0);
                        let turn = turn_for(block, y, p1, Side::Up);
                        let place = put(block, y, turn, p1, [out_x, pin.end[1] + GRID]);
                        adds.push((y, place));
                        let ys = Shape::of(&block.parts[y], place);
                        for (j, &c) in idiom.parts[1..].iter().enumerate() {
                            let e = ends(block, c);
                            let top = if ground(e[0]) { 1 } else { 0 };
                            let target_net = block.parts[c].pins[top].net;
                            let kk = block.parts[y]
                                .pins
                                .iter()
                                .position(|p| p.net == target_net)
                                .unwrap_or(0);
                            let at = step(ys.pins[kk].end, side, (3 + 3 * j as Nm) * GRID);
                            let at = [at[0], at[1] - 2 * GRID];
                            adds.push((
                                c,
                                put(block, c, turn_for(block, c, top, Side::Up), top, at),
                            ));
                        }
                    }
                    IdiomKind::Buck => {
                        // The inductor across the page out of the switch
                        // pin, its switch end four steps out.
                        let sw = block.parts[a].pins[k].net;
                        let l = idiom.parts[0];
                        let lk = pin_on(block, l, sw);
                        let l_place = put(
                            block,
                            l,
                            turn_for(block, l, lk, side.opposite()),
                            lk,
                            step(pin.end, side, 4 * GRID),
                        );
                        let far = Shape::of(&block.parts[l], l_place).pins[1 - lk].end;
                        adds.push((l, l_place));
                        let mut caps = 0;
                        for &q in &idiom.parts[1..] {
                            if two_pin(block, q, &["D"]) {
                                // The diode standing under the switch node,
                                // its switch end up, two steps out and down.
                                let dk = pin_on(block, q, sw);
                                let at = step(pin.end, side, 2 * GRID);
                                adds.push((
                                    q,
                                    put(
                                        block,
                                        q,
                                        turn_for(block, q, dk, Side::Up),
                                        dk,
                                        [at[0], at[1] - 2 * GRID],
                                    ),
                                ));
                            } else {
                                // An output capacitor standing on the
                                // inductor's far end, one column each.
                                let e = ends(block, q);
                                let top = if ground(e[0]) { 1 } else { 0 };
                                let at = step(far, side, (3 + 3 * caps) * GRID);
                                caps += 1;
                                adds.push((
                                    q,
                                    put(
                                        block,
                                        q,
                                        turn_for(block, q, top, Side::Up),
                                        top,
                                        [at[0], at[1] - 2 * GRID],
                                    ),
                                ));
                            }
                        }
                    }
                    IdiomKind::OpampStage => {
                        // Feedback elements across the page above the
                        // amplifier, one row each; the input element across
                        // the page into the inverting input.
                        let pins = &block.parts[a].pins;
                        let on = pins
                            .iter()
                            .position(crate::topology::out_pin)
                            .and_then(|o| pins[o].net);
                        let inn = pins[k].net;
                        let mid = snap((body.min[0] + body.max[0]) / 2);
                        let mut row = 0;
                        for &q in &idiom.parts {
                            if ends(block, q).contains(&on) {
                                let qi = pin_on(block, q, inn);
                                let at = [
                                    mid - 3 * GRID,
                                    snap(shape.extent.max[1]) + (4 + 4 * row) * GRID,
                                ];
                                row += 1;
                                adds.push((
                                    q,
                                    put(block, q, turn_for(block, q, qi, Side::Left), qi, at),
                                ));
                            } else {
                                let qi = pin_on(block, q, inn);
                                adds.push((
                                    q,
                                    put(
                                        block,
                                        q,
                                        turn_for(block, q, qi, side.opposite()),
                                        qi,
                                        step(pin.end, side, 2 * GRID),
                                    ),
                                ));
                            }
                        }
                    }
                    IdiomKind::LedResistor
                    | IdiomKind::RcFilter
                    | IdiomKind::LcFilter
                    | IdiomKind::Connector => {}
                }
                for (p, place) in adds {
                    let place = clear_of(block, &members, p, place, side);
                    if let Some(old) = of.get(&p).copied()
                        && old != ci
                    {
                        out[old] = None;
                    }
                    members.push((p, place));
                    of.insert(p, ci);
                }
                out[ci] = Some(Cluster {
                    members,
                    idiom: cluster.idiom,
                });
            }
            _ => {
                let made = standalone(block, idiom);
                let Some(made) = made else { continue };
                let home = of[&made.members[0].0];
                for (p, _) in &made.members {
                    if let Some(old) = of.get(p).copied() {
                        out[old] = None;
                    }
                    of.insert(*p, home);
                }
                out[home] = Some(made);
            }
        }
    }
    // An out port's flag whose net one pin drives, a pin pointing right
    // (an output), stands on that pin's line a few steps out, so the
    // output is a short wire and whatever else is on it joins that wire.
    for (p, part) in block.parts.iter().enumerate() {
        if !matches!(part.kind, PartKind::Port { flow: Flow::Out }) {
            continue;
        }
        let Some((d, k)) = crate::place::sole_driver(block, p) else {
            continue;
        };
        let (Some(&ci), Some(&pi)) = (of.get(&d), of.get(&p)) else {
            continue;
        };
        if ci == pi || out[pi].as_ref().is_none_or(|c| c.members.len() != 1) {
            continue;
        }
        let Some(mut cluster) = out[ci].clone() else {
            continue;
        };
        let rel = cluster
            .members
            .iter()
            .find(|(q, _)| *q == d)
            .expect("member")
            .1;
        let pin = Shape::of(&block.parts[d], rel).pins[k];
        if pin.side != Side::Right {
            continue;
        }
        let place = put(
            block,
            p,
            (0, false),
            0,
            step(pin.end, Side::Right, 6 * GRID),
        );
        let place = clear_of(block, &cluster.members, p, place, Side::Right);
        cluster.members.push((p, place));
        out[pi] = None;
        of.insert(p, ci);
        out[ci] = Some(cluster);
    }
    out.into_iter().flatten().collect()
}

/// `place` moved out along `side` (and then down) a step at a time until
/// its part's room keeps a step from every member's.
fn clear_of(
    block: &Block,
    members: &[(usize, Place)],
    p: usize,
    place: Place,
    side: Side,
) -> Place {
    // Each part's room: its extent, its text and its rails' ports.
    let room = |pl: Place| Cluster::single(p, pl).room(block, [0, 0]).grow(GRID);
    let others: Vec<Rect> = members
        .iter()
        .map(|(q, pl)| Cluster::single(*q, *pl).room(block, [0, 0]))
        .collect();
    // Out along the pin's way first, keeping its row; then down or up.
    let tries = (0..40)
        .map(|k| (k, side))
        .chain((1..40).flat_map(|k| [(k, Side::Down), (k, Side::Up)]));
    for (k, d) in tries {
        {
            let off = d.step();
            let pl = Place::new(
                [place.at[0] + off[0] * k, place.at[1] + off[1] * k],
                place.rotation,
                place.mirror,
            );
            if !others.iter().any(|o| o.overlaps(&room(pl))) {
                return pl;
            }
        }
    }
    place
}

/// An idiom of its own: an LED and its resistor in a column, a filter's
/// series part and its capacitor below its output, a connector alone.
fn standalone(block: &Block, idiom: &Idiom) -> Option<Cluster> {
    let ground = |n: Option<usize>| kind_of(block, n) == Some(NetKind::Ground);
    match idiom.kind {
        IdiomKind::LedResistor => {
            let (r, d) = (idiom.parts[0], idiom.parts[1]);
            // The LED's cathode down: anode (pin 1) up.
            let shared = ends(block, r)
                .into_iter()
                .flatten()
                .find(|n| ends(block, d).contains(&Some(*n)))?;
            let dk = pin_on(block, d, Some(shared));
            let rk = pin_on(block, r, Some(shared));
            let led_down = ground(ends(block, d)[1 - dk]);
            let (upper, uk, lower, lk) = if led_down {
                (r, rk, d, dk)
            } else {
                (d, dk, r, rk)
            };
            let up = put(
                block,
                upper,
                turn_for(block, upper, uk, Side::Down),
                uk,
                [0, 0],
            );
            let low = put(
                block,
                lower,
                turn_for(block, lower, lk, Side::Up),
                lk,
                [0, -2 * GRID],
            );
            Some(Cluster {
                members: vec![(upper, up), (lower, low)],
                idiom: Some(IdiomKind::LedResistor),
            })
        }
        IdiomKind::RcFilter | IdiomKind::LcFilter => {
            let (s, c) = (idiom.parts[0], idiom.parts[1]);
            let shared = ends(block, s)
                .into_iter()
                .flatten()
                .find(|n| ends(block, c).contains(&Some(*n)))?;
            let sk = pin_on(block, s, Some(shared));
            let ck = pin_on(block, c, Some(shared));
            let series = put(block, s, turn_for(block, s, sk, Side::Right), sk, [0, 0]);
            let cap = put(
                block,
                c,
                turn_for(block, c, ck, Side::Up),
                ck,
                [2 * GRID, -2 * GRID],
            );
            Some(Cluster {
                members: vec![(s, series), (c, cap)],
                idiom: Some(idiom.kind),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Net;
    use crate::testing::{capacitor, ic, led, net, resistor};

    fn block() -> Block {
        let nets: Vec<Net> = vec![
            net("rail:vcc", "3V3", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("sda", "SDA", NetKind::Signal),
            net("led", "LED", NetKind::Signal),
            net("io", "IO", NetKind::Signal),
        ];
        Block {
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "MCU",
                    &[
                        ("VDD", PinKind::PowerIn, Some(0)),
                        ("SDA", PinKind::Bidirectional, Some(2)),
                    ],
                    &[
                        ("GND", PinKind::PowerIn, Some(1)),
                        ("IO", PinKind::Output, Some(4)),
                    ],
                ),
                capacitor("c1", "C1", "100n", [Some(0), Some(1)]),
                capacitor("c2", "C2", "100n", [Some(1), Some(0)]),
                resistor("r1", "R1", "4k7", [Some(0), Some(2)]),
                resistor("r2", "R2", "1k", [Some(4), Some(3)]),
                led("d1", "D1", "red", [Some(3), Some(1)]),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        }
    }

    #[test]
    fn patterns_are_found_in_a_fixed_order() {
        let b = block();
        let found: Vec<(IdiomKind, Vec<&str>)> = find(&b, &BTreeMap::new())
            .iter()
            .map(|i| {
                (
                    i.kind,
                    i.parts.iter().map(|p| b.parts[*p].key.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (IdiomKind::Decoupling, vec!["c1"]),
                (IdiomKind::Decoupling, vec!["c2"]),
                (IdiomKind::PullUp, vec!["r1"]),
                (IdiomKind::LedResistor, vec!["r2", "d1"]),
            ]
        );
        // An instruction keeps one out.
        let none: BTreeMap<usize, IdiomChoice> = [(3, IdiomChoice::None)].into();
        assert!(find(&b, &none).iter().all(|i| !i.parts.contains(&3)));
    }

    #[test]
    fn satellites_join_their_anchor_clear_of_each_other() {
        let b = block();
        let idioms = find(&b, &BTreeMap::new());
        let singles = (0..b.parts.len())
            .map(|p| Cluster::single(p, Place::default()))
            .collect();
        let cl = clusters(&b, &idioms, singles);
        // U1 with its two caps and its pull-up; the LED column; nothing
        // else.
        assert_eq!(cl.len(), 2);
        let u1 = cl.iter().find(|c| c.first() == 0).unwrap();
        assert_eq!(u1.members.len(), 4);
        let rects: Vec<Rect> = u1
            .members
            .iter()
            .map(|(p, pl)| Shape::of(&b.parts[*p], *pl).extent)
            .collect();
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                assert!(!rects[i].overlaps(&rects[j]), "{i} {j}");
            }
        }
        // Caps stand with their supply up, out along the line of the pin
        // they serve.
        let c2 = u1.members.iter().find(|(p, _)| *p == 2).unwrap().1;
        let s = Shape::of(&b.parts[2], c2);
        assert_eq!(s.pins[1].side, Side::Up);
        let (a, k) = idioms
            .iter()
            .find(|i| i.parts == [2])
            .unwrap()
            .anchor
            .unwrap();
        let ua = u1.members.iter().find(|(p, _)| *p == a).unwrap().1;
        let served = Shape::of(&b.parts[a], ua).pins[k];
        let out = if served.side.horizontal() {
            (s.pins[1].end[0] - served.end[0]) * served.side.step()[0].signum()
        } else {
            s.pins[1].end[0] - rects[0].max[0]
        };
        assert!(out >= 3 * GRID, "{out}");
        assert!(s.pins[1].end[1] < served.end[1] || !served.side.horizontal());
    }

    #[test]
    fn a_bypass_capacitor_stands_past_a_lower_pins_pull_down() {
        // U1's VIN over EN, both pointing left; C1 decouples VIN and R1
        // pulls EN down. EN's wire must be taken off before C1, so C1
        // stands farther out than all of R1.
        let block = Block {
            nets: vec![
                net("rail:v", "+5V", NetKind::Supply),
                net("rail:gnd", "GND", NetKind::Ground),
                net("en", "EN", NetKind::Signal),
            ],
            parts: vec![
                ic(
                    "u0",
                    "U1",
                    "x",
                    &[
                        ("VIN", PinKind::PowerIn, Some(0)),
                        ("EN", PinKind::Input, Some(2)),
                    ],
                    &[("GND", PinKind::PowerIn, Some(1))],
                ),
                capacitor("c0", "C1", "10uF", [Some(0), Some(1)]),
                resistor("r0", "R1", "10k", [Some(1), Some(2)]),
            ],
            ..Block::default()
        };
        let idioms = find(&block, &BTreeMap::new());
        let singles = (0..block.parts.len())
            .map(|p| Cluster::single(p, Place::new([0, 0], 0, false)))
            .collect();
        let c = clusters(&block, &idioms, singles);
        let u = c
            .iter()
            .find(|c| c.members.iter().any(|(p, _)| *p == 0))
            .unwrap();
        let at = |p: usize| u.members.iter().find(|(q, _)| *q == p).unwrap().1;
        let room = |p: usize| Cluster::single(p, at(p)).room(&block, [0, 0]);
        // VIN points left: the capacitor stands farther left than all of R1.
        assert!(
            room(1).max[0] < room(2).min[0],
            "{:?} {:?}",
            room(1),
            room(2)
        );
    }

    #[test]
    fn an_opamp_stage_is_matched_and_placed_in_its_shape() {
        // U1: OUT, IN-, IN+; R1 and C1 from OUT to IN- (feedback), R2 from
        // a signal into IN- (input).
        let block = Block {
            nets: vec![
                net("out", "OUT", NetKind::Signal),
                net("inn", "INN", NetKind::Signal),
                net("in", "IN", NetKind::Signal),
                net("inp", "INP", NetKind::Signal),
            ],
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "OPA",
                    &[
                        ("IN-", PinKind::Input, Some(1)),
                        ("IN+", PinKind::Input, Some(3)),
                    ],
                    &[("OUT", PinKind::Output, Some(0))],
                ),
                resistor("r1", "R1", "10k", [Some(1), Some(0)]),
                capacitor("c1", "C1", "100p", [Some(0), Some(1)]),
                resistor("r2", "R2", "1k", [Some(2), Some(1)]),
            ],
            ..Block::default()
        };
        let idioms = find(&block, &BTreeMap::new());
        assert!(
            idioms.iter().all(|i| i.kind != IdiomKind::Feedback),
            "{idioms:?}"
        );
        let stage: Vec<&Idiom> = idioms
            .iter()
            .filter(|i| i.kind == IdiomKind::OpampStage)
            .collect();
        assert_eq!(stage.len(), 1, "{idioms:?}");
        let keys: Vec<&str> = stage[0]
            .parts
            .iter()
            .map(|p| block.parts[*p].key.as_str())
            .collect();
        assert_eq!(keys, ["c1", "r1", "r2"]);
        assert_eq!(stage[0].anchor, Some((0, 0)));
        let singles = (0..block.parts.len())
            .map(|p| Cluster::single(p, Place::new([0, 0], 0, false)))
            .collect();
        let c = clusters(&block, &idioms, singles);
        let u = c
            .iter()
            .find(|c| c.members.iter().any(|(p, _)| *p == 0))
            .unwrap();
        let at = |p: usize| u.members.iter().find(|(q, _)| *q == p).unwrap().1;
        let amp = Shape::of(&block.parts[0], at(0));
        for p in [1, 2] {
            let sh = Shape::of(&block.parts[p], at(p));
            // Across the page, above the amplifier.
            assert_eq!(sh.pins[0].end[1], sh.pins[1].end[1], "{p} across");
            assert!(sh.body.min[1] > amp.extent.max[1], "{p} above");
        }
        let r2 = Shape::of(&block.parts[3], at(3));
        assert_eq!(r2.pins[0].end[1], r2.pins[1].end[1], "R2 across");
        assert!((r2.body.min[0] + r2.body.max[0]) / 2 < amp.pins[0].end[0]);
    }
}
