//! The frontier walk (design 399): which pieces are the result, decided
//! by counting. From the seeds, every edge a kept piece uses must be used
//! by exactly one other kept piece the other way round; an edge with
//! none brings in the one piece of another face that uses it the other
//! way. Edges the input had come first, and an edge whose partner may yet
//! be reached is deferred while anything else is forced, so a face's own
//! other piece is found before a stranger is pulled in.

use crate::topo::Sense;
use crate::work::Meter;
use std::collections::{BTreeMap, BTreeSet};

/// The pieces as the walk sees them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Arrangement {
    /// Each piece's face (any index) and the edges its loops use, with
    /// the sense each runs them.
    pub(crate) pieces: Vec<(usize, Vec<(usize, Sense)>)>,
    /// Edges that are a working face's clipped side.
    pub(crate) clipped: BTreeSet<usize>,
    /// Edges the input had laminar: one kept use is all they take.
    pub(crate) laminar: BTreeSet<usize>,
    /// Edges that are pieces of the input's own edges: settled first.
    pub(crate) old: BTreeSet<usize>,
}

/// Why the walk couldn't close, at a piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalkError {
    /// No piece of another face takes one of this piece's edges back.
    Open { piece: usize },
    /// Two could, or a third would use an edge.
    Ambiguous { piece: usize },
    /// The piece reaches a working face's clipped side.
    Clipped { piece: usize },
    /// The meter stopped.
    Halted { piece: usize },
}

fn flip(s: Sense) -> Sense {
    match s {
        Sense::Forward => Sense::Reversed,
        Sense::Reversed => Sense::Forward,
    }
}

/// The pieces kept, by index, from `seeds`: a unit charged per piece.
pub(crate) fn walk(arr: &Arrangement, seeds: &[usize], m: &Meter) -> Result<Vec<bool>, WalkError> {
    // Keyed by edge and whether the arc runs it forwards.
    let mut users: BTreeMap<(usize, bool), Vec<usize>> = BTreeMap::new();
    for (i, (_, arcs)) in arr.pieces.iter().enumerate() {
        for &(g, s) in arcs {
            users.entry((g, s == Sense::Forward)).or_default().push(i);
        }
    }
    let none: Vec<usize> = Vec::new();
    let of = |g: usize, s: Sense| users.get(&(g, s == Sense::Forward)).unwrap_or(&none);
    let mut kept = vec![false; arr.pieces.len()];
    let mut queue: Vec<usize> = Vec::new();
    for &s in seeds {
        if !kept[s] {
            kept[s] = true;
            queue.push(s);
        }
    }
    // Arcs waiting for a partner: new edges after old ones, then by edge,
    // sense and piece.
    let mut pending: BTreeSet<(bool, usize, bool, usize)> = BTreeSet::new();
    loop {
        // Drain what is forced.
        while let Some(p) = queue.pop() {
            m.charge(1).map_err(|_| WalkError::Halted { piece: p })?;
            for &(g, s) in &arr.pieces[p].1 {
                if arr.clipped.contains(&g) {
                    return Err(WalkError::Clipped { piece: p });
                }
                if of(g, s).iter().filter(|&&q| kept[q]).count() > 1 {
                    return Err(WalkError::Ambiguous { piece: p });
                }
                if arr.laminar.contains(&g) {
                    continue;
                }
                match of(g, flip(s)).iter().filter(|&&q| kept[q]).count() {
                    1 => {}
                    0 => {
                        pending.insert((!arr.old.contains(&g), g, s == Sense::Forward, p));
                    }
                    _ => return Err(WalkError::Ambiguous { piece: p }),
                }
            }
        }
        // Settle the first arc still waiting.
        let Some(&first) = pending.iter().next() else {
            return Ok(kept);
        };
        pending.remove(&first);
        let (_, g, forward, p) = first;
        let s = if forward {
            Sense::Forward
        } else {
            Sense::Reversed
        };
        let back = of(g, flip(s));
        match back.iter().filter(|&&q| kept[q]).count() {
            1 => continue,
            0 => {}
            _ => return Err(WalkError::Ambiguous { piece: p }),
        }
        let face = arr.pieces[p].0;
        let others: Vec<usize> = back
            .iter()
            .copied()
            .filter(|&q| arr.pieces[q].0 != face)
            .collect();
        match others.as_slice() {
            [q] => {
                kept[*q] = true;
                queue.push(*q);
            }
            [] => return Err(WalkError::Open { piece: p }),
            _ => return Err(WalkError::Ambiguous { piece: p }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Sense::{Forward as F, Reversed as R};

    fn arrangement(pieces: Vec<(usize, Vec<(usize, Sense)>)>) -> Arrangement {
        Arrangement {
            pieces,
            clipped: BTreeSet::new(),
            laminar: BTreeSet::new(),
            old: BTreeSet::new(),
        }
    }

    /// A cube's six faces, one piece each, every edge used once each way.
    fn cube() -> Arrangement {
        // Edges 0..12; faces: bottom 0, top 1, sides 2..6.
        arrangement(vec![
            (0, vec![(0, F), (1, F), (2, F), (3, F)]),
            (1, vec![(4, F), (5, F), (6, F), (7, F)]),
            (2, vec![(0, R), (9, F), (4, R), (8, R)]),
            (3, vec![(1, R), (10, F), (5, R), (9, R)]),
            (4, vec![(2, R), (11, F), (6, R), (10, R)]),
            (5, vec![(3, R), (8, F), (7, R), (11, R)]),
        ])
    }

    // unmetered: unit tests of a combinatorial walk.
    fn free() -> Meter<'static> {
        Meter::free()
    }

    #[test]
    fn a_cube_closes_from_one_seed() {
        assert_eq!(walk(&cube(), &[0], &free()).unwrap(), vec![true; 6]);
    }

    #[test]
    fn an_edge_no_piece_takes_back_is_open() {
        let mut a = cube();
        a.pieces.pop();
        assert!(matches!(
            walk(&a, &[0], &free()),
            Err(WalkError::Open { .. })
        ));
    }

    #[test]
    fn two_partners_are_ambiguous() {
        let mut a = cube();
        a.pieces.push((6, vec![(0, R)]));
        assert!(matches!(
            walk(&a, &[0], &free()),
            Err(WalkError::Ambiguous { .. })
        ));
    }

    #[test]
    fn a_clipped_side_is_reached() {
        let mut a = cube();
        a.clipped.insert(11);
        assert!(matches!(
            walk(&a, &[0], &free()),
            Err(WalkError::Clipped { .. })
        ));
    }

    #[test]
    fn a_faces_own_other_piece_wins_over_a_stranger_reached_first() {
        // O (face 0) is the seed; A1 and A2 (face 1) share edge 2; X1
        // (face 2) also uses 2 the way A2 does. O reaches A2 by old edge
        // 0 and A1 by old edge 1; deferring new edges means A1's edge 2
        // finds A2 kept, never X1.
        let mut a = arrangement(vec![
            (0, vec![(0, F), (1, F)]),
            (1, vec![(2, F), (1, R)]),
            (1, vec![(2, R), (0, R)]),
            (2, vec![(2, R)]),
        ]);
        a.old = BTreeSet::from([0, 1]);
        assert_eq!(
            walk(&a, &[0], &free()).unwrap(),
            vec![true, true, true, false]
        );
    }

    #[test]
    fn laminar_edges_take_one_use() {
        let mut a = arrangement(vec![(0, vec![(0, F)])]);
        a.laminar.insert(0);
        assert_eq!(walk(&a, &[0], &free()).unwrap(), vec![true]);
    }
}
