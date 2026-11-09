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
pub(crate) mod region;
pub(crate) mod seeds;
pub(crate) mod walk;
pub(crate) mod working;

use crate::boolean::build::Kept;
use crate::boolean::table::Table;
use crate::boolean::{Fail, past_cap};
use crate::check::{Fault, check_intersections_in};
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
    let changed = changes.motion.keys().copied().collect();
    let mut region = region::region(body, &changed);
    let reach = reach(body, changes);
    // The ladder: the region and its box; the box doubled; the region
    // grown by a ring, twice. It stops once the region is every face.
    let mut last = None;
    for rung in 0..4 {
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
        match attempt(key, body, changes, &region, &bx, m) {
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
    for (&f, mo) in &changes.motion {
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
    for &f in &region.faces {
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
            Ok(w) => {
                ws.insert(f, w);
            }
            Err(e) => return done(Err(e)),
        }
    }
    let w = match work_body(key, body, region, &ws) {
        Ok(w) => w,
        Err(e) => return Rung::Next(e),
    };
    let walked: Mutex<Option<(WalkError, FaceId)>> = Mutex::new(None);
    let select = |_: &[Body], table: &Table, pieces: &[Piece], _: &[usize], m: &Meter| {
        let arr = seeds::arrangement(&w, table, pieces);
        let mut seeds = seeds::outside_seeds(&w, pieces);
        let mut kept = walk::walk(&arr, &seeds, m);
        // Shells no outside face reaches take a seed each, once.
        if let Ok(k) = &kept {
            let more = seeds::shell_seeds(body, changes, &w, &arr, table, pieces, k, m);
            if !more.is_empty() {
                seeds.extend(more);
                kept = walk::walk(&arr, &seeds, m);
            }
        }
        match kept {
            Ok(k) => Ok(pieces
                .iter()
                .zip(k)
                .filter(|(_, keep)| *keep)
                .map(|(p, _)| Kept {
                    piece: p.clone(),
                    reversed: false,
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
    if !crate::check::check(&out).is_empty() {
        return done(Err(Error::Topology(TopologyError::Inconsistent)));
    }
    for fault in check_intersections_in(&out, m) {
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
    done(Ok((out, journal)))
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

#[cfg(test)]
pub(crate) mod tests;
