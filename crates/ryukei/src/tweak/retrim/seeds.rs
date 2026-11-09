//! The walk's arrangement read from the table, and its seeds: every
//! piece of an outside face, and, for a shell no outside face seeds, one
//! region face's piece holding a point of its old self.

use super::body::WorkBody;
use super::walk::Arrangement;
use crate::boolean::classify::inner_point;
use crate::boolean::table::{Boundary, GeId, Member, Table};
use crate::geom::Point2;
use crate::intersect::{Place, point_in_face};
use crate::split::Piece;
use crate::topo::{Body, FaceId, ShellId};
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

/// Every piece of an outside face.
pub(crate) fn outside_seeds(w: &WorkBody, pieces: &[Piece]) -> Vec<usize> {
    pieces
        .iter()
        .enumerate()
        .filter(|(_, p)| !w.working.contains(&p.face.face))
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
            let found: Vec<usize> = pieces
                .iter()
                .enumerate()
                .filter(|(i, p)| {
                    p.face.face == wf
                        && arr.pieces[*i]
                            .1
                            .iter()
                            .all(|(g, _)| !arr.clipped.contains(g))
                        && inside_old(body, changes, w, &ops, table, p)
                })
                .map(|(i, _)| i)
                .collect();
            if let [one] = found.as_slice() {
                out.push(*one);
                break;
            }
        }
    }
    out
}

/// Whether piece `p`'s inner point, taken back to its face's old surface,
/// lies inside the old face (the first of four inner points that is
/// inside or outside, not on the boundary, decides).
fn inside_old(
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
        let w = work_body(K, &b, &r, &ws).unwrap();
        let ops = [w.body.clone()];
        // unmetered: a unit test.
        let m = Meter::free();
        let table = build_in(&ops, Scope::Pairs(&w.pairs()), &m).unwrap();
        let pieces = split_all(&ops, &table, &m).unwrap();
        let arr = arrangement(&w, &table, &pieces);
        let seeds = outside_seeds(&w, &pieces);
        // The bottom alone, whole.
        assert_eq!(seeds.len(), 1);
        assert_eq!(arr.old.len(), 4);
        assert!(!arr.clipped.is_empty());
    }
}
