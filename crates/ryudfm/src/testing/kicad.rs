//! KiCad's own outputs of a board, to compare the reader with.

use std::collections::{BTreeMap, BTreeSet};

use ryu_board::geom::decimal::{Unit, nm_value};
use ryu_board::model::{Board, Component, Side};

/// Compares a board's components and pin nets with KiCad's own outputs
/// for it: its position file (`kicad-cli pcb export pos --format csv
/// --units mm --side both`) and its IPC-D-356 netlist. Returns every
/// disagreement.
pub fn compare_with_kicad(b: &Board, pos: &str, d356: &str) -> Vec<String> {
    let s = &b.steps[b.board_step];
    let mut out = Vec::new();
    let by_ref: BTreeMap<&str, &Component> = s
        .components
        .iter()
        .map(|c| (c.refdes.as_str(), c))
        .collect();
    // Footprints with pins: those the netlist names. KiCad's positions
    // also list footprints without (logos), which ODB++ has no component
    // for.
    let pinned: BTreeSet<&str> = d356
        .lines()
        .filter(|l| (l.starts_with("317") || l.starts_with("327")) && l.len() >= 26)
        .map(|l| l[20..26].trim())
        .collect();
    let mut rows = 0;
    for line in pos.lines().skip(1) {
        let f: Vec<String> = line
            .split(',')
            .map(|x| x.trim_matches('"').to_string())
            .collect();
        if f.len() < 7 {
            continue;
        }
        rows += 1;
        let Some(c) = by_ref.get(f[0].as_str()) else {
            if !pinned.contains(f[0].as_str()) {
                continue;
            }
            out.push(format!(
                "{}: in KiCad's positions, not among the components",
                f[0]
            ));
            continue;
        };
        let x = nm_value(&f[3], Unit::Mm).unwrap();
        let y = nm_value(&f[4], Unit::Mm).unwrap();
        if (c.at.x - x).abs() > 1 || (c.at.y - y).abs() > 1 {
            out.push(format!("{}: at {:?}, KiCad says ({x}, {y})", f[0], c.at));
        }
        let side = if f[6] == "bottom" {
            Side::Bottom
        } else {
            Side::Top
        };
        if c.side != side {
            out.push(format!("{}: on {:?}, KiCad says {}", f[0], c.side, f[6]));
        }
    }
    if rows == 0 {
        out.push("no rows in KiCad's positions".into());
    }
    // IPC-D-356 pin records: the net in columns 4-17 (its last 14
    // characters, in capitals, with `?` for some it can't write), the
    // reference designator in 21-26, the pin in 28-31.
    for (net, refdes, pin) in d356_records(d356) {
        let (net, refdes, pin) = (net.as_str(), refdes.as_str(), pin.as_str());
        let found = by_designator(s, refdes).map(|i| &s.components[i]);
        let Some(c) = found else {
            if refdes.len() < 6 {
                out.push(format!(
                    "{refdes}.{pin}: in the netlist, not among the components"
                ));
            }
            continue;
        };
        let Some(p) = c.pins.iter().find(|p| p.name == pin) else {
            out.push(format!("{refdes}.{pin}: no such pin"));
            continue;
        };
        let ours = p.net.map(|n| s.nets[n as usize].name.as_str());
        let agrees = match ours {
            None | Some("$NONE$") => net == "N/C",
            Some(n) => {
                // Names cut alike are told apart by a `#n` at the end.
                let (net, cut) = match net.rsplit_once('#') {
                    Some((head, k)) if !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()) => {
                        (head, true)
                    }
                    _ => (net, n.chars().count() > 14),
                };
                let ours: Vec<char> = n.chars().collect();
                let theirs: Vec<char> = net.chars().collect();
                // A cut name is the end of ours; an uncut one all of it.
                let tail = if cut {
                    &ours[ours.len().saturating_sub(theirs.len())..]
                } else {
                    &ours[..]
                };
                tail.len() == theirs.len()
                    && tail
                        .iter()
                        .zip(&theirs)
                        .all(|(a, b)| *b == '?' || a.eq_ignore_ascii_case(b))
            }
        };
        if !agrees {
            out.push(format!("{refdes}.{pin}: net {ours:?}, KiCad says {net}"));
        }
    }
    out
}

/// IPC-D-356 pin records: the net (columns 4-17, cut as KiCad cuts it),
/// the reference designator (21-26) and the pin (28-31), but vias.
pub fn d356_records(d356: &str) -> Vec<(String, String, String)> {
    d356.lines()
        .filter(|l| (l.starts_with("317") || l.starts_with("327")) && l.len() >= 31)
        .map(|l| {
            (
                l[3..17].trim().to_string(),
                l[20..26].trim().to_string(),
                l[27..31].trim().to_string(),
            )
        })
        .filter(|(_, r, _)| r != "VIA" && !r.is_empty())
        .collect()
}

/// The component a designator names, cut to six characters as IPC-D-356
/// cuts it: the one it begins, if only one does.
fn by_designator(s: &ryu_board::model::Step, refdes: &str) -> Option<usize> {
    if let Some(i) = s.components.iter().position(|c| c.refdes == refdes) {
        return Some(i);
    }
    let mut m = s
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| refdes.len() == 6 && c.refdes.starts_with(refdes));
    match (m.next(), m.next()) {
        (Some((i, _)), None) => Some(i),
        _ => None,
    }
}

/// Our islands against KiCad's IPC-D-356 netlist of the board: each of its
/// pin records is a point of its net at our pin's place (on its
/// component's side, or through the hole there), and the compare must find
/// no open, no short and no point off copper. Returns each one found.
pub fn compare_islands_with_d356(cb: &crate::copper::CopperBoard, d356: &str) -> Vec<String> {
    use crate::copper::Key;
    use ryu_copper::layout::{NetPoint, PointSide};
    let s = &cb.board.steps[cb.built.step];
    let mut points = Vec::new();
    for (net, refdes, pin) in d356_records(d356) {
        if net == "N/C" {
            continue;
        }
        let Some(c) = by_designator(s, &refdes) else {
            continue;
        };
        let Some(p) = s.components[c].pins.iter().position(|p| p.name == pin) else {
            continue;
        };
        points.push(NetPoint {
            net,
            at: s.components[c].pins[p].at,
            side: match s.components[c].side {
                Side::Top => Some(PointSide::Top),
                Side::Bottom => Some(PointSide::Bottom),
                Side::Inner => None,
            },
            key: Key::Pin {
                component: c as u32,
                pin: p as u32,
            },
        });
    }
    let r = ryu_copper::netlist::compare(&cb.copper, &points);
    let mut out = Vec::new();
    for o in &r.opens {
        out.push(format!(
            "open: {} lies on {} islands",
            o.net,
            o.islands.len()
        ));
    }
    for x in &r.shorts {
        let nets: Vec<&str> = x.nets.iter().map(|(n, _)| n.as_str()).collect();
        out.push(format!(
            "short: island {} joins {}",
            cb.island_name(None, &x.island),
            nets.join(" + ")
        ));
    }
    for o in &r.off_copper {
        out.push(format!(
            "off copper: {} of {} at {:?}{}",
            cb.reference(None, o.key),
            o.net,
            o.at,
            if o.uncertain { " (uncertain)" } else { "" }
        ));
    }
    out
}

/// How far from the clearance a gap only one of us reports is tolerated:
/// KiCad's default maximum arc error (5 µm) and a micrometre of its
/// reports' precision.
pub const TOLERANCE: ryu_board::geom::Nm = 6_000;

/// KiCad 9's DRC stops reporting a type near 500 violations (499 to 502
/// seen on boards with thousands, run to run): a report this long is
/// cut, so only its gaps are checked against ours, not ours against it.
pub const KICAD_REPORT_CAP: usize = 450;

/// A net's name as ODB++ writes it: KiCad's export puts `_` for spaces.
/// KiCad's `<no net>` is ours as it is.
fn odb_net(n: &str) -> String {
    if n == "<no net>" {
        return n.to_string();
    }
    n.replace(' ', "_")
}

/// The net in a KiCad DRC item's description: `Track [GND] on F.Cu, ...`.
fn drc_net(d: &str) -> String {
    d.split_once('[')
        .and_then(|(_, r)| r.split_once(']'))
        .map(|(n, _)| odb_net(n))
        .unwrap_or_else(|| "<no net>".into())
}

/// The layer in a description, upper case as ODB++ names it: `... on F.Cu`;
/// `None` for one on a span (`Via [GND] on F.Cu - B.Cu`).
fn drc_layer(d: &str) -> Option<String> {
    let (_, r) = d.rsplit_once(" on ")?;
    if r.contains(" - ") {
        return None;
    }
    Some(r.split([',', ' ']).next()?.to_ascii_uppercase())
}

/// The nets KiCad's DRC report says are unconnected, as ODB++ names them.
pub fn drc_unconnected(drc: &serde_json::Value) -> BTreeSet<String> {
    drc["unconnected_items"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|v| {
            v["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| i["description"].as_str().map(drc_net))
        })
        .collect()
}

/// The distance a violation's description gives: `... actual 0.1500 mm)`.
fn drc_actual(d: &str) -> Option<ryu_board::geom::Nm> {
    let i = d.find("actual ")?;
    let num = d[i + 7..].split_whitespace().next()?;
    nm_value(num, Unit::Mm).ok()
}

fn sorted_pair(a: String, b: String) -> (String, String) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Our opens, shorts and gaps under `clearance` against KiCad's DRC report
/// at that clearance (`kicad-cli pcb drc --format json`): nets open, pairs
/// of nets shorted, and pairs of nets closer than the clearance on a
/// layer. Gaps only one of us reports are tolerated within [`TOLERANCE`]
/// of the clearance, and printed. Returns every other disagreement.
///
/// Two rules: when the board's CAD netlist places no point on copper
/// (#933), our compare judged nothing, so opens and shorts are printed,
/// not compared; and when KiCad's clearance violations reach
/// [`KICAD_REPORT_CAP`], its report is cut, so each of its gaps must be
/// ours but ours needn't be its (their count is printed).
pub fn compare_with_drc(
    cb: &crate::copper::CopperBoard,
    drc: &serde_json::Value,
    clearance: ryu_board::geom::Nm,
) -> Vec<String> {
    let items = |v: &serde_json::Value| -> Vec<String> {
        v["items"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|i| i["description"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let violations: Vec<serde_json::Value> =
        drc["violations"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    let points = cb.built.layout.points.len();
    let unjudged = points > 0 && cb.report.off_copper.len() == points;
    // Opens.
    let theirs = drc_unconnected(drc);
    let ours: BTreeSet<String> = cb.report.opens.iter().map(|o| odb_net(&o.net)).collect();
    if unjudged {
        println!(
            "no CAD point lies on copper (#933): KiCad's unconnected nets {theirs:?} aren't compared"
        );
    } else {
        for n in theirs.difference(&ours) {
            out.push(format!("net {n} open for KiCad, not for ryudfm"));
        }
        for n in ours.difference(&theirs) {
            out.push(format!("net {n} open for ryudfm, not for KiCad"));
        }
    }
    // Shorts.
    let theirs: BTreeSet<(String, String)> = violations
        .iter()
        .filter(|v| v["type"] == "shorting_items")
        .filter_map(|v| {
            let d = items(v);
            Some(sorted_pair(drc_net(d.first()?), drc_net(d.get(1)?)))
        })
        .collect();
    let mut ours = BTreeSet::new();
    for s in &cb.report.shorts {
        for (i, (a, _)) in s.nets.iter().enumerate() {
            for (b, _) in &s.nets[i + 1..] {
                ours.insert(sorted_pair(odb_net(a), odb_net(b)));
            }
        }
    }
    for p in theirs.symmetric_difference(&ours) {
        if unjudged {
            println!("not compared (#933): {} and {} shorted", p.0, p.1);
            continue;
        }
        let who = if ours.contains(p) { "ryudfm" } else { "KiCad" };
        out.push(format!("{} and {} shorted for {who} alone", p.0, p.1));
    }
    // Gaps: by layer and pair of nets, KiCad's with the distance it gives.
    type Gaps<T> = BTreeMap<(String, String, String), T>;
    let theirs: Gaps<(Option<ryu_board::geom::Nm>, String)> = violations
        .iter()
        .filter(|v| v["type"] == "clearance")
        .filter_map(|v| {
            let d = items(v);
            let (a, b) = sorted_pair(drc_net(d.first()?), drc_net(d.get(1)?));
            let layer = d.iter().find_map(|x| drc_layer(x))?;
            Some((
                (layer, a, b),
                (
                    drc_actual(v["description"].as_str().unwrap_or("")),
                    d.join(" | "),
                ),
            ))
        })
        .collect();
    let capped = violations
        .iter()
        .filter(|v| v["type"] == "clearance")
        .count()
        >= KICAD_REPORT_CAP;
    let net = |id: &ryu_copper::connect::IslandId<crate::copper::Key>| {
        cb.nets
            .get(id)
            .and_then(|s| s.iter().next().cloned())
            .or_else(|| cb.claims().get(id).and_then(|c| c.first().cloned()))
            .map(|n| odb_net(&n))
            .unwrap_or_else(|| "<no net>".into())
    };
    let mut ours: Gaps<ryu_board::geom::Nm> = BTreeMap::new();
    for l in 0..cb.built.copper.len() {
        for g in cb.gaps(l, clearance).iter() {
            let (a, b) = sorted_pair(net(&g.islands.0), net(&g.islands.1));
            if a == b {
                continue; // KiCad never checks clearance within a net.
            }
            let e = ours
                .entry((cb.layer_name(l).to_ascii_uppercase(), a, b))
                .or_insert(g.distance.nm);
            *e = (*e).min(g.distance.nm);
        }
    }
    if capped {
        let only = ours.keys().filter(|k| !theirs.contains_key(*k)).count();
        println!(
            "KiCad's report is cut at {KICAD_REPORT_CAP} clearance violations: {only} of our {} pairs aren't in it, as expected",
            ours.len()
        );
    }
    for (k, d) in &ours {
        if !capped && !theirs.contains_key(k) {
            if *d >= clearance - TOLERANCE {
                println!(
                    "tolerated: ryudfm's gap {k:?} of {d} nm, within {TOLERANCE} nm of {clearance}"
                );
            } else {
                out.push(format!("gap {k:?} of {d} nm for ryudfm, not for KiCad"));
            }
        }
    }
    for (k, (actual, what)) in &theirs {
        if !ours.contains_key(k) {
            match actual {
                Some(a) if *a >= clearance - TOLERANCE => {
                    println!("tolerated: KiCad's gap {k:?} of {a} nm")
                }
                // Copper of a net touched by copper of none: one island
                // for us, so no gap.
                Some(0) => println!("a contact, not a gap: KiCad's {k:?} ({what})"),
                _ => out.push(format!(
                    "gap {k:?} of {actual:?} nm for KiCad, not for ryudfm ({what})"
                )),
            }
        }
    }
    out
}
