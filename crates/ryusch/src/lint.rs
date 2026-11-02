//! Lint: soft findings about a layout, with stable codes, and a score per
//! sheet out of 100. Thresholds are fixed here (tuned on the corpus in
//! plan 1.A) and named in each finding's details.

use crate::engine::Outcome;
use crate::geom::{GRID, Nm, Place, Rect, Side};
use crate::input::{Block, Flow, NetKind, PartKind};
use crate::label;
use crate::layout::{IdiomKind, LabelKind};
use crate::route;
use crate::shape::{Shape, pins_on_grid};
use crate::text;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// A wire run longer than this share of the sheet's width.
pub const LONG_WIRE: (Nm, Nm) = (1, 4);
/// A net's wires longer in all than this share of the width read better
/// as labels.
pub const WIRE_NOT_LABEL: (Nm, Nm) = (1, 5);
/// A net whose wires cross more than this many others reads better as
/// labels.
pub const MANY_CROSSINGS: usize = 4;
/// A labelled net whose pins lie closer than this (a spanning tree's
/// length) could be a short wire.
pub const SHORT_NET: Nm = 12 * GRID;
/// Bends past the fewest a net could have, before it is a finding.
pub const EXTRA_BENDS: usize = 3;
/// What share of a sheet's area (per mille) may be ink before it is
/// dense.
pub const DENSITY: i64 = 300;
/// A signal running more than this far right to left is against the flow.
pub const AGAINST: Nm = 4 * GRID;
/// An idiom's part further than this from the part it serves is drawn
/// apart from its shape.
pub const IDIOM_REACH: Nm = 16 * GRID;

/// What a supply not pointing up, or ground not pointing down, costs.
pub const POWER_SIDEWAYS: i64 = 3;
/// What a net label or port reading up or down the page costs: it should
/// never be drawn so.
pub const LABEL_VERTICAL: i64 = 15;

/// What a bypass capacitor drawn apart from the pin it serves costs.
pub const BYPASS_APART: i64 = 3;
/// What a part joined to nothing by a wire costs, where a wire could have
/// joined it: schematics are read along their wires.
pub const FLOATING: i64 = 8;
/// What a port's flag whose wire turns costs (besides its jogs).
pub const PORT_BENT: i64 = 3;
/// What a pin's name past its body, or into another's, costs.
pub const NAME_OUTSIDE: i64 = 3;
/// What a jog costs: a wire stepping aside and carrying on the way it
/// went, where it could have run straight.
pub const JOG: i64 = 2;

/// One finding.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub sheet: String,
    pub message: String,
    /// The parts and nets concerned, by key.
    pub refs: Vec<String>,
    pub details: Value,
}

/// A sheet's measures and its score.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Score {
    pub sheet: String,
    /// Out of 100.
    pub score: u32,
    pub crossings: usize,
    pub bends: usize,
    /// Every wire's length, in nanometres.
    pub wire_nm: Nm,
    /// Ink per sheet area, per mille.
    pub density: i64,
    pub findings: usize,
}

/// The lint of a laid out block, sheet by sheet: findings in a fixed
/// order (code, then what they name) and each sheet's score.
pub fn lint(block: &Block, out: &Outcome) -> (Vec<Finding>, Vec<Score>) {
    let layout = &out.layout;
    let mut findings = Vec::new();
    let mut scores = Vec::new();
    let width = block.sheet[0];
    for sid in layout.sheets.keys() {
        let mut found: Vec<Finding> = Vec::new();
        let f = |code: &'static str, message: String, refs: Vec<String>, details: Value| Finding {
            code,
            sheet: sid.clone(),
            message,
            refs,
            details,
        };
        let shapes: BTreeMap<usize, Shape> = block
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let x = layout.placed.get(&p.key).filter(|x| &x.sheet == sid)?;
                Some((i, Shape::of(p, Place::new(x.at, x.rotation, x.mirror))))
            })
            .collect();
        // Wires: each node's nets, with arms.
        let mut at_node: BTreeMap<route::Node, BTreeSet<&str>> = BTreeMap::new();
        let mut bends = 0usize;
        let mut wire_nm = 0;
        let mut crossed_by: BTreeMap<&str, usize> = BTreeMap::new();
        for (key, by_sheet) in &layout.routes {
            let Some(runs) = by_sheet.get(sid) else {
                continue;
            };
            let mut extra = 0usize;
            let mut total = 0;
            let mut net_jogs = 0usize;
            for run in runs {
                net_jogs += jogs(run);
                let turns = run.len().saturating_sub(2);
                bends += turns;
                let (a, b) = (run[0], *run.last().expect("a run has ends"));
                let fewest = usize::from(a[0] != b[0] && a[1] != b[1]);
                extra += turns.saturating_sub(fewest);
                let len = route::length(run);
                total += len;
                if len > width * LONG_WIRE.0 / LONG_WIRE.1 {
                    found.push(f(
                        "sch_long_wire",
                        format!("a wire of {key} runs {} mm", len / 1_000_000),
                        vec![key.clone()],
                        json!({"length_nm": len, "threshold_nm": width * LONG_WIRE.0 / LONG_WIRE.1}),
                    ));
                }
                for n in route::expand(run) {
                    at_node.entry(n).or_default().insert(key.as_str());
                }
            }
            wire_nm += total;
            if net_jogs > 0 {
                found.push(f(
                    "sch_jog",
                    format!("{key}'s wires jog {net_jogs} times where they could run straight on"),
                    vec![key.clone()],
                    json!({"jogs": net_jogs}),
                ));
            }
            if extra > EXTRA_BENDS {
                found.push(f(
                    "sch_bends",
                    format!("{key}'s wires bend {extra} times more than they need"),
                    vec![key.clone()],
                    json!({"extra": extra, "threshold": EXTRA_BENDS}),
                ));
            }
            // A rail is drawn by ports; its wires are bypass and local ties.
            let rail = block
                .net(key)
                .is_some_and(|n| block.nets[n].kind != NetKind::Signal);
            if total > width * WIRE_NOT_LABEL.0 / WIRE_NOT_LABEL.1 && !rail {
                found.push(f(
                    "sch_wire_where_label",
                    format!("{key}'s wires run {} mm; labels would read better", total / 1_000_000),
                    vec![key.clone()],
                    json!({"length_nm": total, "threshold_nm": width * WIRE_NOT_LABEL.0 / WIRE_NOT_LABEL.1}),
                ));
            }
        }
        let mut crossings = 0;
        for nets in at_node.values() {
            if nets.len() >= 2 {
                crossings += 1;
                for n in nets {
                    *crossed_by.entry(n).or_default() += 1;
                }
            }
        }
        if crossings > 0 {
            found.push(f(
                "sch_crossings",
                format!("wires cross {crossings} times on sheet {sid}"),
                Vec::new(),
                json!({"crossings": crossings}),
            ));
        }
        for (key, n) in &crossed_by {
            if *n > MANY_CROSSINGS
                && !found
                    .iter()
                    .any(|x| x.code == "sch_wire_where_label" && x.refs[0] == *key)
            {
                found.push(f(
                    "sch_wire_where_label",
                    format!("{key}'s wires cross {n} others; labels would read better"),
                    vec![(*key).to_owned()],
                    json!({"crossings": n, "threshold": MANY_CROSSINGS}),
                ));
            }
        }
        // Labels a short wire would replace.
        for (key, list) in &layout.labels {
            let Some(n) = block.net(key) else { continue };
            if block.nets[n].kind != NetKind::Signal {
                continue;
            }
            let here: Vec<_> = list
                .iter()
                .filter(|l| &l.sheet == sid && l.kind == LabelKind::Net)
                .collect();
            if here.len() < 2 {
                continue;
            }
            let pts: Vec<_> = here.iter().map(|l| l.at).collect();
            let span = spanning(&pts);
            if span < SHORT_NET {
                found.push(f(
                    "sch_label_where_wire",
                    format!(
                        "{key}'s labels are {} mm apart; a wire would read better",
                        span / 1_000_000
                    ),
                    vec![key.clone()],
                    json!({"span_nm": span, "threshold_nm": SHORT_NET}),
                ));
            }
        }
        // Ports and labels the way they read best: supplies up, ground
        // down, names across the page.
        for (key, list) in &layout.labels {
            for l in list.iter().filter(|l| &l.sheet == sid) {
                let (code, what) = match l.kind {
                    LabelKind::Power if l.side != Side::Up => ("sch_power_sideways", "supply"),
                    LabelKind::Ground if l.side != Side::Down => ("sch_ground_sideways", "ground"),
                    LabelKind::Net if !l.side.horizontal() => ("sch_label_vertical", "label"),
                    _ => continue,
                };
                found.push(f(
                    code,
                    format!("{key}'s {what} at {} points {}", l.on, l.side.as_str()),
                    vec![key.clone(), l.on.clone()],
                    json!({"side": l.side.as_str()}),
                ));
            }
        }
        for (p, s) in &shapes {
            let part = &block.parts[*p];
            if matches!(part.kind, PartKind::Port { .. })
                && s.pins.first().is_some_and(|pin| !pin.side.horizontal())
            {
                found.push(f(
                    "sch_label_vertical",
                    format!("port {} is drawn up the page", part.key),
                    vec![part.key.clone()],
                    json!({"side": s.pins[0].side.as_str()}),
                ));
            }
        }
        // A part floating in space: every signal pin reaches its net by a
        // label though some net of it has another pin on this sheet, so a
        // wire could have joined it. Ports and connectors (whose pins
        // labels name, as schematics draw them) aren't judged.
        // Every pin end on this sheet, by net.
        let mut ends_of: BTreeMap<usize, Vec<route::Node>> = BTreeMap::new();
        for (p, sh) in &shapes {
            for (k, pin) in block.parts[*p].pins.iter().enumerate() {
                if let Some(n) = pin.drawn_net() {
                    ends_of
                        .entry(n)
                        .or_default()
                        .push(route::node_of(sh.pins[k].end));
                }
            }
        }
        for (p, sh) in &shapes {
            let part = &block.parts[*p];
            if matches!(part.kind, PartKind::Port { .. }) || crate::cluster::is_connector(block, *p)
            {
                continue;
            }
            let mut signals = 0;
            let mut wired = false;
            let mut local = false;
            for (k, pin) in part.pins.iter().enumerate() {
                let Some(n) = pin.drawn_net() else { continue };
                if block.nets[n].kind != NetKind::Signal {
                    continue;
                }
                signals += 1;
                let ends = ends_of.get(&n).map(Vec::as_slice).unwrap_or_default();
                local |= ends.len() > 1;
                let runs = layout
                    .routes
                    .get(&block.nets[n].key)
                    .and_then(|r| r.get(sid))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                wired |= wired_to_another(runs, route::node_of(sh.pins[k].end), ends);
            }
            if signals > 0 && !wired && local {
                found.push(f(
                    "sch_floating",
                    format!(
                        "{} is joined to nothing by a wire: its nets on this sheet are labels",
                        part.key
                    ),
                    vec![part.key.clone()],
                    json!({"signals": signals}),
                ));
            }
        }
        // A port's flag reads best straight out to the pin it meets.
        for (p, sh) in &shapes {
            let part = &block.parts[*p];
            if !matches!(part.kind, PartKind::Port { .. }) || part.pins.len() != 1 {
                continue;
            }
            let Some(n) = part.pins[0].net else { continue };
            let end = sh.pins[0].end;
            let Some(runs) = layout
                .routes
                .get(&block.nets[n].key)
                .and_then(|r| r.get(sid))
            else {
                continue;
            };
            let turns: usize = runs
                .iter()
                .filter(|r| r.first() == Some(&end) || r.last() == Some(&end))
                .map(|r| r.len().saturating_sub(2))
                .sum();
            if turns > 0 {
                found.push(f(
                    "sch_port_bent",
                    format!("port {}'s wire turns {turns} times", part.key),
                    vec![part.key.clone()],
                    json!({"turns": turns}),
                ));
            }
        }
        // Pins' names inside their body, apart from each other.
        for (p, sh) in &shapes {
            let part = &block.parts[*p];
            if !crate::paint::draws_names(part) {
                continue;
            }
            let boxes: Vec<(usize, Rect)> = part
                .pins
                .iter()
                .enumerate()
                .filter(|(_, pin)| !pin.name.is_empty())
                .map(|(k, pin)| (k, crate::paint::name_box(&pin.name, &sh.pins[k])))
                .collect();
            for (i, (k, b)) in boxes.iter().enumerate() {
                let outside = !sh.body.contains(b);
                let crowded = boxes[i + 1..].iter().any(|(_, o)| o.overlaps(b));
                if outside || crowded {
                    let pin = &part.pins[*k];
                    found.push(f(
                        "sch_pin_name_outside",
                        format!(
                            "{}'s pin {} name {} {}",
                            part.key,
                            pin.number,
                            pin.name,
                            if outside {
                                "runs past its body"
                            } else {
                                "runs into another pin's"
                            }
                        ),
                        vec![part.key.clone()],
                        json!({"pin": pin.number, "outside": outside}),
                    ));
                }
            }
        }
        // Density: the ink's area against the sheet's.
        let mut ink: i128 = 0;
        for (p, s) in &shapes {
            ink += s.extent.area();
            if let Some(fields) = layout.text.get(&block.parts[*p].key) {
                for (field, v) in [
                    ("refdes", &block.parts[*p].refdes),
                    ("value", &block.parts[*p].value),
                ] {
                    if let Some(at) = fields.get(field) {
                        ink += text::boxes(&[v.as_str()], *at)[0].area();
                    }
                }
            }
        }
        for list in layout.labels.values() {
            for l in list.iter().filter(|l| &l.sheet == sid) {
                ink += label::label_box(l.kind, &l.text, l.at, l.side).area();
            }
        }
        let density = (ink * 1000 / Rect::new([0, 0], block.sheet).area().max(1)) as i64;
        if density > DENSITY {
            found.push(f(
                "sch_density",
                format!("sheet {sid} is {}% ink", density / 10),
                Vec::new(),
                json!({"per_mille": density, "threshold": DENSITY}),
            ));
        }
        // Flow: drivers left of what they drive.
        let mut reversed: BTreeSet<String> = BTreeSet::new();
        for (n, net) in block.nets.iter().enumerate() {
            if net.kind != NetKind::Signal {
                continue;
            }
            let mut drivers = Vec::new();
            let mut sinks = Vec::new();
            for (p, s) in &shapes {
                let part = &block.parts[*p];
                for (k, pin) in part.pins.iter().enumerate() {
                    if pin.net != Some(n) {
                        continue;
                    }
                    let (drives, takes) = match part.kind {
                        PartKind::Port { flow } => (flow != Flow::Out, flow == Flow::Out),
                        _ => (pin.kind.drives(), pin.kind.takes()),
                    };
                    if drives {
                        drivers.push((*p, s.pins[k].end[0]));
                    }
                    if takes {
                        sinks.push((*p, s.pins[k].end[0]));
                    }
                }
            }
            for (d, dx) in &drivers {
                for (t, tx) in &sinks {
                    let free = out.flow.contains(&block.parts[*d].key)
                        || out.flow.contains(&block.parts[*t].key);
                    if *tx < *dx - AGAINST && !free {
                        reversed.insert(net.key.clone());
                    }
                }
            }
        }
        for key in reversed {
            found.push(f(
                "sch_flow_reversed",
                format!("{key} runs right to left; `flow` lets it, or move what it drives"),
                vec![key.clone()],
                json!({"threshold_nm": AGAINST}),
            ));
        }
        for p in shapes.keys() {
            if !pins_on_grid(&block.parts[*p]) {
                let key = block.parts[*p].key.clone();
                found.push(f(
                    "sch_pin_off_grid",
                    format!("{key}'s pins aren't on the 50 mil grid relative to each other; it is placed by its first"),
                    vec![key],
                    Value::Null,
                ));
            }
        }
        let mut bypass_apart: BTreeSet<usize> = BTreeSet::new();
        // A bypass capacitor reads best on the line out of the pin it
        // serves, wired to it, not standing apart under ports of its own.
        for idiom in &out.idioms {
            if idiom.kind != IdiomKind::Decoupling {
                continue;
            }
            let Some((a, k)) = idiom.anchor else { continue };
            let c = idiom.parts[0];
            let (Some(sa), Some(sc)) = (shapes.get(&a), shapes.get(&c)) else {
                continue;
            };
            let Some(n) = block.parts[a].pins[k].net else {
                continue;
            };
            let Some(kc) = block.parts[c].pins.iter().position(|x| x.net == Some(n)) else {
                continue;
            };
            let nodes: BTreeSet<route::Node> = layout
                .routes
                .get(&block.nets[n].key)
                .and_then(|r| r.get(sid))
                .into_iter()
                .flatten()
                .flat_map(|run| route::expand(run))
                .collect();
            let wired = [sa.pins[k].end, sc.pins[kc].end]
                .iter()
                .all(|e| nodes.contains(&route::node_of(*e)));
            if !wired {
                bypass_apart.insert(c);
                let (ka, kc) = (block.parts[a].key.clone(), block.parts[c].key.clone());
                found.push(f(
                    "sch_bypass_apart",
                    format!("{kc} decouples {ka}, but isn't drawn on the line from its pin"),
                    vec![kc, ka],
                    json!({"net": block.nets[n].key}),
                ));
            }
        }
        for idiom in &out.idioms {
            let Some((a, _)) = idiom.anchor else { continue };
            let Some(anchor) = shapes.get(&a) else {
                continue;
            };
            for p in &idiom.parts {
                let Some(s) = shapes.get(p) else { continue };
                // A bypass capacitor wired on its pin's line reads as its
                // pin's however far out along it.
                if idiom.kind == IdiomKind::Decoupling && !bypass_apart.contains(p) {
                    continue;
                }
                if !s.extent.grow(IDIOM_REACH).overlaps(&anchor.extent) {
                    let key = block.parts[*p].key.clone();
                    found.push(f(
                        "sch_idiom_missed",
                        format!(
                            "{key} is the {} of {}, but drawn far from it",
                            idiom.kind.as_str(),
                            block.parts[a].key
                        ),
                        vec![key, block.parts[a].key.clone()],
                        json!({"idiom": idiom.kind.as_str(), "threshold_nm": IDIOM_REACH}),
                    ));
                }
            }
        }
        found.sort_by(|a, b| (a.code, &a.refs).cmp(&(b.code, &b.refs)));
        let count = |c: &str| found.iter().filter(|x| x.code == c).count() as i64;
        let extra: i64 = found
            .iter()
            .filter(|x| x.code == "sch_bends")
            .map(|x| x.details["extra"].as_i64().unwrap_or(0))
            .sum();
        let penalty = crossings as i64 * 2
            + extra
            + count("sch_long_wire") * 4
            + count("sch_label_where_wire") * 2
            + count("sch_wire_where_label") * 2
            + (density - DENSITY).max(0) / 10
            + count("sch_flow_reversed") * 3
            + count("sch_pin_off_grid")
            + count("sch_idiom_missed") * 3
            + count("sch_power_sideways") * POWER_SIDEWAYS
            + count("sch_ground_sideways") * POWER_SIDEWAYS
            + count("sch_label_vertical") * LABEL_VERTICAL
            + count("sch_bypass_apart") * BYPASS_APART
            + count("sch_pin_name_outside") * NAME_OUTSIDE
            + count("sch_port_bent") * PORT_BENT
            + count("sch_floating") * FLOATING
            + found
                .iter()
                .filter(|x| x.code == "sch_jog")
                .map(|x| x.details["jogs"].as_i64().unwrap_or(0))
                .sum::<i64>()
                * JOG;
        scores.push(Score {
            sheet: sid.clone(),
            score: (100 - penalty).clamp(0, 100) as u32,
            crossings,
            bends,
            wire_nm,
            density,
            findings: found.len(),
        });
        findings.extend(found);
    }
    (findings, scores)
}

/// Whether the wires of a net touching the pin end `at` reach another of
/// the net's pin ends (`ends`), not only its own label's stub.
fn wired_to_another(runs: &[Vec<[Nm; 2]>], at: route::Node, ends: &[route::Node]) -> bool {
    let nodes: Vec<BTreeSet<route::Node>> = runs
        .iter()
        .map(|r| route::expand(r).into_iter().collect())
        .collect();
    let mut reached: BTreeSet<route::Node> = [at].into();
    let mut taken = vec![false; nodes.len()];
    loop {
        let mut grew = false;
        for (i, n) in nodes.iter().enumerate() {
            if !taken[i] && n.iter().any(|x| reached.contains(x)) {
                taken[i] = true;
                reached.extend(n.iter().copied());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    ends.iter().any(|e| *e != at && reached.contains(e))
}

/// A run's jogs: each two turns that leave it going the way it went
/// before them (a step aside, a Z).
pub fn jogs(run: &[[Nm; 2]]) -> usize {
    let dir = |a: [Nm; 2], b: [Nm; 2]| [(b[0] - a[0]).signum(), (b[1] - a[1]).signum()];
    let dirs: Vec<[Nm; 2]> = run.windows(2).map(|w| dir(w[0], w[1])).collect();
    dirs.windows(3)
        .filter(|w| w[0] == w[2] && w[0] != w[1])
        .count()
}

/// The length of a spanning tree through `pts`, by Manhattan distance.
fn spanning(pts: &[[Nm; 2]]) -> Nm {
    let mut joined = vec![false; pts.len()];
    let mut best = vec![Nm::MAX; pts.len()];
    let mut total = 0;
    if pts.is_empty() {
        return 0;
    }
    best[0] = 0;
    for _ in 0..pts.len() {
        let (i, d) = best
            .iter()
            .enumerate()
            .filter(|(i, _)| !joined[*i])
            .min_by_key(|(_, d)| **d)
            .map(|(i, d)| (i, *d))
            .expect("one left");
        joined[i] = true;
        total += d;
        for j in 0..pts.len() {
            let dd = (pts[i][0] - pts[j][0]).abs() + (pts[i][1] - pts[j][1]).abs();
            if !joined[j] && dd < best[j] {
                best[j] = dd;
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::seeded;

    #[test]
    fn seeded_sheets_score_and_findings_are_stable() {
        let b = seeded(3, 6);
        let out = crate::engine::lay_out(&b).unwrap();
        let (findings, scores) = lint(&b, &out);
        assert_eq!(scores.len(), out.layout.sheets.len());
        assert!(scores.iter().all(|s| s.score <= 100));
        // Sorted and the same run to run.
        let (again, _) = lint(&b, &crate::engine::lay_out(&b).unwrap());
        assert_eq!(findings, again);
        let codes: Vec<&str> = findings.iter().map(|f| f.code).collect();
        let mut sorted = codes.clone();
        sorted.sort();
        assert_eq!(codes, sorted);
    }

    #[test]
    fn a_pin_is_wired_when_its_wires_reach_another_pin() {
        let g = GRID;
        let n = |p: [Nm; 2]| route::node_of(p);
        let (a, b, c) = ([0, 0], [6 * g, 0], [6 * g, 4 * g]);
        // A stub to its own label reaches no other pin.
        let stub = vec![vec![a, [2 * g, 0]]];
        assert!(!wired_to_another(&stub, n(a), &[n(a), n(b)]));
        // A wire to b does, and c through b's wire on to it.
        let wires = vec![vec![a, b], vec![c, [6 * g, 0]]];
        assert!(wired_to_another(&wires, n(a), &[n(a), n(b)]));
        assert!(wired_to_another(&wires, n(c), &[n(a), n(c)]));
    }

    #[test]
    fn a_part_joined_only_by_labels_floats() {
        let b = seeded(1, 2);
        let mut out = crate::engine::lay_out(&b).unwrap();
        // Every signal's wires taken away and each pin labelled instead.
        let wired: Vec<String> = out
            .layout
            .routes
            .keys()
            .filter(|k| b.net(k).is_some_and(|n| b.nets[n].kind == NetKind::Signal))
            .cloned()
            .collect();
        for key in &wired {
            out.layout.routes.remove(key);
        }
        let (findings, _) = lint(&b, &out);
        assert!(findings.iter().any(|f| f.code == "sch_floating"));
    }

    #[test]
    fn a_pin_name_past_its_body_is_found() {
        let mut b = seeded(1, 2);
        let out = crate::engine::lay_out(&b).unwrap();
        let (before, _) = lint(&b, &out);
        assert!(!before.iter().any(|f| f.code == "sch_pin_name_outside"));
        let ic = b
            .parts
            .iter()
            .position(|p| crate::paint::draws_names(p) && !p.pins.is_empty())
            .unwrap();
        b.parts[ic].pins[0].name = "A_NAME_FAR_TOO_LONG_FOR_ANY_BODY".into();
        let (after, _) = lint(&b, &out);
        assert!(after.iter().any(|f| f.code == "sch_pin_name_outside"));
    }

    #[test]
    fn a_step_aside_is_a_jog() {
        let g = GRID;
        assert_eq!(jogs(&[[0, 0], [4 * g, 0]]), 0);
        assert_eq!(jogs(&[[0, 0], [4 * g, 0], [4 * g, 2 * g]]), 0);
        assert_eq!(
            jogs(&[[0, 0], [2 * g, 0], [2 * g, 2 * g], [4 * g, 2 * g]]),
            1
        );
        // Round a corner and back is a U, not a jog.
        assert_eq!(jogs(&[[0, 0], [2 * g, 0], [2 * g, 2 * g], [0, 2 * g]]), 0);
    }

    #[test]
    fn a_short_labelled_net_is_a_label_where_a_wire_would_do() {
        let b = seeded(1, 2);
        let mut out = crate::engine::lay_out(&b).unwrap();
        // Draw a wired net by two labels a step apart instead.
        let key = out
            .layout
            .routes
            .keys()
            .find(|k| !k.starts_with("rail"))
            .unwrap()
            .clone();
        out.layout.routes.remove(&key);
        for at in [[50 * GRID, 50 * GRID], [52 * GRID, 50 * GRID]] {
            out.layout
                .labels
                .entry(key.clone())
                .or_default()
                .push(crate::layout::Label {
                    sheet: "s1".into(),
                    at,
                    side: crate::geom::Side::Right,
                    kind: LabelKind::Net,
                    on: "x/1".into(),
                    text: "X".into(),
                });
        }
        let (findings, _) = lint(&b, &out);
        assert!(
            findings
                .iter()
                .any(|f| f.code == "sch_label_where_wire" && f.refs == [key.clone()])
        );
    }
}
