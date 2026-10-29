//! Placement by the flow of the schematic: signals run left to right,
//! power sits above. Clusters become a graph (edges from pin types and
//! port directions; rails are never edges, so they never pull everything
//! into one column), longest-path layering gives columns, barycentre
//! sweeps order them, and columns are packed on the grid, each cluster
//! nudged so its wires run straight where it can.

use crate::cluster::{Cluster, is_connector};
use crate::geom::{GRID, Nm, Point, Side, snap};
use crate::input::{Block, Flow, NetKind, PartKind, PinKind};
use std::collections::{BTreeMap, BTreeSet};

/// The margin kept clear inside a sheet's area.
pub const MARGIN: Nm = 4 * GRID;
/// Space between clusters stacked in a column.
pub const ROW_GAP: Nm = 5 * GRID;
/// Space between clusters side by side in a band.
pub const BAND_GAP: Nm = 6 * GRID;

/// What instructions tell placement about clusters.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// A cluster held to an edge of its sheet.
    pub edge: BTreeMap<usize, Side>,
}

/// Where each cluster goes on one sheet: its origin, or `None` for those
/// the sheet has no room for.
pub struct Placement {
    pub origins: BTreeMap<usize, Point>,
    pub overflow: Vec<usize>,
}

/// How a pin takes part in the flow: as a driver, a sink, or neither.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Drives,
    Takes,
    Either,
}

fn role(block: &Block, part: usize, kind: PinKind) -> Role {
    if let PartKind::Port { flow } = block.parts[part].kind {
        return match flow {
            Flow::Out => Role::Takes,
            _ => Role::Drives,
        };
    }
    if kind.drives() || kind == PinKind::PowerOut {
        Role::Drives
    } else if kind.takes() || kind == PinKind::PowerIn {
        Role::Takes
    } else {
        Role::Either
    }
}

/// The graph of clusters: directed edges driver to sink, undirected edges
/// between clusters sharing a signal net any other way, each with weight.
struct Graph {
    directed: BTreeMap<(usize, usize), u32>,
    undirected: BTreeMap<(usize, usize), u32>,
}

fn graph(block: &Block, cluster_of: &[Option<usize>], which: &BTreeSet<usize>) -> Graph {
    let mut directed = BTreeMap::new();
    let mut undirected = BTreeMap::new();
    let mut on_net: BTreeMap<usize, Vec<(usize, Role)>> = BTreeMap::new();
    for (pi, part) in block.parts.iter().enumerate() {
        let Some(c) = cluster_of[pi].filter(|c| which.contains(c)) else {
            continue;
        };
        for pin in &part.pins {
            if let Some(n) = pin.net
                && block.nets[n].kind == NetKind::Signal
            {
                on_net
                    .entry(n)
                    .or_default()
                    .push((c, role(block, pi, pin.kind)));
            }
        }
    }
    for members in on_net.values() {
        for &(a, ra) in members {
            for &(b, rb) in members {
                if a == b {
                    continue;
                }
                if ra == Role::Drives && rb == Role::Takes {
                    *directed.entry((a, b)).or_default() += 1;
                } else if a < b && !(ra == Role::Takes && rb == Role::Drives) {
                    *undirected.entry((a, b)).or_default() += 1;
                }
            }
        }
    }
    Graph {
        directed,
        undirected,
    }
}

impl Graph {
    fn neighbours(&self, c: usize) -> Vec<(usize, u32)> {
        let mut out: BTreeMap<usize, u32> = BTreeMap::new();
        for (&(a, b), &w) in self.directed.iter().chain(self.undirected.iter()) {
            if a == c {
                *out.entry(b).or_default() += w;
            } else if b == c {
                *out.entry(a).or_default() += w;
            }
        }
        out.into_iter().collect()
    }
}

/// Longest-path layers of the clusters in `nodes`: directed edges first
/// (back edges of a depth-first walk in order dropped), then clusters
/// reached only by undirected edges one layer past their first layered
/// neighbour; `left` clusters take the first layer and `right` the last.
fn layers(
    g: &Graph,
    nodes: &[usize],
    left: &BTreeSet<usize>,
    right: &BTreeSet<usize>,
) -> BTreeMap<usize, usize> {
    let set: BTreeSet<usize> = nodes.iter().copied().collect();
    let mut succ: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(a, b) in g.directed.keys() {
        if set.contains(&a) && set.contains(&b) && !right.contains(&a) && !left.contains(&b) {
            succ.entry(a).or_default().push(b);
        }
    }
    // Drop back edges.
    let mut state: BTreeMap<usize, u8> = BTreeMap::new();
    let mut kept: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &start in nodes {
        if state.contains_key(&start) {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state.insert(start, 1);
        while let Some((u, i)) = stack.pop() {
            let next = succ.get(&u).and_then(|s| s.get(i)).copied();
            match next {
                Some(v) => {
                    stack.push((u, i + 1));
                    match state.get(&v) {
                        None => {
                            kept.entry(u).or_default().push(v);
                            state.insert(v, 1);
                            stack.push((v, 0));
                        }
                        Some(2) => kept.entry(u).or_default().push(v),
                        _ => {}
                    }
                }
                None => {
                    state.insert(u, 2);
                }
            }
        }
    }
    let has_directed: BTreeSet<usize> = kept
        .iter()
        .flat_map(|(a, bs)| std::iter::once(*a).chain(bs.iter().copied()))
        .collect();
    let mut layer: BTreeMap<usize, usize> = BTreeMap::new();
    // Longest path, by repeated relaxation over the acyclic edges.
    for &n in nodes {
        if has_directed.contains(&n) || left.contains(&n) {
            layer.insert(n, 0);
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (a, bs) in &kept {
            let la = layer[a];
            for b in bs {
                if layer[b] < la + 1 {
                    layer.insert(*b, la + 1);
                    changed = true;
                }
            }
        }
    }
    // Undirected reach, one past the first layered neighbour.
    let mut changed = true;
    while changed {
        changed = false;
        for &n in nodes {
            if layer.contains_key(&n) {
                continue;
            }
            let found = g
                .neighbours(n)
                .into_iter()
                .filter(|(m, _)| set.contains(m))
                .filter_map(|(m, w)| layer.get(&m).map(|l| (w, *l, m)))
                .max_by(|a, b| a.0.cmp(&b.0).then(b.2.cmp(&a.2)));
            if let Some((_, l, _)) = found {
                layer.insert(n, l + 1);
                changed = true;
            }
        }
    }
    for &n in nodes {
        layer.entry(n).or_insert(0);
    }
    // Left clusters alone in the first layer; right ones alone in the last.
    if !left.is_empty() {
        for (n, l) in layer.iter_mut() {
            if left.contains(n) {
                *l = 0;
            } else {
                *l += 1;
            }
        }
    }
    let last = layer
        .iter()
        .filter(|(n, _)| !right.contains(n))
        .map(|(_, l)| *l)
        .max()
        .unwrap_or(0);
    for (n, l) in layer.iter_mut() {
        if right.contains(n) {
            *l = last + 1;
        }
    }
    // Close gaps.
    let used: BTreeSet<usize> = layer.values().copied().collect();
    let rank: BTreeMap<usize, usize> = used.iter().enumerate().map(|(i, l)| (*l, i)).collect();
    layer.into_iter().map(|(n, l)| (n, rank[&l])).collect()
}

/// Orders each column by barycentre sweeps, ties by the cluster's first
/// part's key.
fn order(block: &Block, clusters: &[Cluster], g: &Graph, columns: &mut [Vec<usize>]) {
    let key = |c: usize| block.parts[clusters[c].first()].key.clone();
    for col in columns.iter_mut() {
        col.sort_by_key(|c| key(*c));
    }
    for _ in 0..4 {
        for dir in [true, false] {
            let n = columns.len();
            let range: Vec<usize> = if dir {
                (1..n).collect()
            } else {
                (0..n.saturating_sub(1)).rev().collect()
            };
            for i in range {
                let other = if dir { i - 1 } else { i + 1 };
                let pos: BTreeMap<usize, usize> = columns[other]
                    .iter()
                    .enumerate()
                    .map(|(j, c)| (*c, j))
                    .collect();
                let mine: BTreeMap<usize, usize> = columns[i]
                    .iter()
                    .enumerate()
                    .map(|(j, c)| (*c, j))
                    .collect();
                let mut scored: Vec<(i64, String, usize)> = columns[i]
                    .iter()
                    .map(|&c| {
                        let mut sum = 0i64;
                        let mut w = 0i64;
                        for (m, weight) in g.neighbours(c) {
                            if let Some(p) = pos.get(&m) {
                                sum += *p as i64 * 1000 * i64::from(weight);
                                w += i64::from(weight);
                            }
                        }
                        let bary = if w == 0 {
                            mine[&c] as i64 * 1000
                        } else {
                            sum / w
                        };
                        (bary, key(c), c)
                    })
                    .collect();
                scored.sort();
                columns[i] = scored.into_iter().map(|(_, _, c)| c).collect();
            }
        }
    }
}

/// The room a cluster takes with its origin at `[0, 0]`.
fn extent0(block: &Block, c: &Cluster) -> crate::geom::Rect {
    c.room(block, [0, 0])
}

/// The origin putting a cluster's extent's top left corner at `corner`,
/// on the grid at or right of and below it.
fn origin_for(block: &Block, c: &Cluster, corner: Point) -> Point {
    let e = extent0(block, c);
    let x = corner[0] - e.min[0];
    let y = corner[1] - e.max[1];
    // Up to the grid rightwards and downwards, so the corner stays inside.
    [
        x.div_euclid(GRID) * GRID + if x.rem_euclid(GRID) == 0 { 0 } else { GRID },
        y.div_euclid(GRID) * GRID,
    ]
}

/// Places the clusters `which` on a sheet of `size` by flow.
pub fn flow(
    block: &Block,
    clusters: &[Cluster],
    which: &[usize],
    size: [Nm; 2],
    hints: &Hints,
) -> Placement {
    let mut cluster_of = vec![None; block.parts.len()];
    for (ci, c) in clusters.iter().enumerate() {
        for (p, _) in &c.members {
            cluster_of[*p] = Some(ci);
        }
    }
    let set: BTreeSet<usize> = which.iter().copied().collect();
    let g = graph(block, &cluster_of, &set);
    // What sits in the bands, and what is held to the side edges.
    let mut top = Vec::new();
    let mut bottom = Vec::new();
    let mut main = Vec::new();
    let mut left = BTreeSet::new();
    let mut right = BTreeSet::new();
    let connected: BTreeSet<usize> = g
        .directed
        .keys()
        .chain(g.undirected.keys())
        .flat_map(|(a, b)| [*a, *b])
        .collect();
    for &c in which {
        let first = clusters[c].first();
        match hints.edge.get(&c) {
            Some(Side::Up) => top.push(c),
            Some(Side::Down) => bottom.push(c),
            Some(Side::Left) => {
                left.insert(c);
                main.push(c);
            }
            Some(Side::Right) => {
                right.insert(c);
                main.push(c);
            }
            None => {
                let part = &block.parts[first];
                let port = match part.kind {
                    PartKind::Port { flow } => Some(flow),
                    _ => None,
                };
                if port == Some(Flow::Out) {
                    right.insert(c);
                    main.push(c);
                } else if port.is_some() || is_connector(block, first) {
                    left.insert(c);
                    main.push(c);
                } else if !connected.contains(&c) {
                    // Joined to the rest only by rails: power sits above.
                    top.push(c);
                } else {
                    main.push(c);
                }
            }
        }
    }
    let layer = layers(&g, &main, &left, &right);
    let n = layer.values().max().map_or(0, |m| m + 1);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (c, l) in &layer {
        columns[*l].push(*c);
    }
    order(block, clusters, &g, &mut columns);
    let key = |c: &usize| block.parts[clusters[*c].first()].key.clone();
    top.sort_by_key(key);
    bottom.sort_by_key(key);
    let mut origins: BTreeMap<usize, Point> = BTreeMap::new();
    let mut overflow = Vec::new();
    // The top band.
    let mut x = MARGIN;
    let mut band_bottom = size[1] - MARGIN;
    let band_top = size[1] - MARGIN;
    for c in &top {
        let e = extent0(block, &clusters[*c]);
        if x + e.width() > size[0] - MARGIN {
            overflow.push(*c);
            continue;
        }
        let o = origin_for(block, &clusters[*c], [x, band_top]);
        let placed = clusters[*c].room(block, o);
        band_bottom = band_bottom.min(placed.min[1]);
        x = placed.max[0] + BAND_GAP;
        origins.insert(*c, o);
    }
    let main_top = if top.len() > overflow.len() {
        band_bottom - BAND_GAP
    } else {
        size[1] - MARGIN
    };
    // Columns, wrapped where they run past the sheet's foot.
    let net_cols = |cols: &[Vec<usize>]| -> Vec<BTreeSet<usize>> {
        cols.iter()
            .map(|col| {
                col.iter()
                    .flat_map(|c| clusters[*c].members.iter())
                    .flat_map(|(p, _)| block.parts[*p].pins.iter())
                    .filter_map(|pin| pin.net)
                    .filter(|n| block.nets[*n].kind == NetKind::Signal)
                    .collect()
            })
            .collect()
    };
    let floor = MARGIN + if bottom.is_empty() { 0 } else { 8 * GRID };
    let mut cols: Vec<Vec<usize>> = Vec::new();
    for col in columns {
        let mut cur = Vec::new();
        let mut y = main_top;
        for c in col {
            let h = extent0(block, &clusters[c]).height();
            if !cur.is_empty() && y - h < floor {
                cols.push(std::mem::take(&mut cur));
                y = main_top;
            }
            cur.push(c);
            y -= h + ROW_GAP;
        }
        if !cur.is_empty() {
            cols.push(cur);
        }
    }
    let nets = net_cols(&cols);
    let mut x = MARGIN;
    let mut pin_y: BTreeMap<usize, Vec<(usize, Nm)>> = BTreeMap::new();
    for (i, col) in cols.iter().enumerate() {
        let width = col
            .iter()
            .map(|c| extent0(block, &clusters[*c]).width())
            .max()
            .unwrap_or(0);
        if x + width > size[0] - MARGIN && i > 0 {
            overflow.extend(col.iter().copied());
            continue;
        }
        let mut limit = main_top;
        for c in col {
            let cl = &clusters[*c];
            let mut o = origin_for(block, cl, [x, limit]);
            // Straight wires: line a pin up with one it meets to the left.
            let mut best: Option<Nm> = None;
            'search: for (p, place) in cl.places(o) {
                let shape = crate::shape::Shape::of(&block.parts[p], place);
                for (k, pin) in block.parts[p].pins.iter().enumerate() {
                    let Some(n) = pin.net else { continue };
                    if let Some(list) = pin_y.get(&n)
                        && let Some(&(_, y)) = list.first()
                    {
                        let dy = y - shape.pins[k].end[1];
                        if dy <= 0 {
                            best = Some(dy);
                            break 'search;
                        }
                    }
                }
            }
            if let Some(dy) = best {
                let moved = [o[0], o[1] + dy];
                if cl.room(block, moved).min[1] >= floor {
                    o = moved;
                }
            }
            let e = cl.room(block, o);
            limit = e.min[1] - ROW_GAP;
            origins.insert(*c, o);
            for (p, place) in cl.places(o) {
                let shape = crate::shape::Shape::of(&block.parts[p], place);
                for (k, pin) in block.parts[p].pins.iter().enumerate() {
                    if let Some(n) = pin.net {
                        pin_y.entry(n).or_default().push((i, shape.pins[k].end[1]));
                    }
                }
            }
        }
        // The channel past this column: room for the nets crossing it.
        let crossing = if i + 1 < cols.len() {
            let before: BTreeSet<usize> = nets[..=i].iter().flatten().copied().collect();
            let after: BTreeSet<usize> = nets[i + 1..].iter().flatten().copied().collect();
            before.intersection(&after).count() as Nm
        } else {
            0
        };
        x += width + (crossing + 6).clamp(6, 16) * GRID;
    }
    // The bottom band.
    let mut x = MARGIN;
    for c in &bottom {
        let e = extent0(block, &clusters[*c]);
        let o = origin_for(block, &clusters[*c], [x, MARGIN + e.height()]);
        if x + e.width() > size[0] - MARGIN {
            overflow.push(*c);
            continue;
        }
        x += e.width() + BAND_GAP;
        origins.insert(*c, o);
    }
    for o in origins.values_mut() {
        *o = [snap(o[0]), snap(o[1])];
    }
    Placement { origins, overflow }
}
