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
    for line in d356.lines() {
        if !(line.starts_with("317") || line.starts_with("327")) || line.len() < 31 {
            continue;
        }
        let net = line[3..17].trim();
        let refdes = line[20..26].trim();
        let pin = line[27..31].trim();
        if refdes == "VIA" || refdes.is_empty() {
            continue;
        }
        // A designator cut to six characters names the one component it
        // begins, if only one does.
        let found = by_ref.get(refdes).copied().or_else(|| {
            let mut m = by_ref
                .iter()
                .filter(|(r, _)| refdes.len() == 6 && r.starts_with(refdes));
            match (m.next(), m.next()) {
                (Some((_, c)), None) => Some(*c),
                _ => None,
            }
        });
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
