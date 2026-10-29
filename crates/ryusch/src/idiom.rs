//! Idioms: connectivity patterns drawn in their canonical shapes. Each is
//! matched deterministically (parts in key order, kinds in a fixed order,
//! a part in one idiom at most; where a part could serve several, the
//! first in that order takes it and the idiom says so) and becomes part of
//! a rigid cluster: decoupling capacitors in a row under the part they
//! serve, pull-ups standing above it, pull-downs hanging below, a crystal
//! beside its pins with its load capacitors, and so on.

use crate::cluster::{Cluster, is_connector};
use crate::geom::{GRID, Nm, Place, Point, Rect, Side, snap};
use crate::input::{Block, NetKind, PartKind, PinKind};
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
        let Some(&best) = users.iter().min_by_key(|(p, _)| {
            (
                served_by.get(p).copied().unwrap_or(0),
                block.parts[*p].key.clone(),
            )
        }) else {
            continue;
        };
        *served_by.entry(best.0).or_default() += 1;
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
                out.push(Idiom {
                    kind,
                    parts: vec![r],
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

fn step(p: Point, s: Side, k: Nm) -> Point {
    let d = s.step();
    [p[0] + d[0] * k, p[1] + d[1] * k]
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
    for idiom in idioms {
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
                        let x = snap(body.min[0]) + 2 * GRID + i * 4 * GRID;
                        let y = snap(body.min[1]) - 4 * GRID;
                        adds.push((
                            c,
                            put(block, c, turn_for(block, c, top, Side::Up), top, [x, y]),
                        ));
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
                        let out_x = if side.horizontal() {
                            pin.end[0] + side.step()[0] / GRID * (5 + 3 * i) * GRID
                        } else {
                            pin.end[0] + (2 + 3 * i) * GRID
                        };
                        let edge = snap(if up { body.max[1] } else { body.min[1] });
                        let y = edge + dir.step()[1] / GRID * 3 * GRID;
                        adds.push((r, put(block, r, turn, sig, [out_x, y])));
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
    out.into_iter().flatten().collect()
}

/// `place` moved out along `side` (and then down) a step at a time until
/// its part keeps two steps from every member.
fn clear_of(
    block: &Block,
    members: &[(usize, Place)],
    p: usize,
    place: Place,
    side: Side,
) -> Place {
    let room = |pl: Place| Shape::of(&block.parts[p], pl).extent.grow(GRID);
    let others: Vec<Rect> = members
        .iter()
        .map(|(q, pl)| Shape::of(&block.parts[*q], *pl).extent.grow(GRID))
        .collect();
    let dirs = [side, Side::Down, Side::Up];
    for k in 0..40 {
        for d in dirs {
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
        // Caps stand with their supply up, under the IC.
        let c2 = u1.members.iter().find(|(p, _)| *p == 2).unwrap().1;
        let s = Shape::of(&b.parts[2], c2);
        assert_eq!(s.pins[1].side, Side::Up);
        assert!(s.extent.max[1] < rects[0].min[1]);
    }
}
