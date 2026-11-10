//! Buses: which of a bus's members a sheet draws as one bundle, its name,
//! and where its entries and spine go. A bundle's ends are each one side
//! of one part (or, where the members leave the sheet, one side of one
//! part and an off-sheet connector); its members are four or more nets
//! with no other pin on the sheet. Design 348's figure 5: from each
//! member pin a two-step stub carrying the member's name, a one-step 45°
//! entry, then a spine through every entry's end; a trunk joins the
//! spines.

use crate::geom::{GRID, Nm, Point, Side};
use crate::input::{Block, PartKind};
use std::collections::BTreeMap;

/// The fewest members drawn as a bus.
pub const MIN_MEMBERS: usize = 4;
/// A member pin's stub, from its end to its entry.
pub const STUB: Nm = 2 * GRID;
/// An entry's run along the stub and across it: 45°.
pub const ENTRY: Nm = GRID;

/// One side of one part on a sheet: where a bundle may end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct End {
    pub part: usize,
    pub side: Side,
}

/// A bus drawn on a sheet: its members and each member's pin at each end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    /// Members drawn, by net index, in the bus's order.
    pub members: Vec<usize>,
    /// Each end, and each member's pin there (by pin index on the end's
    /// part), in `members`' order. One end where the members leave the
    /// sheet; two otherwise.
    pub ends: Vec<(End, Vec<usize>)>,
}

/// For each net, the `(part, pin)`s on the sheet's parts `on`, a port's
/// flag excepted (a port's net is its flag's).
pub fn sides_on_sheet(block: &Block, on: &[usize]) -> BTreeMap<usize, Vec<(usize, usize)>> {
    let mut out: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for &p in on {
        if matches!(block.parts[p].kind, PartKind::Port { .. }) {
            continue;
        }
        for (k, pin) in block.parts[p].pins.iter().enumerate() {
            if let Some(n) = pin.drawn_net() {
                out.entry(n).or_default().push((p, k));
            }
        }
    }
    out
}

/// Bus `bus`'s bundle on a sheet whose pins `pins` gives: the members
/// whose pins there are exactly one on each of two ends (or one, where
/// `leaves` says the net continues on another sheet), at least
/// [`MIN_MEMBERS`] of them sharing the same ends; `None` otherwise.
/// Pin sides are read in the part's own frame (a part turned turns its
/// sides with it: the engine reads placed sides through [`bundle_with`]).
pub fn bundle(
    block: &Block,
    bus: usize,
    pins: &BTreeMap<usize, Vec<(usize, usize)>>,
) -> Option<Bundle> {
    bundle_with(block, bus, pins, &|_| false, &|p, k| {
        block.parts[p].pins[k].side
    })
}

/// [`bundle`], with which nets continue on another sheet and each pin's
/// side as placed.
pub fn bundle_with(
    block: &Block,
    bus: usize,
    pins: &BTreeMap<usize, Vec<(usize, usize)>>,
    leaves: &dyn Fn(usize) -> bool,
    side_of: &dyn Fn(usize, usize) -> Side,
) -> Option<Bundle> {
    // Each member's ends, grouped by the pair of ends they make.
    let mut by_ends: BTreeMap<Vec<End>, Vec<(usize, Vec<usize>)>> = BTreeMap::new();
    for &n in &block.buses[bus].members {
        let Some(list) = pins.get(&n) else { continue };
        let want = if leaves(n) { 1 } else { 2 };
        if list.len() != want {
            continue;
        }
        let mut ends: Vec<(End, usize)> = list
            .iter()
            .map(|&(p, k)| {
                (
                    End {
                        part: p,
                        side: side_of(p, k),
                    },
                    k,
                )
            })
            .collect();
        ends.sort();
        if ends.len() == 2 && ends[0].0.part == ends[1].0.part {
            continue;
        }
        if ends.iter().any(|(e, _)| !e.side.horizontal()) {
            continue;
        }
        let key: Vec<End> = ends.iter().map(|(e, _)| *e).collect();
        let ks: Vec<usize> = ends.iter().map(|(_, k)| *k).collect();
        by_ends.entry(key).or_default().push((n, ks));
    }
    // The largest group, ties to the least ends.
    let (ends, members) = by_ends
        .into_iter()
        .max_by(|a, b| a.1.len().cmp(&b.1.len()).then(b.0.cmp(&a.0)))?;
    if members.len() < MIN_MEMBERS {
        return None;
    }
    Some(Bundle {
        members: members.iter().map(|(n, _)| *n).collect(),
        ends: ends
            .iter()
            .enumerate()
            .map(|(i, e)| (*e, members.iter().map(|(_, ks)| ks[i]).collect()))
            .collect(),
    })
}

/// What a bundle is called: the bus's name and its members' range where
/// every member is the name (or a prefix of it ending where the member's
/// number starts) and a number, the numbers running without a gap;
/// else the bus's name.
pub fn name(block: &Block, bus: usize, members: &[usize]) -> String {
    let base = &block.buses[bus].name;
    let mut nums: Vec<u64> = Vec::new();
    for &n in members {
        let m = &block.nets[n].name;
        let digits = m.len() - m.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (stem, num) = m.split_at(m.len() - digits);
        if digits == 0 || !base.starts_with(stem) || stem.is_empty() {
            return base.clone();
        }
        let Ok(v) = num.parse::<u64>() else {
            return base.clone();
        };
        nums.push(v);
    }
    nums.sort_unstable();
    let contiguous = nums.windows(2).all(|w| w[1] == w[0] + 1);
    match (nums.first(), nums.last()) {
        (Some(a), Some(z)) if contiguous && nums.len() > 1 => format!("{base}[{a}..{z}]"),
        _ => base.clone(),
    }
}

/// How long a bundle's stubs run from their pins: its longest member's
/// name and a step, in whole steps, at least [`STUB`], so each name reads
/// on its stub as a net label's on its wire.
pub fn stub_len<'a>(names: impl IntoIterator<Item = &'a str>) -> Nm {
    let w = names
        .into_iter()
        .map(|n| crate::font::width(n, crate::font::SIZE))
        .max()
        .unwrap_or(0);
    ((w + 2 * GRID - 1) / GRID * GRID).max(STUB)
}

/// From a member pin's end pointing `side`, with stubs `stub` long: the
/// stub's end and the entry's end on the spine (the entry turns down the
/// page from the stub, so a column of entries steps one way).
pub fn entry(end: Point, side: Side, stub: Nm) -> (Point, Point) {
    let d = side.step();
    let s = [end[0] + d[0] / GRID * stub, end[1]];
    let on_spine = [s[0] + d[0] / GRID * ENTRY, s[1] - ENTRY];
    (s, on_spine)
}

/// The spine through a column of entries' ends: from the highest to the
/// lowest, at their common x.
pub fn spine(entries: &[Point]) -> Option<[Point; 2]> {
    let x = entries.first()?[0];
    if entries.iter().any(|p| p[0] != x) {
        return None;
    }
    let top = entries.iter().map(|p| p[1]).max()?;
    let bottom = entries.iter().map(|p| p[1]).min()?;
    Some([[x, top], [x, bottom]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Bus, NetKind};
    use crate::testing;

    fn named(names: &[&str]) -> Block {
        let mut b = Block::default();
        for n in names {
            b.nets
                .push(testing::net(&n.to_ascii_lowercase(), n, NetKind::Signal));
        }
        b.buses.push(Bus {
            key: "bus:d".into(),
            name: "QSPI_SD".into(),
            members: (0..names.len()).collect(),
        });
        b
    }

    #[test]
    fn a_bus_is_named_by_its_members_range() {
        let b = named(&["QSPI_SD0", "QSPI_SD1", "QSPI_SD2", "QSPI_SD3"]);
        assert_eq!(name(&b, 0, &[0, 1, 2, 3]), "QSPI_SD[0..3]");
        assert_eq!(name(&b, 0, &[1, 2]), "QSPI_SD[1..2]");
        // A gap, or a member that isn't the name and a number: the name.
        assert_eq!(name(&b, 0, &[0, 2]), "QSPI_SD");
        let odd = named(&["QSPI_SD0", "CLK"]);
        assert_eq!(name(&odd, 0, &[0, 1]), "QSPI_SD");
    }

    #[test]
    fn the_members_between_two_sides_make_the_bundle() {
        // Two ICs, four nets each from u1's right to u2's left: one bundle.
        let mut b = named(&["D0", "D1", "D2", "D3", "X"]);
        b.buses[0].members = vec![0, 1, 2, 3];
        let side = io_pins();
        b.parts.push(testing::ic("u1", "U1", "A", &[], &side));
        b.parts.push(testing::ic("u2", "U2", "B", &side, &[]));
        let ends = sides_on_sheet(&b, &every_part(&b));
        let found = bundle(&b, 0, &ends).expect("a bundle");
        assert_eq!(found.members, vec![0, 1, 2, 3]);
        assert_eq!(found.ends.len(), 2);
        // A member with a third pin on the sheet leaves the bundle; under
        // four left, none.
        b.parts
            .push(testing::resistor("r1", "R1", "1k", [Some(3), Some(4)]));
        let ends = sides_on_sheet(&b, &every_part(&b));
        assert!(bundle(&b, 0, &ends).is_none(), "three members aren't a bus");
    }

    /// Four IO pins on nets 0 to 3.
    fn io_pins() -> Vec<(&'static str, crate::input::PinKind, Option<usize>)> {
        ["IO0", "IO1", "IO2", "IO3"]
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, crate::input::PinKind::Bidirectional, Some(i)))
            .collect()
    }

    fn every_part(b: &Block) -> Vec<usize> {
        (0..b.parts.len()).collect()
    }
}
