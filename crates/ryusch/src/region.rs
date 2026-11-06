//! Functional regions: titled rectangles on a sheet, each holding one
//! function's parts (a recognised circuit, a dense IC with its
//! satellites, a connector with its breakout, a connected group). They
//! are layout, never connectivity: nothing electrical reads them.

use crate::cluster::Cluster;
use crate::geom::{GRID, Nm, Place};
use crate::idiom::Idiom;
use crate::input::{Block, NetKind, PartKind};
use crate::instr;
use crate::layout::IdiomKind;
use std::collections::{BTreeMap, BTreeSet};

/// A region's margin inside its frame, on every side.
pub const FRAME: Nm = 2 * GRID;
/// The band at the top of a frame its title stands in.
pub const TITLE: Nm = 3 * GRID;
/// How far paper fit widens the channels between regions, at most.
pub const WIDEN: Nm = 4 * GRID;

/// What a region was proposed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Basis {
    /// A `region` instruction.
    Named,
    /// A recognised circuit: a buck, an op-amp stage.
    Topology,
    /// An active part of more than 8 pins, its idioms and its guests.
    Ic,
    /// A connector and its breakout.
    Connector,
    /// A connected group of what is left.
    Rest,
}

/// A proposed region: its title and its parts, by index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposed {
    pub title: String,
    pub basis: Basis,
    pub parts: BTreeSet<usize>,
}

/// Pins past which an active part leads a region of its own.
pub const DENSE: usize = 8;

/// `<refdes> <value>`, trimmed: `J1` where it has no value.
fn name_of(block: &Block, p: usize) -> String {
    let part = &block.parts[p];
    format!("{} {}", part.refdes, part.value).trim().to_owned()
}

/// Proposes regions for `block`'s parts, block ports aside, in design
/// 348's order: named regions (`r.regions`), each recognised topology,
/// each active part of more than 8 pins with the idioms it anchors and the
/// guests whose host it is, each connector with its idioms and guests,
/// then each connected group of what is left over signal nets. Each part
/// is in the first region that takes it, and every idiom's parts follow
/// their anchor (clusters are rigid).
pub fn propose(block: &Block, idioms: &[Idiom], r: &instr::Resolved) -> Vec<Proposed> {
    let n = block.parts.len();
    let is_port = |p: usize| matches!(block.parts[p].kind, PartKind::Port { .. });
    let title_of = |p: usize, fallback: String| {
        let f = &block.parts[p].function;
        if f.is_empty() { fallback } else { f.clone() }
    };
    let mut by_key: Vec<usize> = (0..n).collect();
    by_key.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    let mut out: Vec<Proposed> = Vec::new();
    let push = |out: &mut Vec<Proposed>,
                taken: &mut BTreeSet<usize>,
                title: String,
                basis: Basis,
                parts: BTreeSet<usize>| {
        let parts: BTreeSet<usize> = parts
            .into_iter()
            .filter(|p| !taken.contains(p) && !is_port(*p))
            .collect();
        if parts.is_empty() {
            return;
        }
        taken.extend(parts.iter().copied());
        out.push(Proposed {
            title,
            basis,
            parts,
        });
    };
    // Named regions first: the agent's title wins over a function.
    for (title, parts) in &r.regions {
        push(
            &mut out,
            &mut taken,
            title.clone(),
            Basis::Named,
            parts.clone(),
        );
    }
    // Topologies.
    for i in idioms {
        if !matches!(i.kind, IdiomKind::Buck | IdiomKind::OpampStage) {
            continue;
        }
        let Some((a, _)) = i.anchor else { continue };
        if taken.contains(&a) {
            continue;
        }
        let part = &block.parts[a];
        let what = if part.value.is_empty() {
            &part.refdes
        } else {
            &part.value
        };
        let fallback = if i.kind == IdiomKind::Buck {
            format!("{what} converter")
        } else {
            format!("{what} stage")
        };
        let mut parts: BTreeSet<usize> = i.parts.iter().copied().collect();
        parts.insert(a);
        push(
            &mut out,
            &mut taken,
            title_of(a, fallback),
            Basis::Topology,
            parts,
        );
    }
    // Clusters as the engine makes them, for guests.
    let singles: Vec<Cluster> = (0..n)
        .map(|p| Cluster::single(p, Place::default()))
        .collect();
    let clusters = crate::idiom::clusters(block, idioms, singles);
    let leads = |out: &mut Vec<Proposed>, taken: &mut BTreeSet<usize>, p: usize, basis: Basis| {
        let mut parts: BTreeSet<usize> = BTreeSet::from([p]);
        for i in idioms {
            if i.anchor.is_some_and(|(a, _)| a == p) {
                parts.extend(i.parts.iter().copied().filter(|q| !taken.contains(q)));
            }
        }
        for cl in &clusters {
            let free = cl
                .members
                .iter()
                .all(|(q, _)| !taken.contains(q) && !parts.contains(q));
            if free
                && crate::engine::guest(block, cl, r)
                && crate::engine::host(block, cl) == Some(p)
            {
                parts.extend(cl.members.iter().map(|(q, _)| *q));
            }
        }
        push(out, taken, title_of(p, name_of(block, p)), basis, parts);
    };
    // Dense ICs.
    for &p in &by_key {
        let dense = matches!(block.parts[p].kind, PartKind::Symbol { .. })
            && !crate::cluster::is_connector(block, p)
            && block.parts[p].pins.len() > DENSE;
        if dense && !taken.contains(&p) {
            leads(&mut out, &mut taken, p, Basis::Ic);
        }
    }
    // Connectors.
    for &p in &by_key {
        if crate::cluster::is_connector(block, p) && !taken.contains(&p) {
            leads(&mut out, &mut taken, p, Basis::Connector);
        }
    }
    // The rest: connected groups over signal nets.
    let rest: Vec<usize> = by_key
        .iter()
        .copied()
        .filter(|p| !taken.contains(p) && !is_port(*p))
        .collect();
    let mut root: BTreeMap<usize, usize> = rest.iter().map(|p| (*p, *p)).collect();
    fn find(root: &mut BTreeMap<usize, usize>, p: usize) -> usize {
        let mut x = p;
        while root[&x] != x {
            x = root[&x];
        }
        root.insert(p, x);
        x
    }
    let mut on_net: BTreeMap<usize, usize> = BTreeMap::new();
    for &p in &rest {
        for pin in &block.parts[p].pins {
            for net in pin.nets() {
                if block.nets[net].kind != NetKind::Signal {
                    continue;
                }
                match on_net.get(&net) {
                    Some(&q) => {
                        let (a, b) = (find(&mut root, p), find(&mut root, q));
                        if a != b {
                            let (lo, hi) = if block.parts[a].key < block.parts[b].key {
                                (a, b)
                            } else {
                                (b, a)
                            };
                            root.insert(hi, lo);
                        }
                    }
                    None => {
                        on_net.insert(net, p);
                    }
                }
            }
        }
    }
    let mut groups: Vec<(usize, BTreeSet<usize>)> = Vec::new();
    for &p in &rest {
        let g = find(&mut root, p);
        match groups.iter_mut().find(|(r0, _)| *r0 == g) {
            Some((_, set)) => {
                set.insert(p);
            }
            None => groups.push((g, BTreeSet::from([p]))),
        }
    }
    // Pins on each signal net, the whole block's: a group's busiest net.
    let mut pins_on: BTreeMap<usize, usize> = BTreeMap::new();
    for part in &block.parts {
        for pin in &part.pins {
            for net in pin.nets() {
                *pins_on.entry(net).or_default() += 1;
            }
        }
    }
    for (_, parts) in groups {
        let mut ordered: Vec<usize> = parts.iter().copied().collect();
        ordered.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
        let title = match ordered
            .iter()
            .find(|p| !block.parts[**p].function.is_empty())
        {
            Some(p) => block.parts[*p].function.clone(),
            None => {
                let nets: BTreeSet<usize> = parts
                    .iter()
                    .flat_map(|p| block.parts[*p].pins.iter().flat_map(|x| x.nets()))
                    .filter(|n| block.nets[*n].kind == NetKind::Signal)
                    .collect();
                nets.into_iter()
                    .max_by(|a, b| {
                        pins_on[a]
                            .cmp(&pins_on[b])
                            .then_with(|| block.nets[*b].name.cmp(&block.nets[*a].name))
                    })
                    .map_or_else(|| "Misc".to_owned(), |n| block.nets[n].name.clone())
            }
        };
        push(&mut out, &mut taken, title, Basis::Rest, parts);
    }
    // Every idiom's parts follow their anchor: clusters are rigid.
    for i in idioms {
        let Some((a, _)) = i.anchor else { continue };
        let Some(home) = out.iter().position(|g| g.parts.contains(&a)) else {
            continue;
        };
        for &q in &i.parts {
            if let Some(was) = out.iter().position(|g| g.parts.contains(&q))
                && was != home
                && out[was].basis != Basis::Named
            {
                out[was].parts.remove(&q);
                out[home].parts.insert(q);
            }
        }
    }
    out.retain(|g| !g.parts.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_are_proposed_from_function_in_order() {
        let block = crate::testing::regions_block();
        let r = crate::instr::resolve(&block).unwrap();
        let idioms = crate::idiom::find(&block, &r.idiom);
        let got = propose(&block, &idioms, &r);
        let titled: Vec<(&str, Vec<&str>)> = got
            .iter()
            .map(|g| {
                let mut keys: Vec<&str> = g
                    .parts
                    .iter()
                    .map(|p| block.parts[*p].key.as_str())
                    .collect();
                keys.sort();
                (g.title.as_str(), keys)
            })
            .collect();
        assert_eq!(
            titled,
            vec![
                ("TPS54331 converter", vec!["c2", "l1", "u2"]),
                ("U1 LAN8710A", vec!["c1", "r1", "u1"]),
                ("J1", vec!["j1"]),
                ("F1", vec!["c9", "r9"]),
            ],
            "topologies, dense ICs, connectors, then connected groups; ports in none"
        );
        let mut named = block.clone();
        named.parts[7].function = "Anti-alias filter".into();
        let got = propose(&named, &idioms, &r);
        assert_eq!(
            got[3].title, "Anti-alias filter",
            "a function names its region"
        );
    }
}
