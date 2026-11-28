//! A board's connectivity: `ryu-copper`'s islands over the board's
//! layout, compared with the design's own netlist (one point per pad), and
//! for each open net the connections an agent draws next.

use super::layout::{self, Built, Key};
use super::pour::PourInfo;
use super::shapes::{BoardShapes, Obj, ObjShapes, Pieces};
use rayon::prelude::*;
use ryu_copper::connect::{Copper, IslandId, Landing, Node, Placement};
use ryu_copper::layout::{Hole, Item, NetPoint};
use ryu_copper::measure::{Body, BodyOptions, Distance};
use ryu_copper::netlist::{self, Open, Report};
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
    /// The nodes each of the layout's points landed on, in the points'
    /// order (ryueda 7.E: kept while no edit comes near them).
    pub landed: Vec<Landing<Key>>,
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
    let landed: Vec<Landing<Key>> = built
        .layout
        .points
        .par_iter()
        .map(|p| copper.land(p.at, p.side))
        .collect();
    let placed: Vec<Placement<Key>> = landed.iter().map(|l| copper.placement_of(l)).collect();
    let report = netlist::compare_placed(&copper, &built.layout.points, &placed);
    Connectivity {
        built,
        copper,
        landed,
        placed,
        report,
    }
}

/// What an update changed, by object (ryueda 7.E): the pieces before and
/// after, and the pours before. The layout is edited for the objects
/// whose pieces changed and the pours that differ, never built whole.
pub struct Delta<'a> {
    pub was: &'a Pieces,
    pub now: &'a Pieces,
    pub old_pours: &'a [PourInfo],
}

/// The items and holes of one side of a delta's changed objects.
#[derive(Default)]
struct Part {
    items: BTreeMap<Key, Item>,
    holes: BTreeMap<Key, Hole>,
}

impl Part {
    fn of_piece(&mut self, n: usize, p: &ObjShapes) {
        layout::put_items(n, &p.pads, &p.tracks, &p.vias, &mut self.items);
        layout::put_holes(n, &p.holes, &mut self.holes);
    }
}

impl Connectivity {
    /// This connectivity carried to `s` (ryueda 7.E): the copper edited
    /// from the old layout to the new one, the net points placed and
    /// compared again. Equal to `connect(board, s)`. `None`, unchanged,
    /// when the layers differ (a stackup change): build anew.
    pub fn apply(&mut self, board: &str, s: &BoardShapes) -> Option<Changed> {
        self.apply_delta(board, s, None)
    }

    /// [`Self::apply`], told by `delta` which objects changed: only their
    /// layout is built and compared, and only the net points near copper
    /// that changed are placed again (the rest keep their landings, read
    /// as islands anew).
    pub fn apply_delta(
        &mut self,
        board: &str,
        s: &BoardShapes,
        delta: Option<&Delta>,
    ) -> Option<Changed> {
        let n = s.n();
        let stage = crate::profile::stage("apply.build");
        let (layers, names) = layout::layers_of(n);
        if layers != self.built.layout.layers {
            return None;
        }
        // Each changed key's item or hole before and after (`None`: none).
        type Diff<T> = BTreeMap<Key, (Option<T>, Option<T>)>;
        let mut items: Diff<Item> = BTreeMap::new();
        let mut holes: Diff<Hole> = BTreeMap::new();
        let mut pads_gone: Vec<(String, String)> = Vec::new();
        let mut pads_new: Vec<((String, String), String)> = Vec::new();
        let rebuilt = match delta {
            Some(d) => {
                let (mut before, mut after) = (Part::default(), Part::default());
                let mut objs: BTreeSet<&Obj> = BTreeSet::new();
                for (k, (h, _)) in &d.now.by {
                    if d.was.by.get(k).is_none_or(|(g, _)| g != h) {
                        objs.insert(k);
                    }
                }
                for k in d.was.by.keys() {
                    if !d.now.by.contains_key(k) {
                        objs.insert(k);
                    }
                }
                for o in objs {
                    if let Some((_, p)) = d.was.by.get(o) {
                        before.of_piece(n, p);
                        pads_gone.extend(
                            p.pads
                                .iter()
                                .map(|pd| (pd.occurrence.clone(), pd.pad.clone())),
                        );
                    }
                    if let Some((_, p)) = d.now.by.get(o) {
                        after.of_piece(n, p);
                        pads_new.extend(p.pads.iter().map(|pd| {
                            (
                                (pd.occurrence.clone(), pd.pad.clone()),
                                pd.reference.clone(),
                            )
                        }));
                    }
                }
                let by_id = |ps: &[PourInfo]| -> BTreeMap<String, usize> {
                    ps.iter()
                        .enumerate()
                        .map(|(i, f)| (f.id.to_string(), i))
                        .collect()
                };
                let (was, now) = (by_id(d.old_pours), by_id(&s.pours));
                let ids: BTreeSet<&String> = was.keys().chain(now.keys()).collect();
                for id in ids {
                    let a = was.get(id).map(|&i| &d.old_pours[i]);
                    let b = now.get(id).map(|&i| &s.pours[i]);
                    if a == b {
                        continue;
                    }
                    if let Some(f) = a {
                        layout::put_pour(f, &mut before.items);
                    }
                    if let Some(f) = b {
                        layout::put_pour(f, &mut after.items);
                    }
                }
                for (k, it) in before.items {
                    items.insert(k, (Some(it), None));
                }
                for (k, it) in after.items {
                    items.entry(k).or_insert((None, None)).1 = Some(it);
                }
                for (k, h) in before.holes {
                    holes.insert(k, (Some(h), None));
                }
                for (k, h) in after.holes {
                    holes.entry(k).or_insert((None, None)).1 = Some(h);
                }
                items.retain(|_, (a, b)| a != b);
                holes.retain(|_, (a, b)| a != b);
                None
            }
            None => {
                let built = layout::build(board, s);
                let (old, new) = (&self.built.layout, &built.layout);
                for (k, it) in &old.items {
                    if new.items.get(k) != Some(it) {
                        items.insert(k.clone(), (Some(it.clone()), new.items.get(k).cloned()));
                    }
                }
                for (k, it) in &new.items {
                    if !old.items.contains_key(k) {
                        items.insert(k.clone(), (None, Some(it.clone())));
                    }
                }
                for (k, h) in &old.holes {
                    if new.holes.get(k) != Some(h) {
                        holes.insert(k.clone(), (Some(h.clone()), new.holes.get(k).cloned()));
                    }
                }
                for (k, h) in &new.holes {
                    if !old.holes.contains_key(k) {
                        holes.insert(k.clone(), (None, Some(h.clone())));
                    }
                }
                Some(built)
            }
        };
        let mut keys: BTreeSet<Key> = items.keys().chain(holes.keys()).cloned().collect();
        drop(stage);
        // Every item and hole of an island holding a changed key, before
        // and after, with that island's ID and claims: those that differ
        // are changed too (a net joined or split, an island renamed).
        let stage = crate::profile::stage("apply.diff");
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
        let before = {
            let old = &self.built.layout;
            read(
                &self.copper,
                old,
                &mut keys
                    .iter()
                    .filter(|k| old.items.contains_key(*k) || old.holes.contains_key(*k))
                    .cloned(),
            )
        };
        let mut refs: BTreeSet<String> = keys.iter().map(|k| self.built.reference(k)).collect();
        drop(stage);
        // The layout carried over: rebuilt whole, or edited by key.
        let stage = crate::profile::stage("apply.layout");
        let old_points = match rebuilt {
            Some(built) => std::mem::replace(&mut self.built, built).layout.points,
            None => {
                let l = &mut self.built.layout;
                for (k, (_, b)) in &items {
                    match b {
                        Some(it) => l.items.insert(k.clone(), it.clone()),
                        None => l.items.remove(k),
                    };
                }
                for (k, (_, b)) in &holes {
                    match b {
                        Some(h) => l.holes.insert(k.clone(), h.clone()),
                        None => l.holes.remove(k),
                    };
                }
                for k in &pads_gone {
                    self.built.pads.remove(k);
                }
                self.built.pads.extend(pads_new);
                self.built.board = board.to_owned();
                self.built.layers = names;
                std::mem::replace(
                    &mut self.built.layout.points,
                    layout::points_of(n, &s.pads, &s.holes),
                )
            }
        };
        drop(stage);
        // The copper edited as `Copper::apply_layout` does: items and
        // holes gone or changed out, then holes and items in.
        let stage = crate::profile::stage("apply.copper");
        self.copper.mark_changes();
        for (k, (a, _)) in &items {
            if a.is_some() {
                self.copper.remove_item(k);
            }
        }
        for (k, (a, _)) in &holes {
            if a.is_some() {
                self.copper.remove_hole(k);
            }
        }
        for (k, (_, b)) in &holes {
            if let Some(h) = b {
                self.copper.insert_hole(k.clone(), h.clone());
            }
        }
        for (k, (_, b)) in &items {
            if let Some(it) = b {
                self.copper.insert_item(k.clone(), it.clone());
            }
        }
        let marks = self.copper.take_marks();
        drop(stage);
        let stage = crate::profile::stage("apply.read");
        let new = &self.built.layout;
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
        refs.extend(keys.iter().map(|k| self.built.reference(k)));
        drop(stage);
        // A point lands again only when it moved or an edit marked a box
        // holding it; every landing is read as islands anew.
        let stage = crate::profile::stage("apply.places");
        let old_at: HashMap<&NetPoint<Key>, usize> =
            old_points.iter().enumerate().map(|(i, p)| (p, i)).collect();
        let copper = &self.copper;
        let landed: Vec<Landing<Key>> = new
            .points
            .par_iter()
            .map(|p| {
                let kept = old_at
                    .get(p)
                    .filter(|_| !marks.iter().any(|r| r.contains(p.at)))
                    .map(|&i| self.landed[i].clone());
                kept.unwrap_or_else(|| copper.land(p.at, p.side))
            })
            .collect();
        self.placed = landed.iter().map(|l| copper.placement_of(l)).collect();
        self.landed = landed;
        drop(stage);
        let _stage = crate::profile::stage("apply.compare");
        self.report = netlist::compare_placed(&self.copper, &new.points, &self.placed);
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

/// A link between two islands, by their indices (the lesser first).
type Link = ((usize, usize), Nearest);

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
    let mut best: Vec<Option<Link>> = vec![None; n];
    let mut joined = 0usize;
    inside[0] = true;
    for _ in 1..n {
        let updates: Vec<(usize, Option<Link>)> = (0..n)
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
