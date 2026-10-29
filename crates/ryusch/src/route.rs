//! Routing on the grid: a sheet's occupancy (bodies, pins, labels and
//! text no wire may cross, and each net's wires) and A* over it with
//! costs for length, bends, crossings and passing near symbols. A wire
//! meets its net's tree only where three wires would meet (never four),
//! leaves and enters pins along them, and crosses another net's wire only
//! straight through, never at a bend or a junction. Ties are broken by
//! the order moves are tried, never by a hash.

use crate::geom::{GRID, Nm, Point, Rect, Side};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// What a pin end of no net marks: nothing may connect there.
pub const NO_NET: usize = usize::MAX;

const STEP: u32 = 2;
const BEND: u32 = 7;
const CROSS: u32 = 14;
const NEAR: u32 = 1;
/// The most nodes one connection's search may expand.
const BUDGET: usize = 400_000;

/// Arms: which ways a node's wires leave it.
fn arm(s: Side) -> u8 {
    match s {
        Side::Left => 1,
        Side::Right => 2,
        Side::Up => 4,
        Side::Down => 8,
    }
}

const H: u8 = 1 | 2;
const V: u8 = 4 | 8;

#[derive(Clone, Debug, Default)]
struct Cell {
    /// No wire may pass: inside a body, on a pin's line, under text.
    blocked: bool,
    /// Next to a body: wires may pass, at a cost.
    near: bool,
    /// The net of a pin end here.
    pin: Option<usize>,
    /// The net whose pin leaves through here: a step out from its end,
    /// kept for it.
    exit: Option<usize>,
    /// The net owning this node's horizontal arms, and its vertical ones.
    h: Option<usize>,
    v: Option<usize>,
    arms: u8,
}

/// One sheet's occupancy.
#[derive(Clone)]
pub struct Field {
    cols: i64,
    rows: i64,
    cells: Vec<Cell>,
}

/// A grid node.
pub type Node = (i64, i64);

pub fn node_of(p: Point) -> Node {
    (p[0].div_euclid(GRID), p[1].div_euclid(GRID))
}

pub fn point_of(n: Node) -> Point {
    [n.0 * GRID, n.1 * GRID]
}

fn step(n: Node, s: Side) -> Node {
    match s {
        Side::Left => (n.0 - 1, n.1),
        Side::Right => (n.0 + 1, n.1),
        Side::Up => (n.0, n.1 + 1),
        Side::Down => (n.0, n.1 - 1),
    }
}

/// The side a step from `a` to its neighbour `b` goes.
fn way(a: Node, b: Node) -> Side {
    match (b.0 - a.0, b.1 - a.1) {
        (-1, 0) => Side::Left,
        (1, 0) => Side::Right,
        (0, 1) => Side::Up,
        _ => Side::Down,
    }
}

/// Why a wire can't be marked: what it runs into.
#[derive(Debug, PartialEq, Eq)]
pub struct Clash(pub Node);

impl Field {
    /// An empty sheet of `size`.
    pub fn new(size: [Nm; 2]) -> Field {
        let cols = size[0] / GRID + 1;
        let rows = size[1] / GRID + 1;
        Field {
            cols,
            rows,
            cells: vec![Cell::default(); (cols * rows) as usize],
        }
    }

    fn at(&self, n: Node) -> Option<&Cell> {
        (n.0 >= 0 && n.1 >= 0 && n.0 < self.cols && n.1 < self.rows)
            .then(|| &self.cells[(n.1 * self.cols + n.0) as usize])
    }

    fn at_mut(&mut self, n: Node) -> Option<&mut Cell> {
        (n.0 >= 0 && n.1 >= 0 && n.0 < self.cols && n.1 < self.rows)
            .then(|| &mut self.cells[(n.1 * self.cols + n.0) as usize])
    }

    /// Blocks every node inside or on `r`, marking the ring around it
    /// near; `pins` ends stay open.
    pub fn block_body(&mut self, r: Rect) {
        let (a, b) = (node_lo(r.min), node_hi(r.max));
        for x in a.0 - 1..=b.0 + 1 {
            for y in a.1 - 1..=b.1 + 1 {
                if let Some(c) = self.at_mut((x, y)) {
                    if x < a.0 || x > b.0 || y < a.1 || y > b.1 {
                        c.near = true;
                    } else {
                        c.blocked = true;
                    }
                }
            }
        }
    }

    /// Blocks every node within half a step of `r`: text and labels.
    pub fn block(&mut self, r: Rect) {
        let (a, b) = near(r);
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                if let Some(c) = self.at_mut((x, y)) {
                    c.blocked = true;
                }
            }
        }
    }

    /// Whether a box touches no wire, pin or blocked node: where text and
    /// labels may go.
    pub fn clear(&self, r: Rect) -> bool {
        let (a, b) = near(r);
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                match self.at((x, y)) {
                    None => return false,
                    Some(c) if c.blocked || c.pin.is_some() || c.arms != 0 => return false,
                    _ => {}
                }
            }
        }
        true
    }

    /// A pin: its line from `base` to `end` blocked, its end a terminal of
    /// `net` (`NO_NET` for none) with an arm back along the pin.
    pub fn pin(&mut self, base: Point, end: Point, side: Side, net: usize) {
        let (eb, ee) = (node_of(base), node_of(end));
        let mut n = eb;
        while n != ee {
            if let Some(c) = self.at_mut(n) {
                c.blocked = true;
            }
            n = step(n, side);
        }
        if net != NO_NET
            && let Some(c) = self.at_mut(step(ee, side))
            && c.exit.is_none()
        {
            c.exit = Some(net);
        }
        if let Some(c) = self.at_mut(ee) {
            c.pin = Some(net);
            c.blocked = false;
            let back = side.opposite();
            c.arms |= arm(back);
            if back.horizontal() {
                c.h = Some(net);
            } else {
                c.v = Some(net);
            }
        }
    }

    /// How many other nets' wires a path of `net` crosses.
    pub fn crossings(&self, net: usize, path: &[Node]) -> usize {
        path.iter()
            .filter(|n| {
                self.at(**n).is_some_and(|c| {
                    c.arms != 0 && (c.h.is_some_and(|o| o != net) || c.v.is_some_and(|o| o != net))
                })
            })
            .count()
    }

    /// Whether the pin of `net` ending at `end` already has a wire: a
    /// search from another pin reached it.
    pub fn wired(&self, end: Point, net: usize) -> bool {
        self.at(node_of(end))
            .is_some_and(|c| (c.arms & owned(c, net)).count_ones() >= 2)
    }

    /// Whether `n` belongs to `net`'s tree: a pin end or a wire of it.
    fn on_tree(&self, n: Node, net: usize) -> bool {
        self.at(n).is_some_and(|c| {
            c.pin == Some(net)
                || (c.arms & H != 0 && c.h == Some(net))
                || (c.arms & V != 0 && c.v == Some(net))
        })
    }

    /// Marks a run of `net` through its points, refusing what clashes.
    pub fn mark(&mut self, net: usize, run: &[Point]) -> Result<(), Clash> {
        let nodes = expand(run);
        // Check first, so a clash marks nothing.
        for (i, &n) in nodes.iter().enumerate() {
            let c = self.at(n).ok_or(Clash(n))?;
            if c.blocked {
                return Err(Clash(n));
            }
            if c.pin.is_some_and(|p| p != net) || c.exit.is_some_and(|e| e != net) {
                return Err(Clash(n));
            }
            let mut want = 0u8;
            if i > 0 {
                want |= arm(way(n, nodes[i - 1]));
            }
            if i + 1 < nodes.len() {
                want |= arm(way(n, nodes[i + 1]));
            }
            if want & H != 0 && c.h.is_some_and(|o| o != net) {
                return Err(Clash(n));
            }
            if want & V != 0 && c.v.is_some_and(|o| o != net) {
                return Err(Clash(n));
            }
            if c.arms & want & owned(c, net) != 0 {
                return Err(Clash(n));
            }
            // Crossing another net: straight through, at no joint of it.
            let other = c.arms & !owned(c, net);
            if other != 0 && (want == H || want == V) {
                if !(other == H || other == V) || c.pin.is_some() {
                    return Err(Clash(n));
                }
            } else if other != 0 {
                return Err(Clash(n));
            }
            let mine = (c.arms & owned(c, net)) | want;
            if mine.count_ones() > 3 {
                return Err(Clash(n));
            }
        }
        for (i, &n) in nodes.iter().enumerate() {
            let mut want = 0u8;
            if i > 0 {
                want |= arm(way(n, nodes[i - 1]));
            }
            if i + 1 < nodes.len() {
                want |= arm(way(n, nodes[i + 1]));
            }
            let c = self.at_mut(n).expect("checked");
            c.arms |= want;
            if want & H != 0 {
                c.h = Some(net);
            }
            if want & V != 0 {
                c.v = Some(net);
            }
        }
        Ok(())
    }

    /// The cheapest path from pin end `from` (leaving it along `side`) to
    /// `net`'s tree, as nodes; `None` when there is none in the budget.
    pub fn search(&self, net: usize, from: Point, side: Side) -> Option<Vec<Node>> {
        let start = node_of(from);
        let targets: Vec<Node> = (0..self.cols)
            .flat_map(|x| (0..self.rows).map(move |y| (x, y)))
            .filter(|&n| n != start && self.on_tree(n, net))
            .collect();
        if targets.is_empty() {
            return None;
        }
        let bound = targets
            .iter()
            .fold((i64::MAX, i64::MAX, i64::MIN, i64::MIN), |b, n| {
                (b.0.min(n.0), b.1.min(n.1), b.2.max(n.0), b.3.max(n.1))
            });
        let h = |n: Node| -> u32 {
            let dx = (bound.0 - n.0).max(n.0 - bound.2).max(0);
            let dy = (bound.1 - n.1).max(n.1 - bound.3).max(0);
            ((dx + dy) as u32) * STEP
        };
        let idx = |n: Node, s: Side| ((n.1 * self.cols + n.0) * 4) as usize + s as usize;
        let mut best = vec![u32::MAX; (self.cols * self.rows * 4) as usize];
        let mut from_of: Vec<u32> = vec![u32::MAX; best.len()];
        let mut heap = BinaryHeap::new();
        let mut order = 0u64;
        // The first step leaves the pin along it.
        let first = step(start, side);
        let mut expanded = 0usize;
        let mut push = |heap: &mut BinaryHeap<_>,
                        cost: u32,
                        n: Node,
                        s: Side,
                        prev: u32,
                        best: &mut Vec<u32>,
                        from_of: &mut Vec<u32>| {
            let i = idx(n, s);
            if cost < best[i] {
                best[i] = cost;
                from_of[i] = prev;
                order += 1;
                heap.push(Reverse((cost + h(n), order, cost, n.0, n.1, s as u8)));
            }
        };
        let sides = [Side::Left, Side::Right, Side::Up, Side::Down];
        let enter = |n: Node, s: Side| -> Option<(bool, u32)> {
            // Whether a step arriving at `n` moving `s` may stop there (it
            // reaches the tree), or pass on at what cost.
            let c = self.at(n)?;
            let back = arm(s.opposite());
            if n == start {
                return None;
            }
            if self.on_tree(n, net) {
                if c.pin == Some(net) && c.arms & owned(c, net) & !back != 0 {
                    // A pin end: only from outward, along the pin.
                    let pin_back = c.arms & owned(c, net);
                    if pin_back.count_ones() == 1 && pin_back != arm(s) {
                        return None;
                    }
                }
                let mine = c.arms & owned(c, net);
                if mine & back != 0 || (mine | back).count_ones() > 3 {
                    return None;
                }
                let axis = if s.horizontal() { c.h } else { c.v };
                if axis.is_some_and(|o| o != net) {
                    return None;
                }
                return Some((true, 0));
            }
            if c.blocked || c.pin.is_some() || c.exit.is_some_and(|e| e != net) {
                return None;
            }
            let axis = if s.horizontal() { c.h } else { c.v };
            if axis.is_some() {
                return None;
            }
            let other = if s.horizontal() { c.v } else { c.h };
            let mut cost = STEP;
            if other.is_some() {
                // Only straight across another net's straight wire.
                let bits = if s.horizontal() {
                    c.arms & V
                } else {
                    c.arms & H
                };
                if bits != V && bits != H {
                    return None;
                }
                cost += CROSS;
            }
            if c.near {
                cost += NEAR;
            }
            Some((false, cost))
        };
        match enter(first, side) {
            Some((true, _)) => return Some(vec![start, first]),
            Some((false, c)) => {
                push(&mut heap, c, first, side, u32::MAX, &mut best, &mut from_of);
            }
            None => return None,
        }
        while let Some(Reverse((_, _, cost, x, y, s))) = heap.pop() {
            let n = (x, y);
            let s = sides[s as usize];
            if cost > best[idx(n, s)] {
                continue;
            }
            expanded += 1;
            if expanded > BUDGET {
                return None;
            }
            let c = self.at(n).expect("in the field");
            let crossing = if s.horizontal() {
                c.v.is_some()
            } else {
                c.h.is_some()
            };
            for t in sides {
                if t == s.opposite() {
                    continue;
                }
                let bend = t != s;
                if bend && crossing {
                    continue;
                }
                let m = step(n, t);
                let Some((stop, add)) = enter(m, t) else {
                    continue;
                };
                let total = cost + add + if bend { BEND } else { 0 };
                if stop {
                    // Walk back.
                    let mut path = vec![m, n];
                    let mut at = idx(n, s);
                    while from_of[at] != u32::MAX {
                        let p = from_of[at] as usize;
                        let node_i = p / 4;
                        path.push(((node_i as i64) % self.cols, (node_i as i64) / self.cols));
                        at = p;
                    }
                    path.push(start);
                    path.reverse();
                    return Some(path);
                }
                push(
                    &mut heap,
                    total,
                    m,
                    t,
                    idx(n, s) as u32,
                    &mut best,
                    &mut from_of,
                );
            }
        }
        None
    }
}

/// The arms of `c` that `net` owns.
fn owned(c: &Cell, net: usize) -> u8 {
    let mut m = 0;
    if c.h == Some(net) {
        m |= H;
    }
    if c.v == Some(net) {
        m |= V;
    }
    m
}

/// The nodes within half a step of `r` (exactly half a step away not
/// counted): every node a wire touching the box would pass through.
fn near(r: Rect) -> (Node, Node) {
    let h = GRID / 2 - 1;
    (
        (
            -(-(r.min[0] - h)).div_euclid(GRID),
            -(-(r.min[1] - h)).div_euclid(GRID),
        ),
        (
            (r.max[0] + h).div_euclid(GRID),
            (r.max[1] + h).div_euclid(GRID),
        ),
    )
}

fn node_lo(p: Point) -> Node {
    (p[0].div_euclid(GRID), p[1].div_euclid(GRID))
}

fn node_hi(p: Point) -> Node {
    (-(-p[0]).div_euclid(GRID), -(-p[1]).div_euclid(GRID))
}

/// Every node along a run's segments.
pub fn expand(run: &[Point]) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for w in run.windows(2) {
        let (a, b) = (node_of(w[0]), node_of(w[1]));
        let mut n = a;
        if out.last() != Some(&n) {
            out.push(n);
        }
        if a.0 != b.0 && a.1 != b.1 {
            // A slant (which no wire of ours is): its ends only.
            out.push(b);
            continue;
        }
        let s = way(a, if a == b { a } else { towards(a, b) });
        while n != b {
            n = step(n, s);
            out.push(n);
        }
    }
    if run.len() == 1 {
        out.push(node_of(run[0]));
    }
    out
}

fn towards(a: Node, b: Node) -> Node {
    (a.0 + (b.0 - a.0).signum(), a.1 + (b.1 - a.1).signum())
}

/// A path of nodes as a run: its ends and the points where it turns.
pub fn run_of(path: &[Node]) -> Vec<Point> {
    let mut out = vec![point_of(path[0])];
    for i in 1..path.len() - 1 {
        if way(path[i - 1], path[i]) != way(path[i], path[i + 1]) {
            out.push(point_of(path[i]));
        }
    }
    out.push(point_of(*path.last().expect("a path has an end")));
    out
}

/// A run's length in grid steps.
pub fn length(run: &[Point]) -> Nm {
    run.windows(2)
        .map(|w| (w[1][0] - w[0][0]).abs() + (w[1][1] - w[0][1]).abs())
        .sum::<Nm>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(x: i64, y: i64) -> Point {
        [x * GRID, y * GRID]
    }

    #[test]
    fn wires_go_round_bodies_and_join_their_tree() {
        let mut f = Field::new([40 * GRID, 30 * GRID]);
        f.block_body(Rect::new(g(10, 5), g(14, 20)));
        f.pin(g(9, 10), g(7, 10), Side::Left, 0);
        f.pin(g(18, 10), g(20, 10), Side::Right, 1);
        f.pin(g(3, 10), g(4, 10), Side::Right, 2);
        f.pin(g(27, 10), g(26, 10), Side::Left, 2);
        let path = f.search(2, g(4, 10), Side::Right).unwrap();
        let run = run_of(&path);
        assert_eq!(run.first(), Some(&g(4, 10)));
        assert_eq!(run.last(), Some(&g(26, 10)));
        // It may not cross the body or either pin's end.
        for n in expand(&run) {
            assert!(!(n.0 >= 10 && n.0 <= 14 && n.1 >= 5 && n.1 <= 20), "{n:?}");
        }
        f.mark(2, &run).unwrap();
        // A second net crossing it straight is allowed; along it is not.
        assert!(f.mark(3, &[g(2, 2), g(2, 25)]).is_ok());
        let along = run.windows(2).find(|w| w[0][1] == w[1][1]).unwrap();
        assert!(f.mark(5, along).is_err());
    }

    #[test]
    fn junctions_never_join_four() {
        let mut f = Field::new([20 * GRID, 20 * GRID]);
        f.mark(0, &[g(2, 10), g(18, 10)]).unwrap();
        f.mark(0, &[g(10, 10), g(10, 18)]).unwrap();
        assert!(f.mark(0, &[g(10, 10), g(10, 2)]).is_err());
        assert!(f.mark(1, &[g(10, 2), g(10, 9)]).is_ok());
    }

    #[test]
    fn runs_list_their_turns() {
        let path = [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2)];
        assert_eq!(run_of(&path), vec![g(0, 0), g(2, 0), g(2, 2)]);
        assert_eq!(expand(&run_of(&path)).len(), 5);
        assert_eq!(length(&run_of(&path)), 4 * GRID);
    }
}
