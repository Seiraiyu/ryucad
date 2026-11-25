//! A board's connectivity: `ryu-copper`'s islands over the board's
//! layout, compared with the design's own netlist (one point per pad), and
//! for each open net the connections an agent draws next.

use super::layout::{self, Built, Key};
use super::shapes::BoardShapes;
use ryu_copper::connect::{Copper, IslandId, Node, Placement};
use ryu_copper::measure::{Body, BodyOptions, Distance};
use ryu_copper::netlist::{self, Open, Report};
use std::collections::BTreeMap;

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
    pub fn apply(&mut self, board: &str, s: &BoardShapes) -> Option<ryu_copper::connect::Applied> {
        let built = layout::build(board, s);
        if built.layout.layers != self.built.layout.layers {
            return None;
        }
        let edits = self.copper.apply_layout(&self.built.layout, &built.layout);
        self.placed = netlist::placements(&self.copper, &built.layout.points);
        self.report = netlist::compare_placed(&self.copper, &built.layout.points, &self.placed);
        self.built = built;
        Some(edits)
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

#[cfg(test)]
mod tests {
    use super::*;
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
