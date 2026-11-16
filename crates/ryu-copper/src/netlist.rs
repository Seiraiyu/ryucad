//! The netlist compare: the copper's islands against a netlist's points.
//!
//! Each point is placed (`Copper::place`). A net whose points lie on two
//! islands or more is open; an island holding points of two nets or more
//! is a short, with where it is when the source claims nets (touching
//! items whose claims differ, and plated holes joining such items); an
//! island whose points all carry one net while an item on it claims
//! another is a mismatch: the source's own data contradicts its copper. A
//! point no island holds, or that the bounds can't place, is off copper.
//! Findings are typed; the products give them codes and severities.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;

use rayon::prelude::*;
use ryu_board::geom::Point;

use crate::connect::{Copper, IslandId, Node, Placement};
use crate::layout::NetPoint;

/// A net on two islands or more.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Open<K> {
    pub net: String,
    /// Each island with the net's points on it, sorted.
    pub islands: Vec<(IslandId<K>, Vec<K>)>,
}

/// An island holding points of two nets or more.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Short<K> {
    pub island: IslandId<K>,
    /// Each net with its points on the island, sorted.
    pub nets: Vec<(String, Vec<K>)>,
    /// Touching items of the island whose claimed nets differ, the lesser
    /// key first, sorted.
    pub contacts: Vec<(K, K)>,
    /// Plated holes of the island joining items whose claims differ.
    pub holes: Vec<K>,
}

/// A point no island holds, or that the bounds can't place.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OffCopper<K> {
    pub key: K,
    pub net: String,
    pub at: Point,
    /// Whether the bounds left it open rather than no copper holding it.
    pub uncertain: bool,
}

/// Items claiming a net other than the one their island's points agree on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Mismatch<K> {
    pub island: IslandId<K>,
    /// The net the island's points carry.
    pub net: String,
    /// Each item with the net it claims, sorted.
    pub items: Vec<(K, String)>,
}

/// What the compare found.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Report<K> {
    /// False when there were no points: nothing is judged.
    pub judged: bool,
    /// By net.
    pub opens: Vec<Open<K>>,
    /// By island.
    pub shorts: Vec<Short<K>>,
    /// By key.
    pub off_copper: Vec<OffCopper<K>>,
    /// By island.
    pub mismatches: Vec<Mismatch<K>>,
}

fn sorted<K: Ord + Clone>(keys: &[K]) -> Vec<K> {
    let mut v = keys.to_vec();
    v.sort();
    v
}

/// Where each point lands, in parallel, in the points' order.
pub fn placements<K: Ord + Hash + Clone + Send + Sync>(
    copper: &Copper<K>,
    points: &[NetPoint<K>],
) -> Vec<Placement<K>> {
    points
        .par_iter()
        .map(|p| copper.place(p.at, p.side))
        .collect()
}

/// Compares the copper with a netlist's points, placed in parallel.
pub fn compare<K: Ord + Hash + Clone + Send + Sync>(
    copper: &Copper<K>,
    points: &[NetPoint<K>],
) -> Report<K> {
    compare_placed(copper, points, &placements(copper, points))
}

/// [`compare`], the points already placed (`placed[i]` is `points[i]`'s).
pub fn compare_placed<K: Ord + Hash + Clone + Send + Sync>(
    copper: &Copper<K>,
    points: &[NetPoint<K>],
    placed: &[Placement<K>],
) -> Report<K> {
    let mut r = Report {
        judged: !points.is_empty(),
        opens: Vec::new(),
        shorts: Vec::new(),
        off_copper: Vec::new(),
        mismatches: Vec::new(),
    };
    let mut nets: BTreeMap<&str, BTreeMap<IslandId<K>, Vec<K>>> = BTreeMap::new();
    let mut islands: BTreeMap<IslandId<K>, BTreeMap<&str, Vec<K>>> = BTreeMap::new();
    for (p, at) in points.iter().zip(placed.iter().cloned()) {
        let uncertain = match at {
            Placement::On(id) => {
                nets.entry(p.net.as_str())
                    .or_default()
                    .entry(id.clone())
                    .or_default()
                    .push(p.key.clone());
                islands
                    .entry(id)
                    .or_default()
                    .entry(p.net.as_str())
                    .or_default()
                    .push(p.key.clone());
                continue;
            }
            Placement::Off => false,
            Placement::Uncertain => true,
        };
        r.off_copper.push(OffCopper {
            key: p.key.clone(),
            net: p.net.clone(),
            at: p.at,
            uncertain,
        });
    }
    for (net, on) in &nets {
        if on.len() >= 2 {
            r.opens.push(Open {
                net: net.to_string(),
                islands: on.iter().map(|(id, k)| (id.clone(), sorted(k))).collect(),
            });
        }
    }
    for (id, by) in &islands {
        let Some(island) = copper.island(&Node::Piece(id.0.clone(), id.1)) else {
            continue;
        };
        if by.len() >= 2 {
            let (contacts, holes) = conflicts(copper, &island.pieces, &island.holes);
            r.shorts.push(Short {
                island: id.clone(),
                nets: by.iter().map(|(n, k)| (n.to_string(), sorted(k))).collect(),
                contacts,
                holes,
            });
        } else if let Some((net, _)) = by.iter().next() {
            let items: Vec<(K, String)> = island
                .items
                .iter()
                .filter_map(|k| {
                    let claim = copper.item(k)?.net.as_ref()?;
                    (claim.as_str() != *net).then(|| (k.clone(), claim.clone()))
                })
                .collect();
            if !items.is_empty() {
                r.mismatches.push(Mismatch {
                    island: id.clone(),
                    net: net.to_string(),
                    items,
                });
            }
        }
    }
    r.off_copper.sort_by(|a, b| a.key.cmp(&b.key));
    r
}

/// Where a short is: touching items whose claimed nets differ, and plated
/// holes joining items whose claims differ.
fn conflicts<K: Ord + Hash + Clone + Send + Sync>(
    copper: &Copper<K>,
    pieces: &[(K, u32)],
    holes: &[K],
) -> (Vec<(K, K)>, Vec<K>) {
    let claim = |k: &K| copper.item(k).and_then(|it| it.net.clone());
    let mut pairs = BTreeSet::new();
    for (k, i) in pieces {
        let Some(a) = claim(k) else {
            continue;
        };
        for n in copper.contacts(&Node::Piece(k.clone(), *i)) {
            if let Node::Piece(j, _) = n
                && j > *k
                && let Some(b) = claim(&j)
                && a != b
            {
                pairs.insert((k.clone(), j));
            }
        }
    }
    let holes = holes
        .iter()
        .filter(|h| {
            let claims: BTreeSet<String> = copper
                .contacts(&Node::Hole((*h).clone()))
                .iter()
                .filter_map(|n| match n {
                    Node::Piece(k, _) => claim(k),
                    Node::Hole(_) => None,
                })
                .collect();
            claims.len() >= 2
        })
        .cloned()
        .collect();
    (pairs.into_iter().collect(), holes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::fixtures::*;
    use crate::layout::{Layout, PointSide};
    use crate::measure::BodyOptions;

    fn point(key: &'static str, net: &str, at: Point) -> NetPoint<&'static str> {
        NetPoint {
            net: net.into(),
            at,
            side: Some(PointSide::Top),
            key,
        }
    }

    /// a and b touch end to end, claiming different nets; c and d too,
    /// apart from them; the points open GND, short it with SIG, and leave
    /// one off copper.
    fn shorted_and_open() -> (Layout<&'static str>, Copper<&'static str>) {
        let mut l = board(1);
        // a and b touch end to end, claiming different nets; c and d too,
        // apart from them.
        let at = |x: i64, y: i64| p(x, y);
        l.items.insert(
            "a",
            trace(0, at(0, 0), at(1_000_000, 0), 100_000, Some("GND")),
        );
        l.items.insert(
            "b",
            trace(0, at(1_000_000, 0), at(2_000_000, 0), 100_000, Some("SIG")),
        );
        let y = 5_000_000;
        l.items.insert(
            "c",
            trace(0, at(0, y), at(1_000_000, y), 100_000, Some("GND")),
        );
        l.items.insert(
            "d",
            trace(0, at(1_000_000, y), at(2_000_000, y), 100_000, Some("VCC")),
        );
        l.points = vec![
            point("p1", "GND", at(0, 0)),
            point("p2", "SIG", at(2_000_000, 0)),
            point("p3", "GND", at(0, y)),
            point("p4", "SIG", at(9_000_000, 9_000_000)),
        ];
        let c = Copper::new(&l, BodyOptions::default());
        (l, c)
    }

    #[test]
    fn opens_shorts_strays_and_claims() {
        let (l, c) = shorted_and_open();
        let at = |x: i64, y: i64| p(x, y);
        let r = compare(&c, &l.points);
        assert!(r.judged);
        assert_eq!(
            r.opens,
            vec![Open {
                net: "GND".into(),
                islands: vec![(("a", 0), vec!["p1"]), (("c", 0), vec!["p3"])],
            }]
        );
        assert_eq!(
            r.shorts,
            vec![Short {
                island: ("a", 0),
                nets: vec![("GND".into(), vec!["p1"]), ("SIG".into(), vec!["p2"])],
                contacts: vec![("a", "b")],
                holes: vec![],
            }]
        );
        assert_eq!(
            r.off_copper,
            vec![OffCopper {
                key: "p4",
                net: "SIG".into(),
                at: at(9_000_000, 9_000_000),
                uncertain: false,
            }]
        );
        assert_eq!(
            r.mismatches,
            vec![Mismatch {
                island: ("c", 0),
                net: "GND".into(),
                items: vec![("d", "VCC".into())],
            }]
        );
        // No netlist: nothing judged.
        let none = compare(&c, &[]);
        assert!(!none.judged);
        assert!(none.opens.is_empty() && none.shorts.is_empty() && none.mismatches.is_empty());
    }

    #[test]
    fn placed_apart_the_compare_is_the_same() {
        let (l, c) = shorted_and_open();
        let placed = placements(&c, &l.points);
        assert_eq!(placed.len(), l.points.len());
        assert_eq!(
            compare_placed(&c, &l.points, &placed),
            compare(&c, &l.points)
        );
    }
}
