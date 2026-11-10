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

/// A resistor of two pins: prefix R, or an array's unit (RN, RM, RA...),
/// as `lint::resistor_style` reads one.
fn is_resistor(block: &Block, p: usize) -> bool {
    let pre = prefix(block, p);
    block.parts[p].pins.len() == 2
        && (pre == "R" || ["RN", "RM", "RA"].iter().any(|x| pre.starts_with(x)))
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
/// pin, by part key, of more than `region::DENSE` pins where one is on the
/// net, else of any; and how many it chose among.
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
    // The part of more than `region::DENSE` pins among them first: a bank
    // stands on its rows (plan 2.B).
    let first = list
        .iter()
        .copied()
        .find(|(p, _)| block.parts[*p].pins.len() > crate::region::DENSE)
        .or_else(|| list.first().copied())?;
    Some((first, list.len()))
}

/// A bank: two or more pull-ups and pull-downs (their jumpers with them)
/// on one sideways side of a part of more than `region::DENSE` pins,
/// each to stand across the page on its pin's row (design 348's figure
/// 2, option A).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bank {
    pub part: usize,
    pub side: Side,
    /// Its rows, top to bottom: each pin and the indices (into the idioms
    /// given) of the pulls on it, resistors first, then by part key.
    pub rows: Vec<(usize, Vec<usize>)>,
}

/// The banks of `idioms`, each anchor placed as `at` says (in the frame
/// its cluster places it in).
pub fn banks(block: &Block, idioms: &[Idiom], at: &BTreeMap<usize, Place>) -> Vec<Bank> {
    let mut by: BTreeMap<(usize, Side), BTreeMap<usize, Vec<usize>>> = BTreeMap::new();
    for (i, idiom) in idioms.iter().enumerate() {
        if !matches!(idiom.kind, IdiomKind::PullUp | IdiomKind::PullDown) {
            continue;
        }
        let Some((a, k)) = idiom.anchor else { continue };
        if !matches!(block.parts[a].kind, PartKind::Symbol { .. })
            || block.parts[a].pins.len() <= crate::region::DENSE
        {
            continue;
        }
        let Some(place) = at.get(&a) else { continue };
        let side = Shape::of(&block.parts[a], *place).pins[k].side;
        if side.horizontal() {
            by.entry((a, side))
                .or_default()
                .entry(k)
                .or_default()
                .push(i);
        }
    }
    let mut out = Vec::new();
    for ((a, side), rows) in by {
        if rows.values().map(Vec::len).sum::<usize>() < 2 {
            continue;
        }
        let shape = Shape::of(&block.parts[a], at[&a]);
        let mut rows: Vec<(usize, Vec<usize>)> = rows.into_iter().collect();
        for (_, list) in &mut rows {
            list.sort_by_key(|i| {
                let p = idioms[*i].parts[0];
                (!is_resistor(block, p), block.parts[p].key.clone())
            });
        }
        rows.sort_by_key(|(k, _)| (std::cmp::Reverse(shape.pins[*k].end[1]), *k));
        out.push(Bank {
            part: a,
            side,
            rows,
        });
    }
    out
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
    // inductor on its net, the catch diode on it if there is one, the
    // capacitors from the inductor's far end to ground, and the bootstrap
    // capacitor from the switch node to another of the part's pins (its
    // BOOT), if there is one.
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
            let own = |n: Option<usize>| {
                n.is_some_and(|n| {
                    signal(Some(n))
                        && block.parts[ic]
                            .pins
                            .iter()
                            .enumerate()
                            .any(|(j, x)| j != k && x.net == Some(n))
                })
            };
            parts.extend(order.iter().copied().find(|c| {
                !used.contains(c)
                    && !parts.contains(c)
                    && is_capacitor(block, *c)
                    && allowed(choices, *c, IdiomKind::Buck)
                    && {
                        let ce = ends(block, *c);
                        (ce[0] == Some(sw) && own(ce[1])) || (ce[1] == Some(sw) && own(ce[0]))
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
        // A capacitor the previous layout wired to some of these pins (its
        // bypass wire) chooses among those alone, so deleting or adding
        // another capacitor reassigns none (and over an unedited layout it
        // chooses as it did: its choice was the least of all, and is
        // among them).
        let wired = wired_before(block, c, s, &users);
        let all = users.clone();
        let users = if wired.is_empty() { users } else { wired };
        // Else the part with the fewest so far, and of its pins on the
        // supply the one with the fewest, so each pin gets its own
        // capacitor before any gets a second.
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
        let distinct: BTreeSet<usize> = all.iter().map(|(p, _)| *p).collect();
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
    // Local circuits: a two-pin part no idiom took, with a signal net that
    // reaches one active part's pin and otherwise only two-pin parts,
    // belongs beside that pin, so
    // the net is a short wire: standing off it as a pull-up or pull-down
    // does where its other end is a rail, else across from it as a series
    // element (its local end two steps out). A series element's far net
    // is local to it in turn (a compensation network's C and R).
    let mut series: BTreeSet<usize> = BTreeSet::new();
    // Any two-pin symbol (resistors, capacitors, diodes, switches,
    // jumpers) but a connector.
    let small = |p: usize| {
        block.parts[p].pins.len() == 2
            && matches!(block.parts[p].kind, PartKind::Symbol { .. })
            && !is_connector(block, p)
    };
    for &q in order.iter().chain(order.iter()) {
        if used.contains(&q) || !small(q) {
            continue;
        }
        let e = ends(block, q);
        for (mine, other) in [(0, 1), (1, 0)] {
            let Some(n) = e[mine].filter(|n| signal(Some(*n))) else {
                continue;
            };
            // The net's one active pin (or series element), every other
            // pin on it a small part's (two pins at most) or a port's.
            let on = by_net.get(&n).cloned().unwrap_or_default();
            let passive = |p: usize| {
                matches!(block.parts[p].kind, PartKind::Port { .. })
                    || (block.parts[p].pins.len() <= 2 && !is_active(block, p))
            };
            let mut heads: Vec<(usize, usize)> = on
                .iter()
                .copied()
                .filter(|(p, _)| *p != q && is_active(block, *p))
                .collect();
            if heads.is_empty() {
                heads = on
                    .iter()
                    .copied()
                    .filter(|(p, _)| *p != q && series.contains(p))
                    .collect();
            }
            let [(a, k)] = heads[..] else {
                continue;
            };
            if on.iter().any(|(p, _)| *p != q && *p != a && !passive(*p)) {
                continue;
            }
            let kind = if supply(e[other]) {
                IdiomKind::PullUp
            } else if ground(e[other]) {
                IdiomKind::PullDown
            } else {
                IdiomKind::SeriesTermination
            };
            if !allowed(choices, q, kind) {
                continue;
            }
            used.insert(q);
            if kind == IdiomKind::SeriesTermination {
                series.insert(q);
            }
            out.push(Idiom {
                kind,
                parts: vec![q],
                anchor: Some((a, k)),
                note: None,
            });
            break;
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

/// The pins of `users` the previous layout's wires of `net` joined
/// capacitor `c` to, on its sheet.
fn wired_before(
    block: &Block,
    c: usize,
    net: usize,
    users: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    wired_to(block, c, net, users).unwrap_or_default()
}

fn wired_to(
    block: &Block,
    c: usize,
    net: usize,
    users: &[(usize, usize)],
) -> Option<Vec<(usize, usize)>> {
    let prev = block.previous.as_ref()?;
    let at = prev.placed.get(&block.parts[c].key)?;
    let runs = prev.routes.get(&block.nets[net].key)?.get(&at.sheet)?;
    let end_of = |p: usize, k: usize| -> Option<crate::route::Node> {
        let x = prev.placed.get(&block.parts[p].key)?;
        (x.sheet == at.sheet).then(|| {
            crate::route::node_of(
                Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror)).pins[k].end,
            )
        })
    };
    let ck = block.parts[c]
        .pins
        .iter()
        .position(|x| x.net == Some(net))?;
    // The runs joined to the capacitor's pin, through shared nodes.
    let sets: Vec<BTreeSet<crate::route::Node>> = runs
        .iter()
        .map(|r| crate::route::expand(r).into_iter().collect())
        .collect();
    let mut reached: BTreeSet<crate::route::Node> = [end_of(c, ck)?].into();
    let mut taken = vec![false; sets.len()];
    loop {
        let mut grew = false;
        for (i, set) in sets.iter().enumerate() {
            if !taken[i] && set.iter().any(|n| reached.contains(n)) {
                taken[i] = true;
                reached.extend(set.iter().copied());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    Some(
        users
            .iter()
            .copied()
            .filter(|&(p, k)| end_of(p, k).is_some_and(|n| reached.contains(&n)))
            .collect(),
    )
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
    let mut on_side: BTreeMap<(usize, Side), Nm> = BTreeMap::new();
    // Each part's pull-ups and pull-downs by pin, with how many parts each
    // brings (a resistor and its jumpers).
    let mut pulls: BTreeMap<(usize, IdiomKind), Vec<(usize, usize)>> = BTreeMap::new();
    for i in idioms
        .iter()
        .filter(|i| matches!(i.kind, IdiomKind::PullUp | IdiomKind::PullDown))
    {
        if let Some((a, k)) = i.anchor {
            pulls
                .entry((a, i.kind))
                .or_default()
                .push((k, i.parts.len()));
        }
    }
    // Each pin's decoupling capacitors.
    let mut served_caps: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for i in idioms.iter().filter(|i| i.kind == IdiomKind::Decoupling) {
        if let Some(a) = i.anchor {
            served_caps.entry(a).or_default().push(i.parts[0]);
        }
    }
    // Decoupling capacitors are placed last, so each can stand past what
    // the other idioms put beside the pins below its own.
    let decoupling: BTreeSet<usize> = idioms
        .iter()
        .filter(|i| i.kind == IdiomKind::Decoupling)
        .flat_map(|i| i.parts.iter().copied())
        .collect();
    let later = |i: &&Idiom| i.kind == IdiomKind::Decoupling;
    // What hangs off sideways pins stands in stairs before decoupling is
    // placed, which then stands past it.
    let mut stairs_done = false;
    for idiom in idioms
        .iter()
        .filter(|i| !later(i))
        .chain(idioms.iter().filter(later))
    {
        if !stairs_done && later(&idiom) {
            hanging_stairs(block, idioms, &mut out);
            stairs_done = true;
        }
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
                // The way a part is moved to clear the others.
                let mut clear = side;
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
                            // One part's capacitors on one side stand in one
                            // row, under its lowest decoupled pin there: the
                            // lowest pin's nearest, each higher pin's further
                            // out, so every wire is an L and none crosses
                            // another.
                            let decoupled: Vec<usize> = (0..shape.pins.len())
                                .filter(|kk| {
                                    shape.pins[*kk].side == side
                                        && served_caps.contains_key(&(a, *kk))
                                })
                                .collect();
                            let below = |kk: usize| {
                                let (ya, yb) = (shape.pins[kk].end[1], pin.end[1]);
                                ya < yb || (ya == yb && kk < k)
                            };
                            let lower: usize = decoupled
                                .iter()
                                .filter(|kk| below(**kk))
                                .map(|kk| served_caps[&(a, *kk)].len())
                                .sum();
                            // One slot a capacitor, as wide as the widest's
                            // room (its text beside it) and a step; the
                            // first far enough out that a room reaching back
                            // towards the part keeps a step clear of the pin.
                            let rooms: Vec<Rect> = decoupled
                                .iter()
                                .flat_map(|kk| served_caps[&(a, *kk)].iter())
                                .map(|&q| {
                                    let qe = ends(block, q);
                                    let qt = if ground(qe[0]) { 1 } else { 0 };
                                    let pl =
                                        put(block, q, turn_for(block, q, qt, Side::Up), qt, [0, 0]);
                                    Cluster::single(q, pl).room(block, [0, 0])
                                })
                                .collect();
                            let up_steps = |v: Nm| (v.max(0) + GRID - 1) / GRID;
                            let slot = rooms
                                .iter()
                                .map(|r| up_steps(r.max[0] - r.min[0]) + 1)
                                .max()
                                .unwrap_or(3)
                                .max(3);
                            let back = rooms
                                .iter()
                                .map(|r| match side {
                                    Side::Left => up_steps(r.max[0]),
                                    _ => up_steps(-r.min[0]),
                                })
                                .max()
                                .unwrap_or(0);
                            let base = decoupled
                                .iter()
                                .map(|kk| shape.pins[*kk].end[1])
                                .min()
                                .unwrap_or(pin.end[1])
                                .min(pin.end[1]);
                            // Past every other idiom's part beside a pin
                            // below the row's line on its side, so that
                            // pin's wire is taken off before the capacitors.
                            let reach = members
                                .iter()
                                .filter(|(q, _)| *q != a && !decoupling.contains(q))
                                .map(|(q, pl)| Cluster::single(*q, *pl).room(block, [0, 0]))
                                .filter(|r| r.min[1] < base)
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
                            // And past the part's own room (its rails' ports
                            // stand out past its pins).
                            let own = members
                                .iter()
                                .find(|(q, _)| *q == a)
                                .map(|(q, pl)| Cluster::single(*q, *pl).room(block, [0, 0]))
                                .map(|r| match side {
                                    Side::Left => pin.end[0] - r.min[0],
                                    _ => r.max[0] - pin.end[0],
                                })
                                .filter(|d| *d > 0);
                            let reach = reach.max(own);
                            // A capacitor's text faces the part, so the row
                            // starts that far past whatever stands out there.
                            let first = (up_steps(reach.unwrap_or(0)) + back + 2).max(3);
                            let at = step(pin.end, side, (first + slot * (lower as Nm + i)) * GRID);
                            [at[0], base - 2 * GRID]
                        } else {
                            // Beside the part past its body, one row for all
                            // its capacitors on that side.
                            let n = on_side.entry((a, side)).or_default();
                            let j = *n;
                            *n += 1;
                            clear = Side::Right;
                            let x = snap(body.max[0]) + (3 + 3 * j) * GRID;
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
                        // From a pin pointing sideways, in stairs: the pin
                        // nearest the rail's side (the lowest for a
                        // pull-down) nearest the part, each further pin's a
                        // column further out, so no pin's wire crosses
                        // another's drop.
                        let col = if side.horizontal() {
                            pulls
                                .get(&(a, idiom.kind))
                                .into_iter()
                                .flatten()
                                .filter(|(kk, _)| {
                                    let pk = shape.pins[*kk];
                                    let (yk, y0) = (pk.end[1], pin.end[1]);
                                    pk.side == side
                                        && (if up { yk > y0 } else { yk < y0 }
                                            || (yk == y0 && *kk < k))
                                })
                                .map(|(_, np)| *np as Nm)
                                .sum()
                        } else {
                            i
                        };
                        let out_x = if side.horizontal() {
                            pin.end[0] + side.step()[0] / GRID * (5 + 3 * col) * GRID
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
                            let qe = ends(block, q);
                            if is_capacitor(block, q) && qe.contains(&sw) {
                                // The bootstrap capacitor standing on the
                                // switch node three steps out, between the
                                // diode and the inductor, towards its BOOT
                                // pin, so BOOT's wire reaches its top.
                                let other = if qe[0] == sw { qe[1] } else { qe[0] };
                                let boot = shape
                                    .pins
                                    .iter()
                                    .zip(&block.parts[a].pins)
                                    .find(|(_, x)| x.net == other && other.is_some())
                                    .map(|(sp, _)| sp.end[1]);
                                let up = boot.is_none_or(|y| y >= pin.end[1]);
                                let ck = pin_on(block, q, sw);
                                adds.push((
                                    q,
                                    put(
                                        block,
                                        q,
                                        turn_for(
                                            block,
                                            q,
                                            ck,
                                            if up { Side::Down } else { Side::Up },
                                        ),
                                        ck,
                                        step(pin.end, side, 3 * GRID),
                                    ),
                                ));
                            } else if two_pin(block, q, &["D"]) {
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
                    let place = clear_of(block, &members, p, place, clear);
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
                    frame: 0,
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
    // A port's flag whose net meets one part's pin (an in port feeding
    // it, an out port it drives) stands on that pin's line a few steps
    // out, its own pin towards it, so the net is a short straight wire and
    // whatever else is on it joins that wire.
    for (p, part) in block.parts.iter().enumerate() {
        let PartKind::Port { flow } = part.kind else {
            continue;
        };
        let Some((d, k)) = crate::place::sole_pin(block, p) else {
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
        if !pin.side.horizontal() {
            continue;
        }
        // An out flag's pin points left, an in flag's right.
        let own = if flow == Flow::Out {
            Side::Left
        } else {
            Side::Right
        };
        // An in flag beside a pin pointing right would read right to left
        // (`sch_flow_reversed`): it keeps its edge. An out flag beside a
        // pin pointing left is mirrored.
        if own == pin.side && flow != Flow::Out {
            continue;
        }
        let mirror = own == pin.side;
        let place = put(block, p, (0, mirror), 0, step(pin.end, pin.side, 6 * GRID));
        let place = clear_of(block, &cluster.members, p, place, pin.side);
        cluster.members.push((p, place));
        out[pi] = None;
        of.insert(p, ci);
        out[ci] = Some(cluster);
    }
    if !stairs_done {
        hanging_stairs(block, idioms, &mut out);
    }
    out.into_iter().flatten().collect()
}

/// Satellites hanging off a part's sideways pins stand in stairs: each
/// pin's pull-downs and series elements (with what hangs off them in turn,
/// a compensation network's resistor off its capacitor) are one group,
/// the lowest pin's nearest the part and each higher pin's starting past
/// everything the lower pins' groups put out, so no higher pin's drop
/// crosses a lower pin's wire (the buck's EN, SS and COMP, plan 1.D). Run
/// before decoupling is placed, which then stands past them; a side with
/// any other idiom (pull-ups, which have stairs of their own) is left as
/// placed.
fn hanging_stairs(block: &Block, idioms: &[Idiom], clusters: &mut [Option<Cluster>]) {
    let hanging = |k: IdiomKind| matches!(k, IdiomKind::PullDown | IdiomKind::SeriesTermination);
    // Each pin's group: its own idioms' parts, then what is anchored on
    // those, and so on.
    let mut groups: BTreeMap<(usize, usize), BTreeSet<usize>> = BTreeMap::new();
    let mut other: BTreeSet<usize> = BTreeSet::new();
    for i in idioms {
        let Some((a, k)) = i.anchor else { continue };
        if !matches!(block.parts[a].kind, PartKind::Symbol { .. }) || block.parts[a].pins.len() < 3
        {
            continue;
        }
        if hanging(i.kind) {
            groups
                .entry((a, k))
                .or_default()
                .extend(i.parts.iter().copied());
        } else if i.kind != IdiomKind::Decoupling {
            other.insert(a);
        }
    }
    loop {
        let mut grew = false;
        for i in idioms.iter().filter(|i| hanging(i.kind)) {
            let Some((c, _)) = i.anchor else { continue };
            for g in groups.values_mut() {
                if g.contains(&c) && !i.parts.iter().all(|p| g.contains(p)) {
                    g.extend(i.parts.iter().copied());
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    for cluster in clusters.iter_mut().flatten() {
        let place =
            |c: &Cluster, p: usize| c.members.iter().find(|(q, _)| *q == p).map(|(_, x)| *x);
        let anchors: BTreeSet<usize> = groups.keys().map(|(a, _)| *a).collect();
        for a in anchors {
            let Some(rel) = place(cluster, a) else {
                continue;
            };
            let shape = Shape::of(&block.parts[a], rel);
            for side in [Side::Left, Side::Right] {
                // This side's groups, lowest pin first; every member must
                // be in this cluster and hang off this side alone.
                let mut list: Vec<(Nm, Point, &BTreeSet<usize>)> = groups
                    .iter()
                    .filter(|((q, k), _)| *q == a && shape.pins[*k].side == side)
                    .map(|((_, k), g)| (shape.pins[*k].end[1], shape.pins[*k].end, g))
                    .collect();
                if list.len() < 2
                    || other.contains(&a)
                        && idioms.iter().any(|i| {
                            !hanging(i.kind)
                                && i.kind != IdiomKind::Decoupling
                                && i.anchor
                                    .is_some_and(|(q, k)| q == a && shape.pins[k].side == side)
                        })
                    || list
                        .iter()
                        .any(|(_, _, g)| g.iter().any(|p| place(cluster, *p).is_none()))
                {
                    continue;
                }
                list.sort_by_key(|(y, _, _)| *y);
                let out = |x: Nm, e: Point| {
                    if side == Side::Left {
                        e[0] - x
                    } else {
                        x - e[0]
                    }
                };
                // How far out a room reaches from the pin, its near and
                // far edges.
                let span = |r: Rect, e: Point| {
                    let (a, b) = (out(r.min[0], e), out(r.max[0], e));
                    (a.min(b), a.max(b))
                };
                let mut reach: Option<Nm> = None;
                for (_, e, g) in list {
                    let rooms: Vec<Rect> = g
                        .iter()
                        .map(|p| {
                            Cluster::single(*p, place(cluster, *p).expect("in")).room(block, [0, 0])
                        })
                        .collect();
                    let near = rooms.iter().map(|r| span(*r, e).0).min().unwrap_or(0);
                    if let Some(r) = reach {
                        let want = r + 2 * GRID;
                        if near < want {
                            let by = snap(want - near + GRID - 1);
                            let dx = if side == Side::Left { -by } else { by };
                            for (p, x) in cluster.members.iter_mut() {
                                if g.contains(p) {
                                    *x = Place::new([x.at[0] + dx, x.at[1]], x.rotation, x.mirror);
                                }
                            }
                        }
                    }
                    let far = g
                        .iter()
                        .map(|p| {
                            let r = Cluster::single(*p, place(cluster, *p).expect("in"))
                                .room(block, [0, 0]);
                            span(r, e).1
                        })
                        .max()
                        .unwrap_or(0);
                    reach = Some(reach.map_or(far, |r| r.max(far)));
                }
            }
        }
    }
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
                frame: 0,
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
                frame: 0,
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

    #[test]
    fn one_parts_bypass_capacitors_on_one_side_stand_in_one_row() {
        // U1's VDD1, VDD2 and VDD3 down its left side, each given a 100 nF
        // capacitor; U2 has two of its own. U1's three stand at one height,
        // in stairs (the lowest pin's nearest), and the lint finds no bank
        // out of its row: U2's capacitor is a bank of its own.
        let block = Block {
            nets: vec![
                net("rail:v", "3V3", NetKind::Supply),
                net("rail:gnd", "GND", NetKind::Ground),
                net("q", "Q", NetKind::Signal),
            ],
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "x",
                    &[
                        ("VDD1", PinKind::PowerIn, Some(0)),
                        ("VDD2", PinKind::PowerIn, Some(0)),
                        ("VDD3", PinKind::PowerIn, Some(0)),
                    ],
                    &[("GND", PinKind::PowerIn, Some(1))],
                ),
                ic(
                    "u2",
                    "U2",
                    "y",
                    &[("VDD", PinKind::PowerIn, Some(0))],
                    &[
                        ("GND", PinKind::PowerIn, Some(1)),
                        ("Q", PinKind::Output, Some(2)),
                    ],
                ),
                capacitor("c1", "C1", "100n", [Some(0), Some(1)]),
                capacitor("c2", "C2", "100n", [Some(0), Some(1)]),
                capacitor("c3", "C3", "100n", [Some(0), Some(1)]),
                capacitor("c4", "C4", "100n", [Some(0), Some(1)]),
                capacitor("c5", "C5", "100n", [Some(0), Some(1)]),
            ],
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let out = crate::engine::lay_out(&block).unwrap();
        let served = |c: &str| {
            out.idioms
                .iter()
                .find(|i| block.parts[i.parts[0]].key == c)
                .and_then(|i| i.anchor)
                .unwrap()
        };
        let ones: Vec<&str> = ["c1", "c2", "c3", "c4", "c5"]
            .into_iter()
            .filter(|c| served(c).0 == 0)
            .collect();
        assert_eq!(ones.len(), 3, "{:?}", out.idioms);
        let at = |c: &str| out.layout.placed[c].at;
        assert!(
            ones.iter().all(|c| at(c)[1] == at(ones[0])[1]),
            "{:?}",
            out.layout.placed
        );
        // The lowest pin's capacitor nearest the part.
        let pin_y = |c: &str| {
            let (a, k) = served(c);
            let x = &out.layout.placed[&block.parts[a].key];
            Shape::of(&block.parts[a], Place::new(x.at, x.rotation, x.mirror)).pins[k].end[1]
        };
        let mut by_pin = ones.clone();
        by_pin.sort_by_key(|c| pin_y(c));
        let mut by_x = ones.clone();
        by_x.sort_by_key(|c| std::cmp::Reverse(at(c)[0]));
        assert_eq!(by_pin, by_x);
        let (found, _) = crate::lint::lint(&block, &out);
        assert!(
            !found.iter().any(|f| f.code == "sch_bank_not_in_row"),
            "{found:#?}"
        );
    }

    #[test]
    fn a_two_pin_part_on_a_local_net_joins_the_pin_it_serves() {
        // U1's input A shares its net only with R1, whose other end goes to
        // U2's input B: R1 belongs beside U1's pin, wired to it. And C1,
        // from U1's input C to ground, stands off that pin.
        let block = Block {
            nets: vec![
                net("rail:v", "3V3", NetKind::Supply),
                net("rail:gnd", "GND", NetKind::Ground),
                net("a", "A", NetKind::Signal),
                net("b", "B", NetKind::Signal),
                net("c", "C", NetKind::Signal),
                net("q", "Q", NetKind::Signal),
            ],
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "x",
                    &[
                        ("VDD", PinKind::PowerIn, Some(0)),
                        ("A", PinKind::Input, Some(2)),
                        ("C", PinKind::Input, Some(4)),
                    ],
                    &[
                        ("GND", PinKind::PowerIn, Some(1)),
                        ("Q", PinKind::Output, Some(5)),
                    ],
                ),
                ic(
                    "u2",
                    "U2",
                    "y",
                    &[
                        ("VDD", PinKind::PowerIn, Some(0)),
                        ("B", PinKind::Input, Some(3)),
                        ("D", PinKind::Input, Some(5)),
                    ],
                    &[("GND", PinKind::PowerIn, Some(1))],
                ),
                resistor("r1", "R1", "1k", [Some(2), Some(3)]),
                capacitor("c1", "C1", "1n", [Some(4), Some(1)]),
            ],
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let idioms = find(&block, &BTreeMap::new());
        let of = |key: &str| {
            idioms
                .iter()
                .find(|i| block.parts[i.parts[0]].key == key)
                .map(|i| (i.kind, i.anchor))
        };
        assert_eq!(
            of("r1"),
            Some((IdiomKind::SeriesTermination, Some((0, 1)))),
            "{idioms:?}"
        );
        assert_eq!(
            of("c1"),
            Some((IdiomKind::PullDown, Some((0, 2)))),
            "{idioms:?}"
        );
        let out = crate::engine::lay_out(&block).unwrap();
        let (found, _) = crate::lint::lint(&block, &out);
        let floating: Vec<_> = found.iter().filter(|f| f.code == "sch_floating").collect();
        assert!(floating.is_empty(), "{floating:#?}");
    }

    /// An array's two-pin unit (prefix RM, as the Ethernet PHY's RM1A) is a
    /// resistor, as `lint::resistor_style` reads it: a pull-up with its
    /// jumper, by the primary pull rule.
    #[test]
    fn an_arrays_units_are_resistors() {
        use crate::testing::connector;
        let mut unit = resistor("rm1:1", "RM1A", "2.2k", [Some(1), Some(0)]);
        unit.kind = PartKind::Symbol {
            prefix: "RM".into(),
        };
        let block = Block {
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "PHY",
                    &[
                        ("A", PinKind::Input, Some(1)),
                        ("B", PinKind::Input, None),
                        ("C", PinKind::Input, None),
                    ],
                    &[],
                ),
                unit,
                connector("j1", "J1", &[Some(1), Some(0)]),
            ],
            nets: vec![
                net("rail:v", "3V3", NetKind::Supply),
                net("a", "A", NetKind::Signal),
            ],
            ..Block::default()
        };
        let found = find(&block, &BTreeMap::new());
        let pull = found
            .iter()
            .find(|i| i.kind == IdiomKind::PullUp)
            .expect("a pull-up");
        assert_eq!(pull.parts, [1, 2], "the unit and its jumper");
        assert_eq!(pull.anchor, Some((0, 0)));
    }

    /// A pull on a net several active parts share serves the part of more
    /// than 8 pins among them (its bank's), whatever their keys.
    #[test]
    fn a_pull_serves_the_dense_part_on_its_net() {
        let mut many: Vec<(&str, PinKind, Option<usize>)> = vec![("TXP", PinKind::Output, Some(1))];
        for name in ["A", "B", "C", "D", "E", "F", "G", "H"] {
            many.push((name, PinKind::Passive, None));
        }
        let block = Block {
            parts: vec![
                ic(
                    "a1",
                    "A1",
                    "ESD",
                    &[
                        ("IO1", PinKind::Passive, Some(1)),
                        ("X", PinKind::Passive, None),
                        ("Y", PinKind::Passive, None),
                    ],
                    &[],
                ),
                ic("u4", "U4", "PHY", &many, &[]),
                resistor("r13", "R13", "49.9", [Some(1), Some(0)]),
            ],
            nets: vec![
                net("rail:v", "3V3A", NetKind::Supply),
                net("td", "TD+", NetKind::Signal),
            ],
            ..Block::default()
        };
        let found = find(&block, &BTreeMap::new());
        let pull = found
            .iter()
            .find(|i| i.kind == IdiomKind::PullUp)
            .expect("a pull-up");
        assert_eq!(pull.anchor, Some((1, 0)), "U4's TXP, not A1's IO1");
    }

    /// A part of more than 8 pins with two pulls or more on one side has a
    /// bank there, its rows top to bottom; a side with one pull, or a part
    /// of 8 pins or fewer, has none.
    #[test]
    fn banks_are_a_dense_parts_sides_of_two_pulls_or_more() {
        let sig = |n: usize| Some(n);
        let mut left: Vec<(&str, PinKind, Option<usize>)> = vec![("L0", PinKind::Input, sig(6))];
        for name in ["L1", "L2", "L3", "L4"] {
            left.push((name, PinKind::Input, None));
        }
        let right: Vec<(&str, PinKind, Option<usize>)> = vec![
            ("A", PinKind::Bidirectional, sig(2)),
            ("B", PinKind::Bidirectional, sig(3)),
            ("C", PinKind::Bidirectional, None),
            ("D", PinKind::Bidirectional, sig(4)),
            ("E", PinKind::Bidirectional, sig(5)),
        ];
        let mut parts = vec![ic("u1", "U1", "PHY", &left, &right)];
        parts.push(resistor("r1", "R1", "10k", [sig(2), Some(0)]));
        parts.push(resistor("r2", "R2", "10k", [sig(3), Some(1)]));
        parts.push(resistor("r3", "R3", "10k", [sig(5), Some(0)]));
        parts.push(resistor("r4", "R4", "10k", [sig(6), Some(1)]));
        let block = Block {
            parts,
            nets: vec![
                net("rail:v", "3V3", NetKind::Supply),
                net("rail:gnd", "GND", NetKind::Ground),
                net("a", "A", NetKind::Signal),
                net("b", "B", NetKind::Signal),
                net("d", "D", NetKind::Signal),
                net("e", "E", NetKind::Signal),
                net("l0", "L0", NetKind::Signal),
            ],
            ..Block::default()
        };
        let idioms = find(&block, &BTreeMap::new());
        let at: BTreeMap<usize, Place> = [(0, Place::new([0, 0], 0, false))].into();
        let found = banks(&block, &idioms, &at);
        assert_eq!(found.len(), 1, "{found:#?}");
        let b = &found[0];
        assert_eq!((b.part, b.side), (0, Side::Right));
        let pins: Vec<&str> = b
            .rows
            .iter()
            .map(|(k, _)| block.parts[0].pins[*k].name.as_str())
            .collect();
        assert_eq!(pins, ["A", "B", "E"], "top to bottom");
        for (_, list) in &b.rows {
            assert_eq!(list.len(), 1);
        }
        // The same pulls on a part of 8 pins: no bank.
        let mut small = block.clone();
        small.parts[0] = ic("u1", "U1", "PHY", &left[..3], &right);
        assert!(
            banks(&small, &find(&small, &BTreeMap::new()), &at).is_empty(),
            "8 pins"
        );
    }
}
