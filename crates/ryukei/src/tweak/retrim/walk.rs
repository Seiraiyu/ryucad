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

/// How many choices the search tries in all before it calls the
/// arrangement ambiguous.
pub(crate) const CHOICES: usize = 4096;

/// A state of the search: each piece's verdict, the pieces to drain and
/// the arcs waiting for a partner.
#[derive(Clone)]
struct State {
    kept: Vec<Option<bool>>,
    queue: Vec<usize>,
    pending: BTreeSet<(bool, usize, bool, usize)>,
}

/// The sense piece `q` uses an arc of sense `s` with, as kept.
fn as_kept(s: Sense, reversed: bool) -> Sense {
    if reversed { flip(s) } else { s }
}

/// The pieces kept, and which reversed, from `seeds` (kept as they are):
/// the one closure where every edge a kept piece uses is used by exactly
/// one other kept piece the other way (a laminar edge by none), and no
/// kept piece uses a clipped edge. Forced partners are taken as they come;
/// where an edge's partner isn't forced, each piece of another face that
/// uses it the other way, then each that uses it the same way (kept
/// reversed), is tried in piece order, and the search goes on from each.
/// One closure is the answer; two are `Ambiguous`, as is a search past
/// [`CHOICES`] tries; none gives the first failure met. A unit is charged
/// per piece drained and per choice.
pub(crate) fn walk(
    arr: &Arrangement,
    seeds: &[usize],
    m: &Meter,
) -> Result<Vec<Option<bool>>, WalkError> {
    let mut users: BTreeMap<(usize, bool), Vec<usize>> = BTreeMap::new();
    for (i, (_, arcs)) in arr.pieces.iter().enumerate() {
        for &(g, s) in arcs {
            users.entry((g, s == Sense::Forward)).or_default().push(i);
        }
    }
    let mut state = State {
        kept: vec![None; arr.pieces.len()],
        queue: Vec::new(),
        pending: BTreeSet::new(),
    };
    for &s in seeds {
        if state.kept[s].is_none() {
            state.kept[s] = Some(false);
            state.queue.push(s);
        }
    }
    let mut tries = 0usize;
    let mut found: Vec<Vec<Option<bool>>> = Vec::new();
    let mut first: Option<WalkError> = None;
    search(arr, &users, state, m, &mut tries, &mut found, &mut first)?;
    match found.len() {
        1 => Ok(found.pop().expect("one")),
        0 => Err(first.unwrap_or(WalkError::Open {
            piece: seeds.first().copied().unwrap_or(0),
        })),
        _ => Err(WalkError::Ambiguous {
            piece: seeds.first().copied().unwrap_or(0),
        }),
    }
}

/// The kept pieces that use edge `g` with sense `s` as kept.
fn kept_using(
    users: &BTreeMap<(usize, bool), Vec<usize>>,
    kept: &[Option<bool>],
    g: usize,
    s: Sense,
) -> usize {
    [Sense::Forward, Sense::Reversed]
        .into_iter()
        .flat_map(|raw| {
            users
                .get(&(g, raw == Sense::Forward))
                .into_iter()
                .flatten()
                .map(move |&q| (q, raw))
        })
        .filter(|&(q, raw)| kept[q].is_some_and(|r| as_kept(raw, r) == s))
        .count()
}

/// Drains `state`, then branches on its first waiting arc; closures go
/// into `found` (two are enough to stop), the first failure into `first`.
/// Only a halt is an error.
fn search(
    arr: &Arrangement,
    users: &BTreeMap<(usize, bool), Vec<usize>>,
    mut st: State,
    m: &Meter,
    tries: &mut usize,
    found: &mut Vec<Vec<Option<bool>>>,
    first: &mut Option<WalkError>,
) -> Result<(), WalkError> {
    let fail = |e: WalkError, first: &mut Option<WalkError>| {
        first.get_or_insert(e);
    };
    loop {
        while let Some(p) = st.queue.pop() {
            m.charge(1).map_err(|_| WalkError::Halted { piece: p })?;
            let r = st.kept[p].expect("queued pieces are kept");
            for &(g, raw) in &arr.pieces[p].1 {
                let s = as_kept(raw, r);
                if arr.clipped.contains(&g) {
                    fail(WalkError::Clipped { piece: p }, first);
                    return Ok(());
                }
                if kept_using(users, &st.kept, g, s) > 1 {
                    fail(WalkError::Ambiguous { piece: p }, first);
                    return Ok(());
                }
                if arr.laminar.contains(&g) {
                    continue;
                }
                match kept_using(users, &st.kept, g, flip(s)) {
                    1 => {}
                    0 => {
                        st.pending
                            .insert((!arr.old.contains(&g), g, s == Sense::Forward, p));
                    }
                    _ => {
                        fail(WalkError::Ambiguous { piece: p }, first);
                        return Ok(());
                    }
                }
            }
        }
        let Some(&arc) = st.pending.iter().next() else {
            found.push(st.kept);
            return Ok(());
        };
        st.pending.remove(&arc);
        let (_, g, forward, p) = arc;
        let s = if forward {
            Sense::Forward
        } else {
            Sense::Reversed
        };
        if kept_using(users, &st.kept, g, flip(s)) == 1 {
            continue;
        }
        let face = arr.pieces[p].0;
        // Undecided pieces of other faces: the other way as they are,
        // then the same way kept reversed.
        let mut choices: Vec<(usize, bool)> = Vec::new();
        for (raw, reversed) in [(flip(s), false), (s, true)] {
            for &q in users.get(&(g, raw == Sense::Forward)).into_iter().flatten() {
                if st.kept[q].is_none() && arr.pieces[q].0 != face {
                    choices.push((q, reversed));
                }
            }
        }
        if choices.is_empty() {
            fail(WalkError::Open { piece: p }, first);
            return Ok(());
        }
        for (q, reversed) in choices {
            *tries += 1;
            if *tries > CHOICES {
                found.push(Vec::new());
                found.push(Vec::new());
                return Ok(());
            }
            m.charge(1).map_err(|_| WalkError::Halted { piece: q })?;
            let mut next = st.clone();
            next.kept[q] = Some(reversed);
            next.queue.push(q);
            search(arr, users, next, m, tries, found, first)?;
            if found.len() > 1 {
                return Ok(());
            }
        }
        return Ok(());
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
        assert_eq!(walk(&cube(), &[0], &free()).unwrap(), vec![Some(false); 6]);
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
    fn a_partner_that_closes_nothing_is_passed_over() {
        // A stray piece takes the bottom's edge 0 back too; only the
        // cube's own side closes, so the search keeps the cube.
        let mut a = cube();
        a.pieces.push((6, vec![(0, R)]));
        let mut want = vec![Some(false); 6];
        want.push(None);
        assert_eq!(walk(&a, &[0], &free()).unwrap(), want);
    }

    #[test]
    fn two_partners_are_ambiguous() {
        // A stray piece using edge 0 the bottom's way: a third user.
        let mut a = cube();
        a.pieces.push((6, vec![(0, F)]));
        assert!(matches!(
            walk(&a, &[0, 6], &free()),
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
            vec![Some(false), Some(false), Some(false), None]
        );
    }

    #[test]
    fn laminar_edges_take_one_use() {
        let mut a = arrangement(vec![(0, vec![(0, F)])]);
        a.laminar.insert(0);
        assert_eq!(walk(&a, &[0], &free()).unwrap(), vec![Some(false)]);
    }
    #[test]
    fn a_wrong_first_choice_is_backed_out_of() {
        // Seed O (face 0) uses edge 0 forward. Candidates the other way:
        // U (face 1), which also uses edge 1 that nothing takes back
        // (Open), and, the same way round, V (face 2), kept reversed,
        // whose edge 2 (reversed with it) W (face 3) closes as it is.
        let a = arrangement(vec![
            (0, vec![(0, F)]),
            (1, vec![(0, R), (1, F)]),
            (2, vec![(0, F), (2, F)]),
            (3, vec![(2, F)]),
        ]);
        assert_eq!(
            walk(&a, &[0], &free()).unwrap(),
            vec![Some(false), None, Some(true), Some(false)]
        );
    }

    #[test]
    fn two_closures_are_ambiguous() {
        // Seed O's edge 0 is taken back by U or by V alone, each closing.
        let a = arrangement(vec![
            (0, vec![(0, F)]),
            (1, vec![(0, R)]),
            (2, vec![(0, R)]),
        ]);
        assert!(matches!(
            walk(&a, &[0], &free()),
            Err(WalkError::Ambiguous { .. })
        ));
    }

    #[test]
    fn no_closure_gives_the_first_failure() {
        let a = arrangement(vec![(0, vec![(0, F)]), (1, vec![(0, R), (1, F)])]);
        assert!(matches!(
            walk(&a, &[0], &free()),
            Err(WalkError::Open { .. })
        ));
    }

    #[test]
    fn the_search_is_bounded() {
        // A chain of 64 edges, each with two candidates that both close
        // until the last: past `CHOICES` tries it is Ambiguous, not slow.
        let mut pieces = vec![(0, vec![(0, F)])];
        for k in 0..64 {
            pieces.push((2 * k + 1, vec![(k, R), (k + 1, F)]));
            pieces.push((2 * k + 2, vec![(k, R), (k + 1, F)]));
        }
        let a = arrangement(pieces);
        assert!(matches!(
            walk(&a, &[0], &free()),
            Err(WalkError::Ambiguous { .. })
        ));
    }
}
