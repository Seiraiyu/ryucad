//! The contact graph and its components, kept as nodes come and go.
//!
//! Each component holds its members in order, so its least member (an
//! island's ID) is its first. Joining moves the smaller component's
//! members into the larger's. Removing nodes may split a component: the
//! nodes they touched are settled by searches that run from each of them
//! a step at a time, joining as they meet; a search that runs out first
//! holds a component of its own, and the last one left keeps the label.
//! So removing a trace from a plane's island costs the trace's side, not
//! the plane's.
//!
//! Labels and the order searches run in depend on the edits; the
//! components, and every answer built from them, don't.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::Hash;

pub(crate) struct Graph<N> {
    adj: HashMap<N, HashSet<N>>,
    comp: HashMap<N, u64>,
    members: HashMap<u64, BTreeSet<N>>,
    next: u64,
}

impl<N: Ord + Hash + Clone> Graph<N> {
    pub(crate) fn new() -> Graph<N> {
        Graph {
            adj: HashMap::new(),
            comp: HashMap::new(),
            members: HashMap::new(),
            next: 0,
        }
    }

    fn label(&mut self) -> u64 {
        self.next += 1;
        self.next - 1
    }

    /// Adds a node on its own; nothing if it is there already.
    pub(crate) fn add_node(&mut self, n: N) {
        if self.adj.contains_key(&n) {
            return;
        }
        let c = self.label();
        self.adj.insert(n.clone(), HashSet::new());
        self.comp.insert(n.clone(), c);
        self.members.insert(c, BTreeSet::from([n]));
    }

    /// Joins two nodes already added.
    pub(crate) fn add_edge(&mut self, a: &N, b: &N) {
        if a == b {
            return;
        }
        self.adj.get_mut(a).expect("a node").insert(b.clone());
        self.adj.get_mut(b).expect("a node").insert(a.clone());
        let (ca, cb) = (self.comp[a], self.comp[b]);
        if ca == cb {
            return;
        }
        let (from, to) = if self.members[&ca].len() < self.members[&cb].len() {
            (ca, cb)
        } else {
            (cb, ca)
        };
        let moved = self.members.remove(&from).expect("a component");
        for n in &moved {
            self.comp.insert(n.clone(), to);
        }
        self.members
            .get_mut(&to)
            .expect("a component")
            .extend(moved);
    }

    /// Removes nodes and their edges, giving back the nodes they touched
    /// that remain, sorted: what [`Graph::settle`] needs.
    pub(crate) fn remove_nodes(&mut self, gone: &[N]) -> Vec<N> {
        let mut touched = BTreeSet::new();
        for n in gone {
            let Some(next) = self.adj.remove(n) else {
                continue;
            };
            for m in next {
                if let Some(s) = self.adj.get_mut(&m) {
                    s.remove(n);
                    touched.insert(m);
                }
            }
            let c = self.comp.remove(n).expect("a component");
            let members = self.members.get_mut(&c).expect("a component");
            members.remove(n);
            if members.is_empty() {
                self.members.remove(&c);
            }
        }
        touched
            .into_iter()
            .filter(|m| self.adj.contains_key(m))
            .collect()
    }

    /// Splits the components that removals may have cut. `seeds` must
    /// hold, for every component an edit changed, a node of each part it
    /// may have fallen into: the nodes removals touched, and the nodes
    /// added since. Seeds no longer in the graph are skipped.
    pub(crate) fn settle(&mut self, seeds: &[N]) {
        let mut by: BTreeMap<u64, BTreeSet<N>> = BTreeMap::new();
        for s in seeds {
            if let Some(&c) = self.comp.get(s) {
                by.entry(c).or_default().insert(s.clone());
            }
        }
        for (c, s) in by {
            if s.len() >= 2 {
                self.split(c, s.into_iter().collect());
            }
        }
    }

    /// One search per seed, a step each in turn. Searches that meet are
    /// one from then on; one that runs out holds a whole component and is
    /// labelled anew; the last left keeps `c`.
    fn split(&mut self, c: u64, seeds: Vec<N>) {
        let k = seeds.len();
        let mut root: Vec<usize> = (0..k).collect();
        let mut queue: Vec<VecDeque<N>> =
            seeds.iter().map(|s| VecDeque::from([s.clone()])).collect();
        let mut seen: Vec<Vec<N>> = seeds.iter().map(|s| vec![s.clone()]).collect();
        let mut owner: HashMap<N, usize> =
            seeds.into_iter().enumerate().map(|(i, s)| (s, i)).collect();
        let mut done = vec![false; k];
        let mut live = k;
        fn find(root: &mut [usize], mut i: usize) -> usize {
            while root[i] != i {
                root[i] = root[root[i]];
                i = root[i];
            }
            i
        }
        while live > 1 {
            for i in 0..k {
                if live <= 1 {
                    break;
                }
                if root[i] != i || done[i] {
                    continue;
                }
                let Some(n) = queue[i].pop_front() else {
                    // Ran out: a whole component, apart from the rest.
                    done[i] = true;
                    live -= 1;
                    let id = self.label();
                    let set: BTreeSet<N> = std::mem::take(&mut seen[i]).into_iter().collect();
                    let members = self.members.get_mut(&c).expect("a component");
                    for x in &set {
                        members.remove(x);
                        self.comp.insert(x.clone(), id);
                    }
                    self.members.insert(id, set);
                    continue;
                };
                let next: Vec<N> = self.adj[&n].iter().cloned().collect();
                for m in next {
                    match owner.get(&m).copied() {
                        None => {
                            owner.insert(m.clone(), i);
                            seen[i].push(m.clone());
                            queue[i].push_back(m);
                        }
                        Some(j) => {
                            let r = find(&mut root, j);
                            if r != i {
                                // Two searches met: one from now on.
                                root[r] = i;
                                let q = std::mem::take(&mut queue[r]);
                                queue[i].extend(q);
                                let s = std::mem::take(&mut seen[r]);
                                seen[i].extend(s);
                                live -= 1;
                            }
                        }
                    }
                }
            }
        }
    }

    /// The component holding `n`, its members in order.
    pub(crate) fn component(&self, n: &N) -> Option<&BTreeSet<N>> {
        self.comp.get(n).map(|c| &self.members[c])
    }

    /// The least member of `n`'s component.
    pub(crate) fn first(&self, n: &N) -> Option<&N> {
        self.component(n)?.first()
    }

    /// Every component, in no particular order.
    pub(crate) fn components(&self) -> impl Iterator<Item = &BTreeSet<N>> {
        self.members.values()
    }

    /// The nodes joined to `n`, sorted.
    pub(crate) fn neighbours(&self, n: &N) -> Vec<N> {
        let mut v: Vec<N> = self
            .adj
            .get(n)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sets(g: &Graph<u32>) -> Vec<Vec<u32>> {
        let mut out: Vec<Vec<u32>> = g
            .components()
            .map(|c| c.iter().copied().collect())
            .collect();
        out.sort();
        out
    }

    /// Components by a search from scratch.
    fn brute(g: &Graph<u32>) -> Vec<Vec<u32>> {
        let mut nodes: Vec<u32> = g.adj.keys().copied().collect();
        nodes.sort_unstable();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for n in nodes {
            if !seen.insert(n) {
                continue;
            }
            let mut comp = vec![n];
            let mut q = vec![n];
            while let Some(m) = q.pop() {
                for &x in &g.adj[&m] {
                    if seen.insert(x) {
                        comp.push(x);
                        q.push(x);
                    }
                }
            }
            comp.sort_unstable();
            out.push(comp);
        }
        out.sort();
        out
    }

    #[test]
    fn components_split_and_join() {
        let mut g: Graph<u32> = Graph::new();
        for n in 0..6 {
            g.add_node(n);
        }
        for (a, b) in [(0, 1), (1, 2), (3, 4)] {
            g.add_edge(&a, &b);
        }
        assert_eq!(sets(&g), vec![vec![0, 1, 2], vec![3, 4], vec![5]]);
        // Removing the middle of a path splits it.
        let seeds = g.remove_nodes(&[1]);
        assert_eq!(seeds, vec![0, 2]);
        g.settle(&seeds);
        assert_eq!(sets(&g), vec![vec![0], vec![2], vec![3, 4], vec![5]]);
        // A cycle survives losing a node.
        g.add_node(1);
        for (a, b) in [(0, 1), (1, 2), (2, 5), (5, 0)] {
            g.add_edge(&a, &b);
        }
        let seeds = g.remove_nodes(&[1]);
        g.settle(&seeds);
        assert_eq!(sets(&g), vec![vec![0, 2, 5], vec![3, 4]]);
        assert_eq!(g.neighbours(&0), vec![5]);
        assert_eq!(g.first(&2), Some(&0));
        assert!(g.component(&1).is_none());
    }

    #[test]
    fn random_edits_match_a_search_from_scratch() {
        let mut state = 0x1234_5678_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut g: Graph<u32> = Graph::new();
        for step in 0..2_000 {
            let r = next();
            let (a, b) = ((r % 40) as u32, ((r >> 8) % 40) as u32);
            match (r >> 16) % 4 {
                0 | 1 => {
                    g.add_node(a);
                    g.add_node(b);
                    g.add_edge(&a, &b);
                }
                2 => {
                    let s = g.remove_nodes(&[a]);
                    g.settle(&s);
                }
                _ => {
                    // Two go, one comes back on its own.
                    let mut s = g.remove_nodes(&[a, b]);
                    g.add_node(a);
                    s.push(a);
                    g.settle(&s);
                }
            }
            assert_eq!(sets(&g), brute(&g), "step {step}");
        }
    }
}
