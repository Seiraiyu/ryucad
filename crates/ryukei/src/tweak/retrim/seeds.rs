//! The walk's arrangement read from the table, and its seeds: every
//! piece of an outside face, and, for a shell no outside face seeds, one
//! region face's piece holding a point of its old self.

use super::body::WorkBody;
use super::walk::{Arrangement, WalkError};
use crate::boolean::classify::inner_point;
use crate::boolean::table::{Boundary, GeId, Member, Table};
use crate::geom::Point2;
use crate::intersect::{Place, point_in_face};
use crate::split::Piece;
use crate::topo::{Body, FaceId, Sense, ShellId};
use crate::tweak::engine::Changes;
use crate::work::Meter;
use std::collections::{BTreeMap, BTreeSet};

/// The global edges an edge of the working body in `set` became.
fn ges_of(table: &Table, set: &BTreeSet<crate::topo::EdgeId>) -> BTreeSet<GeId> {
    table
        .edges
        .iter()
        .enumerate()
        .filter(|(_, ge)| {
            ge.members
                .iter()
                .any(|m| matches!(m, Member::Edge(e) if set.contains(&e.edge)))
        })
        .map(|(g, _)| g)
        .collect()
}

/// The pieces as the walk sees them: their faces, arcs, the working
/// faces' clipped sides and the input's own edges.
pub(crate) fn arrangement(w: &WorkBody, table: &Table, pieces: &[Piece]) -> Arrangement {
    let own: BTreeSet<crate::topo::EdgeId> =
        w.body.edges().filter(|e| !w.clipped.contains(e)).collect();
    Arrangement {
        pieces: pieces
            .iter()
            .map(|p| {
                let arcs = p
                    .loops
                    .iter()
                    .flat_map(|l| match l {
                        Boundary::Arcs(a) => a.clone(),
                        Boundary::Lone(_) => Vec::new(),
                    })
                    .collect();
                (p.face.face.index(), arcs)
            })
            .collect(),
        clipped: ges_of(table, &w.clipped),
        laminar: BTreeSet::new(),
        old: ges_of(table, &own),
    }
}

/// The edges seed set `seeds` uses on its own more than twice in all, or
/// more than once the same way round (design 683, plan 21.C): a set in
/// conflict, which no closure can keep whole. Each distinct piece counts
/// once per use; a slit piece uses its edge once each way, which is no
/// conflict by itself.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn conflicts(arr: &Arrangement, seeds: &[usize]) -> BTreeSet<usize> {
    let distinct: BTreeSet<usize> = seeds.iter().copied().collect();
    // Per edge: uses reversed, uses forward.
    let mut uses: BTreeMap<usize, [usize; 2]> = BTreeMap::new();
    for &s in &distinct {
        for &(g, sense) in &arr.pieces[s].1 {
            uses.entry(g).or_default()[usize::from(sense == Sense::Forward)] += 1;
        }
    }
    uses.into_iter()
        .filter(|&(_, [r, f])| r + f > 2 || r > 1 || f > 1)
        .map(|(g, _)| g)
        .collect()
}

/// `seeds` less the seeds rule 3's shortcut alone chose (in `shortcut`)
/// that use an edge the set is in conflict over ([`conflicts`]) and whose
/// inner point doesn't lie inside their old face (`holds_old`, asked only
/// of those): the weakest seeds yield (design 683, question 983). `None`
/// where the set is in no conflict or no seed yields.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn yielded(
    arr: &Arrangement,
    seeds: &[usize],
    shortcut: &BTreeSet<usize>,
    holds_old: &mut dyn FnMut(usize) -> bool,
) -> Option<Vec<usize>> {
    let bad = conflicts(arr, seeds);
    if bad.is_empty() {
        return None;
    }
    let mut drop: BTreeSet<usize> = BTreeSet::new();
    for &s in seeds {
        if drop.contains(&s) || !shortcut.contains(&s) {
            continue;
        }
        if arr.pieces[s].1.iter().any(|(g, _)| bad.contains(g)) && !holds_old(s) {
            drop.insert(s);
        }
    }
    if drop.is_empty() {
        return None;
    }
    Some(
        seeds
            .iter()
            .copied()
            .filter(|s| !drop.contains(s))
            .collect(),
    )
}

/// After every try of `tries` failed: the first try from which a seed
/// yields ([`yielded`]) walked once more without it (design 683, plan
/// 21.C). Its seeds and answer where that walk closes or the meter halts
/// it; `None` otherwise, where the first try's failure stands. A seed
/// only says where the walk starts: a yielded seed's piece is kept only
/// if the one closure reaches it.
#[cfg_attr(not(test), allow(dead_code))]
#[allow(clippy::type_complexity)]
pub(crate) fn walk_again(
    arr: &Arrangement,
    tries: &[Vec<usize>],
    shortcut: &BTreeSet<usize>,
    holds_old: &mut dyn FnMut(usize) -> bool,
    m: &Meter,
) -> Option<(Vec<usize>, Result<Vec<Option<bool>>, WalkError>)> {
    for t in tries {
        if let Some(fewer) = yielded(arr, t, shortcut, holds_old) {
            let r = super::walk::walk(arr, &fewer, m);
            return (r.is_ok() || matches!(r, Err(WalkError::Halted { .. }))).then_some((fewer, r));
        }
    }
    None
}

/// Every piece of an outside face. Never a pinned face's (plan 21.A):
/// part of one may be cut away; rule 3 seeds it ([`face_seeds`]).
pub(crate) fn outside_seeds(w: &WorkBody, pieces: &[Piece]) -> Vec<usize> {
    pieces
        .iter()
        .enumerate()
        .filter(|(_, p)| !w.working.contains(&p.face.face) && !w.pinned.contains(&p.face.face))
        .map(|(i, _)| i)
        .collect()
}

/// A seed for each shell of `body` that `kept` holds no piece of: its
/// least-named region face with exactly one candidate piece (one whose
/// inner point, taken back to the face's old surface, lies inside the old
/// face) that uses no clipped edge. `None` where no face of the shell
/// gives one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shell_seeds(
    body: &Body,
    changes: &Changes,
    w: &WorkBody,
    arr: &Arrangement,
    table: &Table,
    pieces: &[Piece],
    kept: &[bool],
    m: &Meter,
) -> Vec<usize> {
    let ops = [w.body.clone()];
    let mut seeded: BTreeSet<ShellId> = BTreeSet::new();
    for (i, p) in pieces.iter().enumerate() {
        if kept[i] {
            seeded.insert(body.faces[w.origin[&p.face.face]].shell);
        }
    }
    // Each unseeded shell's working faces, by name.
    let mut by_shell: BTreeMap<ShellId, Vec<(crate::name::Name, FaceId)>> = BTreeMap::new();
    for &wf in &w.working {
        let f = w.origin[&wf];
        let s = body.faces[f].shell;
        if !seeded.contains(&s) {
            by_shell
                .entry(s)
                .or_default()
                .push((body.faces[f].name, wf));
        }
    }
    let mut out = Vec::new();
    for (_, mut faces) in by_shell {
        faces.sort();
        for (_, wf) in faces {
            if m.charge(1).is_err() {
                return out;
            }
            let (found, _) = candidates(body, changes, w, arr, &ops, table, pieces, wf);
            if let [one] = found.as_slice() {
                out.push(*one);
                break;
            }
        }
    }
    out
}

/// Rule 3's seeds ([`face_seeds`]): the unchanged faces', the moved
/// faces', and which of either the shortcut alone chose (a deleting face's
/// one unclipped piece, its inner point not tested: plan 21.C).
#[derive(Clone, Debug, Default)]
pub(crate) struct FaceSeeds {
    pub(crate) same: Vec<usize>,
    pub(crate) moved: Vec<usize>,
    pub(crate) shortcut: BTreeSet<usize>,
}

/// Design 399's rule 3: each region face with no frontier edge (all its
/// neighbours in the region), and each pinned face (plan 21.A), seeded
/// by its one candidate piece, the one
/// whose inner point, taken back to the face's old surface, lies inside
/// the old face, using no clipped edge. A face with no candidate, or more
/// than one, seeds nothing. The unchanged faces' seeds, then the moved
/// faces'.
pub(crate) fn face_seeds(
    body: &Body,
    changes: &Changes,
    w: &WorkBody,
    arr: &Arrangement,
    table: &Table,
    pieces: &[Piece],
    m: &Meter,
) -> FaceSeeds {
    let ops = [w.body.clone()];
    // A face with an edge on the frontier the outside faces seed; an edge
    // with a pinned face doesn't count (plan 21.A), so a working face is
    // seeded as it was without pins.
    let on_frontier: BTreeSet<FaceId> = w
        .frontier_faces
        .iter()
        .filter(|(e, _)| {
            !w.body.edge_fins(**e).any(|fin| {
                w.pinned
                    .contains(&w.body.loops[w.body.fins[fin].loop_].face)
            })
        })
        .flat_map(|(_, fs)| fs.iter().copied())
        .collect();
    // Every pinned face by rule 3, frontier or not: its candidate piece
    // holding its old interior (design 683).
    let mut faces: Vec<(crate::name::Name, FaceId)> = w
        .working
        .iter()
        .filter(|wf| !on_frontier.contains(&w.origin[wf]))
        .chain(w.pinned.iter())
        .map(|&wf| (body.faces[w.origin[&wf]].name, wf))
        .collect();
    faces.sort();
    let mut out = FaceSeeds::default();
    for (_, wf) in faces {
        if m.charge(1).is_err() {
            break;
        }
        let (found, short) = candidates(body, changes, w, arr, &ops, table, pieces, wf);
        if let [one] = found.as_slice() {
            if short {
                out.shortcut.insert(*one);
            }
            if changes.motion.contains_key(&w.origin[&wf]) {
                out.moved.push(*one);
            } else {
                out.same.push(*one);
            }
        }
    }
    out
}

/// Face `wf`'s candidate pieces: those using no clipped edge whose inner
/// point lies inside the old face; where faces are deleted, a face with
/// one piece using no clipped edge takes it (a face growing over the gap
/// has its inner points in the strip it grew by); and whether the shortcut
/// chose them (plan 21.C).
#[allow(clippy::too_many_arguments)]
fn candidates(
    body: &Body,
    changes: &Changes,
    w: &WorkBody,
    arr: &Arrangement,
    ops: &[Body],
    table: &Table,
    pieces: &[Piece],
    wf: FaceId,
) -> (Vec<usize>, bool) {
    let unclipped: Vec<usize> = pieces
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            p.face.face == wf
                && arr.pieces[*i]
                    .1
                    .iter()
                    .all(|(g, _)| !arr.clipped.contains(g))
        })
        .map(|(i, _)| i)
        .collect();
    if !changes.deleted.is_empty() && unclipped.len() == 1 {
        return (unclipped, true);
    }
    (
        unclipped
            .into_iter()
            .filter(|&i| inside_old(body, changes, w, ops, table, &pieces[i]))
            .collect(),
        false,
    )
}

/// Whether piece `p`'s inner point, taken back to its face's old surface,
/// lies inside the old face (the first of four inner points that is
/// inside or outside, not on the boundary, decides).
pub(crate) fn inside_old(
    body: &Body,
    changes: &Changes,
    w: &WorkBody,
    ops: &[Body],
    table: &Table,
    p: &Piece,
) -> bool {
    let f = w.origin[&p.face.face];
    let old = &body.surfaces[body.faces[f].surface];
    let keeps = super::working::same_params(changes.motion.get(&f));
    for attempt in 0..4 {
        let Some((q, uv)) = inner_point(ops, table, p, attempt) else {
            continue;
        };
        let uv = if keeps {
            uv
        } else {
            let (u, v) = old.project(q).param;
            Point2::new(u, v)
        };
        match point_in_face(body, f, uv) {
            Ok(Place::Inside) => return true,
            Ok(Place::Outside) => return false,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::body::{region_box, work_body};
    use super::super::region::region;
    use super::super::tests::{K, block, face_of};
    use super::super::working::working;
    use super::*;
    use crate::boolean::table::{Scope, build_in};
    use crate::name::FaceRole;
    use crate::split::split_all;

    #[test]
    fn a_raised_tops_bottom_seeds_and_its_frontier_is_old() {
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let r = region(&b, &BTreeSet::from([top]));
        let up = crate::geom::Rigid::translation(crate::geom::Vector3::new(0.0, 0.0, 0.1));
        let bx = region_box(&b, &r, 0.1);
        let ws: BTreeMap<FaceId, super::super::working::Working> = r
            .faces
            .iter()
            .map(|&f| {
                let new = (f == top).then(|| b.surfaces[b.faces[f].surface].transformed(&up));
                (
                    f,
                    working(&b, f, new.as_ref(), b.faces[f].sense, true, &bx).unwrap(),
                )
            })
            .collect();
        // unmetered: a unit test.
        let m = Meter::free();
        let w = work_body(K, &b, &r, &ws, &m).unwrap();
        let ops = [w.body.clone()];
        let table = build_in(&ops, Scope::Pairs(&w.pairs()), &m).unwrap();
        let pieces = split_all(&ops, &table, &m).unwrap();
        let arr = arrangement(&w, &table, &pieces);
        let seeds = outside_seeds(&w, &pieces);
        // The bottom alone, whole.
        assert_eq!(seeds.len(), 1);
        assert_eq!(arr.old.len(), 4);
        assert!(!arr.clipped.is_empty());
    }

    use super::super::walk::Arrangement;
    use crate::topo::Sense::{Forward as F, Reversed as R};

    /// A ball's piece B (face 0) slit along edge 0 (both ways round), its
    /// edge 1 closed by C (face 2); a NURBS piece N (face 1) using edge 0
    /// forward too, with an edge 2 nothing takes back. Plan 21.C.
    fn slit() -> Arrangement {
        Arrangement {
            pieces: vec![
                (0, vec![(0, F), (0, R), (1, F)]),
                (2, vec![(1, R)]),
                (1, vec![(0, F), (2, F)]),
            ],
            ..Arrangement::default()
        }
    }

    #[test]
    fn a_slit_with_a_third_use_is_in_conflict_and_alone_isnt() {
        let a = slit();
        assert_eq!(conflicts(&a, &[0, 2]), BTreeSet::from([0]));
        assert!(conflicts(&a, &[0]).is_empty());
        assert!(conflicts(&a, &[0, 1]).is_empty());
        // A seed named twice is one piece.
        assert!(conflicts(&a, &[0, 0]).is_empty());
    }

    #[test]
    fn two_seeds_using_an_edge_the_same_way_are_in_conflict() {
        let a = Arrangement {
            pieces: vec![(0, vec![(5, F)]), (1, vec![(5, F)]), (2, vec![(5, R)])],
            ..Arrangement::default()
        };
        assert_eq!(conflicts(&a, &[0, 1]), BTreeSet::from([5]));
        assert!(conflicts(&a, &[0, 2]).is_empty());
    }

    use super::super::walk::walk;

    fn free() -> Meter<'static> {
        // unmetered: unit tests of a combinatorial walk.
        Meter::free()
    }

    #[test]
    fn a_slits_shortcut_seed_yields_and_the_walk_closes_without_it() {
        let a = slit();
        let tries = vec![vec![0, 2]];
        // Every seed set fails: three uses of edge 0.
        assert!(matches!(
            walk(&a, &tries[0], &free()),
            Err(WalkError::Ambiguous { .. })
        ));
        let shortcut = BTreeSet::from([2]);
        let mut holds = |_: usize| false;
        assert_eq!(yielded(&a, &tries[0], &shortcut, &mut holds), Some(vec![0]));
        let (fewer, r) = walk_again(&a, &tries, &shortcut, &mut holds, &free()).unwrap();
        assert_eq!(fewer, vec![0]);
        // The ball and its partner kept; the NURBS's piece never reached.
        assert_eq!(r.unwrap(), vec![Some(false), Some(false), None]);
    }

    #[test]
    fn a_set_with_no_conflict_is_never_walked_again() {
        // The slit alone, with C missing: Open, and no conflict.
        let mut a = slit();
        a.pieces[1].1.clear();
        let tries = vec![vec![0]];
        assert!(walk(&a, &tries[0], &free()).is_err());
        let mut holds = |_: usize| false;
        assert_eq!(
            yielded(&a, &tries[0], &BTreeSet::from([0]), &mut holds),
            None
        );
        assert!(walk_again(&a, &tries, &BTreeSet::from([0]), &mut holds, &free()).is_none());
    }

    #[test]
    fn seeds_holding_their_old_faces_never_yield() {
        let a = slit();
        let tries = vec![vec![0, 2]];
        // N chosen by rule 3 proper: not a shortcut seed.
        let mut holds = |_: usize| false;
        assert!(walk_again(&a, &tries, &BTreeSet::new(), &mut holds, &free()).is_none());
        // N a shortcut seed whose inner point lies inside its old face.
        let mut inside = |i: usize| i == 2;
        assert!(walk_again(&a, &tries, &BTreeSet::from([2]), &mut inside, &free()).is_none());
    }

    #[test]
    fn a_walk_again_that_fails_answers_nothing() {
        // N yields, but B's edge 1 then has no partner: Open, so the first
        // try's failure is reported by the caller.
        let mut a = slit();
        a.pieces[1].1.clear();
        let tries = vec![vec![0, 2]];
        let mut holds = |_: usize| false;
        assert!(walk_again(&a, &tries, &BTreeSet::from([2]), &mut holds, &free()).is_none());
    }

    #[test]
    fn the_first_try_that_yields_is_the_one_walked_again() {
        // Try 0 has no shortcut seed; try 1 does.
        let a = slit();
        let tries = vec![vec![0, 1], vec![0, 2]];
        let mut holds = |_: usize| false;
        let (fewer, _) = walk_again(&a, &tries, &BTreeSet::from([2]), &mut holds, &free()).unwrap();
        assert_eq!(fewer, vec![0]);
    }

    /// Plan 21.C: a deleting face's one unclipped piece is a shortcut
    /// seed, and `face_seeds` says so. Every edge of the block rounded and
    /// every round deleted: the region is every face, and each plane's
    /// one closed piece seeds the walk by the shortcut.
    #[test]
    fn a_deleting_faces_one_unclipped_piece_is_a_shortcut_seed() {
        let b = block();
        let edges: Vec<crate::blend::EdgeBlend> = b
            .edges()
            .map(|e| crate::blend::EdgeBlend {
                edge: b.edges[e].name,
                kind: crate::blend::BlendKind::Fillet { radius: 0.05 },
            })
            .collect();
        let (r, _) = crate::blend::blend(crate::name::OpKey(0x13b7), &b, &edges).unwrap();
        let deleted: BTreeSet<FaceId> = r
            .faces()
            .filter(|&f| {
                !matches!(
                    r.surfaces[r.faces[f].surface],
                    crate::geom::Surface::Plane(_)
                )
            })
            .collect();
        assert_eq!(deleted.len(), 20);
        let changes = Changes {
            deleted: deleted.clone(),
            ..Changes::default()
        };
        let reg = region(&r, &deleted);
        let bx = region_box(&r, &reg, 0.1);
        let ws: BTreeMap<FaceId, super::super::working::Working> = reg
            .faces
            .iter()
            .filter(|f| !deleted.contains(f))
            .map(|&f| {
                (
                    f,
                    working(&r, f, None, r.faces[f].sense, true, &bx).unwrap(),
                )
            })
            .collect();
        // unmetered: a unit test.
        let m = Meter::free();
        let w = work_body(K, &r, &reg, &ws, &m).unwrap();
        let ops = [w.body.clone()];
        let table = build_in(&ops, Scope::Pairs(&w.pairs()), &m).unwrap();
        let pieces = split_all(&ops, &table, &m).unwrap();
        let arr = arrangement(&w, &table, &pieces);
        assert!(outside_seeds(&w, &pieces).is_empty());
        let fs = face_seeds(&r, &changes, &w, &arr, &table, &pieces, &m);
        assert_eq!(fs.same.len(), 6);
        assert!(fs.moved.is_empty());
        assert_eq!(
            fs.shortcut,
            fs.same.iter().copied().collect::<BTreeSet<_>>()
        );
    }
}
