//! A board's connectivity: `ryu-copper`'s islands over the board's
//! layout, compared with the design's own netlist (one point per pad), and
//! for each open net the connections an agent draws next.

use super::layout::{self, Built, Key};
use super::shapes::BoardShapes;
use rayon::prelude::*;
use ryu_copper::connect::{Copper, IslandId, Node, Placement};
use ryu_copper::measure::{Body, BodyOptions, Distance};
use ryu_copper::netlist::{self, Open, Report};
use std::collections::{BTreeMap, BTreeSet};

/// What an [`Connectivity::apply`] changed: the layout keys, and their
/// references before and after.
#[derive(Clone, Debug, Default)]
pub struct Changed {
    pub keys: BTreeSet<Key>,
    pub refs: BTreeSet<String>,
}

/// A board's copper, connected and compared.
pub struct Connectivity {
    pub built: Built,
    pub copper: Copper<Key>,
    /// Where each of the layout's points landed, in the points' order.
    pub placed: Vec<Placement<Key>>,
    pub report: Report<Key>,
}

/// An unrouted connection: the nearest pair of pieces between two islands
/// of an open net.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub a: (Key, u32),
    pub b: (Key, u32),
    pub distance: Distance,
}

/// Board `board`'s copper from its shapes.
pub fn connect(board: &str, s: &BoardShapes) -> Connectivity {
    let built = layout::build(board, s);
    let copper = Copper::new(&built.layout, BodyOptions::default());
    let placed = netlist::placements(&copper, &built.layout.points);
    let report = netlist::compare_placed(&copper, &built.layout.points, &placed);
    Connectivity {
        built,
        copper,
        placed,
        report,
    }
}

impl Connectivity {
    /// This connectivity carried to `s` (ryueda 7.E): the copper edited
    /// from the old layout to the new one, the net points placed and
    /// compared again. Equal to `connect(board, s)`. `None`, unchanged,
    /// when the layers differ (a stackup change): build anew.
    pub fn apply(&mut self, board: &str, s: &BoardShapes) -> Option<Changed> {
        let built = layout::build(board, s);
        if built.layout.layers != self.built.layout.layers {
            return None;
        }
        let (old, new) = (&self.built.layout, &built.layout);
        let mut keys: BTreeSet<Key> = BTreeSet::new();
        for (k, it) in &old.items {
            if new.items.get(k) != Some(it) {
                keys.insert(k.clone());
            }
        }
        for (k, it) in &new.items {
            if old.items.get(k) != Some(it) {
                keys.insert(k.clone());
            }
        }
        for (k, h) in &old.holes {
            if new.holes.get(k) != Some(h) {
                keys.insert(k.clone());
            }
        }
        for (k, h) in &new.holes {
            if old.holes.get(k) != Some(h) {
                keys.insert(k.clone());
            }
        }
        // Every item and hole of an island holding a changed key, before
        // and after, with that island's ID and claims: those that differ
        // are changed too (a net joined or split, an island renamed).
        let node = |l: &ryu_copper::layout::Layout<Key>, k: &Key| {
            if l.holes.contains_key(k) {
                Node::Hole(k.clone())
            } else {
                Node::Piece(k.clone(), 0)
            }
        };
        type Seen = BTreeMap<Key, (IslandId<Key>, Vec<String>)>;
        let read = |copper: &Copper<Key>,
                    l: &ryu_copper::layout::Layout<Key>,
                    ks: &mut dyn Iterator<Item = Key>|
         -> Seen {
            let mut seen = Seen::new();
            let mut done: BTreeSet<IslandId<Key>> = BTreeSet::new();
            for k in ks {
                if seen.contains_key(&k) {
                    continue;
                }
                let Some(isl) = copper.island(&node(l, &k)) else {
                    continue;
                };
                if !done.insert(isl.id.clone()) {
                    continue;
                }
                for x in isl.items.iter().chain(&isl.holes) {
                    seen.insert(x.clone(), (isl.id.clone(), isl.claims.clone()));
                }
            }
            seen
        };
        let before = read(
            &self.copper,
            old,
            &mut keys
                .iter()
                .filter(|k| old.items.contains_key(*k) || old.holes.contains_key(*k))
                .cloned(),
        );
        let mut refs: BTreeSet<String> = keys.iter().map(|k| self.built.reference(k)).collect();
        self.copper.apply_layout(old, new);
        let mut again = keys
            .iter()
            .filter(|k| new.items.contains_key(*k) || new.holes.contains_key(*k))
            .cloned()
            .chain(before.keys().cloned())
            .collect::<Vec<_>>()
            .into_iter();
        let after = read(&self.copper, new, &mut again);
        for (k, was) in &before {
            if after.get(k) != Some(was) {
                refs.insert(self.built.reference(k));
                keys.insert(k.clone());
            }
        }
        for (k, now) in &after {
            if before.get(k) != Some(now) {
                keys.insert(k.clone());
            }
        }
        refs.extend(keys.iter().map(|k| built.reference(k)));
        self.placed = netlist::placements(&self.copper, &built.layout.points);
        self.report = netlist::compare_placed(&self.copper, &built.layout.points, &self.placed);
        self.built = built;
        Some(Changed { keys, refs })
    }

    /// Every piece of an island with its body.
    fn island_bodies(&self, id: &IslandId<Key>) -> Vec<((Key, u32), Body)> {
        let Some(island) = self.copper.island(&Node::Piece(id.0.clone(), id.1)) else {
            return Vec::new();
        };
        let mut by_item: BTreeMap<&Key, Vec<(u32, Body)>> = BTreeMap::new();
        let mut out = Vec::new();
        for (k, i) in &island.pieces {
            let bodies = by_item
                .entry(k)
                .or_insert_with(|| self.copper.piece_bodies(k));
            if let Some((_, b)) = bodies.iter().find(|(j, _)| j == i) {
                out.push(((k.clone(), *i), b.clone()));
            }
        }
        out
    }

    /// The layer a piece lies on.
    pub fn layer_of(&self, k: &Key) -> usize {
        self.copper.item(k).map_or(0, |it| it.layer)
    }
}

/// The nearest pair of pieces between two islands, and their distance.
type Nearest = (Distance, (Key, u32), (Key, u32));

/// The connections that would close an open net: a spanning tree over
/// its islands (Prim's, from the least island), each edge the nearest
/// pair of pieces between an island in the tree and one not yet in it,
/// ties to the lesser pieces. Exact distances; deterministic.
pub fn connections(c: &Connectivity, open: &Open<Key>) -> Vec<Connection> {
    let islands: Vec<Vec<((Key, u32), Body)>> = open
        .islands
        .iter()
        .map(|(id, _)| c.island_bodies(id))
        .collect();
    let n = islands.len();
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    // Prim's, growing from island 0: each island outside keeps its best
    // link to the tree, by (distance, the lesser island's piece, the
    // other's) as the pair is oriented lesser island first, and only the
    // island just joined is measured against the rest (ryueda 7.E: the
    // whole table of pairs first was quadratic in islands).
    let mut inside = vec![false; n];
    let mut best: Vec<Option<((usize, usize), Nearest)>> = vec![None; n];
    let mut joined = 0usize;
    inside[0] = true;
    for _ in 1..n {
        let updates: Vec<(usize, Option<((usize, usize), Nearest)>)> = (0..n)
            .into_par_iter()
            .filter(|&j| !inside[j])
            .map(|j| {
                let (lo, hi) = (joined.min(j), joined.max(j));
                (
                    j,
                    ryu_copper::index::nearest_pair(&islands[lo], &islands[hi])
                        .map(|f| ((lo, hi), f)),
                )
            })
            .collect();
        for (j, found) in updates {
            if let Some(f) = found {
                let better = best[j].as_ref().is_none_or(|(at, b)| {
                    (f.1.0.nm, &f.1.1, &f.1.2, f.0) < (b.0.nm, &b.1, &b.2, *at)
                });
                if better {
                    best[j] = Some(f);
                }
            }
        }
        let pick = (0..n)
            .filter(|&j| !inside[j])
            .filter_map(|j| best[j].as_ref().map(|b| (j, b)))
            .min_by(|(_, (x_at, x)), (_, (y_at, y))| {
                (x.0.nm, &x.1, &x.2, x_at).cmp(&(y.0.nm, &y.1, &y.2, y_at))
            });
        let Some((j, ((lo, _), (d, a, b)))) = pick.map(|(j, b)| (j, b.clone())) else {
            break;
        };
        let (a, b) = if inside[lo] { (a, b) } else { (b, a) };
        out.push(Connection { a, b, distance: d });
        inside[j] = true;
        joined = j;
    }
    out
}

/// A connection as answers print it: each end's reference, layer and
/// point, and the distance.
pub fn connection_json(c: &Connectivity, x: &Connection) -> serde_json::Value {
    serde_json::json!({
        "a": c.built.reference(&x.a.0),
        "a_layer": c.built.layer_name(c.layer_of(&x.a.0)),
        "a_at": [x.distance.a.x, x.distance.a.y],
        "b": c.built.reference(&x.b.0),
        "b_layer": c.built.layer_name(c.layer_of(&x.b.0)),
        "b_at": [x.distance.b.x, x.distance.b.y],
        "distance_nm": x.distance.nm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 7.D's connections: every pair of islands measured first.
    fn connections_all_pairs(c: &Connectivity, open: &Open<Key>) -> Vec<Connection> {
        let islands: Vec<Vec<((Key, u32), Body)>> = open
            .islands
            .iter()
            .map(|(id, _)| c.island_bodies(id))
            .collect();
        let n = islands.len();
        let mut near: BTreeMap<(usize, usize), Nearest> = BTreeMap::new();
        for i in 0..n {
            for j in i + 1..n {
                if let Some(found) = ryu_copper::index::nearest_pair(&islands[i], &islands[j]) {
                    near.insert((i, j), found);
                }
            }
        }
        let mut inside = vec![false; n];
        let mut out = Vec::new();
        if n == 0 {
            return out;
        }
        inside[0] = true;
        for _ in 1..n {
            let best = near
                .iter()
                .filter(|((i, j), _)| inside[*i] != inside[*j])
                .min_by(|(_, x), (_, y)| (x.0.nm, &x.1, &x.2).cmp(&(y.0.nm, &y.1, &y.2)));
            let Some(((i, j), (d, a, b))) = best else {
                break;
            };
            let (i, j) = (*i, *j);
            let (a, b) = if inside[i] {
                (a.clone(), b.clone())
            } else {
                (b.clone(), a.clone())
            };
            out.push(Connection { a, b, distance: *d });
            inside[i] = true;
            inside[j] = true;
        }
        out
    }
    use crate::board::file::CopperLayer;
    use crate::board::testing::{MM, UM, board};

    /// Net a on two pads 8 mm apart, net b on one pad between them, above.
    fn two_and_one() -> BoardShapes {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (2 * MM, 5 * MM), (600 * UM, 600 * UM), Some("a"))
            .smd(
                "r2",
                "1",
                (10 * MM, 5 * MM),
                (600 * UM, 600 * UM),
                Some("a"),
            )
            .smd("r3", "1", (6 * MM, 8 * MM), (600 * UM, 600 * UM), Some("b"));
        s
    }

    /// The connections grown one island at a time (ryueda 7.E) are those
    /// the table of every pair gave, on seeded scatters of one net's pads.
    #[test]
    fn connections_are_the_all_pairs_ones() {
        let mut g = 0x7261_7473u64;
        let mut next = || {
            g = g
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (g >> 33) as i64
        };
        for case in 0..12 {
            let mut s = board(40 * MM, 30 * MM);
            for i in 0..(3 + case % 10) {
                let (x, y) = (MM + next() % (38 * MM), MM + next() % (28 * MM));
                s.smd(
                    &format!("r{i}"),
                    "1",
                    (x, y),
                    (600 * UM, 600 * UM),
                    Some("a"),
                );
            }
            let c = connect("b", &s);
            for o in &c.report.opens {
                assert_eq!(
                    connections(&c, o),
                    connections_all_pairs(&c, o),
                    "case {case}"
                );
            }
        }
    }

    #[test]
    fn an_unrouted_net_is_open_with_its_next_connection() {
        let c = connect("b", &two_and_one());
        assert!(c.report.judged);
        assert_eq!(c.report.opens.len(), 1);
        assert_eq!(c.report.opens[0].net, "a");
        assert!(c.report.shorts.is_empty());
        let next = connections(&c, &c.report.opens[0]);
        assert_eq!(next.len(), 1);
        // Pad edges 8 mm - 600 µm apart.
        assert_eq!(next[0].distance.nm, 8 * MM - 600 * UM);
        assert_eq!(c.built.reference(&next[0].a.0), "b:r1/1");
        assert_eq!(c.built.reference(&next[0].b.0), "b:r2/1");
    }

    #[test]
    fn a_track_closes_the_net_and_one_through_another_pad_shorts() {
        let mut s = two_and_one();
        s.track(
            "t1",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 5 * MM), (10 * MM, 5 * MM)],
        );
        let c = connect("b", &s);
        assert!(c.report.opens.is_empty(), "{:?}", c.report.opens);
        assert!(c.report.shorts.is_empty());
        s.track(
            "t2",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(6 * MM, 5 * MM), (6 * MM, 8 * MM)],
        );
        let c = connect("b", &s);
        assert_eq!(c.report.shorts.len(), 1);
        let nets: Vec<&str> = c.report.shorts[0]
            .nets
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(nets, ["a", "b"]);
        assert!(
            c.report.shorts[0].contacts.iter().any(|(x, y)| {
                let r = [c.built.reference(x), c.built.reference(y)];
                r.contains(&"b:track/t2".to_owned()) && r.contains(&"b:r3/1".to_owned())
            }),
            "{:?}",
            c.report.shorts[0].contacts
        );
    }

    #[test]
    fn a_via_joins_the_layers() {
        let mut s = two_and_one();
        s.track(
            "t1",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 5 * MM), (4 * MM, 5 * MM)],
        )
        .via("v1", "a", (4 * MM, 5 * MM), 300 * UM, 600 * UM)
        .track(
            "t2",
            "a",
            CopperLayer::Bottom,
            250 * UM,
            &[(4 * MM, 5 * MM), (8 * MM, 5 * MM)],
        )
        .via("v2", "a", (8 * MM, 5 * MM), 300 * UM, 600 * UM)
        .track(
            "t3",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(8 * MM, 5 * MM), (10 * MM, 5 * MM)],
        );
        let c = connect("b", &s);
        assert!(c.report.opens.is_empty(), "{:?}", c.report.opens);
        assert!(c.report.mismatches.is_empty());
        // The same on one thread and eight.
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| {
                    let c = connect("b", &s);
                    format!("{:?}{:?}", c.report, c.copper.islands())
                })
        };
        assert_eq!(run(1), run(8));
    }

    #[test]
    fn a_track_claiming_the_wrong_net_is_a_mismatch() {
        let mut s = two_and_one();
        s.track(
            "t1",
            "b",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 5 * MM), (10 * MM, 5 * MM)],
        );
        let c = connect("b", &s);
        assert_eq!(c.report.mismatches.len(), 1);
        assert_eq!(c.report.mismatches[0].net, "a");
    }
}
