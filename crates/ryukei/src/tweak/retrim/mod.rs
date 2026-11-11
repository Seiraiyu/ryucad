//! The local re-trim (design 399): the tweak engine for changes of
//! topology. The faces a tweak changes and their neighbours are the
//! region; every face of it becomes a working face, its surface carried
//! past its domain over the region's box; the faces outside the region
//! stay as they are. The working faces and the outside faces go through
//! the self-union's one-body boolean pipeline ([`crate::repair::resolve`]),
//! and the pieces kept are those the frontier walk reaches from the
//! outside faces (see `walk`). Nothing about which side of a surface is
//! material is guessed: the walk only counts.

pub(crate) mod body;
pub(crate) mod names;
pub(crate) mod region;
pub(crate) mod seeds;
pub(crate) mod walk;
pub(crate) mod working;

use crate::boolean::build::Kept;
use crate::boolean::table::Table;
use crate::boolean::{Fail, past_cap};
use crate::check::Fault;
use crate::name::{Journal, Name, OpKey};
use crate::split::Piece;
use crate::topo::{Body, FaceId};
use crate::tweak::engine::Changes;
use crate::work::Meter;
use crate::{Error, TopologyError, TweakError};
use body::{WorkBody, doubled, region_box, work_body};
use region::Region;
use std::collections::BTreeMap;
use std::sync::Mutex;
use walk::WalkError;

/// One rung of the retry ladder: whether it may go on to the next.
enum Rung {
    Done(Box<Result<(Body, Journal), Error>>),
    Next(Error),
}

/// A rung that answers.
fn done(r: Result<(Body, Journal), Error>) -> Rung {
    Rung::Done(Box::new(r))
}

/// `body` tweaked by `changes` with its topology free to change, named
/// from `key`. Refusals: `NotASolid` for a sheet, `LaminarFace` for a
/// region face with a laminar edge, `GapNotHealed`, `HealAmbiguous`,
/// `RegionTooSmall` and `ExtensionUnsupported` from the walk,
/// `SelfIntersecting`, `Uncertain` and `ToleranceTooLarge` from the
/// result's checks.
pub(crate) fn retrim(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    body.require_solid("re-trim")?;
    let changed = changes
        .motion
        .keys()
        .chain(&changes.deleted)
        .copied()
        .collect();
    let mut region = region::region(body, &changed);
    let reach = reach(body, changes);
    // The ladder: the region and its box; the box doubled; the region
    // grown by a ring, twice. It stops once the region is every face.
    let mut last = None;
    for rung in 0..4 {
        // Polled, never charged: a rung's working faces and their sheets
        // are unmetered (13.D: NIST FTC 10's blends ran 17 minutes past a
        // cancel here).
        m.poll().map_err(|_| Error::Cancelled)?;
        if rung >= 2 {
            if region.whole(body) {
                break;
            }
            region = region.grown(body);
        }
        laminar_free(body, &region)?;
        let mut bx = region_box(body, &region, reach);
        if rung == 1 {
            bx = doubled(&bx);
        }
        // Each rung under its own cap: one that can't settle refuses
        // `Uncertain` in bounded units, as the caps do.
        match m.capped(crate::work::Scope::Rung, || {
            attempt(key, body, changes, &region, &bx, m)
        }) {
            Rung::Done(r) => return *r,
            Rung::Next(e) => last = Some(e),
        }
    }
    Err(last.expect("one rung ran"))
}

/// How far a changed face's vertex moves, at most, and at least ten
/// resolutions.
fn reach(body: &Body, changes: &Changes) -> f64 {
    let mut r: f64 = 10.0 * crate::tol::LINEAR_RESOLUTION;
    // A gap is as wide as the box of the faces taken away.
    if let Some(g) = changes
        .deleted
        .iter()
        .map(|&f| crate::intersect::face_box(body, f))
        .reduce(|a, b| a.union(&b))
    {
        r = r.max(g.min().distance(g.max()));
    }
    for (&f, mo) in &changes.motion {
        // An offset moves every point by its distance (`apply` needs the
        // normal for that, which a vertex alone doesn't give).
        if let crate::tweak::Motion::Offset(d) = mo {
            r = r.max(d.abs());
            continue;
        }
        let new = changes
            .surface
            .get(&f)
            .unwrap_or(&body.surfaces[body.faces[f].surface]);
        let sense = changes
            .senses
            .get(&f)
            .copied()
            .unwrap_or(body.faces[f].sense);
        for l in body.face_loops(f) {
            for fin in body.loop_fins(l) {
                let e = &body.edges[body.fins[fin].edge];
                for v in [e.start, e.end] {
                    let p = body.vertices[v].point;
                    let (q, _) =
                        crate::tweak::motion::apply(mo, new, sense, p, crate::geom::Vector3::ZERO);
                    r = r.max(p.distance(q));
                }
            }
        }
    }
    r
}

/// `LaminarFace` for a region face with a laminar edge.
fn laminar_free(body: &Body, region: &Region) -> Result<(), Error> {
    for &f in &region.faces {
        if body.face_loops(f).any(|l| {
            body.loop_fins(l)
                .any(|fin| body.is_laminar(body.fins[fin].edge))
        }) {
            return Err(TweakError::LaminarFace {
                face: body.faces[f].name,
            }
            .into());
        }
    }
    Ok(())
}

/// One rung: the working body, the table, the walk, the result's checks.
fn attempt(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    region: &Region,
    bx: &crate::geom::Box3,
    m: &Meter,
) -> Rung {
    let mut ws = BTreeMap::new();
    let absorbed = absorbed(body, changes, region);
    let absorbing: std::collections::BTreeSet<FaceId> = absorbed.values().copied().collect();
    for &f in region
        .faces
        .iter()
        .filter(|f| !changes.deleted.contains(f) && !absorbed.contains_key(f))
    {
        if m.poll().is_err() {
            return done(Err(Error::Cancelled));
        }
        let mo = changes.motion.get(&f);
        let sense = changes
            .senses
            .get(&f)
            .copied()
            .unwrap_or(body.faces[f].sense);
        match working::working(
            body,
            f,
            changes.surface.get(&f),
            sense,
            working::same_params(mo),
            bx,
        ) {
            Ok(mut w) => {
                // A face standing for others on its surface covers them
                // all: its whole turn (NIST FTC 09's hole halves).
                if !absorbing.contains(&f) {
                    for (k, t) in working::delete_turn(body, changes, f, &w.surface)
                        .into_iter()
                        .enumerate()
                    {
                        if let Some(t) = t {
                            w.rect[k] = t;
                        }
                    }
                }
                ws.insert(f, w);
            }
            Err(e) => return done(Err(e)),
        }
    }
    let w = match work_body(key, body, region, &ws, m) {
        Ok(w) => w,
        Err(e) => return Rung::Next(e),
    };
    let walked: Mutex<Option<(WalkError, FaceId)>> = Mutex::new(None);
    let select = |_: &[Body], table: &Table, pieces: &[Piece], _: &[usize], m: &Meter| {
        let arr = seeds::arrangement(&w, table, pieces);
        let outside = seeds::outside_seeds(&w, pieces);
        // Design 399's rule 3: region faces with no frontier edge seeded by
        // the piece holding their old interior point. A moved face's seed
        // says it survives, which a face moved out of the body doesn't: so
        // the walk runs with every such seed, then with the unchanged
        // faces' alone. A region with no outside face takes the plan's
        // one shell seed instead (below).
        let mut tries: Vec<Vec<usize>> = vec![outside.clone()];
        // A delete whose region is the whole body: every face it keeps
        // seeds by rule 3 too, not one face alone (#692: a drilled plate's
        // side deleted closed from the hole's wall alone, its plug).
        if outside.is_empty() && !changes.deleted.is_empty() && changes.motion.is_empty() {
            let (same, _) = seeds::face_seeds(body, changes, &w, &arr, table, pieces, m);
            if same.len() > 1 {
                tries = vec![same];
            }
        }
        if !outside.is_empty() {
            let (same, moved) = seeds::face_seeds(body, changes, &w, &arr, table, pieces, m);
            if !same.is_empty() || !moved.is_empty() {
                let mut with_same = outside.clone();
                with_same.extend(&same);
                let mut all = with_same.clone();
                all.extend(&moved);
                tries = if moved.is_empty() {
                    vec![with_same]
                } else {
                    vec![all, with_same]
                };
            }
        }
        let mut kept = Err(WalkError::Open { piece: 0 });
        let mut seeds = Vec::new();
        for (k, t) in tries.into_iter().enumerate() {
            let r = walk::walk(&arr, &t, m);
            let stop = r.is_ok() || matches!(r, Err(WalkError::Halted { .. }));
            // The first try's failure is the one reported.
            if stop || k == 0 {
                kept = r;
                seeds = t;
            }
            if stop {
                break;
            }
        }
        // Shells no outside face reaches take a seed each, once.
        if let Ok(k) = &kept {
            let seen: Vec<bool> = k.iter().map(Option::is_some).collect();
            let more = seeds::shell_seeds(body, changes, &w, &arr, table, pieces, &seen, m);
            if !more.is_empty() {
                seeds.extend(more);
                kept = walk::walk(&arr, &seeds, m);
            }
        }
        match kept {
            Ok(k) => Ok(pieces
                .iter()
                .zip(k)
                .filter_map(|(p, keep)| {
                    keep.map(|reversed| Kept {
                        piece: p.clone(),
                        reversed,
                    })
                })
                .collect()),
            Err(e) => {
                let (WalkError::Open { piece }
                | WalkError::Ambiguous { piece }
                | WalkError::Clipped { piece }
                | WalkError::Halted { piece }) = e;
                let f = pieces[piece].face;
                *walked.lock().expect("unpoisoned") = Some((e, f.face));
                // A marker: the walk's own error is read from `walked`.
                Err(Fail::Uncertain(f, f))
            }
        }
    };
    let pairs = w.pairs();
    let resolved = crate::repair::resolve(key, &w.body, &pairs, &select, &|_| false, m);
    let name_of = |f: FaceId| w.body.faces[f].name;
    let (out, journal) = match (resolved, walked.lock().expect("unpoisoned").take()) {
        // A halt answers at once (`work::run` reports it); every other
        // walk failure climbs the ladder.
        (_, Some((e @ WalkError::Halted { .. }, f))) => {
            return done(Err(walk_error(&w, e, f)));
        }
        (_, Some((e, f))) => return Rung::Next(walk_error(&w, e, f)),
        (Ok(Some(r)), None) => r,
        (Ok(None), None) => {
            let least = region.changed.iter().map(|&f| body.faces[f].name).min();
            return Rung::Next(
                TweakError::GapNotHealed {
                    face: least.unwrap_or(Name::from_u128(0)),
                }
                .into(),
            );
        }
        (Err(Fail::Inconsistent), None) => {
            return done(Err(Error::Topology(TopologyError::Inconsistent)));
        }
        (Err(Fail::Uncertain(a, _) | Fail::Unsupported(a, _)), None) => {
            return done(Err(TweakError::Uncertain {
                at: name_of(a.face),
            }
            .into()));
        }
        (Err(Fail::NonManifold { faces, .. }), None) => {
            let at = faces
                .first()
                .map_or(Name::from_u128(0), |f| name_of(f.face));
            return Rung::Next(TweakError::HealAmbiguous { face: at }.into());
        }
    };
    let faults = crate::check::check(&out);
    if !faults.is_empty() {
        // Inside out: a face carried past the opposite side (#704); the
        // solid between them would have to collapse.
        if faults.iter().all(crate::check::inside_out) {
            let face = region
                .changed
                .iter()
                .map(|&f| body.faces[f].name)
                .min()
                .unwrap_or(Name::from_u128(0));
            return done(Err(TweakError::FaceCollapses { face }.into()));
        }
        return done(Err(Error::Topology(TopologyError::Inconsistent)));
    }
    // Only pairs with a face the re-trim made: the faces outside the
    // region come back as they were (it never cuts them), so their pairs
    // are the input's (NIST CTC 04: 4 s of a fillet's 8.2 s delete).
    let outside: std::collections::BTreeSet<Name> = body
        .faces()
        .filter(|f| !region.faces.contains(f))
        .map(|f| body.faces[f].name)
        .collect();
    let made = |f: FaceId| !outside.contains(&out.faces[f].name);
    for fault in crate::check::check_pairs_in(&out, m, |a, b| made(a) || made(b)) {
        match fault {
            Fault::FacesCross { a, b } | Fault::FacesOverlap { a, b } => {
                let mut faces = [out.faces[a].name, out.faces[b].name];
                faces.sort();
                return Rung::Next(TweakError::SelfIntersecting { faces }.into());
            }
            Fault::CannotCheck { a, .. } => {
                return done(Err(TweakError::Uncertain {
                    at: out.faces[a].name,
                }
                .into()));
            }
            _ => {}
        }
    }
    if let Some((at, needed)) = past_cap(&out) {
        return done(Err(TweakError::ToleranceTooLarge { at, needed }.into()));
    }
    let mut out = out;
    let moved = changes.motion.keys().map(|&f| body.faces[f].name).collect();
    let mut journal = names::finish(key, body, &w, &mut out, journal, &moved);
    // Faces absorbed into a neighbour on their surface are merged into it.
    let mut into: BTreeMap<Name, Vec<Name>> = BTreeMap::new();
    for (&g, &a) in &absorbed {
        into.entry(body.faces[a].name)
            .or_default()
            .push(body.faces[g].name);
    }
    for (a, mut parents) in into {
        if out.by_name(a).is_some() {
            parents.push(a);
            parents.sort();
            journal.push(crate::name::Event::Merged { parents, child: a });
        }
    }
    done(Ok((out, journal)))
}

/// Region faces left as they are that share an edge with another such
/// face on the same surface, the same way round: each mapped to the
/// least-named face of its group, whose working face covers them all.
/// Their own working faces would coincide with it (NIST FTC 09's hole
/// walls, each two half cylinders carried a whole turn each, closed two
/// ways).
fn absorbed(body: &Body, changes: &Changes, region: &Region) -> BTreeMap<FaceId, FaceId> {
    let still = |f: FaceId| {
        region.faces.contains(&f)
            && !changes.deleted.contains(&f)
            && !changes.motion.contains_key(&f)
            && !changes.surface.contains_key(&f)
    };
    let mut up: BTreeMap<FaceId, FaceId> = BTreeMap::new();
    fn root(up: &BTreeMap<FaceId, FaceId>, mut f: FaceId) -> FaceId {
        while let Some(&g) = up.get(&f) {
            if g == f {
                break;
            }
            f = g;
        }
        f
    }
    for e in body.edges() {
        let fs: Vec<FaceId> = body
            .edge_fins(e)
            .map(|fin| body.loops[body.fins[fin].loop_].face)
            .collect();
        let [a, b] = fs.as_slice() else { continue };
        let (a, b) = (*a, *b);
        if a == b || !still(a) || !still(b) {
            continue;
        }
        let (fa, fb) = (&body.faces[a], &body.faces[b]);
        if fa.sense != fb.sense || body.surfaces[fa.surface] != body.surfaces[fb.surface] {
            continue;
        }
        let (ra, rb) = (root(&up, a), root(&up, b));
        if ra != rb {
            let (keep, go) = if body.faces[ra].name < body.faces[rb].name {
                (ra, rb)
            } else {
                (rb, ra)
            };
            up.insert(go, keep);
            up.entry(keep).or_insert(keep);
        }
    }
    up.keys()
        .filter_map(|&f| {
            let r = root(&up, f);
            (r != f).then_some((f, r))
        })
        .collect()
}

/// A walk's failure at face `face` of the working body as a refusal.
fn walk_error(w: &WorkBody, e: WalkError, face: FaceId) -> Error {
    let name = w.body.faces[face].name;
    match e {
        WalkError::Open { .. } => TweakError::GapNotHealed { face: name },
        WalkError::Ambiguous { .. } => TweakError::HealAmbiguous { face: name },
        WalkError::Clipped { .. } if w.fixed.contains(&face) => {
            TweakError::ExtensionUnsupported { face: name }
        }
        WalkError::Clipped { .. } => TweakError::RegionTooSmall { face: name },
        WalkError::Halted { .. } => TweakError::Uncertain { at: name },
    }
    .into()
}

/// Whether the fixed engine's refusal `e` means the topology must change
/// (so `Topology::Change` re-trims): an edge vanishing, a vertex
/// splitting, a face collapsing, faces crossing, or a lone vertex. An
/// `Uncertain` at one of `body`'s vertices is the fixed engine's
/// three-face form of a vertex splitting (`tweak::vertices`: a curve there
/// doesn't pass the point the others meet at), so it re-trims too; every
/// other `Uncertain` answers at once.
pub(crate) fn takes(body: &Body, e: &Error) -> bool {
    match e {
        Error::InvalidTweak(TweakError::Uncertain { at }) => {
            matches!(body.by_name(*at), Some(crate::topo::EntityId::Vertex(_)))
        }
        _ => matches!(
            e,
            Error::InvalidTweak(
                TweakError::EdgeVanishes { .. }
                    | TweakError::VertexSplits { .. }
                    | TweakError::FaceCollapses { .. }
                    | TweakError::SelfIntersecting { .. }
                    | TweakError::Unsupported {
                        case: crate::TweakCase::LoneVertex,
                        ..
                    }
            )
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests;
