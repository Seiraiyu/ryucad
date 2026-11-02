//! Lint: soft findings about a layout, with stable codes, and a score per
//! sheet out of 100. Thresholds are fixed here (tuned on the corpus in
//! plan 1.A) and named in each finding's details. Each finding costs its
//! rule's weight per unit of it; a project's preferences ([`Prefs`]) set
//! weights, severities (`off` drops a rule) and styles, and a finding they
//! drove says so.

use crate::engine::Outcome;
use crate::geom::{GRID, Nm, Place, Rect, Side};
use crate::input::{Block, Flow, NetKind, PartKind};
use crate::label;
use crate::layout::{IdiomKind, LabelKind};
use crate::route;
use crate::shape::{Shape, pins_on_grid};
use crate::text;
use serde::{Deserialize, Serialize};
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

/// Every rule, its default weight (points per unit of a finding) and what
/// a unit is. Codes are stable and never reused.
pub const RULES: &[(&str, i64, &str)] = &[
    ("sch_crossings", 2, "a crossing"),
    ("sch_bends", 1, "a bend past the fewest"),
    ("sch_long_wire", 4, "a long wire"),
    ("sch_label_where_wire", 2, "a net"),
    ("sch_wire_where_label", 2, "a net"),
    ("sch_density", 1, "10 per mille of ink past the threshold"),
    ("sch_flow_reversed", 3, "a net"),
    ("sch_pin_off_grid", 1, "a part"),
    ("sch_idiom_missed", 3, "a part"),
    ("sch_power_sideways", POWER_SIDEWAYS, "a port"),
    ("sch_ground_sideways", POWER_SIDEWAYS, "a port"),
    ("sch_label_vertical", LABEL_VERTICAL, "a label or port"),
    ("sch_jog", JOG, "a jog"),
    ("sch_bypass_apart", BYPASS_APART, "a capacitor"),
    ("sch_pin_name_outside", NAME_OUTSIDE, "a pin"),
    ("sch_port_bent", PORT_BENT, "a port"),
    ("sch_floating", FLOATING, "a part"),
    ("sch_resistor_style", 1, "a resistor"),
    ("sch_out_of_line", 2, "a part"),
    ("sch_bank_not_in_row", 3, "a bank"),
    ("sch_topology", 10, "a topology"),
];

/// How much a finding counts: a rule's lint, warned of, or an error that
/// fails a check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Off,
    #[default]
    Warning,
    Error,
}

impl Severity {
    /// As files write it.
    pub fn word(self) -> &'static str {
        match self {
            Severity::Off => "off",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// How a resistor is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResistorStyle {
    /// The zigzag.
    #[default]
    Us,
    /// The box.
    Iec,
}

/// How a net between parts on one sheet is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalNets {
    /// Wires: a part joined only by labels floats.
    #[default]
    Wires,
    /// Labels: no part floats, and no labels want a wire.
    Labels,
}

/// The styles a project prefers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefer {
    #[serde(default)]
    pub resistor: ResistorStyle,
    #[serde(default)]
    pub local_nets: LocalNets,
}

/// A project's lint preferences: weights and severities by rule, and the
/// styles it prefers. The default is lint as this module sets it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefs {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub weights: BTreeMap<String, i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub severity: BTreeMap<String, Severity>,
    #[serde(default)]
    pub prefer: Prefer,
}

impl Prefs {
    /// A rule's weight, and the preference that set it if one did.
    pub fn weight(&self, code: &str) -> (i64, Option<String>) {
        match self.weights.get(code) {
            Some(w) => (*w, Some(format!("weights.{code} = {w}"))),
            None => (default_weight(code), None),
        }
    }

    /// A rule's severity, and the preference that set it if one did.
    pub fn severity(&self, code: &str) -> (Severity, Option<String>) {
        match self.severity.get(code) {
            Some(s) => (*s, Some(format!("severity.{code} = {}", s.word()))),
            None => (Severity::Warning, None),
        }
    }
}

/// A rule's weight as this module sets it (0 for a code it doesn't have).
pub fn default_weight(code: &str) -> i64 {
    RULES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map_or(0, |(_, w, _)| *w)
}

/// One finding.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub sheet: String,
    pub message: String,
    /// The parts and nets concerned, by key.
    pub refs: Vec<String>,
    pub details: Value,
    /// What it costs the sheet's score: its rule's weight per unit.
    pub points: i64,
    pub severity: Severity,
    /// The preferences that drove it, as the project's file writes them
    /// (`prefer.resistor = us`, `weights.sch_jog = 5`), when any did.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prefs: Vec<String>,
}

/// How many units of its rule a finding is.
fn units(f: &Finding) -> i64 {
    match f.code {
        "sch_crossings" => f.details["crossings"].as_i64().unwrap_or(1),
        "sch_bends" => f.details["extra"].as_i64().unwrap_or(1),
        "sch_jog" => f.details["jogs"].as_i64().unwrap_or(1),
        "sch_density" => (f.details["per_mille"].as_i64().unwrap_or(DENSITY) - DENSITY).max(0) / 10,
        _ => 1,
    }
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
    /// Preferences that took rules out of play on the sheet.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// The lint of a laid out block, sheet by sheet: findings in a fixed
/// order (code, then what they name) and each sheet's score, under the
/// default preferences.
pub fn lint(block: &Block, out: &Outcome) -> (Vec<Finding>, Vec<Score>) {
    lint_with(block, out, &Prefs::default())
}

/// A bank's key: prefix, pin count, nets, and for decoupling capacitors
/// the part and side they serve.
type BankKey = (String, usize, Vec<usize>, Option<(usize, Side)>);

/// [`lint`] under a project's preferences.
pub fn lint_with(block: &Block, out: &Outcome, prefs: &Prefs) -> (Vec<Finding>, Vec<Score>) {
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
            points: 0,
            severity: Severity::Warning,
            prefs: Vec::new(),
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
        // The circuits with a standard drawing on this sheet, judged once;
        // the parts at the heart of its buck converters.
        let judged = crate::topology::judge(block, layout, sid, &shapes);
        let bucks: BTreeSet<String> = judged
            .iter()
            .filter(|j| j.kind == "buck_converter")
            .map(|j| j.parts[0].clone())
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
            let mut around: BTreeSet<usize> = BTreeSet::new();
            for run in runs {
                net_jogs += jogs(run);
                around.extend(crate::topology::jog_obstacles(run, &shapes));
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
                let name = block
                    .net(key)
                    .map_or(key.clone(), |n| block.nets[n].name.clone());
                // A net's own parts are what it reaches, not what it steps
                // round.
                let n = block.net(key);
                around.retain(|p| {
                    !block.parts[*p]
                        .pins
                        .iter()
                        .any(|x| n.is_some() && x.net == n)
                });
                let mut message = format!(
                    "{name}'s wires jog {} where they could run straight on",
                    times(net_jogs)
                );
                if !around.is_empty() {
                    let names: Vec<String> = around.iter().map(|p| part_shown(block, *p)).collect();
                    message.push_str(&format!(", stepping round {}", names.join(", ")));
                }
                // Round a pin's bypass capacitors: say what the standard
                // drawing does instead.
                let mut served: Vec<(usize, usize)> = out
                    .idioms
                    .iter()
                    .filter(|i| i.kind == IdiomKind::Decoupling && around.contains(&i.parts[0]))
                    .filter_map(|i| i.anchor)
                    .collect();
                served.sort();
                served.dedup();
                if let Some(&(a, k)) = served.first() {
                    let pin = &block.parts[a].pins[k].name;
                    let caps: Vec<String> = out
                        .idioms
                        .iter()
                        .filter(|i| i.kind == IdiomKind::Decoupling && i.anchor == Some((a, k)))
                        .map(|i| who(&block.parts[i.parts[0]]))
                        .collect();
                    let standard = if bucks.contains(&block.parts[a].key) {
                        "Standard buck input:"
                    } else {
                        "Standard:"
                    };
                    message = format!(
                        "{name} jogs under {pin}'s bypass caps ({}). {standard} the caps stand on the input wire beside {pin}, and {name} is taken off before them. ({message}.)",
                        caps.join(", ")
                    );
                }
                found.push(f(
                    "sch_jog",
                    message,
                    vec![key.clone()],
                    json!({"jogs": net_jogs, "around": around.iter().map(|p| block.parts[*p].key.clone()).collect::<Vec<_>>()}),
                ));
            }
            if extra > EXTRA_BENDS {
                found.push(f(
                    "sch_bends",
                    format!(
                        "{}'s wires bend {extra} times more than they need",
                        net_shown(block, key)
                    ),
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
                    format!("{}'s wires run {} mm; labels would read better", net_shown(block, key), total / 1_000_000),
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
                format!("wires cross {} on sheet {sid}", times(crossings)),
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
                    format!(
                        "{}'s wires cross {n} others; labels would read better",
                        net_shown(block, key)
                    ),
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
                        "{}'s labels are {} mm apart; a wire would read better",
                        net_shown(block, key),
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
                    format!(
                        "{}'s {what} at {} points {}",
                        net_shown(block, key),
                        l.on,
                        l.side.as_str()
                    ),
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
                        part_shown(block, *p)
                    ),
                    vec![part.key.clone()],
                    json!({"signals": signals}),
                ));
            }
        }
        // Clean rows: every part in line with another, across or down, and
        // like parts sharing their nets (a bank) in one row or column.
        let middles: Vec<(usize, [Nm; 2])> = shapes
            .iter()
            .filter(|(p, _)| !matches!(block.parts[**p].kind, PartKind::Port { .. }))
            .map(|(p, sh)| (*p, sh.body.center()))
            .collect();
        let near = |a: Nm, b: Nm| (a - b).abs() <= GRID / 2;
        // The lines a part stands on: its body's middle across and down,
        // and a two-pin part's axis through its pins too.
        let lines = |p: usize| -> (Vec<Nm>, Vec<Nm>) { part_lines(&block.parts[p], &shapes[&p]) };
        if middles.len() > 1 {
            for (p, m) in &middles {
                let (xs, ys) = lines(*p);
                let lined = middles.iter().any(|(q, _)| {
                    let (ox, oy) = lines(*q);
                    q != p
                        && (xs.iter().any(|a| ox.iter().any(|b| near(*a, *b)))
                            || ys.iter().any(|a| oy.iter().any(|b| near(*a, *b))))
                });
                if !lined {
                    let nearest = middles
                        .iter()
                        .filter(|(q, _)| q != p)
                        .min_by_key(|(q, o)| ((m[0] - o[0]).abs() + (m[1] - o[1]).abs(), *q))
                        .map(|(q, _)| part_shown(block, *q))
                        .unwrap_or_default();
                    found.push(f(
                        "sch_out_of_line",
                        format!(
                            "{} lines up with no other part, across or down: put it in a row or a column with its neighbours (the nearest is {nearest})",
                            part_shown(block, *p)
                        ),
                        vec![block.parts[*p].key.clone()],
                        json!({"nearest": nearest}),
                    ));
                }
            }
        }
        // A decoupling capacitor belongs to the pin it serves (the bypass
        // wiring holds it there): a bank of them is one part's on one
        // side, which stand in a row; another part's are another bank.
        let serves: BTreeMap<usize, (usize, Side)> = out
            .idioms
            .iter()
            .filter(|i| i.kind == IdiomKind::Decoupling)
            .filter_map(|i| {
                let (a, k) = i.anchor?;
                Some((i.parts[0], (a, shapes.get(&a)?.pins[k].side)))
            })
            .collect();
        let mut banks: BTreeMap<BankKey, Vec<usize>> = BTreeMap::new();
        for (p, _) in &middles {
            let part = &block.parts[*p];
            let PartKind::Symbol { prefix } = &part.kind else {
                continue;
            };
            let nets: Option<Vec<usize>> = part.pins.iter().map(|x| x.drawn_net()).collect();
            let Some(mut nets) = nets else { continue };
            nets.sort();
            nets.dedup();
            if nets.len() < 2 {
                continue;
            }
            banks
                .entry((
                    prefix.clone(),
                    part.pins.len(),
                    nets,
                    serves.get(p).copied(),
                ))
                .or_default()
                .push(*p);
        }
        for ((_, _, nets, _), members) in &banks {
            if members.len() < 2 {
                continue;
            }
            let at = |p: &usize| middles.iter().find(|(q, _)| q == p).expect("a member").1;
            let first = at(&members[0]);
            let row = members.iter().all(|p| near(at(p)[1], first[1]));
            let column = members.iter().all(|p| near(at(p)[0], first[0]));
            if !row && !column {
                let mut heights: Vec<Nm> = members.iter().map(|p| at(p)[1] / (GRID / 2)).collect();
                heights.sort();
                heights.dedup();
                let names: Vec<String> = members.iter().map(|p| part_shown(block, *p)).collect();
                let between: Vec<&str> =
                    nets.iter().map(|n| block.nets[*n].name.as_str()).collect();
                found.push(f(
                    "sch_bank_not_in_row",
                    format!(
                        "{} all sit between {}: stand them in one row, their tops on one line and evenly spaced, or in one column (now at {} heights)",
                        names.join(", "),
                        between.join(" and "),
                        heights.len()
                    ),
                    members.iter().map(|p| block.parts[*p].key.clone()).collect(),
                    json!({"nets": between, "heights": heights.len()}),
                ));
            }
        }
        // Circuits with a standard drawing (a buck converter, an op-amp
        // stage) drawn out of it.
        for j in judged {
            if j.problems.is_empty() {
                continue;
            }
            let mut what = j.what.clone();
            if let Some(c) = what.get(..1) {
                what = c.to_uppercase() + &what[1..];
            }
            found.push(f(
                "sch_topology",
                format!(
                    "{what} is drawn out of its standard shape: {}. Draw it the standard way: {}.",
                    j.problems.join("; "),
                    j.standard
                ),
                j.parts.clone(),
                json!({"topology": j.kind, "problems": j.problems, "standard": j.standard}),
            ));
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
                format!(
                    "{} runs right to left; `flow` lets it, or move what it drives",
                    net_shown(block, &key)
                ),
                vec![key.clone()],
                json!({"threshold_nm": AGAINST}),
            ));
        }
        for p in shapes.keys() {
            if !pins_on_grid(&block.parts[*p]) {
                let key = block.parts[*p].key.clone();
                found.push(f(
                    "sch_pin_off_grid",
                    format!("{}'s pins aren't on the 50 mil grid relative to each other; it is placed by its first", part_shown(block, *p)),
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
                            "{} is the {} of {}, but drawn far from it",
                            part_shown(block, *p),
                            idiom.kind.as_str(),
                            part_shown(block, a)
                        ),
                        vec![key, block.parts[a].key.clone()],
                        json!({"idiom": idiom.kind.as_str(), "threshold_nm": IDIOM_REACH}),
                    ));
                }
            }
        }
        // Styles the project prefers.
        let mut notes = Vec::new();
        let resistor = match prefs.prefer.resistor {
            ResistorStyle::Us => "prefer.resistor = us",
            ResistorStyle::Iec => "prefer.resistor = iec",
        };
        for p in shapes.keys() {
            let part = &block.parts[*p];
            let Some(drawn) = resistor_style(part) else {
                continue;
            };
            if drawn != prefs.prefer.resistor {
                let (is, wants) = match drawn {
                    ResistorStyle::Iec => ("a box (IEC)", "the US zigzag"),
                    ResistorStyle::Us => ("a zigzag (US)", "the IEC box"),
                };
                let mut x = f(
                    "sch_resistor_style",
                    format!(
                        "{} is drawn as {is}; this project prefers {wants} ({resistor}{})",
                        who(part),
                        if prefs.prefer == Prefer::default() {
                            ", the default"
                        } else {
                            ""
                        }
                    ),
                    vec![part.key.clone()],
                    Value::Null,
                );
                x.prefs.push(resistor.to_owned());
                found.push(x);
            }
        }
        match prefs.prefer.local_nets {
            LocalNets::Wires => {
                for x in found.iter_mut().filter(|x| x.code == "sch_floating") {
                    x.message.push_str(" (prefer.local_nets = wires)");
                    x.prefs.push("prefer.local_nets = wires".into());
                }
            }
            LocalNets::Labels => {
                let before = found.len();
                found.retain(|x| !matches!(x.code, "sch_floating" | "sch_label_where_wire"));
                notes.push(format!(
                    "prefer.local_nets = labels: sch_floating and sch_label_where_wire not judged ({} findings left out)",
                    before - found.len()
                ));
            }
        }
        // Weights and severities.
        found.retain_mut(|x| {
            let (severity, why) = prefs.severity(x.code);
            if severity == Severity::Off {
                return false;
            }
            let (weight, set) = prefs.weight(x.code);
            x.severity = severity;
            x.points = units(x) * weight;
            for p in [why, set].into_iter().flatten() {
                x.message.push_str(&format!(" ({p})"));
                x.prefs.push(p);
            }
            true
        });
        for (code, s) in &prefs.severity {
            if *s == Severity::Off {
                notes.push(format!("severity.{code} = off: not judged"));
            }
        }
        found.sort_by(|a, b| (a.code, &a.refs).cmp(&(b.code, &b.refs)));
        let penalty: i64 = found.iter().map(|x| x.points).sum();
        scores.push(Score {
            sheet: sid.clone(),
            score: (100 - penalty).clamp(0, 100) as u32,
            crossings,
            bends,
            wire_nm,
            density,
            findings: found.len(),
            notes,
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

/// How many times, in words: once, twice, then `n times`.
fn times(n: usize) -> String {
    match n {
        1 => "once".into(),
        2 => "twice".into(),
        n => format!("{n} times"),
    }
}

/// A part as messages name it: its designator, else its key.
/// The lines a part stands on, down and across: its body's middle, and a
/// two-pin part's axis through its pins too. `sch_out_of_line` asks that
/// each part share one with another, within half a step; `align` holds
/// when its parts share one.
pub fn part_lines(part: &crate::input::Part, sh: &Shape) -> (Vec<Nm>, Vec<Nm>) {
    let m = sh.body.center();
    let (mut xs, mut ys) = (vec![m[0]], vec![m[1]]);
    if part.pins.len() == 2 {
        for pin in &sh.pins {
            if pin.side.horizontal() {
                ys.push(pin.end[1]);
            } else {
                xs.push(pin.end[0]);
            }
        }
    }
    (xs, ys)
}

/// A net as messages name it: by its name, else its key.
fn net_shown(block: &Block, key: &str) -> String {
    block
        .net(key)
        .map_or_else(|| key.to_owned(), |n| block.nets[n].name.clone())
}

/// A part as messages name it: its designator, a port by its net's name
/// (`port:SDA`), else its key.
pub(crate) fn part_shown(block: &Block, p: usize) -> String {
    let part = &block.parts[p];
    if matches!(part.kind, PartKind::Port { .. })
        && let Some(n) = part.pins.first().and_then(|x| x.net)
    {
        return format!("port:{}", block.nets[n].name);
    }
    who(part)
}

pub(crate) fn who(part: &crate::input::Part) -> String {
    if part.refdes.is_empty() {
        part.key.clone()
    } else {
        part.refdes.clone()
    }
}

/// How a resistor is drawn, read from its graphics: a box, or a zigzag (a
/// polyline of six points or more); `None` for a part that isn't a
/// resistor of two pins drawn either way.
pub fn resistor_style(part: &crate::input::Part) -> Option<ResistorStyle> {
    let PartKind::Symbol { prefix } = &part.kind else {
        return None;
    };
    if part.pins.len() != 2
        || !(prefix == "R" || ["RN", "RM", "RA"].iter().any(|x| prefix.starts_with(x)))
    {
        return None;
    }
    use crate::input::Graphic;
    if part
        .graphics
        .iter()
        .any(|g| matches!(g, Graphic::Rect { .. }))
    {
        return Some(ResistorStyle::Iec);
    }
    if part
        .graphics
        .iter()
        .any(|g| matches!(g, Graphic::Polyline { points, .. } if points.len() >= 6))
    {
        return Some(ResistorStyle::Us);
    }
    None
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
        let floats: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.code == "sch_floating")
            .collect();
        assert!(!floats.is_empty());
        assert!(
            floats
                .iter()
                .all(|f| f.message.ends_with("(prefer.local_nets = wires)"))
        );
        // A project that prefers labels for local nets judges none.
        let mut prefs = Prefs::default();
        prefs.prefer.local_nets = LocalNets::Labels;
        let (labelled, scores) = lint_with(&b, &out, &prefs);
        assert!(!labelled.iter().any(|f| f.code == "sch_floating"));
        assert!(scores[0].notes[0].starts_with("prefer.local_nets = labels"));
    }

    #[test]
    fn a_sheet_scored_under_two_preferences_differs() {
        let b = seeded(1, 2);
        let out = crate::engine::lay_out(&b).unwrap();
        let (plain, plain_scores) = lint(&b, &out);
        // Seeded resistors are zigzags: under `iec` each is a finding.
        assert!(!plain.iter().any(|f| f.code == "sch_resistor_style"));
        let mut prefs = Prefs::default();
        prefs.prefer.resistor = ResistorStyle::Iec;
        prefs.weights.insert("sch_resistor_style".into(), 4);
        let (tuned, tuned_scores) = lint_with(&b, &out, &prefs);
        let style: Vec<&Finding> = tuned
            .iter()
            .filter(|f| f.code == "sch_resistor_style")
            .collect();
        assert!(!style.is_empty());
        for f in &style {
            assert!(f.message.contains("prefer.resistor = iec"), "{}", f.message);
            assert!(
                f.message.contains("weights.sch_resistor_style = 4"),
                "{}",
                f.message
            );
            assert_eq!(f.points, 4);
            assert_eq!(
                f.prefs,
                ["prefer.resistor = iec", "weights.sch_resistor_style = 4"]
            );
        }
        let total = |s: &[Score]| s.iter().map(|x| x.score).sum::<u32>();
        assert!(total(&tuned_scores) < total(&plain_scores));
        // A rule turned off drops its findings and says so.
        let mut off = Prefs::default();
        off.severity.insert("sch_crossings".into(), Severity::Off);
        let (quiet, quiet_scores) = lint_with(&b, &out, &off);
        assert!(!quiet.iter().any(|f| f.code == "sch_crossings"));
        assert!(quiet_scores.iter().all(|s| {
            s.notes
                .iter()
                .any(|n| n.contains("severity.sch_crossings = off"))
        }));
        // The defaults are lint itself.
        assert_eq!(lint_with(&b, &out, &Prefs::default()).0, plain);
        // Every rule has a weight.
        assert!(RULES.iter().all(|(_, w, _)| *w > 0));
    }

    #[test]
    fn a_part_out_of_line_and_a_bank_out_of_its_row_are_found() {
        let b = seeded(2, 3);
        let mut out = crate::engine::lay_out(&b).unwrap();
        // Capacitors serving no pin: one bank, whichever parts they stand
        // beside (each IC's own decoupling capacitor is a bank of its own).
        out.idioms.retain(|i| i.kind != IdiomKind::Decoupling);
        // A bank: two parts of one prefix between the same nets.
        let nets = |p: &crate::input::Part| {
            let mut v: Vec<usize> = p.pins.iter().filter_map(|x| x.drawn_net()).collect();
            v.sort();
            v.dedup();
            v
        };
        let (a, c) = (0..b.parts.len())
            .flat_map(|i| (0..b.parts.len()).map(move |j| (i, j)))
            .find(|(i, j)| {
                i < j
                    && b.parts[*i].pins.len() == 2
                    && b.parts[*i].kind == b.parts[*j].kind
                    && nets(&b.parts[*i]).len() == 2
                    && nets(&b.parts[*i]) == nets(&b.parts[*j])
                    && out.layout.placed[&b.parts[*i].key].sheet
                        == out.layout.placed[&b.parts[*j].key].sheet
            })
            .expect("a bank in the seeded block");
        let (ka, kc) = (b.parts[a].key.clone(), b.parts[c].key.clone());
        // Stand the whole bank in one row, turned alike, then move one off
        // it, and off every line.
        let first = out.layout.placed[&ka].clone();
        let row = first.at[1];
        for (j, part) in b.parts.iter().enumerate() {
            if part.kind == b.parts[a].kind && nets(part) == nets(&b.parts[a]) && j != a {
                let x = out.layout.placed.get_mut(&part.key).unwrap();
                if x.sheet == first.sheet {
                    x.at[1] = row;
                    x.rotation = first.rotation;
                    x.mirror = first.mirror;
                }
            }
        }
        let (lined, _) = lint(&b, &out);
        assert!(
            !lined
                .iter()
                .any(|f| f.code == "sch_bank_not_in_row" && f.refs.contains(&ka))
        );
        let at = out.layout.placed[&kc].at;
        out.layout.placed.get_mut(&kc).unwrap().at =
            [at[0] + 3 * GRID + GRID / 4, row - 7 * GRID - GRID / 4];
        let (moved, _) = lint(&b, &out);
        let bank = moved
            .iter()
            .find(|f| f.code == "sch_bank_not_in_row" && f.refs.contains(&kc))
            .expect("the bank out of its row");
        assert!(
            bank.message.contains("stand them in one row"),
            "{}",
            bank.message
        );
        let off = moved
            .iter()
            .find(|f| f.code == "sch_out_of_line" && f.refs == [kc.clone()])
            .expect("the part out of line");
        assert!(
            off.message.contains("lines up with no other part"),
            "{}",
            off.message
        );
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
