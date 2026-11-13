//! Repair: a body whose faces cross or overlap made into the valid solid
//! of the same point set (design 299).
//!
//! [`self_union`] keeps the region where the body's winding number is one
//! or more, regularized: where shells cross, the union of what they
//! enclose; where a shell lies inside another facing out, it is filled;
//! where it faces in, a cavity. It runs a boolean's pipeline on one body:
//! [`crate::check::check_intersections`] lists the pairs of faces that
//! cross or overlap, and only those are met in the table
//! ([`crate::boolean::table::Scope::Pairs`]), so faces meeting only at
//! the edges and vertices they share are not intersected again; a spline
//! face whose flat span lies on a listed partner's plane is first split
//! there and the body checked again ([`crate::boolean::flat`], #585); each face
//! is split by the curves where its partners meet it; each piece is kept
//! by the winding number just outside it, counted by a certified ray
//! ([`crate::intersect::winding_off`]): a piece bounds the region when
//! the winding number is 0 on its outer side and one or more on its inner
//! side. Of pieces on one surface facing one way (faces that overlap) the
//! one of the least face is kept; pieces facing opposite ways cancel. The
//! kept pieces are built, merged and named as a boolean's are, the body
//! being its target: faces kept whole keep their names, split ones are
//! `Piece`s named by the faces that cut them.
//!
//! A body that the check finds valid comes back as it was, bit for bit,
//! with an empty journal: a self-union costs at least the check.

use crate::boolean::build::Kept;
use crate::boolean::classify::inner_point;
use crate::boolean::table::{Boundary, GeId, Member, Scope, Table, build_in};
use crate::boolean::{FaceRef, Fail, assemble, past_cap};
use crate::check::{Fault, check_intersections_in};
use crate::geom::Point2;
use crate::intersect::{Place, point_in_face, winding_off};
use crate::name::{Journal, Name, OpKey};
use crate::split::{Piece, split_all};
use crate::topo::{Body, FaceId, Sense};
use crate::work::{Meter, Work};
use crate::{Error, RepairError, TopologyError};
use std::collections::{BTreeMap, BTreeSet};

/// The valid solid of the points `body`'s winding number is one or more
/// at, named from `key`, and the journal of what happened to `body`'s
/// names. `body` must pass [`crate::check::check_topology`] and be a
/// solid; its faces may cross and overlap.
///
/// Fails with [`Error::InvalidRepair`]: `Uncertain` for faces whose
/// meeting or classification stays uncertain (or that the check couldn't
/// check), `NonManifold` where the region would meet itself at an edge or
/// a point, `Empty` where nothing has winding number one or more, and
/// `ToleranceTooLarge` for a result holding a tolerance past the cap.
pub fn self_union(key: OpKey, body: &Body) -> Result<(Body, Journal), Error> {
    self_union_with(key, body, &Work::unlimited())
}

/// [`self_union`] under `work`: refused with `Cancelled` or
/// `BudgetExceeded` when it stops, building nothing.
pub fn self_union_with(key: OpKey, body: &Body, work: &Work) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |m| self_union_in(key, body, m))
}

/// [`self_union`] charging `m`, for healing.
pub(crate) fn self_union_in(key: OpKey, body: &Body, m: &Meter) -> Result<(Body, Journal), Error> {
    body.require_solid("self_union")?;
    if !crate::check::check_topology(body).is_empty() {
        return Err(Error::Topology(TopologyError::Inconsistent));
    }
    let (mut pairs, mut cannot) = faulted(body, m);
    if cannot.is_empty() && pairs.is_empty() {
        return Ok((body.clone(), Journal::new()));
    }
    // Flat spans of splines lying on a listed partner's plane split off
    // (design 551, #585), and the body checked again: the flat piece is
    // then wholly coincident with its partner. Bodies without the case go
    // on as before, bit for bit.
    let listed: BTreeSet<(FaceId, FaceId)> = pairs
        .iter()
        .copied()
        .chain(cannot.iter().map(|&(a, b)| (a.min(b), a.max(b))))
        .collect();
    let split = crate::boolean::flat::split_listed(body, key, &listed);
    let mut journal = Journal::new();
    let body: &Body = match &split {
        Some((b, j)) => {
            (pairs, cannot) = faulted(b, m);
            journal = j.clone();
            b
        }
        None => body,
    };
    if let Some(&(a, b)) = cannot.first() {
        return Err(uncertain(body, &[a, b]).into());
    }
    if pairs.is_empty() {
        return Ok((body.clone(), journal));
    }
    let named = |f: Fail| -> Error {
        match f {
            Fail::Inconsistent => Error::Topology(TopologyError::Inconsistent),
            f => refusal(body, f).into(),
        }
    };
    let (out, resolved) = resolve(key, body, &pairs, &select, &|_| false, m)
        .map_err(named)?
        .ok_or(RepairError::Empty)?;
    journal.append(resolved);
    // Never an inside-out body, in either build (a debug build's resolve
    // leaves that to its caller).
    if crate::check::check(&out)
        .iter()
        .any(crate::check::inside_out)
    {
        return Err(Error::Topology(TopologyError::Inconsistent));
    }
    if let Some((at, needed)) = past_cap(&out) {
        return Err(RepairError::ToleranceTooLarge { at, needed }.into());
    }
    Ok((out, journal))
}

/// Two faces of one body.
type FacePair = (FaceId, FaceId);

/// The pairs of `body`'s faces the check finds crossing or overlapping,
/// in face order, and those it couldn't check, in its order.
fn faulted(body: &Body, m: &Meter) -> (BTreeSet<FacePair>, Vec<FacePair>) {
    let mut pairs = BTreeSet::new();
    let mut cannot = Vec::new();
    for fault in check_intersections_in(body, m) {
        match fault {
            Fault::FacesCross { a, b } | Fault::FacesOverlap { a, b } => {
                pairs.insert((a.min(b), a.max(b)));
            }
            Fault::CannotCheck { a, b } => cannot.push((a, b)),
            _ => {}
        }
    }
    (pairs, cannot)
}

/// How [`resolve`] chooses which pieces to keep: from the body (one
/// operand), its table, its pieces, each piece's group (pieces joined
/// across the body's own edges nothing else touches, as the least piece
/// index in it).
pub(crate) type Select<'a> =
    dyn Fn(&[Body], &Table, &[Piece], &[usize], &Meter) -> Result<Vec<Kept>, Fail> + Sync + 'a;

/// The boolean's pipeline on one body: the faces of `pairs` met, every
/// face split by the curves its partners meet it in, the pieces `select`
/// keeps built, merged and named as a boolean's target's, under `key`.
/// `Ok(None)` where `select` keeps nothing.
pub(crate) fn resolve(
    key: OpKey,
    body: &Body,
    pairs: &BTreeSet<(FaceId, FaceId)>,
    select: &Select<'_>,
    alone: &(dyn Fn(FaceId) -> bool + Sync),
    m: &Meter,
) -> Result<Option<(Body, Journal)>, Fail> {
    resolve_seamed(key, body, pairs, &crate::intersect::Seams::new(), select, alone, m)
}

/// [`resolve`], the pairs in `seams` known to meet along them (the
/// re-trim's kept faces tangent along an input edge, #652).
pub(crate) fn resolve_seamed(
    key: OpKey,
    body: &Body,
    pairs: &BTreeSet<(FaceId, FaceId)>,
    seams: &crate::intersect::Seams,
    select: &Select<'_>,
    alone: &(dyn Fn(FaceId) -> bool + Sync),
    m: &Meter,
) -> Result<Option<(Body, Journal)>, Fail> {
    let ops = [body.clone()];
    let table = if seams.is_empty() {
        build_in(&ops, Scope::Pairs(pairs), m)?
    } else {
        crate::boolean::table::build_seamed(&ops, Scope::Pairs(pairs), seams, m)?
    };
    let pieces = split_all(&ops, &table, m)?;
    let groups = groups(body, &table, &pieces, alone);
    let kept = select(&ops, &table, &pieces, &groups, m)?;
    if kept.is_empty() {
        return Ok(None);
    }
    let (out, journal) = assemble(key, &ops, &[], &table, (&pieces, &kept), m)?;
    if cfg!(debug_assertions) {
        // An inside-out result is its caller's to refuse (#704: the
        // re-trim's offset past the opposite side); anything else is a bug.
        let faults: Vec<_> = crate::check::check(&out)
            .into_iter()
            .filter(|f| !crate::check::inside_out(f))
            .collect();
        assert!(faults.is_empty(), "a resolved body is invalid: {faults:?}");
    } else if !crate::check::check_topology(&out).is_empty() {
        return Err(Fail::Inconsistent);
    }
    Ok(Some((out, journal)))
}

/// A table failure as a repair's refusal, naming `body`'s faces.
pub(crate) fn refusal(body: &Body, f: Fail) -> RepairError {
    match f {
        Fail::Uncertain(a, b) | Fail::Unsupported(a, b) => uncertain(body, &[a.face, b.face]),
        Fail::NonManifold { at, faces } => {
            let mut faces: Vec<Name> = faces.iter().map(|f| body.faces[f.face].name).collect();
            faces.sort();
            faces.dedup();
            RepairError::NonManifold { at, faces }
        }
        Fail::Inconsistent => unreachable!("taken first"),
    }
}

/// `Uncertain`, naming `faces` of `body`, sorted.
fn uncertain(body: &Body, faces: &[FaceId]) -> RepairError {
    let mut faces: Vec<Name> = faces.iter().map(|&f| body.faces[f].name).collect();
    faces.sort();
    faces.dedup();
    RepairError::Uncertain { faces }
}

/// The pieces that bound the region of winding number one or more, none
/// reversed: a piece is kept when the winding number just outside it (and
/// every face on its surface there) is at most 0 and just inside at least
/// 1, the jump across being 1, and 1 more for every face on its surface
/// there facing its way, 1 less for every one facing the other; and no
/// face facing its way there comes before its own. A lone face is kept
/// when the winding number outside it is 0. No piece needs reversing:
/// where the region's boundary runs against a piece, a piece on its
/// surface facing the other way is there to keep.
fn select(
    ops: &[Body],
    table: &Table,
    pieces: &[Piece],
    groups: &[usize],
    m: &Meter,
) -> Result<Vec<Kept>, Fail> {
    let body = &ops[0];
    // Each face's partners on its surface, and whether their outward
    // normals agree.
    let mut partners: BTreeMap<FaceId, Vec<(FaceId, bool)>> = BTreeMap::new();
    for c in &table.coincident {
        let (fa, fb) = (body.faces[c.a.face].sense, body.faces[c.b.face].sense);
        let same = c.same == (fa == fb);
        partners.entry(c.a.face).or_default().push((c.b.face, same));
        partners.entry(c.b.face).or_default().push((c.a.face, same));
    }
    // One winding number per group of pieces joined across edges nothing
    // else touches.
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &g) in groups.iter().enumerate() {
        members.entry(g).or_default().push(i);
    }
    let lists: Vec<Vec<usize>> = members.into_values().collect();
    let found: Vec<Vec<(usize, i64)>> = m
        .par_map(&lists, |m, list| {
            let w = group_winding(ops, table, pieces, &partners, list, m)?;
            Ok(list.iter().map(|&i| (i, w)).collect())
        })
        // The first failure in order, not the first a thread found.
        .into_iter()
        .collect::<Result<_, Fail>>()?;
    let mut outside = vec![0i64; pieces.len()];
    for (i, w) in found.into_iter().flatten() {
        outside[i] = w;
    }
    // Each piece's place among the faces on its surface.
    let stacks: Vec<(i64, bool)> = m
        .par_map(pieces, |_, p| stack(ops, table, &partners, p))
        // The first failure in order, not the first a thread found.
        .into_iter()
        .collect::<Result<_, Fail>>()?;
    Ok(pieces
        .iter()
        .zip(outside)
        .zip(stacks)
        .filter(|((_, w), (jump, first))| *w <= 0 && w + jump >= 1 && *first)
        .map(|((p, _), _)| Kept {
            piece: p.clone(),
            reversed: false,
        })
        .collect())
}

/// The jump in winding number across a piece's surface, from its outer
/// side to its inner, and whether no face facing its way there comes
/// before its own.
fn stack(
    ops: &[Body],
    table: &Table,
    partners: &BTreeMap<FaceId, Vec<(FaceId, bool)>>,
    p: &Piece,
) -> Result<(i64, bool), Fail> {
    let Some(list) = partners.get(&p.face.face) else {
        return Ok((1, true));
    };
    let body = &ops[0];
    let (q, _) = inner_point(ops, table, p, 0).ok_or(Fail::Uncertain(p.face, p.face))?;
    let mut jump = 1i64;
    let mut first = true;
    for &(g, same) in list {
        let surface = &body.surfaces[body.faces[g].surface];
        let (u, v) = surface.project(q).param;
        let g_ref = FaceRef { op: 0, face: g };
        match point_in_face(body, g, Point2::new(u, v))
            .map_err(|_| Fail::Uncertain(p.face, g_ref))?
        {
            Place::Inside => {
                jump += if same { 1 } else { -1 };
                if same && g < p.face.face {
                    first = false;
                }
            }
            Place::Outside => {}
            // The middle of a piece can't be on the other face's boundary:
            // the boundary was imprinted.
            _ => return Err(Fail::Uncertain(p.face, g_ref)),
        }
    }
    Ok((jump, first))
}

/// The winding number just outside a group's pieces, from a point inside
/// one of them (the first that settles it).
fn group_winding(
    ops: &[Body],
    table: &Table,
    pieces: &[Piece],
    partners: &BTreeMap<FaceId, Vec<(FaceId, bool)>>,
    list: &[usize],
    m: &Meter,
) -> Result<i64, Fail> {
    let body = &ops[0];
    for &i in list {
        let p = &pieces[i];
        let face = &body.faces[p.face.face];
        let surface = &body.surfaces[face.surface];
        let mut skip = vec![p.face.face];
        if let Some(l) = partners.get(&p.face.face) {
            skip.extend(l.iter().map(|x| x.0));
        }
        for attempt in 0..4 {
            let Some((q, uv)) = inner_point(ops, table, p, attempt) else {
                continue;
            };
            let Some(n) = surface.normal(uv.x, uv.y) else {
                continue;
            };
            let n = match face.sense {
                Sense::Forward => n,
                Sense::Reversed => -n,
            };
            if let Ok(w) = winding_off(body, q, n, &skip, m) {
                return Ok(w);
            }
        }
    }
    let f = pieces[list[0]].face;
    Err(Fail::Uncertain(f, f))
}

/// Each piece's group, as the least piece index in it: pieces joined
/// across a piece of the body's own edge that only the edge's two faces
/// use, with no stretch and no other edge along it.
pub(crate) fn groups(
    body: &Body,
    table: &Table,
    pieces: &[Piece],
    alone: &(dyn Fn(FaceId) -> bool + Sync),
) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..pieces.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    // The faces using each global edge, on their boundaries or inside.
    let mut used_by: BTreeMap<GeId, BTreeSet<FaceId>> = BTreeMap::new();
    for (f, w) in &table.faces {
        let ges = w
            .loops
            .iter()
            .flat_map(|l| match l {
                Boundary::Arcs(a) => a.iter().map(|x| x.0).collect(),
                Boundary::Lone(_) => Vec::new(),
            })
            .chain(w.interior.iter().copied());
        for g in ges {
            used_by.entry(g).or_default().insert(f.face);
        }
    }
    let untouched = |g: GeId| -> bool {
        let [Member::Edge(e)] = table.edges[g].members[..] else {
            return false;
        };
        let own: BTreeSet<FaceId> = body
            .edge_fins(e.edge)
            .map(|fin| body.loops[body.fins[fin].loop_].face)
            .collect();
        used_by.get(&g).is_none_or(|u| u.is_subset(&own))
            && table.faces.values().all(|w| !w.interior.contains(&g))
    };
    let mut by_edge: BTreeMap<GeId, Vec<usize>> = BTreeMap::new();
    for (i, p) in pieces.iter().enumerate() {
        for l in &p.loops {
            if let Boundary::Arcs(a) = l {
                for &(g, _) in a {
                    by_edge.entry(g).or_default().push(i);
                }
            }
        }
    }
    for (g, list) in by_edge {
        if !untouched(g) {
            continue;
        }
        let list: Vec<usize> = list
            .into_iter()
            .filter(|&i| !alone(pieces[i].face.face))
            .collect();
        for w in list.windows(2) {
            let (x, y) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
            if x != y {
                parent[x.max(y)] = x.min(y);
            }
        }
    }
    (0..pieces.len()).map(|i| find(&mut parent, i)).collect()
}

#[cfg(test)]
mod tests;
